package capability

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

func TestAuditRecordIsBoundedAndRedactedInDatabase(t *testing.T) {
	dbPath := filepath.Join(t.TempDir(), "ptrack.db")
	s, err := store.Open(dbPath)
	if err != nil {
		t.Fatal(err)
	}
	capability := model.Capability{
		ID: 42, Kind: model.CapabilityHTTP, AgentProfile: "agent-codex",
		Audit: model.CapabilityAuditPolicy{Enabled: true, RetainLast: 2},
	}
	recorder := Recorder{Store: s, Now: func() time.Time { return time.Unix(1, 0) }}
	secret := "CANARY_SUPER_SECRET_TOKEN"
	for index := 0; index < 3; index++ {
		err := recorder.Record(context.Background(), capability, AuditEvent{
			Operation:     "GET",
			Target:        "https://user:" + secret + "@example.com/api/" + secret + "?token=" + secret,
			ErrorClass:    secret,
			Duration:      48 * time.Hour,
			RequestBytes:  -1,
			ResponseBytes: 1 << 50,
			Redirects:     100,
		})
		if err != nil {
			t.Fatal(err)
		}
	}
	audits, err := s.ListCapabilityAudits(capability.ID, 0)
	if err != nil {
		t.Fatal(err)
	}
	if len(audits) != 2 {
		t.Fatalf("audit count = %d want 2", len(audits))
	}
	got := audits[0]
	if got.Target != "https://example.com" || strings.Contains(got.Target, secret) || got.ErrorClass != "internal" || got.DurationMillis != (24*time.Hour).Milliseconds() || got.RequestBytes != 0 || got.ResponseBytes != 1<<40 || got.Redirects != maxRedirects {
		t.Fatalf("audit was not sanitized: %+v", got)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	raw, err := os.ReadFile(dbPath)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(raw), secret) {
		t.Fatal("secret-bearing audit input persisted in raw database")
	}
}

func TestDisabledAuditDoesNotWrite(t *testing.T) {
	s, err := store.Open(filepath.Join(t.TempDir(), "ptrack.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	recorder := Recorder{Store: s}
	if err := recorder.Record(context.Background(), model.Capability{ID: 9}, AuditEvent{Operation: "GET"}); err != nil {
		t.Fatal(err)
	}
	audits, err := s.ListCapabilityAudits(9, 0)
	if err != nil || len(audits) != 0 {
		t.Fatalf("disabled audit wrote records: %+v, %v", audits, err)
	}
}
