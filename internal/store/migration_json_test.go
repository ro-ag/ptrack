package store

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

type jsonExportFixture struct {
	home         string
	globalPath   string
	projectPaths []string
}

type jsonSourceSnapshot struct {
	mode    os.FileMode
	mtime   time.Time
	digest  [sha256.Size]byte
	content []byte
}

func TestExportJSONStageBatchIsDeterministicLosslessAndPrivate(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("JSON exporter deliberately fails closed without Unix file identity")
	}
	fixture := createJSONExportFixture(t, true)
	paths := append([]string{fixture.globalPath}, fixture.projectPaths...)
	before := snapshotJSONSources(t, paths)
	stageOne := os.Getenv("PTRACK_JSON_STAGE_OUTPUT")
	if stageOne == "" {
		stageOne = filepath.Join(privateJSONTestDir(t), "stage-one")
	} else if !filepath.IsAbs(stageOne) {
		t.Fatal("PTRACK_JSON_STAGE_OUTPUT must be absolute")
	}
	stageTwo := filepath.Join(privateJSONTestDir(t), "stage-two")
	if err := ExportJSONStage(fixture.home, stageOne); err != nil {
		t.Fatal(err)
	}
	if err := ExportJSONStage(fixture.home, stageTwo); err != nil {
		t.Fatal(err)
	}
	assertJSONSourcesUnchanged(t, paths, before)

	manifestOne := mustReadJSONFile(t, filepath.Join(stageOne, "manifest.json"))
	manifestTwo := mustReadJSONFile(t, filepath.Join(stageTwo, "manifest.json"))
	if !bytes.Equal(manifestOne, manifestTwo) {
		t.Fatal("identical frozen sources produced different manifests")
	}
	if !bytes.HasSuffix(manifestOne, []byte("\n")) || bytes.Count(manifestOne, []byte("\n")) != 1 {
		t.Fatalf("manifest must be one compact LF-terminated line: %q", manifestOne)
	}
	var manifest jsonStageManifest
	if err := json.Unmarshal(bytes.TrimSuffix(manifestOne, []byte("\n")), &manifest); err != nil {
		t.Fatal(err)
	}
	if manifest.Format != jsonStageFormat || manifest.Version != jsonStageVersion || manifest.DatabaseCount != "3" {
		t.Fatalf("manifest identity/count = %#v", manifest)
	}
	if manifest.QuarantineCount != "1" {
		t.Fatalf("quarantine_count = %q, want 1", manifest.QuarantineCount)
	}
	if manifest.Databases[0].Kind != "global" || manifest.Databases[0].ProjectRoot != nil {
		t.Fatalf("first database is not global: %#v", manifest.Databases[0])
	}
	gotRoots := []string{*manifest.Databases[1].ProjectRoot, *manifest.Databases[2].ProjectRoot}
	if !sort.StringsAreSorted(gotRoots) {
		t.Fatalf("project roots are not sorted: %q", gotRoots)
	}
	if strings.Contains(string(manifestOne), `"backup"`) {
		t.Fatal("manifest unexpectedly contains redundant backup metadata")
	}

	assertMode(t, stageOne, 0o700)
	assertMode(t, filepath.Join(stageOne, "databases"), 0o700)
	assertMode(t, filepath.Join(stageOne, "manifest.json"), 0o600)
	for _, database := range manifest.Databases {
		artifactOne := mustReadJSONFile(t, filepath.Join(stageOne, filepath.FromSlash(database.Data.Path)))
		artifactTwo := mustReadJSONFile(t, filepath.Join(stageTwo, filepath.FromSlash(database.Data.Path)))
		if !bytes.Equal(artifactOne, artifactTwo) {
			t.Fatalf("database artifact %q is nondeterministic", database.ID)
		}
		assertMode(t, filepath.Join(stageOne, filepath.FromSlash(database.Data.Path)), 0o600)
		digest := sha256.Sum256(artifactOne)
		if got := hex.EncodeToString(digest[:]); got != database.Data.SHA256 {
			t.Fatalf("artifact hash = %q, want %q", got, database.Data.SHA256)
		}
		if got := len(artifactOne); database.Data.Bytes != u64s(uint64(got)) {
			t.Fatalf("artifact bytes = %q, want %d", database.Data.Bytes, got)
		}
	}

	globalLines := decodeJSONLines(t, mustReadJSONFile(t, filepath.Join(stageOne, "databases", "global.jsonl")))
	assertRawJSONRecord(t, globalLines, "config", "ff006b", []byte{0xff, 0x00, 'v'})
	assertRawJSONRecord(t, globalLines, "backups", hex.EncodeToString([]byte("7")), []byte("relative-project\trelative-backup"))

	projectLines := decodeJSONLines(t, mustReadJSONFile(t, filepath.Join(stageOne, "databases", "project-000001.jsonl")))
	quarantine := findJSONLine(t, projectLines, "quarantine", "capabilities", "2")
	if quarantine["reason"] != "invalid_capability" || quarantine["legacy_codec"] != "go-gob" || quarantine["legacy_value_hex"] != "" {
		t.Fatalf("empty invalid capability not preserved exactly: %#v", quarantine)
	}
	emptyDigest := sha256.Sum256(nil)
	if quarantine["source_value_sha256"] != hex.EncodeToString(emptyDigest[:]) {
		t.Fatalf("empty quarantine digest = %v", quarantine["source_value_sha256"])
	}
	capability := findJSONLine(t, projectLines, "record", "capabilities", "1")
	value := capability["value"].(map[string]any)
	if value["enabled"] != true || value["migration_disposition"] != "force_reapproval" {
		t.Fatalf("capability authority/disposition = %#v", value)
	}
	if value["approved_at"].(map[string]any)["state"] != "fixed" {
		t.Fatalf("original approval timestamp was not represented: %#v", value["approved_at"])
	}

	sentinel := []byte("do-not-clobber")
	if err := os.WriteFile(filepath.Join(stageOne, "sentinel"), sentinel, 0o600); err != nil {
		t.Fatal(err)
	}
	if err := ExportJSONStage(fixture.home, stageOne); err == nil || !strings.Contains(err.Error(), "already exists") {
		t.Fatalf("second export error = %v, want existing-output rejection", err)
	}
	if got := mustReadJSONFile(t, filepath.Join(stageOne, "sentinel")); !bytes.Equal(got, sentinel) {
		t.Fatalf("existing stage was modified: %q", got)
	}
}

func TestExportJSONStageHoldsEverySourceLockBeforeCreatingStage(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("JSON exporter deliberately fails closed without Unix file identity")
	}
	fixture := createJSONExportFixture(t, false)
	stage := filepath.Join(privateJSONTestDir(t), "stage")
	frozen := make(chan struct{})
	release := make(chan struct{})
	done := make(chan error, 1)
	go func() {
		done <- exportJSONStage(fixture.home, stage, func() error {
			close(frozen)
			<-release
			return nil
		})
	}()
	select {
	case <-frozen:
	case err := <-done:
		t.Fatalf("export failed before freezing sources: %v", err)
	case <-time.After(5 * time.Second):
		t.Fatal("timed out waiting for all source locks")
	}
	if _, err := os.Lstat(stage); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("stage exists before all-source freeze hook completed: %v", err)
	}
	for _, path := range append([]string{fixture.globalPath}, fixture.projectPaths...) {
		db, err := bolt.Open(path, 0o600, &bolt.Options{Timeout: 50 * time.Millisecond})
		if err == nil {
			_ = db.Close()
			t.Fatalf("writer unexpectedly acquired frozen source %q", path)
		}
	}
	close(release)
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(stage, "manifest.json")); err != nil {
		t.Fatalf("manifest missing after successful release: %v", err)
	}
}

func TestExportJSONStageInvalidOrdinaryRecordPublishesNoManifest(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("JSON exporter deliberately fails closed without Unix file identity")
	}
	fixture := createJSONExportFixture(t, false)
	db, err := bolt.Open(fixture.projectPaths[0], 0o600, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := db.Update(func(tx *bolt.Tx) error {
		return tx.Bucket(bucketTasks).Put(itob(1), []byte{})
	}); err != nil {
		_ = db.Close()
		t.Fatal(err)
	}
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	stage := filepath.Join(privateJSONTestDir(t), "invalid-stage")
	if err := ExportJSONStage(fixture.home, stage); err == nil {
		t.Fatal("export with invalid task unexpectedly succeeded")
	}
	if _, err := os.Stat(filepath.Join(stage, "manifest.json")); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("invalid export published a manifest: %v", err)
	}
}

// TestCrossLanguageJSONFixture is the stable Go entry point used by the Rust
// acceptance test. When PTRACK_JSON_STAGE_OUTPUT is set, the complete stage is
// intentionally left at that absolute, initially absent path for Rust to read.
func TestCrossLanguageJSONFixture(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("JSON exporter deliberately fails closed without Unix file identity")
	}
	root := t.TempDir()
	home := filepath.Join(root, "home")
	projectRoot := filepath.Join(root, "project")
	if err := os.MkdirAll(filepath.Join(projectRoot, ".ptrack"), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(home, 0o700); err != nil {
		t.Fatal(err)
	}
	stamp := time.Date(2026, 8, 12, 19, 20, 21, 987654321, time.FixedZone("fixture", -7*60*60))
	projectPath := filepath.Join(projectRoot, ".ptrack", "ptrack.db")
	project, err := Open(projectPath)
	if err != nil {
		t.Fatal(err)
	}
	if err := project.db.Update(func(tx *bolt.Tx) error {
		meta := model.Meta{Goal: "ship safely", Summary: "cross-language fixture", ActivePlan: 1, CreatedAt: stamp, UpdatedAt: stamp, FormatVersion: CurrentFormat, LastWriteVersion: "fixture"}
		if err := putGob(tx.Bucket(bucketMeta), keyMeta, meta); err != nil {
			return err
		}
		milestone := model.Milestone{ID: 1, Title: "migration", Status: model.MilestoneOpen, Due: stamp.Add(24 * time.Hour), Order: 0, CreatedAt: stamp, UpdatedAt: stamp}
		plan := model.Plan{ID: 1, Title: "parity", Status: model.PlanActive, MilestoneID: 1, Order: 0, CreatedAt: stamp, UpdatedAt: stamp}
		task := model.Task{ID: 1, PlanID: 1, Title: "convert", Status: model.TaskDoing, Order: 0, CreatedAt: stamp, UpdatedAt: stamp}
		note := model.Note{ID: 1, Target: model.TargetTask, TargetID: 1, Kind: model.MemoryDecision, Body: "preserve original", CreatedAt: stamp}
		issue := model.Issue{ID: 1, Title: "risk", Body: "verify every hash", Status: model.IssueOpen, Severity: model.SeverityHigh, TaskID: 1, CreatedAt: stamp, UpdatedAt: stamp}
		commit := model.Commit{ID: 1, SHA: strings.Repeat("a", 40), Subject: "fixture", PlanID: 1, TaskID: 1, CreatedAt: stamp}
		capability := model.Capability{
			ID: 1, ModelVersion: model.CapabilityModelVersion, Revision: 2, Name: "origin", Kind: model.CapabilityGit,
			AgentProfile: "agent-codex", Enabled: true, ApprovalDurationSeconds: 3600,
			ApprovedAt: stamp, ExpiresAt: stamp.Add(time.Hour), ScopeDigest: strings.Repeat("2", 64),
			Limits:    model.CapabilityLimits{TimeoutSeconds: 30, MaxRequestBytes: 1024, MaxResponseBytes: 2048, MaxOutputBytes: 4096, MaxRedirects: 0, MaxConcurrent: 1},
			Audit:     model.CapabilityAuditPolicy{Enabled: true, RetainLast: 50},
			Git:       &model.GitScope{RemoteName: "origin", RemoteURL: "https://example.test/repo.git", Operations: []string{"fetch"}, Branches: []string{"main"}, Refspecs: []string{"refs/heads/main:refs/remotes/origin/main"}},
			CreatedAt: stamp.Add(-time.Hour), UpdatedAt: stamp,
		}
		audit := model.CapabilityAudit{ID: 1, CapabilityID: 1, AgentProfile: "agent-codex", Kind: model.CapabilityGit, Operation: "fetch", Target: "origin", Success: true, ErrorClass: "none", DurationMillis: 15, RequestBytes: 10, ResponseBytes: 20, Redirects: 0, CreatedAt: stamp}
		digest := sha256.Sum256([]byte("receipt"))
		receipt := memoryWritebackRecord{Digest: digest, Sequence: 1, Kind: model.MemoryDecision, NoteID: 1}
		values := []struct {
			bucket []byte
			key    []byte
			value  any
		}{
			{bucketMilestones, itob(1), milestone}, {bucketPlans, itob(1), plan},
			{bucketTasks, itob(1), task}, {bucketNotes, itob(1), note},
			{bucketIssues, itob(1), issue}, {bucketCommits, itob(1), commit},
			{bucketCapabilities, itob(1), capability}, {bucketCapabilityAudits, itob(1), audit},
			{bucketMemoryWritebacks, []byte("fixture-request"), receipt},
		}
		for _, item := range values {
			if err := putGob(tx.Bucket(item.bucket), item.key, item.value); err != nil {
				return err
			}
		}
		for _, bucket := range [][]byte{bucketMilestones, bucketPlans, bucketTasks, bucketNotes, bucketIssues, bucketCommits, bucketCapabilityAudits, bucketMemoryWritebacks} {
			if err := tx.Bucket(bucket).SetSequence(1); err != nil {
				return err
			}
		}
		if err := tx.Bucket(bucketCapabilities).Put(itob(2), []byte{}); err != nil {
			return err
		}
		return tx.Bucket(bucketCapabilities).SetSequence(2)
	}); err != nil {
		_ = project.Close()
		t.Fatal(err)
	}
	if err := project.Close(); err != nil {
		t.Fatal(err)
	}

	globalPath := filepath.Join(home, "global.db")
	global, err := bolt.Open(globalPath, 0o600, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := global.Update(func(tx *bolt.Tx) error {
		for _, name := range [][]byte{bucketConfig, bucketProjects, bucketBackups} {
			if _, err := tx.CreateBucket(name); err != nil {
				return err
			}
		}
		if err := tx.Bucket(bucketConfig).Put([]byte{0xff, 'k'}, []byte{0x00, 0xff, 'v'}); err != nil {
			return err
		}
		if err := tx.Bucket(bucketBackups).Put([]byte("42"), []byte("relative-project\trelative-backup")); err != nil {
			return err
		}
		return putGob(tx.Bucket(bucketProjects), []byte(projectRoot), model.ProjectRef{Name: "fixture", Path: projectRoot, LastSeen: stamp})
	}); err != nil {
		_ = global.Close()
		t.Fatal(err)
	}
	if err := global.Close(); err != nil {
		t.Fatal(err)
	}

	output := os.Getenv("PTRACK_JSON_STAGE_OUTPUT")
	if output == "" {
		output = filepath.Join(privateJSONTestDir(t), "stage")
	} else if !filepath.IsAbs(output) {
		t.Fatalf("PTRACK_JSON_STAGE_OUTPUT must be absolute: %q", output)
	}
	if _, err := os.Lstat(output); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("PTRACK_JSON_STAGE_OUTPUT must be absent: %q: %v", output, err)
	}
	if err := ExportJSONStage(home, output); err != nil {
		t.Fatal(err)
	}
	manifest := mustReadJSONFile(t, filepath.Join(output, "manifest.json"))
	if !bytes.Contains(manifest, []byte(`"quarantine_count":"1"`)) {
		t.Fatalf("cross-language fixture did not quarantine malformed capability: %s", manifest)
	}
}

func createJSONExportFixture(t *testing.T, detailed bool) jsonExportFixture {
	t.Helper()
	root := t.TempDir()
	home := filepath.Join(root, "home")
	if err := os.Mkdir(home, 0o700); err != nil {
		t.Fatal(err)
	}
	projectRoots := []string{filepath.Join(root, "zeta"), filepath.Join(root, "alpha")}
	projectPaths := make([]string, 0, len(projectRoots))
	for _, projectRoot := range projectRoots {
		metadata := filepath.Join(projectRoot, ".ptrack")
		if err := os.MkdirAll(metadata, 0o700); err != nil {
			t.Fatal(err)
		}
		path := filepath.Join(metadata, "ptrack.db")
		store, err := Open(path)
		if err != nil {
			t.Fatal(err)
		}
		if detailed && strings.HasSuffix(projectRoot, "alpha") {
			if _, err := store.AddPlan("migrate safely"); err != nil {
				t.Fatal(err)
			}
		}
		if err := store.Close(); err != nil {
			t.Fatal(err)
		}
		projectPaths = append(projectPaths, path)
	}
	if detailed {
		db, err := bolt.Open(projectPaths[1], 0o600, nil)
		if err != nil {
			t.Fatal(err)
		}
		approved := time.Date(2026, 8, 12, 12, 0, 0, 123, time.FixedZone("fixture", -7*60*60))
		capability := model.Capability{
			ID: 1, ModelVersion: model.CapabilityModelVersion, Revision: 1, Name: "api", Kind: model.CapabilityHTTP,
			AgentProfile: "agent-codex", Enabled: true, ApprovalDurationSeconds: 3600,
			ApprovedAt: approved, ExpiresAt: approved.Add(time.Hour), ScopeDigest: strings.Repeat("1", 64),
			Limits:    model.CapabilityLimits{TimeoutSeconds: 30, MaxRequestBytes: 1024, MaxResponseBytes: 2048, MaxOutputBytes: 4096, MaxRedirects: 1, MaxConcurrent: 1},
			Audit:     model.CapabilityAuditPolicy{Enabled: true, RetainLast: 10},
			HTTP:      &model.HTTPScope{BaseURL: "https://example.test", Methods: []string{"GET"}, PathPrefixes: []string{"/v1"}},
			CreatedAt: approved.Add(-time.Hour), UpdatedAt: approved,
		}
		if err := db.Update(func(tx *bolt.Tx) error {
			bucket := tx.Bucket(bucketCapabilities)
			if err := putGob(bucket, itob(1), capability); err != nil {
				return err
			}
			if err := bucket.Put(itob(2), []byte{}); err != nil {
				return err
			}
			return bucket.SetSequence(2)
		}); err != nil {
			_ = db.Close()
			t.Fatal(err)
		}
		if err := db.Close(); err != nil {
			t.Fatal(err)
		}
	}

	globalPath := filepath.Join(home, "global.db")
	db, err := bolt.Open(globalPath, 0o600, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := db.Update(func(tx *bolt.Tx) error {
		for _, bucket := range [][]byte{bucketConfig, bucketProjects, bucketBackups} {
			if _, err := tx.CreateBucket(bucket); err != nil {
				return err
			}
		}
		for _, projectRoot := range projectRoots {
			absolute, err := filepath.Abs(projectRoot)
			if err != nil {
				return err
			}
			if err := putGob(tx.Bucket(bucketProjects), []byte(absolute), model.ProjectRef{Name: filepath.Base(absolute), Path: absolute, LastSeen: time.Unix(123, 456)}); err != nil {
				return err
			}
		}
		if detailed {
			if err := tx.Bucket(bucketConfig).Put([]byte{0xff, 0x00, 'k'}, []byte{0xff, 0x00, 'v'}); err != nil {
				return err
			}
			return tx.Bucket(bucketBackups).Put([]byte("7"), []byte("relative-project\trelative-backup"))
		}
		return nil
	}); err != nil {
		_ = db.Close()
		t.Fatal(err)
	}
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	return jsonExportFixture{home: home, globalPath: globalPath, projectPaths: projectPaths}
}

func privateJSONTestDir(t *testing.T) string {
	t.Helper()
	directory := t.TempDir()
	if err := os.Chmod(directory, 0o700); err != nil {
		t.Fatal(err)
	}
	return directory
}

func snapshotJSONSources(t *testing.T, paths []string) map[string]jsonSourceSnapshot {
	t.Helper()
	result := make(map[string]jsonSourceSnapshot, len(paths))
	for _, path := range paths {
		info, err := os.Stat(path)
		if err != nil {
			t.Fatal(err)
		}
		content := mustReadJSONFile(t, path)
		result[path] = jsonSourceSnapshot{mode: info.Mode(), mtime: info.ModTime(), digest: sha256.Sum256(content), content: content}
	}
	return result
}

func assertJSONSourcesUnchanged(t *testing.T, paths []string, before map[string]jsonSourceSnapshot) {
	t.Helper()
	for _, path := range paths {
		info, err := os.Stat(path)
		if err != nil {
			t.Fatal(err)
		}
		content := mustReadJSONFile(t, path)
		afterDigest := sha256.Sum256(content)
		want := before[path]
		if info.Mode() != want.mode || !info.ModTime().Equal(want.mtime) || afterDigest != want.digest || !bytes.Equal(content, want.content) {
			t.Fatalf("source changed during export: %q", path)
		}
	}
}

func mustReadJSONFile(t *testing.T, path string) []byte {
	t.Helper()
	value, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return value
}

func assertMode(t *testing.T, path string, want os.FileMode) {
	t.Helper()
	info, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	if got := info.Mode().Perm(); got != want {
		t.Fatalf("mode(%q) = %#o, want %#o", path, got, want)
	}
}

func decodeJSONLines(t *testing.T, data []byte) []map[string]any {
	t.Helper()
	if !bytes.HasSuffix(data, []byte("\n")) {
		t.Fatal("JSONL is not LF terminated")
	}
	rawLines := bytes.Split(bytes.TrimSuffix(data, []byte("\n")), []byte("\n"))
	lines := make([]map[string]any, 0, len(rawLines))
	for _, raw := range rawLines {
		var line map[string]any
		if err := json.Unmarshal(raw, &line); err != nil {
			t.Fatalf("decode JSONL %q: %v", raw, err)
		}
		lines = append(lines, line)
	}
	return lines
}

func findJSONLine(t *testing.T, lines []map[string]any, lineType, bucket, key string) map[string]any {
	t.Helper()
	for _, line := range lines {
		encodedKey, _ := line["key"].(map[string]any)
		if line["type"] == lineType && line["bucket"] == bucket && encodedKey["value"] == key {
			return line
		}
	}
	t.Fatalf("missing %s line for %s/%s", lineType, bucket, key)
	return nil
}

func assertRawJSONRecord(t *testing.T, lines []map[string]any, bucket, key string, want []byte) {
	t.Helper()
	line := findJSONLine(t, lines, "record", bucket, key)
	value := line["value"].(map[string]any)
	if value["encoding"] != "hex" || value["bytes"] != hex.EncodeToString(want) || line["model_version"] != "0" {
		t.Fatalf("raw %s record = %#v", bucket, line)
	}
}
