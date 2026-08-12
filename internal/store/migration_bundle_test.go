package store

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"errors"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

type parsedMigrationBucket struct {
	name     string
	sequence uint64
	records  map[string][]byte
}

func TestExportProjectMigrationBundleIsDeterministicAndReadOnly(t *testing.T) {
	requireMigrationExporter(t)
	dir := t.TempDir()
	source := filepath.Join(dir, "project.db")
	createProjectMigrationFixture(t, source, CurrentFormat)
	if err := os.Chmod(source, 0o640); err != nil {
		t.Fatal(err)
	}
	stamp := time.Unix(1_700_000_000, 123_000_000)
	if err := os.Chtimes(source, stamp, stamp); err != nil {
		t.Fatal(err)
	}
	beforeBytes, beforeInfo := readFileAndInfo(t, source)

	first := filepath.Join(dir, "first.bundle")
	second := filepath.Join(dir, "second.bundle")
	if err := ExportMigrationBundle(MigrationKindProject, source, first); err != nil {
		t.Fatal(err)
	}
	if err := ExportMigrationBundle(MigrationKindProject, source, second); err != nil {
		t.Fatal(err)
	}

	afterBytes, afterInfo := readFileAndInfo(t, source)
	if !bytes.Equal(beforeBytes, afterBytes) {
		t.Fatal("source bytes changed during export")
	}
	if beforeInfo.Mode() != afterInfo.Mode() {
		t.Fatalf("source mode changed: %v -> %v", beforeInfo.Mode(), afterInfo.Mode())
	}
	if !beforeInfo.ModTime().Equal(afterInfo.ModTime()) {
		t.Fatalf("source mtime changed: %v -> %v", beforeInfo.ModTime(), afterInfo.ModTime())
	}
	firstBytes, firstInfo := readFileAndInfo(t, first)
	secondBytes, _ := readFileAndInfo(t, second)
	if !bytes.Equal(firstBytes, secondBytes) {
		t.Fatal("repeated exports differ")
	}
	if firstInfo.Mode().Perm() != 0o600 {
		t.Fatalf("output mode = %#o, want 0600", firstInfo.Mode().Perm())
	}

	kind, format, total, buckets := parseMigrationBundle(t, firstBytes)
	if kind != MigrationKindProject || format != uint64(CurrentFormat) || total != 3 {
		t.Fatalf("header = kind %d format %d records %d", kind, format, total)
	}
	wantNames := []string{"capabilities", "capability_audits", "commits", "issues", "memory_writebacks", "meta", "milestones", "notes", "plans", "tasks"}
	gotNames := make([]string, len(buckets))
	for i := range buckets {
		gotNames[i] = buckets[i].name
	}
	if !reflect.DeepEqual(gotNames, wantNames) {
		t.Fatalf("bucket order = %v, want %v", gotNames, wantNames)
	}
	plans := findParsedBucket(t, buckets, "plans")
	if plans.sequence != 9 || !bytes.Equal(plans.records[string(itob(3))], []byte("raw-plan-gob")) {
		t.Fatalf("plans not preserved: %#v", plans)
	}
	memory := findParsedBucket(t, buckets, "memory_writebacks")
	if memory.sequence != 7 || !bytes.Equal(memory.records["request-1"], []byte("raw-writeback-gob")) {
		t.Fatalf("memory writebacks not preserved: %#v", memory)
	}
}

func TestExportGlobalMigrationBundle(t *testing.T) {
	requireMigrationExporter(t)
	dir := t.TempDir()
	source := filepath.Join(dir, "global.db")
	createBoltFixture(t, source, func(tx *bolt.Tx) error {
		for _, name := range []string{"config", "projects", "backups"} {
			if _, err := tx.CreateBucket([]byte(name)); err != nil {
				return err
			}
		}
		if err := tx.Bucket([]byte("config")).Put([]byte("theme"), []byte("dark")); err != nil {
			return err
		}
		return tx.Bucket([]byte("projects")).Put([]byte("/work/example"), []byte("raw-project-ref-gob"))
	})
	output := filepath.Join(dir, "global.bundle")
	if err := ExportMigrationBundle(MigrationKindGlobal, source, output); err != nil {
		t.Fatal(err)
	}
	kind, format, total, buckets := parseMigrationBundle(t, mustReadFile(t, output))
	if kind != MigrationKindGlobal || format != 0 || total != 2 {
		t.Fatalf("header = kind %d format %d records %d", kind, format, total)
	}
	if got := []string{buckets[0].name, buckets[1].name, buckets[2].name}; !reflect.DeepEqual(got, []string{"backups", "config", "projects"}) {
		t.Fatalf("bucket order = %v", got)
	}
	if got := findParsedBucket(t, buckets, "config").records["theme"]; !bytes.Equal(got, []byte("dark")) {
		t.Fatalf("config value = %q", got)
	}
}

func TestExportOlderProjectIncludesOnlyPresentKnownBuckets(t *testing.T) {
	requireMigrationExporter(t)
	dir := t.TempDir()
	source := filepath.Join(dir, "v1.db")
	meta, err := gobEncode(model.Meta{FormatVersion: 1})
	if err != nil {
		t.Fatal(err)
	}
	createBoltFixture(t, source, func(tx *bolt.Tx) error {
		for _, name := range []string{"meta", "plans", "tasks", "notes"} {
			if _, err := tx.CreateBucket([]byte(name)); err != nil {
				return err
			}
		}
		return tx.Bucket(bucketMeta).Put(keyMeta, meta)
	})
	output := filepath.Join(dir, "v1.bundle")
	if err := ExportMigrationBundle(MigrationKindProject, source, output); err != nil {
		t.Fatal(err)
	}
	_, format, _, buckets := parseMigrationBundle(t, mustReadFile(t, output))
	if format != 1 {
		t.Fatalf("source format = %d, want 1", format)
	}
	got := make([]string, len(buckets))
	for i := range buckets {
		got[i] = buckets[i].name
	}
	if want := []string{"meta", "notes", "plans", "tasks"}; !reflect.DeepEqual(got, want) {
		t.Fatalf("buckets = %v, want %v", got, want)
	}
}

func TestExportMigrationBundleNoClobber(t *testing.T) {
	requireMigrationExporter(t)
	dir := t.TempDir()
	source := filepath.Join(dir, "project.db")
	createProjectMigrationFixture(t, source, CurrentFormat)
	output := filepath.Join(dir, "existing.bundle")
	if err := os.WriteFile(output, []byte("keep me"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := ExportMigrationBundle(MigrationKindProject, source, output); err == nil {
		t.Fatal("expected existing output rejection")
	}
	if got := mustReadFile(t, output); !bytes.Equal(got, []byte("keep me")) {
		t.Fatalf("existing output changed: %q", got)
	}
}

func TestExportMigrationBundleRejectsEveryExistingOutputKindUntouched(t *testing.T) {
	requireMigrationExporter(t)
	dir := t.TempDir()
	source := filepath.Join(dir, "project.db")
	createProjectMigrationFixture(t, source, CurrentFormat)
	target := filepath.Join(dir, "target")
	if err := os.WriteFile(target, []byte("target"), 0o600); err != nil {
		t.Fatal(err)
	}

	tests := []struct {
		name   string
		output string
		make   func(string) error
		check  func(*testing.T, string)
	}{
		{
			name: "file", output: filepath.Join(dir, "file.bundle"),
			make: func(path string) error { return os.WriteFile(path, []byte("keep"), 0o600) },
			check: func(t *testing.T, path string) {
				if got := mustReadFile(t, path); !bytes.Equal(got, []byte("keep")) {
					t.Fatalf("file changed: %q", got)
				}
			},
		},
		{
			name: "symlink", output: filepath.Join(dir, "link.bundle"),
			make: func(path string) error { return os.Symlink(target, path) },
			check: func(t *testing.T, path string) {
				info, err := os.Lstat(path)
				if err != nil || info.Mode()&os.ModeSymlink == 0 {
					t.Fatalf("symlink changed: info=%v err=%v", info, err)
				}
			},
		},
		{
			name: "directory", output: filepath.Join(dir, "directory.bundle"),
			make: func(path string) error { return os.Mkdir(path, 0o700) },
			check: func(t *testing.T, path string) {
				info, err := os.Stat(path)
				if err != nil || !info.IsDir() {
					t.Fatalf("directory changed: info=%v err=%v", info, err)
				}
			},
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if err := test.make(test.output); err != nil {
				t.Fatal(err)
			}
			if err := ExportMigrationBundle(MigrationKindProject, source, test.output); err == nil {
				t.Fatal("expected existing output rejection")
			}
			test.check(t, test.output)
		})
	}
}

func TestExportMigrationBundleRejectsUnsafeOrInvalidSources(t *testing.T) {
	requireMigrationExporter(t)
	tests := []struct {
		name    string
		kind    MigrationKind
		prepare func(*testing.T, string)
		want    string
	}{
		{
			name: "corrupt meta", kind: MigrationKindProject, want: "decode project meta",
			prepare: func(t *testing.T, path string) {
				createProjectMigrationFixtureWithMeta(t, path, []byte("not-gob"))
			},
		},
		{
			name: "newer format", kind: MigrationKindProject, want: "newer than this ptrack",
			prepare: func(t *testing.T, path string) { createProjectMigrationFixture(t, path, CurrentFormat+1) },
		},
		{
			name: "unknown bucket", kind: MigrationKindGlobal, want: "unknown top-level bucket",
			prepare: func(t *testing.T, path string) {
				createGlobalFixtureWithMutation(t, path, func(tx *bolt.Tx) error {
					_, err := tx.CreateBucket([]byte("surprise"))
					return err
				})
			},
		},
		{
			name: "nested bucket", kind: MigrationKindGlobal, want: "nested bucket",
			prepare: func(t *testing.T, path string) {
				createGlobalFixtureWithMutation(t, path, func(tx *bolt.Tx) error {
					_, err := tx.Bucket([]byte("config")).CreateBucket([]byte("nested"))
					return err
				})
			},
		},
		{
			name: "nonzero global sequence", kind: MigrationKindGlobal, want: "non-sequenced bucket",
			prepare: func(t *testing.T, path string) {
				createGlobalFixtureWithMutation(t, path, func(tx *bolt.Tx) error {
					return tx.Bucket([]byte("config")).SetSequence(1)
				})
			},
		},
		{
			name: "malformed numeric key", kind: MigrationKindProject, want: "must be 8 bytes",
			prepare: func(t *testing.T, path string) {
				createProjectFixtureWithMutation(t, path, func(tx *bolt.Tx) error {
					return tx.Bucket(bucketPlans).Put([]byte("bad"), []byte("value"))
				})
			},
		},
		{
			name: "id above sequence", kind: MigrationKindProject, want: "exceeds sequence",
			prepare: func(t *testing.T, path string) {
				createProjectFixtureWithMutation(t, path, func(tx *bolt.Tx) error {
					b := tx.Bucket(bucketPlans)
					if err := b.Put(itob(4), []byte("value")); err != nil {
						return err
					}
					return b.SetSequence(3)
				})
			},
		},
		{
			name: "missing required bucket", kind: MigrationKindProject, want: "missing required bucket",
			prepare: func(t *testing.T, path string) {
				createProjectFixtureWithMutation(t, path, func(tx *bolt.Tx) error {
					return tx.DeleteBucket(bucketTasks)
				})
			},
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			dir := t.TempDir()
			source := filepath.Join(dir, "source.db")
			test.prepare(t, source)
			output := filepath.Join(dir, "output.bundle")
			err := ExportMigrationBundle(test.kind, source, output)
			if err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("error = %v, want substring %q", err, test.want)
			}
			if _, statErr := os.Lstat(output); !errors.Is(statErr, os.ErrNotExist) {
				t.Fatalf("final output exists after validation failure: %v", statErr)
			}
			partials, globErr := filepath.Glob(filepath.Join(dir, ".ptrack-migrate-*.partial"))
			if globErr != nil || len(partials) != 0 {
				t.Fatalf("validation created partial outputs: %v, %v", partials, globErr)
			}
		})
	}
}

func TestExportMigrationBundleRequiresAbsoluteRegularSource(t *testing.T) {
	requireMigrationExporter(t)
	dir := t.TempDir()
	output := filepath.Join(dir, "out.bundle")
	if err := ExportMigrationBundle(MigrationKindProject, "relative.db", output); err == nil || !strings.Contains(err.Error(), "absolute") {
		t.Fatalf("relative source error = %v", err)
	}
	source := filepath.Join(dir, "source.db")
	createProjectMigrationFixture(t, source, CurrentFormat)
	if err := ExportMigrationBundle(MigrationKindProject, source, "relative.bundle"); err == nil || !strings.Contains(err.Error(), "absolute") {
		t.Fatalf("relative output error = %v", err)
	}
	symlink := filepath.Join(dir, "source-link.db")
	if err := os.Symlink(source, symlink); err != nil {
		t.Fatal(err)
	}
	if err := ExportMigrationBundle(MigrationKindProject, symlink, output); err == nil || !strings.Contains(err.Error(), "symbolic link") {
		t.Fatalf("symlink error = %v", err)
	}
	if err := ExportMigrationBundle(MigrationKindProject, dir, output); err == nil || !strings.Contains(err.Error(), "regular file") {
		t.Fatalf("directory error = %v", err)
	}
	invalid := filepath.Join(dir, "invalid.db")
	if err := os.WriteFile(invalid, []byte("not a bbolt database"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := ExportMigrationBundle(MigrationKindProject, invalid, output); err == nil {
		t.Fatal("expected corrupt bbolt file rejection")
	}
}

func TestExportMigrationBundleRejectsSourceSwapBeforeBboltUsesIt(t *testing.T) {
	requireMigrationExporter(t)
	dir := t.TempDir()
	source := filepath.Join(dir, "source.db")
	original := filepath.Join(dir, "original.db")
	replacement := filepath.Join(dir, "replacement.db")
	createProjectMigrationFixture(t, source, CurrentFormat)
	createProjectMigrationFixture(t, replacement, CurrentFormat)
	output := filepath.Join(dir, "output.bundle")
	openCalled := false
	err := exportMigrationBundleWithOpen(MigrationKindProject, source, output, func(name string, flag int, mode os.FileMode) (*os.File, error) {
		openCalled = true
		openedOriginal, err := os.OpenFile(name, flag, mode)
		if err != nil {
			return nil, err
		}
		if err := os.Rename(source, original); err != nil {
			_ = openedOriginal.Close()
			return nil, err
		}
		if err := os.Rename(replacement, source); err != nil {
			_ = openedOriginal.Close()
			return nil, err
		}
		return openedOriginal, nil
	})
	if !openCalled || err == nil || !strings.Contains(err.Error(), "source path changed") {
		t.Fatalf("openCalled=%v error=%v", openCalled, err)
	}
	if _, statErr := os.Lstat(output); !errors.Is(statErr, os.ErrNotExist) {
		t.Fatalf("output exists after source swap: %v", statErr)
	}
}

func TestExportMigrationBundleIgnoresRetainedPartialOnRetry(t *testing.T) {
	requireMigrationExporter(t)
	dir := t.TempDir()
	source := filepath.Join(dir, "source.db")
	createProjectMigrationFixture(t, source, CurrentFormat)
	output := filepath.Join(dir, "output.bundle")
	retained := filepath.Join(dir, ".output.bundle.previous.partial")
	if err := os.WriteFile(retained, []byte("retain"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := ExportMigrationBundle(MigrationKindProject, source, output); err != nil {
		t.Fatal(err)
	}
	if got := mustReadFile(t, retained); !bytes.Equal(got, []byte("retain")) {
		t.Fatalf("old partial changed: %q", got)
	}
}

func TestPublishMigrationBundleNoClobberRetainsPartial(t *testing.T) {
	requireMigrationExporter(t)
	dir := t.TempDir()
	output := filepath.Join(dir, "output.bundle")
	outputDirectory, err := openMigrationOutputDirectory(output)
	if err != nil {
		t.Fatal(err)
	}
	defer outputDirectory.close()
	partial, partialName, _, err := outputDirectory.createPartial()
	if err != nil {
		t.Fatal(err)
	}
	if _, err := partial.Write([]byte("new")); err != nil {
		_ = partial.Close()
		t.Fatal(err)
	}
	if err := partial.Sync(); err != nil {
		_ = partial.Close()
		t.Fatal(err)
	}
	if err := partial.Close(); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(output, []byte("existing"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := outputDirectory.publish(partialName); err == nil {
		t.Fatal("expected no-clobber publication failure")
	}
	if got := mustReadFile(t, filepath.Join(dir, partialName)); !bytes.Equal(got, []byte("new")) {
		t.Fatalf("partial changed: %q", got)
	}
	if got := mustReadFile(t, output); !bytes.Equal(got, []byte("existing")) {
		t.Fatalf("final changed: %q", got)
	}
}

func TestMigrationIntegrityCheckStopsBeforeOutputCreation(t *testing.T) {
	requireMigrationExporter(t)
	dir := t.TempDir()
	source := filepath.Join(dir, "source.db")
	createProjectMigrationFixture(t, source, CurrentFormat)
	output := filepath.Join(dir, "output.bundle")
	err := exportMigrationBundleWithOpenAndCheck(
		MigrationKindProject,
		source,
		output,
		os.OpenFile,
		func(*bolt.Tx) <-chan error {
			results := make(chan error, 2)
			results <- errors.New("first integrity defect")
			results <- errors.New("second integrity defect")
			close(results)
			return results
		},
	)
	if err == nil || !strings.Contains(err.Error(), "bbolt integrity check failed") ||
		!strings.Contains(err.Error(), "first integrity defect") || !strings.Contains(err.Error(), "second integrity defect") {
		t.Fatalf("error = %v", err)
	}
	if _, statErr := os.Lstat(output); !errors.Is(statErr, os.ErrNotExist) {
		t.Fatalf("final output exists after integrity failure: %v", statErr)
	}
	partials, globErr := filepath.Glob(filepath.Join(dir, ".ptrack-migrate-*.partial"))
	if globErr != nil || len(partials) != 0 {
		t.Fatalf("integrity failure created partials: %v, %v", partials, globErr)
	}
}

func TestMigrationOutputDirectoryDescriptorSurvivesParentReplacement(t *testing.T) {
	requireMigrationExporter(t)
	root := t.TempDir()
	originalPath := filepath.Join(root, "output-parent")
	movedPath := filepath.Join(root, "held-parent")
	if err := os.Mkdir(originalPath, 0o700); err != nil {
		t.Fatal(err)
	}
	output := filepath.Join(originalPath, "output.bundle")
	outputDirectory, err := openMigrationOutputDirectory(output)
	if err != nil {
		t.Fatal(err)
	}
	defer outputDirectory.close()
	if err := os.Rename(originalPath, movedPath); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(originalPath, 0o700); err != nil {
		t.Fatal(err)
	}
	attackerMarker := filepath.Join(originalPath, "attacker-marker")
	if err := os.WriteFile(attackerMarker, []byte("untouched"), 0o600); err != nil {
		t.Fatal(err)
	}

	partial, partialName, _, err := outputDirectory.createPartial()
	if err != nil {
		t.Fatal(err)
	}
	if _, err := partial.Write([]byte("bound-to-held-directory")); err != nil {
		_ = partial.Close()
		t.Fatal(err)
	}
	if err := partial.Sync(); err != nil {
		_ = partial.Close()
		t.Fatal(err)
	}
	if err := partial.Close(); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(movedPath, partialName)); err != nil {
		t.Fatalf("partial was not created through held directory: %v", err)
	}
	if err := outputDirectory.publish(partialName); err != nil {
		t.Fatal(err)
	}
	if got := mustReadFile(t, filepath.Join(movedPath, "output.bundle")); !bytes.Equal(got, []byte("bound-to-held-directory")) {
		t.Fatalf("published bytes = %q", got)
	}
	if _, err := os.Lstat(filepath.Join(originalPath, "output.bundle")); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("replacement directory received output: %v", err)
	}
	if got := mustReadFile(t, attackerMarker); !bytes.Equal(got, []byte("untouched")) {
		t.Fatalf("replacement directory marker changed: %q", got)
	}
}

func TestMigrationPayloadLimit(t *testing.T) {
	if migrationMaxRecords != 1_000_000 {
		t.Fatalf("record limit = %d", migrationMaxRecords)
	}
	current, err := addMigrationPayload(0, 1, migrationMaxPayloadBytes-migrationRecordOverhead-1)
	if err != nil || current != migrationMaxPayloadBytes {
		t.Fatalf("boundary payload = %d, %v", current, err)
	}
	if _, err := addMigrationPayload(current, 0, 0); err == nil {
		t.Fatal("expected aggregate payload overflow")
	}
	if _, err := addMigrationPayload(0, ^uint64(0), 1); err == nil {
		t.Fatal("expected integer overflow rejection")
	}
}

func TestExportMigrationBundleFailsClosedOnWindows(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("Windows-only behavior")
	}
	err := ExportMigrationBundle(MigrationKindProject, `C:\source.db`, `C:\output.bundle`)
	if err == nil || !strings.Contains(err.Error(), "unsupported on Windows") {
		t.Fatalf("error = %v", err)
	}
}

func requireMigrationExporter(t *testing.T) {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("migration export intentionally fails closed on Windows")
	}
}

func createProjectMigrationFixture(t *testing.T, path string, format uint) {
	t.Helper()
	meta, err := gobEncode(model.Meta{Goal: "ship it", FormatVersion: format})
	if err != nil {
		t.Fatal(err)
	}
	createProjectMigrationFixtureWithMeta(t, path, meta)
}

func createProjectMigrationFixtureWithMeta(t *testing.T, path string, meta []byte) {
	t.Helper()
	createBoltFixture(t, path, func(tx *bolt.Tx) error {
		for _, spec := range projectMigrationBuckets {
			if _, err := tx.CreateBucket([]byte(spec.name)); err != nil {
				return err
			}
		}
		if err := tx.Bucket(bucketMeta).Put(keyMeta, meta); err != nil {
			return err
		}
		plans := tx.Bucket(bucketPlans)
		if err := plans.Put(itob(3), []byte("raw-plan-gob")); err != nil {
			return err
		}
		if err := plans.SetSequence(9); err != nil {
			return err
		}
		memory := tx.Bucket(bucketMemoryWritebacks)
		if err := memory.Put([]byte("request-1"), []byte("raw-writeback-gob")); err != nil {
			return err
		}
		return memory.SetSequence(7)
	})
}

func createProjectFixtureWithMutation(t *testing.T, path string, mutate func(*bolt.Tx) error) {
	t.Helper()
	createProjectMigrationFixture(t, path, CurrentFormat)
	mutateBoltFixture(t, path, mutate)
}

func createGlobalFixtureWithMutation(t *testing.T, path string, mutate func(*bolt.Tx) error) {
	t.Helper()
	createBoltFixture(t, path, func(tx *bolt.Tx) error {
		for _, spec := range globalMigrationBuckets {
			if _, err := tx.CreateBucket([]byte(spec.name)); err != nil {
				return err
			}
		}
		return mutate(tx)
	})
}

func createBoltFixture(t *testing.T, path string, fill func(*bolt.Tx) error) {
	t.Helper()
	db, err := bolt.Open(path, 0o600, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := db.Update(fill); err != nil {
		_ = db.Close()
		t.Fatal(err)
	}
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
}

func mutateBoltFixture(t *testing.T, path string, mutate func(*bolt.Tx) error) {
	t.Helper()
	db, err := bolt.Open(path, 0o600, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := db.Update(mutate); err != nil {
		_ = db.Close()
		t.Fatal(err)
	}
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
}

func readFileAndInfo(t *testing.T, path string) ([]byte, os.FileInfo) {
	t.Helper()
	data := mustReadFile(t, path)
	info, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	return data, info
}

func mustReadFile(t *testing.T, path string) []byte {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return data
}

func parseMigrationBundle(t *testing.T, data []byte) (MigrationKind, uint64, uint64, []parsedMigrationBucket) {
	t.Helper()
	if len(data) < migrationBundleHeaderLen+40 {
		t.Fatal("bundle is too short")
	}
	payload, trailer := data[:len(data)-40], data[len(data)-40:]
	if string(trailer[:4]) != "HASH" || binary.BigEndian.Uint16(trailer[4:6]) != 1 || binary.BigEndian.Uint16(trailer[6:8]) != sha256.Size {
		t.Fatal("invalid trailer")
	}
	wantDigest := sha256.Sum256(payload)
	if !bytes.Equal(trailer[8:], wantDigest[:]) {
		t.Fatal("checksum mismatch")
	}
	reader := bytes.NewReader(payload)
	magic := make([]byte, 8)
	readExactly(t, reader, magic)
	if !bytes.Equal(magic, migrationMagic[:]) {
		t.Fatalf("magic = %q", magic)
	}
	if readU16(t, reader) != migrationBundleVersion || readU16(t, reader) != migrationBundleHeaderLen {
		t.Fatal("invalid version or header length")
	}
	kindByte, err := reader.ReadByte()
	if err != nil {
		t.Fatal(err)
	}
	flags, err := reader.ReadByte()
	if err != nil || flags != 0 || readU16(t, reader) != 0 {
		t.Fatal("invalid header flags")
	}
	format := readU64(t, reader)
	bucketCount := readU32(t, reader)
	if readU32(t, reader) != 0 {
		t.Fatal("invalid header reserved field")
	}
	totalRecords := readU64(t, reader)
	buckets := make([]parsedMigrationBucket, 0, bucketCount)
	for range bucketCount {
		marker := make([]byte, 4)
		readExactly(t, reader, marker)
		if string(marker) != "BUKT" {
			t.Fatalf("bucket marker = %q", marker)
		}
		nameLen := readU16(t, reader)
		if readU16(t, reader) != 0 {
			t.Fatal("invalid bucket flags")
		}
		sequence := readU64(t, reader)
		recordCount := readU64(t, reader)
		name := make([]byte, nameLen)
		readExactly(t, reader, name)
		bucket := parsedMigrationBucket{name: string(name), sequence: sequence, records: make(map[string][]byte)}
		for range recordCount {
			key := make([]byte, readU64(t, reader))
			value := make([]byte, readU64(t, reader))
			readExactly(t, reader, key)
			readExactly(t, reader, value)
			bucket.records[string(key)] = value
		}
		buckets = append(buckets, bucket)
	}
	if reader.Len() != 0 {
		t.Fatalf("unparsed payload bytes = %d", reader.Len())
	}
	return MigrationKind(kindByte), format, totalRecords, buckets
}

func findParsedBucket(t *testing.T, buckets []parsedMigrationBucket, name string) parsedMigrationBucket {
	t.Helper()
	for _, bucket := range buckets {
		if bucket.name == name {
			return bucket
		}
	}
	t.Fatalf("missing parsed bucket %q", name)
	return parsedMigrationBucket{}
}

func readExactly(t *testing.T, reader io.Reader, value []byte) {
	t.Helper()
	if _, err := io.ReadFull(reader, value); err != nil {
		t.Fatal(err)
	}
}

func readU16(t *testing.T, reader io.Reader) uint16 {
	t.Helper()
	var value uint16
	if err := binary.Read(reader, binary.BigEndian, &value); err != nil {
		t.Fatal(err)
	}
	return value
}

func readU32(t *testing.T, reader io.Reader) uint32 {
	t.Helper()
	var value uint32
	if err := binary.Read(reader, binary.BigEndian, &value); err != nil {
		t.Fatal(err)
	}
	return value
}

func readU64(t *testing.T, reader io.Reader) uint64 {
	t.Helper()
	var value uint64
	if err := binary.Read(reader, binary.BigEndian, &value); err != nil {
		t.Fatal(err)
	}
	return value
}

func TestParseMigrationKind(t *testing.T) {
	for input, want := range map[string]MigrationKind{"project": MigrationKindProject, "global": MigrationKindGlobal} {
		got, err := ParseMigrationKind(input)
		if err != nil || got != want {
			t.Fatalf("ParseMigrationKind(%q) = %d, %v", input, got, err)
		}
	}
	if _, err := ParseMigrationKind("PROJECT"); err == nil {
		t.Fatal("expected strict kind rejection")
	}
}
