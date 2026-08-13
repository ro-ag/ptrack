package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"encoding/gob"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

// TestCrossLanguageJSONFixture is the stable entry point used by the Rust
// importer acceptance test. It writes only disposable test databases.
func TestCrossLanguageJSONFixture(t *testing.T) {
	root := filepath.Join(t.TempDir(), "private")
	if err := createPrivateExportDirectory(root); err != nil {
		t.Fatal(err)
	}
	requirePrivateTestPath(t, root, true)
	home := filepath.Join(root, "home")
	projectRoot := filepath.Join(root, "project")
	projectMetadata := filepath.Join(projectRoot, ".ptrack")
	for _, directory := range []string{home, projectRoot, projectMetadata} {
		if err := os.Mkdir(directory, 0o700); err != nil {
			t.Fatal(err)
		}
		requirePrivateTestPath(t, directory, true)
	}

	stamp := time.Date(2026, 8, 12, 19, 20, 21, 987654321, time.FixedZone("fixture", -7*60*60))
	projectPath := filepath.Join(projectMetadata, "ptrack.db")
	project, err := bolt.Open(projectPath, 0o600, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := project.Update(func(tx *bolt.Tx) error {
		for _, spec := range projectMigrationBuckets {
			if _, err := tx.CreateBucket([]byte(spec.name)); err != nil {
				return err
			}
		}
		meta := model.Meta{Goal: "ship safely", Summary: "cross-language fixture", ActivePlan: 1, CreatedAt: stamp, UpdatedAt: stamp, FormatVersion: CurrentFormat, LastWriteVersion: "fixture"}
		milestone := model.Milestone{ID: 1, Title: "migration", Status: model.MilestoneOpen, Due: stamp.Add(24 * time.Hour), CreatedAt: stamp, UpdatedAt: stamp}
		plan := model.Plan{ID: 1, Title: "parity", Status: model.PlanActive, MilestoneID: 1, CreatedAt: stamp, UpdatedAt: stamp}
		task := model.Task{ID: 1, PlanID: 1, Title: "convert", Status: model.TaskDoing, CreatedAt: stamp, UpdatedAt: stamp}
		note := model.Note{ID: 1, Target: model.TargetTask, TargetID: 1, Kind: model.MemoryDecision, Body: "preserve original", CreatedAt: stamp}
		issue := model.Issue{ID: 1, Title: "risk", Body: "verify every hash", Status: model.IssueOpen, Severity: model.SeverityHigh, TaskID: 1, CreatedAt: stamp, UpdatedAt: stamp}
		commit := model.Commit{ID: 1, SHA: strings.Repeat("a", 40), Subject: "fixture", PlanID: 1, TaskID: 1, CreatedAt: stamp}
		capability := model.Capability{
			ID: 1, ModelVersion: model.CapabilityModelVersion, Revision: 2, Name: "origin", Kind: model.CapabilityGit,
			AgentProfile: "agent-codex", Enabled: true, ApprovalDurationSeconds: 3600,
			ApprovedAt: stamp, ExpiresAt: stamp.Add(time.Hour), ScopeDigest: strings.Repeat("2", 64),
			Limits:    model.CapabilityLimits{TimeoutSeconds: 30, MaxRequestBytes: 1024, MaxResponseBytes: 2048, MaxOutputBytes: 4096, MaxConcurrent: 1},
			Audit:     model.CapabilityAuditPolicy{Enabled: true, RetainLast: 50},
			Git:       &model.GitScope{RemoteName: "origin", RemoteURL: "https://example.test/repo.git", Operations: []string{"fetch"}, Branches: []string{"main"}, Refspecs: []string{"refs/heads/main:refs/remotes/origin/main"}},
			CreatedAt: stamp.Add(-time.Hour), UpdatedAt: stamp,
		}
		audit := model.CapabilityAudit{ID: 1, CapabilityID: 1, AgentProfile: "agent-codex", Kind: model.CapabilityGit, Operation: "fetch", Target: "origin", Success: true, ErrorClass: "none", DurationMillis: 15, RequestBytes: 10, ResponseBytes: 20, CreatedAt: stamp}
		digest := sha256.Sum256([]byte("receipt"))
		receipt := memoryWritebackRecord{Digest: digest, Sequence: 1, Kind: model.MemoryDecision, NoteID: 1}
		values := []struct {
			bucket []byte
			key    []byte
			value  any
		}{
			{bucketMeta, keyMeta, meta}, {bucketMilestones, testU64Key(1), milestone},
			{bucketPlans, testU64Key(1), plan}, {bucketTasks, testU64Key(1), task},
			{bucketNotes, testU64Key(1), note}, {bucketIssues, testU64Key(1), issue},
			{bucketCommits, testU64Key(1), commit}, {bucketCapabilities, testU64Key(1), capability},
			{bucketCapabilityAudits, testU64Key(1), audit}, {bucketMemoryWritebacks, []byte("fixture-request"), receipt},
		}
		for _, item := range values {
			if err := testPutGob(tx.Bucket(item.bucket), item.key, item.value); err != nil {
				return err
			}
		}
		for _, bucket := range [][]byte{bucketMilestones, bucketPlans, bucketTasks, bucketNotes, bucketIssues, bucketCommits, bucketCapabilityAudits, bucketMemoryWritebacks} {
			if err := tx.Bucket(bucket).SetSequence(1); err != nil {
				return err
			}
		}
		if err := tx.Bucket(bucketCapabilities).Put(testU64Key(2), nil); err != nil {
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
	requirePrivateTestPath(t, projectPath, false)

	globalPath := filepath.Join(home, "global.db")
	global, err := bolt.Open(globalPath, 0o600, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := global.Update(func(tx *bolt.Tx) error {
		for _, spec := range globalMigrationBuckets {
			if _, err := tx.CreateBucket([]byte(spec.name)); err != nil {
				return err
			}
		}
		if err := tx.Bucket(bucketConfig).Put([]byte{0xff, 'k'}, []byte{0x00, 0xff, 'v'}); err != nil {
			return err
		}
		return testPutGob(tx.Bucket(bucketProjects), []byte(projectRoot), model.ProjectRef{Name: "fixture", Path: projectRoot, LastSeen: stamp})
	}); err != nil {
		_ = global.Close()
		t.Fatal(err)
	}
	if err := global.Close(); err != nil {
		t.Fatal(err)
	}
	requirePrivateTestPath(t, globalPath, false)

	output := os.Getenv("PTRACK_JSON_STAGE_OUTPUT")
	if output == "" {
		output = filepath.Join(root, "stage")
	} else if !filepath.IsAbs(output) {
		t.Fatalf("PTRACK_JSON_STAGE_OUTPUT must be absolute: %q", output)
	}
	if _, err := os.Lstat(output); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("PTRACK_JSON_STAGE_OUTPUT must be absent: %q: %v", output, err)
	}
	if err := ExportJSONStage(home, output); err != nil {
		t.Fatal(err)
	}
	manifest, err := os.ReadFile(filepath.Join(output, "manifest.json"))
	if err != nil || !bytes.Contains(manifest, []byte(`"quarantine_count":"1"`)) {
		t.Fatalf("invalid exported manifest: %v: %s", err, manifest)
	}
}

func testPutGob(bucket *bolt.Bucket, key []byte, value any) error {
	var encoded bytes.Buffer
	if err := gob.NewEncoder(&encoded).Encode(value); err != nil {
		return err
	}
	return bucket.Put(key, encoded.Bytes())
}

func testU64Key(value uint64) []byte {
	key := make([]byte, 8)
	binary.BigEndian.PutUint64(key, value)
	return key
}
