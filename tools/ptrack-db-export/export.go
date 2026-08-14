package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

const (
	jsonStageFormat           = "ptrack-db-stage"
	jsonStageVersion          = "1"
	jsonStageMaxManifestBytes = 8 * 1024 * 1024
	jsonStageMaxDatabaseBytes = 768 * 1024 * 1024
	jsonStageMaxLineBytes     = 513 * 1024 * 1024
)

type jsonStageManifest struct {
	Format          string              `json:"format"`
	Version         string              `json:"version"`
	DatabaseCount   string              `json:"database_count"`
	QuarantineCount string              `json:"quarantine_count"`
	Registry        []jsonStageRegistry `json:"registry"`
	Databases       []jsonStageDatabase `json:"databases"`
}

type jsonStageRegistry struct {
	SourcePath    string `json:"source_path"`
	CanonicalRoot string `json:"canonical_root"`
}

type jsonStageDatabase struct {
	ID             string                  `json:"id"`
	Kind           string                  `json:"kind"`
	ProjectRoot    *string                 `json:"project_root"`
	SourcePath     string                  `json:"source_path"`
	SourceFormat   string                  `json:"source_format"`
	SourceIdentity jsonStageSourceIdentity `json:"source_identity"`
	Data           jsonStageArtifact       `json:"data"`
}

type jsonStageSourceIdentity struct {
	Device       string `json:"device"`
	Inode        string `json:"inode"`
	Size         string `json:"size"`
	MtimeSeconds string `json:"mtime_seconds"`
	MtimeNanos   string `json:"mtime_nanos"`
	SHA256       string `json:"sha256"`
}

type jsonStageArtifact struct {
	Path        string `json:"path"`
	SHA256      string `json:"sha256"`
	Bytes       string `json:"bytes"`
	RecordCount string `json:"record_count"`
	BucketCount string `json:"bucket_count"`
}

type heldJSONSource struct {
	id          string
	kind        MigrationKind
	projectRoot *string
	path        string
	db          *bolt.DB
	openedInfo  os.FileInfo
	identity    jsonStageSourceIdentity
	inspection  jsonDatabaseInspection
}

type jsonDatabaseInspection struct {
	sourceFormat    uint64
	recordCount     uint64
	bucketCount     uint64
	quarantineCount uint64
	retainedBytes   uint64
}

// ExportJSONStage freezes the global database and every registered project,
// validates them, and writes a create-only JSON stage. Source databases are
// opened directly and read-only; no application initialization or migration is
// run, and a manifest is published only after every artifact and source check.
func ExportJSONStage(home, output string) error {
	return exportJSONStage(home, output, nil)
}

func exportJSONStage(home, output string, afterFreeze func() error) (result error) {
	if err := migrationOutputSupported(); err != nil {
		return err
	}
	if !filepath.IsAbs(home) || filepath.Clean(home) != home {
		return errors.New("home path must be absolute and clean")
	}
	if !filepath.IsAbs(output) || filepath.Clean(output) != output {
		return errors.New("output path must be absolute and clean")
	}
	if !utf8.ValidString(home) {
		return errors.New("home path is not valid UTF-8")
	}
	if !utf8.ValidString(output) {
		return errors.New("output path is not valid UTF-8")
	}
	canonicalHome, err := filepath.EvalSymlinks(home)
	if err != nil {
		return fmt.Errorf("canonicalize ptrack home: %w", err)
	}
	canonicalHome = filepath.Clean(canonicalHome)
	if !utf8.ValidString(canonicalHome) {
		return errors.New("canonical ptrack home is not valid UTF-8")
	}
	canonicalOutputParent, err := filepath.EvalSymlinks(filepath.Dir(output))
	if err != nil {
		return fmt.Errorf("canonicalize output parent: %w", err)
	}
	if !utf8.ValidString(canonicalOutputParent) {
		return errors.New("canonical output parent is not valid UTF-8")
	}
	canonicalOutput := filepath.Join(filepath.Clean(canonicalOutputParent), filepath.Base(output))
	if jsonPathWithin(canonicalOutput, canonicalHome) || jsonPathWithin(canonicalHome, canonicalOutput) || canonicalOutput == canonicalHome {
		return errors.New("output path must be outside the ptrack home")
	}
	outputParent, err := pinJSONOutputParent(output)
	if err != nil {
		return err
	}
	defer outputParent.Close()

	global, err := openHeldJSONSource("global", MigrationKindGlobal, nil, filepath.Join(canonicalHome, "global.db"))
	if err != nil {
		return err
	}
	sources := []*heldJSONSource{global}
	defer func() {
		for index := len(sources) - 1; index >= 0; index-- {
			if closeErr := sources[index].db.Close(); result == nil && closeErr != nil {
				result = fmt.Errorf("close frozen source %q: %w", sources[index].path, closeErr)
			}
		}
	}()

	global.inspection, err = inspectJSONSource(global)
	if err != nil {
		return fmt.Errorf("validate global source: %w", err)
	}
	projectRoots, registry, err := registeredProjectRoots(global.db)
	if err != nil {
		return fmt.Errorf("read project registry: %w", err)
	}
	if len(projectRoots) > 9_999 {
		return errors.New("project registry exceeds the 9,999-project batch limit")
	}
	for index, root := range projectRoots {
		if !utf8.ValidString(root) {
			return errors.New("registered project path is not valid UTF-8")
		}
		rootCopy := root
		source, openErr := openHeldJSONSource(
			fmt.Sprintf("project-%06d", index+1),
			MigrationKindProject,
			&rootCopy,
			filepath.Join(root, ".ptrack", "ptrack.db"),
		)
		if openErr != nil {
			return openErr
		}
		sources = append(sources, source)
	}
	for _, source := range sources[1:] {
		source.inspection, err = inspectJSONSource(source)
		if err != nil {
			return fmt.Errorf("validate project source %q: %w", source.path, err)
		}
	}
	var batchRetained uint64
	var batchRecords uint64
	for _, source := range sources {
		if batchRetained > migrationMaxPayloadBytes-source.inspection.retainedBytes {
			return fmt.Errorf("batch source payload exceeds %d bytes", migrationMaxPayloadBytes)
		}
		batchRetained += source.inspection.retainedBytes
		if batchRecords > migrationMaxRecords-source.inspection.recordCount {
			return fmt.Errorf("batch source exceeds %d records", migrationMaxRecords)
		}
		batchRecords += source.inspection.recordCount
	}
	if afterFreeze != nil {
		if err := afterFreeze(); err != nil {
			return fmt.Errorf("after freezing sources: %w", err)
		}
	}

	if err := createPrivateExportDirectory(output); err != nil {
		if errors.Is(err, os.ErrExist) {
			return errors.New("output path already exists")
		}
		return fmt.Errorf("create private staging directory: %w", err)
	}
	if err := requireJSONOutputParent(output, outputParent); err != nil {
		return err
	}
	if err := os.Chmod(output, 0o700); err != nil {
		return fmt.Errorf("set staging directory permissions: %w", err)
	}
	if err := protectPrivatePath(output, true); err != nil {
		return fmt.Errorf("protect staging directory: %w", err)
	}
	stageInfo, err := os.Lstat(output)
	if err != nil || !stageInfo.IsDir() || stageInfo.Mode()&os.ModeSymlink != 0 {
		return errors.New("staging path is not the newly created private directory")
	}
	databasesPath := filepath.Join(output, "databases")
	if err := createPrivateExportDirectory(databasesPath); err != nil {
		return fmt.Errorf("create private databases staging directory: %w", err)
	}
	if err := os.Chmod(databasesPath, 0o700); err != nil {
		return fmt.Errorf("set databases staging permissions: %w", err)
	}
	if err := protectPrivatePath(databasesPath, true); err != nil {
		return fmt.Errorf("protect databases staging directory: %w", err)
	}
	databasesInfo, err := os.Lstat(databasesPath)
	if err != nil {
		return err
	}

	manifest := jsonStageManifest{
		Format:        jsonStageFormat,
		Version:       jsonStageVersion,
		DatabaseCount: strconv.Itoa(len(sources)),
		Registry:      registry,
		Databases:     make([]jsonStageDatabase, 0, len(sources)),
	}
	var totalQuarantine uint64
	for _, source := range sources {
		if err := requireSameJSONDirectory(databasesPath, databasesInfo); err != nil {
			return err
		}
		artifact, writeErr := writeJSONDatabase(databasesPath, source)
		if writeErr != nil {
			return fmt.Errorf("write JSON database %q: %w", source.id, writeErr)
		}
		manifest.Databases = append(manifest.Databases, jsonStageDatabase{
			ID:             source.id,
			Kind:           migrationKindName(source.kind),
			ProjectRoot:    source.projectRoot,
			SourcePath:     source.path,
			SourceFormat:   strconv.FormatUint(source.inspection.sourceFormat, 10),
			SourceIdentity: source.identity,
			Data:           artifact,
		})
		totalQuarantine += source.inspection.quarantineCount
	}
	manifest.QuarantineCount = strconv.FormatUint(totalQuarantine, 10)
	if err := syncDirectory(databasesPath); err != nil {
		return fmt.Errorf("sync database artifacts: %w", err)
	}
	if err := requireSameJSONDirectory(databasesPath, databasesInfo); err != nil {
		return err
	}
	for _, source := range sources {
		if err := verifyFrozenJSONSource(source); err != nil {
			return err
		}
	}
	currentStageInfo, err := os.Lstat(output)
	if err != nil || !os.SameFile(stageInfo, currentStageInfo) || currentStageInfo.Mode()&os.ModeSymlink != 0 {
		return errors.New("staging directory identity changed before manifest publication")
	}
	if err := writeJSONManifest(output, manifest); err != nil {
		return err
	}
	if err := requireSameJSONDirectory(databasesPath, databasesInfo); err != nil {
		return err
	}
	if err := requireJSONOutputParent(output, outputParent); err != nil {
		return err
	}
	currentStageInfo, err = os.Lstat(output)
	if err != nil || !os.SameFile(stageInfo, currentStageInfo) || currentStageInfo.Mode()&os.ModeSymlink != 0 {
		return errors.New("staging directory identity changed after manifest publication")
	}
	return nil
}

func jsonPathWithin(child, parent string) bool {
	relative, err := filepath.Rel(parent, child)
	return err == nil && relative != "." && !filepath.IsAbs(relative) && relative != ".." &&
		!strings.HasPrefix(relative, ".."+string(filepath.Separator))
}

func pinJSONOutputParent(output string) (*os.File, error) {
	parent := filepath.Dir(output)
	before, err := os.Lstat(parent)
	if err != nil {
		return nil, fmt.Errorf("inspect output parent: %w", err)
	}
	if before.Mode()&os.ModeSymlink != 0 || !before.IsDir() {
		return nil, errors.New("output parent must be an existing non-symlink directory")
	}
	if err := requirePrivateExportPath(parent, true); err != nil {
		return nil, fmt.Errorf("output parent is not private: %w", err)
	}
	directory, err := os.Open(parent)
	if err != nil {
		return nil, fmt.Errorf("open output parent: %w", err)
	}
	opened, err := directory.Stat()
	after, afterErr := os.Lstat(parent)
	if err != nil || afterErr != nil || !os.SameFile(before, opened) || !os.SameFile(opened, after) {
		_ = directory.Close()
		return nil, errors.New("output parent changed while it was opened")
	}
	return directory, nil
}

func requireJSONOutputParent(output string, directory *os.File) error {
	opened, err := directory.Stat()
	if err != nil {
		return err
	}
	resolved, err := os.Lstat(filepath.Dir(output))
	if err != nil || resolved.Mode()&os.ModeSymlink != 0 || !os.SameFile(opened, resolved) {
		return errors.New("output parent changed during export")
	}
	return nil
}

func requireSameJSONDirectory(path string, expected os.FileInfo) error {
	current, err := os.Lstat(path)
	if err != nil || current.Mode()&os.ModeSymlink != 0 || !current.IsDir() || !os.SameFile(expected, current) {
		return errors.New("staging databases directory changed during export")
	}
	return nil
}

func openHeldJSONSource(id string, kind MigrationKind, projectRoot *string, sourcePath string) (*heldJSONSource, error) {
	if !filepath.IsAbs(sourcePath) || filepath.Clean(sourcePath) != sourcePath {
		return nil, fmt.Errorf("source path %q must be absolute and clean", sourcePath)
	}
	preOpenInfo, err := os.Lstat(sourcePath)
	if err != nil {
		return nil, fmt.Errorf("inspect source %q: %w", sourcePath, err)
	}
	if preOpenInfo.Mode()&os.ModeSymlink != 0 || !preOpenInfo.Mode().IsRegular() {
		return nil, fmt.Errorf("source %q must be a non-symlink regular file", sourcePath)
	}
	source, err := openLegacyExportSource(sourcePath)
	if err != nil {
		return nil, fmt.Errorf("open source %q read-only: %w", sourcePath, err)
	}
	openedInfo, err := source.Stat()
	if err != nil {
		_ = source.Close()
		return nil, fmt.Errorf("inspect opened source %q: %w", sourcePath, err)
	}
	postOpenInfo, err := os.Lstat(sourcePath)
	if err != nil || !openedInfo.Mode().IsRegular() || !os.SameFile(preOpenInfo, openedInfo) ||
		!os.SameFile(preOpenInfo, postOpenInfo) || postOpenInfo.Mode()&os.ModeSymlink != 0 {
		_ = source.Close()
		return nil, fmt.Errorf("source path %q changed while it was opened", sourcePath)
	}
	openUsed := false
	db, err := bolt.Open(sourcePath, 0o600, &bolt.Options{
		ReadOnly: true,
		Timeout:  5 * time.Second,
		OpenFile: func(name string, flag int, _ os.FileMode) (*os.File, error) {
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
		return nil, fmt.Errorf("freeze source %q read-only: %w", sourcePath, err)
	}
	digest, err := hashOpenFile(source, openedInfo.Size())
	if err != nil {
		_ = db.Close()
		return nil, fmt.Errorf("hash source %q: %w", sourcePath, err)
	}
	device, inode, err := sourceDeviceInode(source, openedInfo)
	if err != nil {
		_ = db.Close()
		return nil, fmt.Errorf("read source identity %q: %w", sourcePath, err)
	}
	return &heldJSONSource{
		id:          id,
		kind:        kind,
		projectRoot: projectRoot,
		path:        sourcePath,
		db:          db,
		openedInfo:  openedInfo,
		identity: jsonStageSourceIdentity{
			Device:       strconv.FormatUint(device, 10),
			Inode:        strconv.FormatUint(inode, 10),
			Size:         strconv.FormatInt(openedInfo.Size(), 10),
			MtimeSeconds: strconv.FormatInt(openedInfo.ModTime().Unix(), 10),
			MtimeNanos:   strconv.Itoa(openedInfo.ModTime().Nanosecond()),
			SHA256:       hex.EncodeToString(digest[:]),
		},
	}, nil
}

func registeredProjectRoots(db *bolt.DB) ([]string, []jsonStageRegistry, error) {
	roots := make(map[string]struct{})
	var registry []jsonStageRegistry
	err := db.View(func(tx *bolt.Tx) error {
		bucket := tx.Bucket(bucketProjects)
		if bucket == nil {
			return errors.New("global source is missing projects bucket")
		}
		return bucket.ForEach(func(key, raw []byte) error {
			if raw == nil {
				return errors.New("nested project registry bucket is not supported")
			}
			var project model.ProjectRef
			if err := strictGobDecode(raw, &project); err != nil {
				return recordError("projects", key, err)
			}
			if _, err := encodeLegacyRecord("projects", key, raw); err != nil {
				return err
			}
			root, err := filepath.EvalSymlinks(project.Path)
			if err != nil {
				return fmt.Errorf("canonicalize registered project %q: %w", project.Path, err)
			}
			root = filepath.Clean(root)
			info, err := os.Stat(root)
			if err != nil || !info.IsDir() {
				return fmt.Errorf("registered project root %q is not a directory", root)
			}
			roots[root] = struct{}{}
			registry = append(registry, jsonStageRegistry{
				SourcePath:    project.Path,
				CanonicalRoot: root,
			})
			return nil
		})
	})
	if err != nil {
		return nil, nil, err
	}
	result := make([]string, 0, len(roots))
	for root := range roots {
		result = append(result, root)
	}
	sort.Strings(result)
	return result, registry, nil
}

func inspectJSONSource(source *heldJSONSource) (jsonDatabaseInspection, error) {
	var inspection jsonDatabaseInspection
	err := source.db.View(func(tx *bolt.Tx) error {
		if err := drainMigrationCheck(tx.Check()); err != nil {
			return err
		}
		var err error
		inspection, err = inspectJSONTransaction(tx, source.kind)
		return err
	})
	return inspection, err
}

func inspectJSONTransaction(tx *bolt.Tx, kind MigrationKind) (jsonDatabaseInspection, error) {
	specs := globalMigrationBuckets
	if kind == MigrationKindProject {
		specs = projectMigrationBuckets
	}
	known := make(map[string]migrationBucketSpec, len(specs))
	for _, spec := range specs {
		known[spec.name] = spec
	}
	if err := tx.ForEach(func(name []byte, _ *bolt.Bucket) error {
		if _, ok := known[string(name)]; !ok {
			return fmt.Errorf("unknown top-level bucket %q", name)
		}
		return nil
	}); err != nil {
		return jsonDatabaseInspection{}, err
	}

	project := newJSONProjectValidation()
	result := jsonDatabaseInspection{bucketCount: uint64(len(specs))}
	present := make(map[string]bool, len(specs))
	for _, spec := range specs {
		bucket := tx.Bucket([]byte(spec.name))
		if bucket == nil {
			continue
		}
		present[spec.name] = true
		if !spec.sequenced && bucket.Sequence() != 0 {
			return jsonDatabaseInspection{}, fmt.Errorf("non-sequenced bucket %q has sequence %d", spec.name, bucket.Sequence())
		}
		var bucketRecords uint64
		var maxNumericKey uint64
		err := bucket.ForEach(func(key, raw []byte) error {
			if raw == nil {
				return fmt.Errorf("nested bucket %q/%x is not supported", spec.name, key)
			}
			if len(key) > migrationMaxKeyBytes || len(raw) > migrationMaxValueBytes {
				return fmt.Errorf("record in bucket %q exceeds fixed key/value limits", spec.name)
			}
			if spec.name == "meta" && !bytes.Equal(key, keyMeta) {
				return fmt.Errorf("project meta bucket has unexpected key %q", key)
			}
			if spec.numericKey {
				if len(key) != 8 {
					return fmt.Errorf("numeric key in bucket %q must be 8 bytes", spec.name)
				}
				id := binary.BigEndian.Uint64(key)
				if id == 0 {
					return fmt.Errorf("numeric key in bucket %q must be nonzero", spec.name)
				}
				if id > maxNumericKey {
					maxNumericKey = id
				}
			}
			valid, retainedLength, err := validateJSONLegacyRecord(spec.name, key, raw)
			if err != nil {
				return err
			}
			var limitErr error
			overhead := uint64(migrationRecordOverhead)
			if !valid {
				overhead = uint64(6 + 41)
			}
			addition := uint64(len(key)) + retainedLength + overhead
			if result.retainedBytes > migrationMaxPayloadBytes || addition > migrationMaxPayloadBytes-result.retainedBytes {
				limitErr = fmt.Errorf("aggregate record payload exceeds %d bytes", migrationMaxPayloadBytes)
			} else {
				result.retainedBytes += addition
			}
			if limitErr != nil {
				return fmt.Errorf("bucket %q: %w", spec.name, limitErr)
			}
			if !valid {
				result.quarantineCount++
			} else if kind == MigrationKindProject {
				if err := project.add(spec.name, raw); err != nil {
					return fmt.Errorf("validate project record in bucket %q: %w", spec.name, err)
				}
			}
			bucketRecords++
			result.recordCount++
			if bucketRecords > migrationMaxRecords || result.recordCount > migrationMaxRecords {
				return fmt.Errorf("source exceeds %d records", migrationMaxRecords)
			}
			return nil
		})
		if err != nil {
			return jsonDatabaseInspection{}, err
		}
		if spec.numericKey && maxNumericKey > bucket.Sequence() {
			return jsonDatabaseInspection{}, fmt.Errorf("bucket %q maximum id %d exceeds sequence %d", spec.name, maxNumericKey, bucket.Sequence())
		}
		if kind == MigrationKindProject && spec.name == "memory_writebacks" {
			project.memorySequence = bucket.Sequence()
		}
	}
	if kind == MigrationKindProject {
		meta := tx.Bucket(bucketMeta)
		if meta == nil || meta.Stats().KeyN != 1 || !project.metaPresent {
			return jsonDatabaseInspection{}, errors.New("project meta bucket must contain exactly the meta record")
		}
		result.sourceFormat = uint64(project.metaFormat)
		if result.sourceFormat > uint64(CurrentFormat) {
			return jsonDatabaseInspection{}, ErrFormatTooNew{Found: project.metaFormat, Supported: CurrentFormat}
		}
		if err := project.validate(); err != nil {
			return jsonDatabaseInspection{}, fmt.Errorf("project reference validation failed: %w", err)
		}
	}
	for _, spec := range specs {
		if spec.introduced <= result.sourceFormat && !present[spec.name] {
			return jsonDatabaseInspection{}, fmt.Errorf("source format %d is missing required bucket %q", result.sourceFormat, spec.name)
		}
	}
	return result, nil
}

type jsonProjectValidation struct {
	metaPresent     bool
	metaFormat      uint
	activePlan      uint64
	plans           map[uint64]uint64
	tasks           map[uint64]uint64
	notes           map[uint64]model.MemoryKind
	milestones      map[uint64]struct{}
	issues          map[uint64]uint64
	memoryWriteback []memoryWritebackRecord
	memorySequence  uint64
}

func newJSONProjectValidation() *jsonProjectValidation {
	return &jsonProjectValidation{
		plans: make(map[uint64]uint64), tasks: make(map[uint64]uint64),
		notes: make(map[uint64]model.MemoryKind), milestones: make(map[uint64]struct{}),
		issues: make(map[uint64]uint64),
	}
}

func (project *jsonProjectValidation) add(collection string, raw []byte) error {
	switch collection {
	case "meta":
		var value model.Meta
		if err := strictGobDecode(raw, &value); err != nil {
			return err
		}
		project.metaPresent, project.metaFormat, project.activePlan = true, value.FormatVersion, value.ActivePlan
	case "plans":
		var value model.Plan
		if err := strictGobDecode(raw, &value); err != nil {
			return err
		}
		project.plans[value.ID] = value.MilestoneID
	case "tasks":
		var value model.Task
		if err := strictGobDecode(raw, &value); err != nil {
			return err
		}
		project.tasks[value.ID] = value.PlanID
	case "notes":
		var value model.Note
		if err := strictGobDecode(raw, &value); err != nil {
			return err
		}
		project.notes[value.ID] = value.Kind
	case "milestones":
		var value model.Milestone
		if err := strictGobDecode(raw, &value); err != nil {
			return err
		}
		project.milestones[value.ID] = struct{}{}
	case "issues":
		var value model.Issue
		if err := strictGobDecode(raw, &value); err != nil {
			return err
		}
		project.issues[value.ID] = value.TaskID
	case "memory_writebacks":
		var value memoryWritebackRecord
		if err := strictGobDecode(raw, &value); err != nil {
			return err
		}
		project.memoryWriteback = append(project.memoryWriteback, value)
	}
	return nil
}

func (project *jsonProjectValidation) validate() error {
	if project.activePlan != 0 {
		if _, ok := project.plans[project.activePlan]; !ok {
			return fmt.Errorf("active plan %d does not exist", project.activePlan)
		}
	}
	for id, milestoneID := range project.plans {
		if milestoneID != 0 {
			if _, ok := project.milestones[milestoneID]; !ok {
				return fmt.Errorf("plan %d references missing milestone %d", id, milestoneID)
			}
		}
	}
	for id, planID := range project.tasks {
		if _, ok := project.plans[planID]; !ok {
			return fmt.Errorf("task %d references missing plan %d", id, planID)
		}
	}
	for id, taskID := range project.issues {
		if taskID != 0 {
			if _, ok := project.tasks[taskID]; !ok {
				return fmt.Errorf("issue %d references missing task %d", id, taskID)
			}
		}
	}
	seenSequences := make(map[uint64]struct{}, len(project.memoryWriteback))
	seenNotes := make(map[uint64]struct{}, len(project.memoryWriteback))
	for _, receipt := range project.memoryWriteback {
		if receipt.Sequence > project.memorySequence {
			return fmt.Errorf("memory write-back sequence %d exceeds bucket sequence %d", receipt.Sequence, project.memorySequence)
		}
		if _, duplicate := seenSequences[receipt.Sequence]; duplicate {
			return fmt.Errorf("memory write-back sequence %d is duplicated", receipt.Sequence)
		}
		seenSequences[receipt.Sequence] = struct{}{}
		if receipt.Kind == model.MemorySummary {
			continue
		}
		if project.notes[receipt.NoteID] != receipt.Kind {
			return fmt.Errorf("memory write-back note %d is missing or has the wrong kind", receipt.NoteID)
		}
		if _, duplicate := seenNotes[receipt.NoteID]; duplicate {
			return fmt.Errorf("memory write-back note %d is referenced more than once", receipt.NoteID)
		}
		seenNotes[receipt.NoteID] = struct{}{}
	}
	return nil
}

// validateJSONLegacyRecord returns false only for capability records which are
// intentionally retained in quarantine. Every other invalid record aborts the
// stage before a manifest can be written.
func validateJSONLegacyRecord(collection string, key, raw []byte) (bool, uint64, error) {
	if collection == "config" {
		if len(key) == 0 {
			return false, 0, errors.New("global config key must be nonempty")
		}
		return true, uint64(len(raw)), nil
	}
	if collection == "backups" {
		if err := validateJSONBackup(key, raw); err != nil {
			return false, 0, recordError(collection, key, err)
		}
		return true, uint64(len(raw)), nil
	}
	encoded, err := encodeLegacyRecord(collection, key, raw)
	if err == nil {
		return true, uint64(len(encoded.Payload)), nil
	}
	if collection == "capabilities" || collection == "capability_audits" {
		return false, uint64(len(raw)), nil
	}
	if collection == "meta" {
		var decoded model.Meta
		if decodeErr := strictGobDecode(raw, &decoded); decodeErr == nil && decoded.FormatVersion > CurrentFormat {
			return false, 0, ErrFormatTooNew{Found: decoded.FormatVersion, Supported: CurrentFormat}
		}
	}
	return false, 0, err
}

func validateJSONBackup(key, raw []byte) error {
	if !utf8.Valid(key) || !utf8.Valid(raw) || len(key) == 0 || strings.HasPrefix(string(key), "+") {
		return errors.New("backup key/value must be valid UTF-8 with a canonical nonnegative decimal key")
	}
	value, err := strconv.ParseInt(string(key), 10, 64)
	if err != nil || value < 0 || strconv.FormatInt(value, 10) != string(key) {
		return errors.New("backup key must be a canonical nonnegative decimal")
	}
	parts := strings.Split(string(raw), "\t")
	if len(parts) != 2 || parts[0] == "" || parts[1] == "" {
		return errors.New("backup value must contain exactly two nonempty tab-separated fields")
	}
	return nil
}

func writeJSONDatabase(databasesPath string, source *heldJSONSource) (jsonStageArtifact, error) {
	relative := filepath.ToSlash(filepath.Join("databases", source.id+".jsonl"))
	path := filepath.Join(databasesPath, source.id+".jsonl")
	file, err := createPrivateExportFile(path)
	if err != nil {
		return jsonStageArtifact{}, err
	}
	if err := file.Chmod(0o600); err != nil {
		_ = file.Close()
		return jsonStageArtifact{}, err
	}
	if err := protectPrivatePath(path, false); err != nil {
		_ = file.Close()
		return jsonStageArtifact{}, err
	}
	openedInfo, err := file.Stat()
	if err != nil {
		_ = file.Close()
		return jsonStageArtifact{}, err
	}
	hash := sha256.New()
	writer := &countingWriter{writer: io.MultiWriter(file, hash), maximum: jsonStageMaxDatabaseBytes}
	writeErr := source.db.View(func(tx *bolt.Tx) error {
		return writeJSONTransaction(writer, tx, source)
	})
	if writeErr == nil {
		writeErr = file.Sync()
	}
	if closeErr := file.Close(); writeErr == nil && closeErr != nil {
		writeErr = closeErr
	}
	if writeErr != nil {
		return jsonStageArtifact{}, writeErr
	}
	resolvedInfo, err := os.Lstat(path)
	if err != nil || resolvedInfo.Mode()&os.ModeSymlink != 0 || !os.SameFile(openedInfo, resolvedInfo) {
		return jsonStageArtifact{}, errors.New("database artifact path changed while it was written")
	}
	return jsonStageArtifact{
		Path:        relative,
		SHA256:      hex.EncodeToString(hash.Sum(nil)),
		Bytes:       strconv.FormatUint(writer.written, 10),
		RecordCount: strconv.FormatUint(source.inspection.recordCount, 10),
		BucketCount: strconv.FormatUint(source.inspection.bucketCount, 10),
	}, nil
}

type countingWriter struct {
	writer  io.Writer
	written uint64
	maximum uint64
}

func (writer *countingWriter) Write(value []byte) (int, error) {
	if uint64(len(value)) > writer.maximum-writer.written {
		return 0, fmt.Errorf("database JSONL exceeds %d bytes", writer.maximum)
	}
	count, err := writer.writer.Write(value)
	writer.written += uint64(count)
	return count, err
}

type jsonHeaderLine struct {
	Type            string `json:"type"`
	Schema          string `json:"schema"`
	DatabaseID      string `json:"database_id"`
	Kind            string `json:"kind"`
	SourceFormat    string `json:"source_format"`
	BucketCount     string `json:"bucket_count"`
	RecordCount     string `json:"record_count"`
	QuarantineCount string `json:"quarantine_count"`
}

type jsonBucketLine struct {
	Type        string  `json:"type"`
	Name        string  `json:"name"`
	Present     bool    `json:"present"`
	Sequence    *string `json:"sequence"`
	RecordCount string  `json:"record_count"`
}

type jsonRecordLine struct {
	Type              string       `json:"type"`
	Bucket            string       `json:"bucket"`
	Key               jsonStageKey `json:"key"`
	Model             string       `json:"model"`
	ModelVersion      string       `json:"model_version"`
	SourceValueSHA256 string       `json:"source_value_sha256"`
	Value             any          `json:"value"`
}

type jsonQuarantineLine struct {
	Type              string       `json:"type"`
	Bucket            string       `json:"bucket"`
	Key               jsonStageKey `json:"key"`
	Reason            string       `json:"reason"`
	LegacyCodec       string       `json:"legacy_codec"`
	SourceValueSHA256 string       `json:"source_value_sha256"`
	LegacyValueHex    string       `json:"legacy_value_hex"`
}

type jsonStageKey struct {
	Encoding string `json:"encoding"`
	Value    string `json:"value"`
}

func writeJSONTransaction(writer io.Writer, tx *bolt.Tx, source *heldJSONSource) error {
	if err := writeJSONLine(writer, jsonHeaderLine{
		Type:            "database",
		Schema:          jsonStageVersion,
		DatabaseID:      source.id,
		Kind:            migrationKindName(source.kind),
		SourceFormat:    strconv.FormatUint(source.inspection.sourceFormat, 10),
		BucketCount:     strconv.FormatUint(source.inspection.bucketCount, 10),
		RecordCount:     strconv.FormatUint(source.inspection.recordCount, 10),
		QuarantineCount: strconv.FormatUint(source.inspection.quarantineCount, 10),
	}); err != nil {
		return err
	}
	specs := globalMigrationBuckets
	if source.kind == MigrationKindProject {
		specs = projectMigrationBuckets
	}
	var writtenRecords uint64
	var writtenQuarantine uint64
	for _, spec := range specs {
		bucket := tx.Bucket([]byte(spec.name))
		present := bucket != nil
		var sequence *string
		var recordCount uint64
		if present {
			recordCount = uint64(bucket.Stats().KeyN)
			if spec.sequenced {
				value := strconv.FormatUint(bucket.Sequence(), 10)
				sequence = &value
			}
		}
		if err := writeJSONLine(writer, jsonBucketLine{
			Type: "bucket", Name: spec.name, Present: present, Sequence: sequence,
			RecordCount: strconv.FormatUint(recordCount, 10),
		}); err != nil {
			return err
		}
		if !present {
			continue
		}
		if err := bucket.ForEach(func(key, raw []byte) error {
			valid, _, err := validateJSONLegacyRecord(spec.name, key, raw)
			if err != nil {
				return err
			}
			encodedKey := jsonKey(spec, key)
			sourceDigest := sha256.Sum256(raw)
			if !valid {
				reason := "invalid_capability"
				if spec.name == "capability_audits" {
					reason = "invalid_capability_audit"
				}
				writtenRecords++
				writtenQuarantine++
				return writeJSONLine(writer, jsonQuarantineLine{
					Type: "quarantine", Bucket: spec.name, Key: encodedKey, Reason: reason,
					LegacyCodec: "go-gob", SourceValueSHA256: hex.EncodeToString(sourceDigest[:]),
					LegacyValueHex: hex.EncodeToString(raw),
				})
			}
			modelName, value, err := jsonRecordValue(spec.name, key, raw)
			if err != nil {
				return err
			}
			modelVersion := "1"
			if modelName == "raw" {
				modelVersion = "0"
			}
			writtenRecords++
			return writeJSONLine(writer, jsonRecordLine{
				Type: "record", Bucket: spec.name, Key: encodedKey, Model: modelName,
				ModelVersion: modelVersion, SourceValueSHA256: hex.EncodeToString(sourceDigest[:]), Value: value,
			})
		}); err != nil {
			return err
		}
	}
	if writtenRecords != source.inspection.recordCount || writtenQuarantine != source.inspection.quarantineCount {
		return errors.New("source record counts changed between validation and export")
	}
	return nil
}

func jsonKey(spec migrationBucketSpec, key []byte) jsonStageKey {
	if spec.name == "meta" {
		return jsonStageKey{Encoding: "singleton", Value: "meta"}
	}
	if spec.numericKey {
		return jsonStageKey{Encoding: "u64", Value: strconv.FormatUint(binary.BigEndian.Uint64(key), 10)}
	}
	return jsonStageKey{Encoding: "hex", Value: hex.EncodeToString(key)}
}

type jsonTimestamp struct {
	State            string `json:"state"`
	UnixSeconds      string `json:"unix_seconds,omitempty"`
	Nanoseconds      string `json:"nanoseconds,omitempty"`
	UTCOffsetSeconds string `json:"utc_offset_seconds,omitempty"`
}

func jsonTime(value time.Time) jsonTimestamp {
	if value.IsZero() {
		return jsonTimestamp{State: "zero"}
	}
	_, offset := value.Zone()
	return jsonTimestamp{
		State: "fixed", UnixSeconds: strconv.FormatInt(value.Unix(), 10),
		Nanoseconds: strconv.Itoa(value.Nanosecond()), UTCOffsetSeconds: strconv.Itoa(offset),
	}
}

type jsonMetaValue struct {
	Goal             string        `json:"goal"`
	Summary          string        `json:"summary"`
	ActivePlan       string        `json:"active_plan"`
	CreatedAt        jsonTimestamp `json:"created_at"`
	UpdatedAt        jsonTimestamp `json:"updated_at"`
	FormatVersion    string        `json:"format_version"`
	LastWriteVersion string        `json:"last_write_version"`
}

type jsonPlanValue struct {
	ID          string        `json:"id"`
	Title       string        `json:"title"`
	Status      string        `json:"status"`
	MilestoneID string        `json:"milestone_id"`
	Order       string        `json:"order"`
	CreatedAt   jsonTimestamp `json:"created_at"`
	UpdatedAt   jsonTimestamp `json:"updated_at"`
}

type jsonTaskValue struct {
	ID        string        `json:"id"`
	PlanID    string        `json:"plan_id"`
	Title     string        `json:"title"`
	Status    string        `json:"status"`
	Order     string        `json:"order"`
	CreatedAt jsonTimestamp `json:"created_at"`
	UpdatedAt jsonTimestamp `json:"updated_at"`
}

type jsonNoteValue struct {
	ID        string        `json:"id"`
	Target    string        `json:"target"`
	TargetID  string        `json:"target_id"`
	Kind      string        `json:"kind"`
	Body      string        `json:"body"`
	CreatedAt jsonTimestamp `json:"created_at"`
}

type jsonMilestoneValue struct {
	ID        string        `json:"id"`
	Title     string        `json:"title"`
	Status    string        `json:"status"`
	Due       jsonTimestamp `json:"due"`
	Order     string        `json:"order"`
	CreatedAt jsonTimestamp `json:"created_at"`
	UpdatedAt jsonTimestamp `json:"updated_at"`
}

type jsonIssueValue struct {
	ID        string        `json:"id"`
	Title     string        `json:"title"`
	Body      string        `json:"body"`
	Status    string        `json:"status"`
	Severity  string        `json:"severity"`
	TaskID    string        `json:"task_id"`
	CreatedAt jsonTimestamp `json:"created_at"`
	UpdatedAt jsonTimestamp `json:"updated_at"`
}

type jsonCommitValue struct {
	ID        string        `json:"id"`
	SHA       string        `json:"sha"`
	Subject   string        `json:"subject"`
	PlanID    string        `json:"plan_id"`
	TaskID    string        `json:"task_id"`
	CreatedAt jsonTimestamp `json:"created_at"`
}

type jsonCapabilityLimits struct {
	TimeoutSeconds   string `json:"timeout_seconds"`
	MaxRequestBytes  string `json:"max_request_bytes"`
	MaxResponseBytes string `json:"max_response_bytes"`
	MaxOutputBytes   string `json:"max_output_bytes"`
	MaxRedirects     string `json:"max_redirects"`
	MaxConcurrent    string `json:"max_concurrent"`
}

type jsonCapabilityAuditPolicy struct {
	Enabled    bool   `json:"enabled"`
	RetainLast string `json:"retain_last"`
}

type jsonHTTPScope struct {
	BaseURL      string   `json:"base_url"`
	Methods      []string `json:"methods"`
	PathPrefixes []string `json:"path_prefixes"`
}

type jsonGitScope struct {
	RemoteName      string   `json:"remote_name"`
	RemoteURL       string   `json:"remote_url"`
	Operations      []string `json:"operations"`
	Branches        []string `json:"branches"`
	Refspecs        []string `json:"refspecs"`
	AllowTags       bool     `json:"allow_tags"`
	AllowForcePush  bool     `json:"allow_force_push"`
	AllowDeleteRefs bool     `json:"allow_delete_refs"`
}

type jsonSSHScope struct {
	Alias                 string   `json:"alias"`
	Host                  string   `json:"host"`
	Port                  string   `json:"port"`
	User                  string   `json:"user"`
	HostKey               string   `json:"host_key"`
	AllowGit              bool     `json:"allow_git"`
	RemoteCommands        []string `json:"remote_commands"`
	AllowUpload           bool     `json:"allow_upload"`
	AllowDownload         bool     `json:"allow_download"`
	UploadRoots           []string `json:"upload_roots"`
	DownloadRoots         []string `json:"download_roots"`
	UploadRemoteRoots     []string `json:"upload_remote_roots"`
	DownloadRemoteRoots   []string `json:"download_remote_roots"`
	AllowInteractiveShell bool     `json:"allow_interactive_shell"`
	LocalForwardTargets   []string `json:"local_forward_targets"`
	RemoteForwardTargets  []string `json:"remote_forward_targets"`
}

type jsonCapabilityValue struct {
	ID                      string                    `json:"id"`
	ModelVersion            string                    `json:"model_version"`
	Revision                string                    `json:"revision"`
	Name                    string                    `json:"name"`
	Kind                    string                    `json:"kind"`
	AgentProfile            string                    `json:"agent_profile"`
	Enabled                 bool                      `json:"enabled"`
	ApprovalDurationSeconds string                    `json:"approval_duration_seconds"`
	ApprovedAt              jsonTimestamp             `json:"approved_at"`
	ExpiresAt               jsonTimestamp             `json:"expires_at"`
	ScopeDigest             string                    `json:"scope_digest"`
	Limits                  jsonCapabilityLimits      `json:"limits"`
	Audit                   jsonCapabilityAuditPolicy `json:"audit"`
	HTTP                    *jsonHTTPScope            `json:"http"`
	Git                     *jsonGitScope             `json:"git"`
	SSH                     *jsonSSHScope             `json:"ssh"`
	CreatedAt               jsonTimestamp             `json:"created_at"`
	UpdatedAt               jsonTimestamp             `json:"updated_at"`
	MigrationDisposition    string                    `json:"migration_disposition"`
}

type jsonCapabilityAuditValue struct {
	ID             string        `json:"id"`
	CapabilityID   string        `json:"capability_id"`
	AgentProfile   string        `json:"agent_profile"`
	Kind           string        `json:"kind"`
	Operation      string        `json:"operation"`
	Target         string        `json:"target"`
	Success        bool          `json:"success"`
	ErrorClass     string        `json:"error_class"`
	DurationMillis string        `json:"duration_millis"`
	RequestBytes   string        `json:"request_bytes"`
	ResponseBytes  string        `json:"response_bytes"`
	Redirects      string        `json:"redirects"`
	CreatedAt      jsonTimestamp `json:"created_at"`
}

type jsonMemoryWritebackValue struct {
	DigestSHA256 string `json:"digest_sha256"`
	Sequence     string `json:"sequence"`
	Kind         string `json:"kind"`
	NoteID       string `json:"note_id"`
}

type jsonProjectRefValue struct {
	Name     string        `json:"name"`
	Path     string        `json:"path"`
	LastSeen jsonTimestamp `json:"last_seen"`
}

type jsonRawValue struct {
	Encoding string `json:"encoding"`
	Bytes    string `json:"bytes"`
}

func jsonRecordValue(collection string, key, raw []byte) (string, any, error) {
	switch collection {
	case "config", "backups":
		return "raw", jsonRawValue{Encoding: "hex", Bytes: hex.EncodeToString(raw)}, nil
	case "meta":
		var value model.Meta
		if err := strictGobDecode(raw, &value); err != nil {
			return "", nil, err
		}
		return "meta", jsonMetaValue{value.Goal, value.Summary, u64s(value.ActivePlan), jsonTime(value.CreatedAt), jsonTime(value.UpdatedAt), u64s(uint64(value.FormatVersion)), value.LastWriteVersion}, nil
	case "plans":
		var value model.Plan
		if err := strictGobDecode(raw, &value); err != nil {
			return "", nil, err
		}
		return "plan", jsonPlanValue{u64s(value.ID), value.Title, string(value.Status), u64s(value.MilestoneID), i64s(int64(value.Order)), jsonTime(value.CreatedAt), jsonTime(value.UpdatedAt)}, nil
	case "tasks":
		var value model.Task
		if err := strictGobDecode(raw, &value); err != nil {
			return "", nil, err
		}
		return "task", jsonTaskValue{u64s(value.ID), u64s(value.PlanID), value.Title, string(value.Status), i64s(int64(value.Order)), jsonTime(value.CreatedAt), jsonTime(value.UpdatedAt)}, nil
	case "notes":
		var value model.Note
		if err := strictGobDecode(raw, &value); err != nil {
			return "", nil, err
		}
		kind := string(value.Kind)
		if kind == "" {
			kind = "legacy"
		}
		return "note", jsonNoteValue{u64s(value.ID), string(value.Target), u64s(value.TargetID), kind, value.Body, jsonTime(value.CreatedAt)}, nil
	case "milestones":
		var value model.Milestone
		if err := strictGobDecode(raw, &value); err != nil {
			return "", nil, err
		}
		return "milestone", jsonMilestoneValue{u64s(value.ID), value.Title, string(value.Status), jsonTime(value.Due), i64s(int64(value.Order)), jsonTime(value.CreatedAt), jsonTime(value.UpdatedAt)}, nil
	case "issues":
		var value model.Issue
		if err := strictGobDecode(raw, &value); err != nil {
			return "", nil, err
		}
		return "issue", jsonIssueValue{u64s(value.ID), value.Title, value.Body, string(value.Status), string(value.Severity), u64s(value.TaskID), jsonTime(value.CreatedAt), jsonTime(value.UpdatedAt)}, nil
	case "commits":
		var value model.Commit
		if err := strictGobDecode(raw, &value); err != nil {
			return "", nil, err
		}
		return "commit", jsonCommitValue{u64s(value.ID), value.SHA, value.Subject, u64s(value.PlanID), u64s(value.TaskID), jsonTime(value.CreatedAt)}, nil
	case "capabilities":
		var value model.Capability
		if err := strictGobDecode(raw, &value); err != nil {
			return "", nil, err
		}
		return "capability", jsonCapability(value), nil
	case "capability_audits":
		var value model.CapabilityAudit
		if err := strictGobDecode(raw, &value); err != nil {
			return "", nil, err
		}
		return "capability_audit", jsonCapabilityAuditValue{
			u64s(value.ID), u64s(value.CapabilityID), value.AgentProfile, string(value.Kind), value.Operation, value.Target,
			value.Success, value.ErrorClass, i64s(value.DurationMillis), i64s(value.RequestBytes), i64s(value.ResponseBytes), i64s(int64(value.Redirects)), jsonTime(value.CreatedAt),
		}, nil
	case "memory_writebacks":
		var value memoryWritebackRecord
		if err := strictGobDecode(raw, &value); err != nil {
			return "", nil, err
		}
		return "memory_writeback", jsonMemoryWritebackValue{hex.EncodeToString(value.Digest[:]), u64s(value.Sequence), string(value.Kind), u64s(value.NoteID)}, nil
	case "projects":
		var value model.ProjectRef
		if err := strictGobDecode(raw, &value); err != nil {
			return "", nil, err
		}
		return "project_ref", jsonProjectRefValue{value.Name, value.Path, jsonTime(value.LastSeen)}, nil
	default:
		return "", nil, fmt.Errorf("unsupported JSON collection %q for key %x", collection, key)
	}
}

func jsonCapability(value model.Capability) jsonCapabilityValue {
	result := jsonCapabilityValue{
		ID: u64s(value.ID), ModelVersion: u64s(uint64(value.ModelVersion)), Revision: u64s(value.Revision),
		Name: value.Name, Kind: string(value.Kind), AgentProfile: value.AgentProfile, Enabled: value.Enabled,
		ApprovalDurationSeconds: i64s(value.ApprovalDurationSeconds), ApprovedAt: jsonTime(value.ApprovedAt),
		ExpiresAt: jsonTime(value.ExpiresAt), ScopeDigest: value.ScopeDigest,
		Limits:    jsonCapabilityLimits{i64s(int64(value.Limits.TimeoutSeconds)), i64s(value.Limits.MaxRequestBytes), i64s(value.Limits.MaxResponseBytes), i64s(value.Limits.MaxOutputBytes), i64s(int64(value.Limits.MaxRedirects)), i64s(int64(value.Limits.MaxConcurrent))},
		Audit:     jsonCapabilityAuditPolicy{value.Audit.Enabled, i64s(int64(value.Audit.RetainLast))},
		CreatedAt: jsonTime(value.CreatedAt), UpdatedAt: jsonTime(value.UpdatedAt), MigrationDisposition: "force_reapproval",
	}
	if value.HTTP != nil {
		result.HTTP = &jsonHTTPScope{value.HTTP.BaseURL, jsonStrings(value.HTTP.Methods), jsonStrings(value.HTTP.PathPrefixes)}
	}
	if value.Git != nil {
		result.Git = &jsonGitScope{value.Git.RemoteName, value.Git.RemoteURL, jsonStrings(value.Git.Operations), jsonStrings(value.Git.Branches), jsonStrings(value.Git.Refspecs), value.Git.AllowTags, value.Git.AllowForcePush, value.Git.AllowDeleteRefs}
	}
	if value.SSH != nil {
		result.SSH = &jsonSSHScope{
			value.SSH.Alias, value.SSH.Host, u64s(uint64(value.SSH.Port)), value.SSH.User, value.SSH.HostKey,
			value.SSH.AllowGit, jsonStrings(value.SSH.RemoteCommands), value.SSH.AllowUpload, value.SSH.AllowDownload,
			jsonStrings(value.SSH.UploadRoots), jsonStrings(value.SSH.DownloadRoots), jsonStrings(value.SSH.UploadRemoteRoots), jsonStrings(value.SSH.DownloadRemoteRoots),
			value.SSH.AllowInteractiveShell, jsonStrings(value.SSH.LocalForwardTargets), jsonStrings(value.SSH.RemoteForwardTargets),
		}
	}
	return result
}

func jsonStrings(values []string) []string {
	if values == nil {
		return []string{}
	}
	return values
}

func u64s(value uint64) string { return strconv.FormatUint(value, 10) }
func i64s(value int64) string  { return strconv.FormatInt(value, 10) }

func writeJSONLine(writer io.Writer, value any) error {
	if lineSizeUpperBound(value) > jsonStageMaxLineBytes {
		return fmt.Errorf("JSONL line exceeds %d bytes", jsonStageMaxLineBytes)
	}
	encoded, err := json.Marshal(value)
	if err != nil {
		return err
	}
	encoded = append(encoded, '\n')
	if len(encoded) > jsonStageMaxLineBytes {
		return fmt.Errorf("JSONL line exceeds %d bytes", jsonStageMaxLineBytes)
	}
	_, err = writer.Write(encoded)
	return err
}

func lineSizeUpperBound(value any) int {
	const structuralAllowance = 64 * 1024
	bound := escapedJSONValueUpperBound(reflect.ValueOf(value))
	if bound > int(^uint(0)>>1)-structuralAllowance {
		return int(^uint(0) >> 1)
	}
	return structuralAllowance + bound
}

func escapedJSONValueUpperBound(value reflect.Value) int {
	if !value.IsValid() {
		return 4
	}
	if value.Kind() == reflect.Pointer || value.Kind() == reflect.Interface {
		if value.IsNil() {
			return 4
		}
		return escapedJSONValueUpperBound(value.Elem())
	}
	switch value.Kind() {
	case reflect.Bool:
		return 5
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
		reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64, reflect.Uintptr:
		return 32
	case reflect.String:
		return exactEscapedJSONStringLength(value.String())
	case reflect.Slice, reflect.Array:
		total := 2
		for index := 0; index < value.Len(); index++ {
			total = saturatingAddJSONBound(total, 1+escapedJSONValueUpperBound(value.Index(index)))
		}
		return total
	case reflect.Struct:
		total := 2
		typeInfo := value.Type()
		for index := 0; index < value.NumField(); index++ {
			field := typeInfo.Field(index)
			name := strings.Split(field.Tag.Get("json"), ",")[0]
			if name == "-" {
				continue
			}
			if name == "" {
				name = field.Name
			}
			total = saturatingAddJSONBound(total, 2+exactEscapedJSONStringLength(name))
			total = saturatingAddJSONBound(total, escapedJSONValueUpperBound(value.Field(index)))
		}
		return total
	case reflect.Map:
		total := 2
		iterator := value.MapRange()
		for iterator.Next() {
			total = saturatingAddJSONBound(total, 2+escapedJSONValueUpperBound(iterator.Key()))
			total = saturatingAddJSONBound(total, escapedJSONValueUpperBound(iterator.Value()))
		}
		return total
	default:
		return 32
	}
}

func exactEscapedJSONStringLength(value string) int {
	length := 2
	for _, character := range value {
		switch {
		case character == '\\' || character == '"' || character == '\b' ||
			character == '\f' || character == '\n' || character == '\r' || character == '\t':
			length = saturatingAddJSONBound(length, 2)
		case character < 0x20 || character == '<' || character == '>' || character == '&' ||
			character == '\u2028' || character == '\u2029':
			length = saturatingAddJSONBound(length, 6)
		case character < 0x80:
			length = saturatingAddJSONBound(length, 1)
		case character < 0x800:
			length = saturatingAddJSONBound(length, 2)
		case character < 0x10000:
			length = saturatingAddJSONBound(length, 3)
		default:
			length = saturatingAddJSONBound(length, 4)
		}
	}
	return length
}

func saturatingAddJSONBound(left, right int) int {
	maximum := int(^uint(0) >> 1)
	if right > maximum-left {
		return maximum
	}
	return left + right
}

func migrationKindName(kind MigrationKind) string {
	if kind == MigrationKindGlobal {
		return "global"
	}
	return "project"
}

func verifyFrozenJSONSource(source *heldJSONSource) error {
	pathInfo, err := os.Lstat(source.path)
	if err != nil || pathInfo.Mode()&os.ModeSymlink != 0 || !os.SameFile(source.openedInfo, pathInfo) ||
		pathInfo.Size() != source.openedInfo.Size() || pathInfo.Mode() != source.openedInfo.Mode() ||
		!pathInfo.ModTime().Equal(source.openedInfo.ModTime()) {
		return fmt.Errorf("source %q changed while its stage was written", source.path)
	}
	file, err := os.Open(source.path)
	if err != nil {
		return fmt.Errorf("reopen frozen source %q for verification: %w", source.path, err)
	}
	openedInfo, statErr := file.Stat()
	if statErr != nil || !os.SameFile(source.openedInfo, openedInfo) {
		_ = file.Close()
		return fmt.Errorf("source %q changed before final verification", source.path)
	}
	digest, hashErr := hashOpenFile(file, pathInfo.Size())
	closeErr := file.Close()
	if hashErr != nil {
		return hashErr
	}
	if closeErr != nil {
		return closeErr
	}
	if hex.EncodeToString(digest[:]) != source.identity.SHA256 {
		return fmt.Errorf("source %q content changed while its stage was written", source.path)
	}
	return nil
}

func hashOpenFile(file *os.File, size int64) ([sha256.Size]byte, error) {
	if size < 0 {
		return [sha256.Size]byte{}, errors.New("source has negative size")
	}
	hash := sha256.New()
	if _, err := io.Copy(hash, io.NewSectionReader(file, 0, size)); err != nil {
		return [sha256.Size]byte{}, err
	}
	var result [sha256.Size]byte
	copy(result[:], hash.Sum(nil))
	return result, nil
}

func writeJSONManifest(stagePath string, manifest jsonStageManifest) error {
	path := filepath.Join(stagePath, "manifest.json")
	partialPath := filepath.Join(stagePath, ".manifest.json.partial")
	file, err := createPrivateExportFile(partialPath)
	if err != nil {
		return fmt.Errorf("create manifest last: %w", err)
	}
	remove := true
	defer func() {
		if remove {
			_ = os.Remove(partialPath)
		}
	}()
	if err := file.Chmod(0o600); err != nil {
		_ = file.Close()
		return err
	}
	if err := protectPrivatePath(partialPath, false); err != nil {
		_ = file.Close()
		return err
	}
	openedInfo, err := file.Stat()
	if err != nil {
		_ = file.Close()
		return err
	}
	encoded, err := json.Marshal(manifest)
	if err == nil {
		encoded = append(encoded, '\n')
		if len(encoded) > jsonStageMaxManifestBytes {
			err = fmt.Errorf("manifest exceeds %d bytes", jsonStageMaxManifestBytes)
		} else {
			_, err = file.Write(encoded)
		}
	}
	if err == nil {
		err = file.Sync()
	}
	if closeErr := file.Close(); err == nil && closeErr != nil {
		err = closeErr
	}
	if err != nil {
		return fmt.Errorf("write manifest last: %w", err)
	}
	resolvedInfo, err := os.Lstat(partialPath)
	if err != nil || resolvedInfo.Mode()&os.ModeSymlink != 0 || !os.SameFile(openedInfo, resolvedInfo) {
		return errors.New("manifest partial path changed while it was written")
	}
	if err := os.Link(partialPath, path); err != nil {
		return fmt.Errorf("publish manifest without clobber: %w", err)
	}
	if err := syncDirectory(stagePath); err != nil {
		_ = os.Remove(path)
		return fmt.Errorf("sync published manifest: %w", err)
	}
	if err := os.Remove(partialPath); err != nil {
		_ = os.Remove(path)
		return fmt.Errorf("remove manifest partial: %w", err)
	}
	if err := syncDirectory(stagePath); err != nil {
		return fmt.Errorf("sync manifest partial removal: %w", err)
	}
	remove = false
	return nil
}
