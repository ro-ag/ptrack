package store

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

// MigrationKind identifies which closed legacy bbolt schema is being exported.
type MigrationKind uint8

const (
	MigrationKindProject MigrationKind = 1
	MigrationKindGlobal  MigrationKind = 2

	migrationBundleVersion   = 1
	migrationBundleHeaderLen = 40
	migrationMaxBuckets      = 13
	migrationMaxBucketName   = 255
	migrationMaxRecords      = 1_000_000
	migrationMaxKeyBytes     = 1 << 20
	migrationMaxValueBytes   = 256 << 20
	migrationMaxPayloadBytes = uint64(256) << 20
	migrationMaxBundleBytes  = uint64(16) << 30
	migrationRecordOverhead  = 20
)

var migrationMagic = [8]byte{'P', 'T', 'R', 'K', 'M', 'I', 'G', '1'}

type migrationBucketSpec struct {
	name       string
	sequenced  bool
	numericKey bool
	introduced uint64
}

type migrationBucket struct {
	spec     migrationBucketSpec
	sequence uint64
	records  uint64
}

var projectMigrationBuckets = []migrationBucketSpec{
	{name: "meta", introduced: 0},
	{name: "plans", sequenced: true, numericKey: true, introduced: 0},
	{name: "tasks", sequenced: true, numericKey: true, introduced: 0},
	{name: "notes", sequenced: true, numericKey: true, introduced: 0},
	{name: "milestones", sequenced: true, numericKey: true, introduced: 2},
	{name: "issues", sequenced: true, numericKey: true, introduced: 2},
	{name: "commits", sequenced: true, numericKey: true, introduced: 3},
	{name: "capabilities", sequenced: true, numericKey: true, introduced: 4},
	{name: "capability_audits", sequenced: true, numericKey: true, introduced: 4},
	{name: "memory_writebacks", sequenced: true, introduced: 5},
}

var globalMigrationBuckets = []migrationBucketSpec{
	{name: "config"},
	{name: "projects"},
	{name: "backups"},
}

// ParseMigrationKind parses the only two accepted migration schema names.
func ParseMigrationKind(value string) (MigrationKind, error) {
	switch value {
	case "project":
		return MigrationKindProject, nil
	case "global":
		return MigrationKindGlobal, nil
	default:
		return 0, fmt.Errorf("kind must be project or global, got %q", value)
	}
}

// ExportMigrationBundle copies a validated legacy bbolt database into the
// deterministic, checksummed PTRKMIG1 interchange format. It opens source
// directly and read-only; it never invokes application store initialization.
func ExportMigrationBundle(kind MigrationKind, sourcePath, outputPath string) error {
	return exportMigrationBundleWithOpen(kind, sourcePath, outputPath, os.OpenFile)
}

func exportMigrationBundleWithOpen(kind MigrationKind, sourcePath, outputPath string, openSource func(string, int, os.FileMode) (*os.File, error)) error {
	return exportMigrationBundleWithOpenAndCheck(kind, sourcePath, outputPath, openSource, func(tx *bolt.Tx) <-chan error {
		return tx.Check()
	})
}

func exportMigrationBundleWithOpenAndCheck(
	kind MigrationKind,
	sourcePath, outputPath string,
	openSource func(string, int, os.FileMode) (*os.File, error),
	checkSource func(*bolt.Tx) <-chan error,
) error {
	if kind != MigrationKindProject && kind != MigrationKindGlobal {
		return fmt.Errorf("unsupported migration kind %d", kind)
	}
	if err := migrationOutputSupported(); err != nil {
		return err
	}
	if !filepath.IsAbs(sourcePath) {
		return errors.New("source path must be absolute")
	}
	if !filepath.IsAbs(outputPath) {
		return errors.New("output path must be absolute")
	}
	if filepath.Clean(sourcePath) == filepath.Clean(outputPath) {
		return errors.New("source and output paths must differ")
	}
	if openSource == nil {
		return errors.New("source opener is required")
	}
	if checkSource == nil {
		return errors.New("source integrity checker is required")
	}
	outputDirectory, err := openMigrationOutputDirectory(outputPath)
	if err != nil {
		return err
	}
	defer outputDirectory.close()

	preOpenInfo, err := os.Lstat(sourcePath)
	if err != nil {
		return fmt.Errorf("inspect source: %w", err)
	}
	if preOpenInfo.Mode()&os.ModeSymlink != 0 {
		return errors.New("source must not be a symbolic link")
	}
	if !preOpenInfo.Mode().IsRegular() {
		return errors.New("source must be a regular file")
	}

	source, err := openSource(sourcePath, os.O_RDONLY, 0)
	if err != nil {
		return fmt.Errorf("open source read-only: %w", err)
	}
	openedInfo, err := source.Stat()
	if err != nil {
		_ = source.Close()
		return fmt.Errorf("inspect opened source: %w", err)
	}
	postOpenInfo, err := os.Lstat(sourcePath)
	if err != nil {
		_ = source.Close()
		return fmt.Errorf("reinspect source after open: %w", err)
	}
	if !openedInfo.Mode().IsRegular() || postOpenInfo.Mode()&os.ModeSymlink != 0 || !postOpenInfo.Mode().IsRegular() ||
		!os.SameFile(preOpenInfo, openedInfo) || !os.SameFile(preOpenInfo, postOpenInfo) {
		_ = source.Close()
		return errors.New("source path changed while it was being opened")
	}

	openUsed := false
	db, err := bolt.Open(sourcePath, 0o600, &bolt.Options{
		ReadOnly: true,
		Timeout:  time.Second,
		OpenFile: func(name string, flag int, mode os.FileMode) (*os.File, error) {
			if openUsed || name != sourcePath || flag != os.O_RDONLY {
				return nil, errors.New("unexpected bbolt source open request")
			}
			openUsed = true
			return source, nil
		},
	})
	if err != nil {
		if !openUsed {
			_ = source.Close()
		}
		return fmt.Errorf("open validated source with bbolt: %w", err)
	}

	var partialName string
	var partialDisplayPath string
	exportErr := db.View(func(tx *bolt.Tx) error {
		if err := drainMigrationCheck(checkSource(tx)); err != nil {
			return err
		}
		buckets, sourceFormat, totalRecords, err := inspectMigrationSource(tx, kind)
		if err != nil {
			return err
		}
		output, name, displayPath, err := outputDirectory.createPartial()
		if err != nil {
			return fmt.Errorf("create partial output: %w", err)
		}
		partialName = name
		partialDisplayPath = displayPath
		writeErr := writeMigrationBundle(output, kind, sourceFormat, totalRecords, buckets, tx)
		if writeErr == nil {
			writeErr = output.Sync()
		}
		if closeErr := output.Close(); writeErr == nil && closeErr != nil {
			writeErr = closeErr
		}
		return writeErr
	})
	if closeErr := db.Close(); exportErr == nil && closeErr != nil {
		exportErr = closeErr
	}
	if exportErr == nil {
		exportErr = outputDirectory.publish(partialName)
	}
	if exportErr != nil {
		if partialName == "" {
			return fmt.Errorf("validate migration source: %w", exportErr)
		}
		return fmt.Errorf("export migration bundle (final absent; partial %q retained at original directory path %s): %w", partialName, partialDisplayPath, exportErr)
	}
	return nil
}

func drainMigrationCheck(checkErrors <-chan error) error {
	if checkErrors == nil {
		return errors.New("bbolt integrity check returned no result channel")
	}
	var integrityErr error
	for err := range checkErrors {
		if err != nil {
			integrityErr = errors.Join(integrityErr, err)
		}
	}
	if integrityErr != nil {
		return fmt.Errorf("bbolt integrity check failed: %w", integrityErr)
	}
	return nil
}

func inspectMigrationSource(tx *bolt.Tx, kind MigrationKind) ([]migrationBucket, uint64, uint64, error) {
	specs := globalMigrationBuckets
	if kind == MigrationKindProject {
		specs = projectMigrationBuckets
	}
	known := make(map[string]migrationBucketSpec, len(specs))
	for _, spec := range specs {
		known[spec.name] = spec
	}

	var buckets []migrationBucket
	var totalRecords uint64
	var totalPayloadBytes uint64
	err := tx.ForEach(func(name []byte, bucket *bolt.Bucket) error {
		spec, ok := known[string(name)]
		if !ok {
			return fmt.Errorf("unknown top-level bucket %q", name)
		}
		if len(name) > migrationMaxBucketName {
			return fmt.Errorf("bucket name exceeds %d bytes", migrationMaxBucketName)
		}
		sequence := bucket.Sequence()
		if !spec.sequenced && sequence != 0 {
			return fmt.Errorf("non-sequenced bucket %q has sequence %d", name, sequence)
		}

		var records uint64
		var maxNumericKey uint64
		if err := bucket.ForEach(func(key, value []byte) error {
			if value == nil {
				return fmt.Errorf("nested bucket %q/%q is not supported", name, key)
			}
			if len(key) > migrationMaxKeyBytes {
				return fmt.Errorf("key in bucket %q exceeds %d bytes", name, migrationMaxKeyBytes)
			}
			if len(value) > migrationMaxValueBytes {
				return fmt.Errorf("value in bucket %q exceeds %d bytes", name, migrationMaxValueBytes)
			}
			var payloadErr error
			totalPayloadBytes, payloadErr = addMigrationPayload(totalPayloadBytes, uint64(len(key)), uint64(len(value)))
			if payloadErr != nil {
				return fmt.Errorf("bucket %q: %w", name, payloadErr)
			}
			if spec.name == "meta" && !bytes.Equal(key, keyMeta) {
				return fmt.Errorf("project meta bucket has unexpected key %q", key)
			}
			if spec.numericKey {
				if len(key) != 8 {
					return fmt.Errorf("numeric key in bucket %q must be 8 bytes", name)
				}
				id := binary.BigEndian.Uint64(key)
				if id == 0 {
					return fmt.Errorf("numeric key in bucket %q must be non-zero", name)
				}
				if id > maxNumericKey {
					maxNumericKey = id
				}
			}
			records++
			if records > migrationMaxRecords {
				return fmt.Errorf("bucket %q exceeds %d records", name, migrationMaxRecords)
			}
			return nil
		}); err != nil {
			return err
		}
		if spec.numericKey && maxNumericKey > sequence {
			return fmt.Errorf("bucket %q maximum id %d exceeds sequence %d", name, maxNumericKey, sequence)
		}
		if totalRecords > migrationMaxRecords-records {
			return fmt.Errorf("source exceeds %d total records", migrationMaxRecords)
		}
		totalRecords += records
		buckets = append(buckets, migrationBucket{spec: spec, sequence: sequence, records: records})
		return nil
	})
	if err != nil {
		return nil, 0, 0, err
	}
	if len(buckets) > migrationMaxBuckets {
		return nil, 0, 0, fmt.Errorf("source exceeds %d buckets", migrationMaxBuckets)
	}

	sourceFormat := uint64(0)
	if kind == MigrationKindProject {
		meta := tx.Bucket(bucketMeta)
		if meta == nil {
			return nil, 0, 0, errors.New("project source is missing meta bucket")
		}
		if meta.Stats().KeyN != 1 {
			return nil, 0, 0, errors.New("project meta bucket must contain exactly one record")
		}
		var decoded model.Meta
		if err := gobDecode(meta.Get(keyMeta), &decoded); err != nil {
			return nil, 0, 0, fmt.Errorf("decode project meta: %w", err)
		}
		sourceFormat = uint64(decoded.FormatVersion)
		if sourceFormat > uint64(CurrentFormat) {
			return nil, 0, 0, ErrFormatTooNew{Found: decoded.FormatVersion, Supported: CurrentFormat}
		}
	}

	present := make(map[string]bool, len(buckets))
	for _, bucket := range buckets {
		present[bucket.spec.name] = true
	}
	for _, spec := range specs {
		if spec.introduced <= sourceFormat && !present[spec.name] {
			return nil, 0, 0, fmt.Errorf("source format %d is missing required bucket %q", sourceFormat, spec.name)
		}
	}
	sort.Slice(buckets, func(i, j int) bool { return buckets[i].spec.name < buckets[j].spec.name })
	return buckets, sourceFormat, totalRecords, nil
}

func addMigrationPayload(current, keyBytes, valueBytes uint64) (uint64, error) {
	if keyBytes > ^uint64(0)-valueBytes || keyBytes+valueBytes > ^uint64(0)-migrationRecordOverhead {
		return current, fmt.Errorf("aggregate record payload exceeds %d bytes", migrationMaxPayloadBytes)
	}
	addition := keyBytes + valueBytes + migrationRecordOverhead
	if current > migrationMaxPayloadBytes || addition > migrationMaxPayloadBytes-current {
		return current, fmt.Errorf("aggregate record payload exceeds %d bytes", migrationMaxPayloadBytes)
	}
	return current + addition, nil
}

func writeMigrationBundle(output *os.File, kind MigrationKind, sourceFormat, totalRecords uint64, buckets []migrationBucket, tx *bolt.Tx) error {
	digest := sha256.New()
	writer := &migrationWriter{writer: io.MultiWriter(output, digest)}
	if _, err := writer.Write(migrationMagic[:]); err != nil {
		return err
	}
	for _, value := range []any{
		uint16(migrationBundleVersion), uint16(migrationBundleHeaderLen), uint8(kind), uint8(0),
		uint16(0), sourceFormat, uint32(len(buckets)), uint32(0), totalRecords,
	} {
		if err := binary.Write(writer, binary.BigEndian, value); err != nil {
			return err
		}
	}
	for _, bucket := range buckets {
		if _, err := writer.Write([]byte("BUKT")); err != nil {
			return err
		}
		for _, value := range []any{uint16(len(bucket.spec.name)), uint16(0), bucket.sequence, bucket.records} {
			if err := binary.Write(writer, binary.BigEndian, value); err != nil {
				return err
			}
		}
		if _, err := writer.Write([]byte(bucket.spec.name)); err != nil {
			return err
		}
		legacyBucket := tx.Bucket([]byte(bucket.spec.name))
		if err := legacyBucket.ForEach(func(key, value []byte) error {
			if err := binary.Write(writer, binary.BigEndian, uint64(len(key))); err != nil {
				return err
			}
			if err := binary.Write(writer, binary.BigEndian, uint64(len(value))); err != nil {
				return err
			}
			if _, err := writer.Write(key); err != nil {
				return err
			}
			_, err := writer.Write(value)
			return err
		}); err != nil {
			return err
		}
	}
	if writer.written > migrationMaxBundleBytes-40 {
		return fmt.Errorf("bundle exceeds %d bytes", migrationMaxBundleBytes)
	}
	checksum := digest.Sum(nil)
	trailer := make([]byte, 40)
	copy(trailer[:4], "HASH")
	binary.BigEndian.PutUint16(trailer[4:6], 1)
	binary.BigEndian.PutUint16(trailer[6:8], uint16(sha256.Size))
	copy(trailer[8:], checksum)
	if _, err := output.Write(trailer); err != nil {
		return err
	}
	return nil
}

type migrationWriter struct {
	writer  io.Writer
	written uint64
}

func (w *migrationWriter) Write(data []byte) (int, error) {
	if uint64(len(data)) > migrationMaxBundleBytes-w.written {
		return 0, fmt.Errorf("bundle exceeds %d bytes", migrationMaxBundleBytes)
	}
	n, err := w.writer.Write(data)
	w.written += uint64(n)
	return n, err
}
