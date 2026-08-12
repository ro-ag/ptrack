package store

import (
	"bytes"
	"encoding/gob"
	"encoding/hex"
	"strings"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
)

func TestEncodeLegacyRecordGoldenPayloads(t *testing.T) {
	stamp := time.Unix(1, 2).In(time.FixedZone("fixture", 3600))
	digest := [32]byte{}
	for index := range digest {
		digest[index] = byte(index)
	}
	capabilityDigest := strings.Repeat("ab", 32)
	fixtures := []struct {
		name       string
		collection string
		key        []byte
		value      any
		kind       nativeRecordKind
		golden     string
	}{
		{
			name: "meta", collection: "meta", key: []byte("meta"), kind: nativeKindMeta,
			value:  model.Meta{Goal: "g", Summary: "s", ActivePlan: 1, CreatedAt: stamp, FormatVersion: 5, LastWriteVersion: "v"},
			golden: "0000000167000000017300000000000000010100000000000000010000000200000e100000000000000000050000000176",
		},
		{
			name: "plan", collection: "plans", key: itob(2), kind: nativeKindPlan,
			value:  model.Plan{ID: 2, Title: "p", Status: model.PlanActive, MilestoneID: 3, Order: 1, CreatedAt: stamp, UpdatedAt: stamp},
			golden: "0000000000000002000000017001000000000000000300000000000000010100000000000000010000000200000e100100000000000000010000000200000e10",
		},
		{
			name: "task", collection: "tasks", key: itob(3), kind: nativeKindTask,
			value:  model.Task{ID: 3, PlanID: 2, Title: "t", Status: model.TaskBlocked, Order: 4, CreatedAt: stamp, UpdatedAt: stamp},
			golden: "0000000000000003000000000000000200000001740400000000000000040100000000000000010000000200000e100100000000000000010000000200000e10",
		},
		{
			name: "note", collection: "notes", key: itob(4), kind: nativeKindNote,
			value:  model.Note{ID: 4, Target: model.TargetTask, TargetID: 3, Kind: model.MemoryDecision, Body: "n", CreatedAt: stamp},
			golden: "000000000000000403000000000000000301000000016e0100000000000000010000000200000e10",
		},
		{
			name: "milestone", collection: "milestones", key: itob(5), kind: nativeKindMilestone,
			value:  model.Milestone{ID: 5, Title: "m", Status: model.MilestoneDone, Due: time.Time{}, Order: 6, CreatedAt: stamp, UpdatedAt: stamp},
			golden: "0000000000000005000000016d020000000000000000060100000000000000010000000200000e100100000000000000010000000200000e10",
		},
		{
			name: "issue", collection: "issues", key: itob(6), kind: nativeKindIssue,
			value:  model.Issue{ID: 6, Title: "i", Body: "b", Status: model.IssueClosed, Severity: model.SeverityCritical, TaskID: 3, CreatedAt: stamp, UpdatedAt: stamp},
			golden: "000000000000000600000001690000000162020400000000000000030100000000000000010000000200000e100100000000000000010000000200000e10",
		},
		{
			name: "commit", collection: "commits", key: itob(7), kind: nativeKindCommit,
			value:  model.Commit{ID: 7, SHA: "a", Subject: "c", PlanID: 2, TaskID: 3, CreatedAt: stamp},
			golden: "000000000000000700000001610000000163000000000000000200000000000000030100000000000000010000000200000e10",
		},
		{
			name: "capability", collection: "capabilities", key: itob(8), kind: nativeKindCapability,
			value: model.Capability{
				ID: 8, ModelVersion: 1, Revision: 2, Name: "c", Kind: model.CapabilityHTTP,
				AgentProfile: "p", Enabled: true, ApprovalDurationSeconds: 60,
				ApprovedAt: stamp, ExpiresAt: stamp.Add(time.Minute), ScopeDigest: capabilityDigest,
				Limits:    model.CapabilityLimits{TimeoutSeconds: 1, MaxRequestBytes: 2, MaxResponseBytes: 3, MaxOutputBytes: 4, MaxConcurrent: 1},
				Audit:     model.CapabilityAuditPolicy{RetainLast: 1},
				HTTP:      &model.HTTPScope{BaseURL: "https://x", Methods: []string{"GET"}, PathPrefixes: []string{"/"}},
				UpdatedAt: stamp,
			},
			golden: "000000000000000800000000000000010000000000000002000000016301000000017000000000000000003c0000abababababababababababababababababababababababababababababababab000000000000000100000000000000020000000000000003000000000000000400000000000000000000000000000001000000000000000001010000000968747470733a2f2f78000000010000000347455400000001000000012f0000000100000000000000010000000200000e10",
		},
		{
			name: "capability audit", collection: "capability_audits", key: itob(9), kind: nativeKindCapabilityAudit,
			value:  model.CapabilityAudit{ID: 9, CapabilityID: 8, AgentProfile: "p", Kind: model.CapabilityHTTP, Operation: "get", Target: "https://x", Success: true, ErrorClass: "none", DurationMillis: 1, RequestBytes: 2, ResponseBytes: 3, Redirects: 4, CreatedAt: stamp},
			golden: "00000000000000090000000000000008000000017001000000036765740000000968747470733a2f2f7801000000046e6f6e6500000000000000010000000000000002000000000000000300000000000000040100000000000000010000000200000e10",
		},
		{
			name: "memory writeback", collection: "memory_writebacks", key: []byte("request-1"), kind: nativeKindMemoryWriteback,
			value:  memoryWritebackRecord{Digest: digest, Sequence: 1, Kind: model.MemorySummary},
			golden: "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f0000000000000001040000000000000000",
		},
		{
			name: "project ref", collection: "projects", key: []byte("/p"), kind: nativeKindProjectRef,
			value:  model.ProjectRef{Name: "n", Path: "/p", LastSeen: stamp},
			golden: "000000016e000000022f700100000000000000010000000200000e10",
		},
	}

	for _, fixture := range fixtures {
		t.Run(fixture.name, func(t *testing.T) {
			raw := mustGobRecord(t, fixture.value)
			encoded, err := encodeLegacyRecord(fixture.collection, fixture.key, raw)
			if err != nil {
				t.Fatal(err)
			}
			if encoded.Kind != fixture.kind || encoded.Codec != nativeRecordCodec || encoded.PayloadSchema != nativePayloadSchema {
				t.Fatalf("metadata = kind %d codec %d schema %d", encoded.Kind, encoded.Codec, encoded.PayloadSchema)
			}
			if actual := hex.EncodeToString(encoded.Payload); actual != fixture.golden {
				t.Fatalf("golden payload mismatch\nactual: %s\nwant:   %s", actual, fixture.golden)
			}
		})
	}
}

func TestEncodeLegacyRawGlobalRecords(t *testing.T) {
	fixtures := []struct {
		collection string
		key        []byte
		value      []byte
		kind       nativeRecordKind
	}{
		{collection: "config", key: []byte("theme"), value: []byte("dark"), kind: nativeKindGlobalConfig},
		{collection: "backups", key: []byte("1"), value: []byte("/p\t/b"), kind: nativeKindGlobalBackup},
	}
	for _, fixture := range fixtures {
		encoded, err := encodeLegacyRecord(fixture.collection, fixture.key, fixture.value)
		if err != nil {
			t.Fatal(err)
		}
		if encoded.Kind != fixture.kind || encoded.Codec != legacyRawCodec || encoded.PayloadSchema != 0 || !bytes.Equal(encoded.Payload, fixture.value) {
			t.Fatalf("unexpected raw encoding: %#v", encoded)
		}
		encoded.Payload[0] ^= 0xff
		if bytes.Equal(encoded.Payload, fixture.value) {
			t.Fatal("raw payload aliases caller memory")
		}
	}
}

func TestEncodeLegacyRecordRejectsTrailingOrCorruptGobForEveryShape(t *testing.T) {
	shapes := []struct {
		collection string
		key        []byte
		value      any
	}{
		{"meta", []byte("meta"), model.Meta{}},
		{"plans", itob(1), model.Plan{ID: 1}},
		{"tasks", itob(1), model.Task{ID: 1}},
		{"notes", itob(1), model.Note{ID: 1}},
		{"milestones", itob(1), model.Milestone{ID: 1}},
		{"issues", itob(1), model.Issue{ID: 1}},
		{"commits", itob(1), model.Commit{ID: 1}},
		{"capabilities", itob(1), model.Capability{ID: 1}},
		{"capability_audits", itob(1), model.CapabilityAudit{ID: 1}},
		{"memory_writebacks", []byte("r"), memoryWritebackRecord{}},
		{"projects", []byte("/p"), model.ProjectRef{Path: "/p"}},
	}
	for _, shape := range shapes {
		t.Run(shape.collection, func(t *testing.T) {
			trailing := mustGobRecords(t, shape.value, uint64(99))
			if _, err := encodeLegacyRecord(shape.collection, shape.key, trailing); err == nil || !strings.Contains(err.Error(), "trailing") && !strings.Contains(err.Error(), "more than one") {
				t.Fatalf("trailing gob error = %v", err)
			}
			valid := mustGobRecord(t, shape.value)
			if _, err := encodeLegacyRecord(shape.collection, shape.key, append(append([]byte(nil), valid...), 0)); err == nil {
				t.Fatal("arbitrary trailing gob byte accepted")
			}
			if _, err := encodeLegacyRecord(shape.collection, shape.key, valid[:len(valid)-1]); err == nil {
				t.Fatal("truncated gob accepted")
			}
		})
	}
}

func TestEncodeLegacyRecordValidatesRecordContracts(t *testing.T) {
	stamp := time.Unix(1, 0)
	validCapability := model.Capability{
		ID: 1, ModelVersion: 1, Revision: 1, Name: "c", Kind: model.CapabilityHTTP,
		AgentProfile: "p", ApprovalDurationSeconds: 60, ScopeDigest: strings.Repeat("ab", 32),
		Limits: model.CapabilityLimits{TimeoutSeconds: 1, MaxRequestBytes: 1, MaxResponseBytes: 1, MaxOutputBytes: 1, MaxConcurrent: 1},
		HTTP:   &model.HTTPScope{BaseURL: "https://x", Methods: []string{"GET"}, PathPrefixes: []string{"/"}},
	}
	tests := []struct {
		name       string
		collection string
		key        []byte
		value      any
		contains   string
	}{
		{"meta key", "meta", []byte("wrong"), model.Meta{}, "meta key"},
		{"id mismatch", "plans", itob(2), model.Plan{ID: 1, Status: model.PlanActive}, "does not match"},
		{"plan enum", "plans", itob(1), model.Plan{ID: 1, Status: "future"}, "unknown plan status"},
		{"task enum", "tasks", itob(1), model.Task{ID: 1, PlanID: 1, Status: "future"}, "unknown task status"},
		{"note target", "notes", itob(1), model.Note{ID: 1, Target: "future"}, "unknown note target"},
		{"note summary", "notes", itob(1), model.Note{ID: 1, Target: model.TargetProject, Kind: model.MemorySummary}, "unsupported memory kind"},
		{"milestone enum", "milestones", itob(1), model.Milestone{ID: 1, Status: "future"}, "unknown milestone status"},
		{"issue status", "issues", itob(1), model.Issue{ID: 1, Status: "future", Severity: model.SeverityLow}, "unknown issue status"},
		{"issue severity", "issues", itob(1), model.Issue{ID: 1, Status: model.IssueOpen, Severity: "future"}, "unknown issue severity"},
		{"capability digest", "capabilities", itob(1), func() model.Capability { value := validCapability; value.ScopeDigest = "bad"; return value }(), "scope digest"},
		{"capability zero digest", "capabilities", itob(1), func() model.Capability {
			value := validCapability
			value.ScopeDigest = strings.Repeat("00", 32)
			return value
		}(), "nonzero"},
		{"capability scopes", "capabilities", itob(1), func() model.Capability { value := validCapability; value.Git = &model.GitScope{}; return value }(), "exactly"},
		{"capability approval", "capabilities", itob(1), func() model.Capability {
			value := validCapability
			value.Enabled = true
			value.ApprovedAt = stamp
			return value
		}(), "approval window"},
		{"audit counters", "capability_audits", itob(1), model.CapabilityAudit{ID: 1, CapabilityID: 1, Kind: model.CapabilityHTTP, Success: true, ErrorClass: "none", DurationMillis: -1}, "nonnegative"},
		{"audit class", "capability_audits", itob(1), model.CapabilityAudit{ID: 1, CapabilityID: 1, Kind: model.CapabilityHTTP, Success: true, ErrorClass: "internal"}, "inconsistent"},
		{"memory summary receipt", "memory_writebacks", []byte("r"), memoryWritebackRecord{Digest: [32]byte{1}, Sequence: 1, Kind: model.MemorySummary, NoteID: 1}, "must not"},
		{"memory typed receipt", "memory_writebacks", []byte("r"), memoryWritebackRecord{Digest: [32]byte{1}, Sequence: 1, Kind: model.MemoryDecision}, "must contain"},
		{"project key", "projects", []byte("/other"), model.ProjectRef{Path: "/p"}, "must equal"},
		{"project path", "projects", []byte("relative"), model.ProjectRef{Name: "n", Path: "relative"}, "absolute"},
		{"invalid UTF-8", "plans", itob(1), model.Plan{ID: 1, Title: string([]byte{0xff}), Status: model.PlanActive}, "UTF-8"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			_, err := encodeLegacyRecord(test.collection, test.key, mustGobRecord(t, test.value))
			if err == nil || !strings.Contains(err.Error(), test.contains) {
				t.Fatalf("error = %v, want containing %q", err, test.contains)
			}
		})
	}
}

func TestEncodeLegacyCapabilityForcesReapproval(t *testing.T) {
	stamp := time.Unix(1, 0)
	value := model.Capability{
		ID: 1, ModelVersion: 1, Revision: 1, Name: "c", Kind: model.CapabilityHTTP,
		AgentProfile: "p", Enabled: true, ApprovalDurationSeconds: 60,
		ApprovedAt: stamp, ExpiresAt: stamp.Add(time.Minute), ScopeDigest: strings.Repeat("ab", 32),
		Limits: model.CapabilityLimits{TimeoutSeconds: 1, MaxRequestBytes: 1, MaxResponseBytes: 1, MaxOutputBytes: 1, MaxConcurrent: 1},
		HTTP:   &model.HTTPScope{BaseURL: "https://x", Methods: []string{"GET"}, PathPrefixes: []string{"/"}},
	}
	encoded, err := encodeLegacyRecord("capabilities", itob(1), mustGobRecord(t, value))
	if err != nil {
		t.Fatal(err)
	}
	// Six fixed/string fields precede Enabled: ID, model version, revision,
	// name, kind, profile. Assert the encoded flag and two timestamp tags are
	// zero, while the following digest is retained.
	offset := 8 + 8 + 8 + 4 + len(value.Name) + 1 + 4 + len(value.AgentProfile)
	if got := encoded.Payload[offset]; got != 0 {
		t.Fatalf("migrated Enabled tag = %d, want revoked", got)
	}
	offset += 1 + 8
	if encoded.Payload[offset] != 0 || encoded.Payload[offset+1] != 0 {
		t.Fatal("migrated approval timestamps were retained")
	}
	digestOffset := offset + 2
	if got := hex.EncodeToString(encoded.Payload[digestOffset : digestOffset+32]); got != value.ScopeDigest {
		t.Fatalf("scope digest = %s", got)
	}
}

func TestEncodeLegacyRawGlobalContracts(t *testing.T) {
	tests := []struct {
		collection string
		key        []byte
		value      []byte
	}{
		{"config", nil, []byte("v")},
		{"config", []byte("k"), []byte{0xff}},
		{"backups", []byte("01"), []byte("/p\t/b")},
		{"backups", []byte("1"), []byte("relative\t/b")},
		{"backups", []byte("1"), []byte("/p\t/b\textra")},
	}
	for _, test := range tests {
		if _, err := encodeLegacyRecord(test.collection, test.key, test.value); err == nil {
			t.Fatalf("accepted %s key %q value %q", test.collection, test.key, test.value)
		}
	}
}

func mustGobRecord(t *testing.T, value any) []byte {
	t.Helper()
	return mustGobRecords(t, value)
}

func mustGobRecords(t *testing.T, values ...any) []byte {
	t.Helper()
	var buffer bytes.Buffer
	encoder := gob.NewEncoder(&buffer)
	for _, value := range values {
		if err := encoder.Encode(value); err != nil {
			t.Fatal(err)
		}
	}
	return buffer.Bytes()
}
