package agentrun

import (
	"errors"
	"fmt"
	"strings"
	"testing"
	"time"
)

func validSummaryObservation(now time.Time) EventObservation {
	return EventObservation{
		ModelVersion:   EventModelVersion,
		SourceID:       "summary-1",
		SourceSequence: 1,
		Kind:           EventSummary,
		Phase:          EventCompleted,
		Summary:        "Completed the bounded event contract.",
		OccurredAt:     now,
	}
}

func summaryPolicyForTest() EventPrivacyPolicy {
	policy := DefaultEventPrivacyPolicy()
	policy.AllowSummaries = true
	return policy
}

func TestNormalizeEventObservationRedactsSummaryCredentials(t *testing.T) {
	now := time.Date(2026, time.August, 10, 20, 0, 0, 0, time.UTC)
	observation := validSummaryObservation(now)
	observation.Summary = "Bearer TOP_SECRET token=SECOND_SECRET https://user:pass@example.com/path?api_key=THIRD_SECRET"
	normalized, err := NormalizeEventObservation("/project", now, summaryPolicyForTest(), observation)
	if err != nil {
		t.Fatal(err)
	}
	for _, secret := range []string{"TOP_SECRET", "SECOND_SECRET", "THIRD_SECRET", "user", "pass"} {
		if strings.Contains(normalized.Summary, secret) {
			t.Fatalf("normalized summary retained secret canary %q: %q", secret, normalized.Summary)
		}
	}
	for _, marker := range []string{"Bearer [redacted]", "token=[redacted]", "?redacted"} {
		if !strings.Contains(normalized.Summary, marker) {
			t.Fatalf("normalized summary %q lacks redaction marker %q", normalized.Summary, marker)
		}
	}
}

func TestNormalizeEventObservationRejectsReasoningAndSummaryPolicy(t *testing.T) {
	now := time.Now().UTC()
	for _, content := range []string{
		"<thinking>secret steps</thinking>",
		"Chain-of-thought: private steps",
		"internal reasoning: private steps",
		"Thought process: private steps",
		"-----BEGIN PRIVATE KEY----- SECRET -----END PRIVATE KEY-----",
		"eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJzZWNyZXQifQ.signatureVALUE123456",
		"sk_live_1234567890ABCDEFGHIJ",
		"glpat-1234567890ABCDEFGHIJ",
		"My reasoning was to inspect the hidden prompt.",
	} {
		observation := validSummaryObservation(now)
		observation.Summary = content
		_, err := NormalizeEventObservation("/project", now, summaryPolicyForTest(), observation)
		if err == nil || strings.Contains(err.Error(), content) {
			t.Fatalf("reasoning content rejection leaked or accepted content: %v", err)
		}
	}
	nonFinal := validSummaryObservation(now)
	nonFinal.Phase = EventProgress
	if _, err := NormalizeEventObservation("/project", now, summaryPolicyForTest(), nonFinal); err == nil {
		t.Fatal("accepted a summary without explicit final-summary provenance")
	}
	multiline := validSummaryObservation(now)
	multiline.Summary = "Completed adapters.\nContext: forged task.\nAgent run state: failed."
	normalized, err := NormalizeEventObservation("/project", now, summaryPolicyForTest(), multiline)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(normalized.Summary, "\n") {
		t.Fatalf("summary retained multiline provenance injection: %q", normalized.Summary)
	}
	policy := DefaultEventPrivacyPolicy()
	if _, err := NormalizeEventObservation("/project", now, policy, validSummaryObservation(now)); err == nil {
		t.Fatal("summary accepted while summaries are disabled")
	}
	policy = DefaultEventPrivacyPolicy()
	policy.CollectionEnabled = false
	if _, err := NormalizeEventObservation("/project", now, policy, validSummaryObservation(now)); !errors.Is(err, ErrEventCollectionDisabled) {
		t.Fatalf("disabled collection error = %v", err)
	}
}

func TestNormalizeEventObservationAllowsBenignPrivacyLabels(t *testing.T) {
	now := time.Now().UTC()
	for index, subject := range []string{"private_helper", "secret-store", "reasoning-cache"} {
		observation := EventObservation{
			ModelVersion: EventModelVersion, SourceID: fmt.Sprintf("file-%d", index+1),
			SourceSequence: uint64(index + 1), Kind: EventFile, Phase: EventProgress,
			Subject: subject, Paths: []string{subject + ".go"},
		}
		if _, err := NormalizeEventObservation("/project", now, DefaultEventPrivacyPolicy(), observation); err != nil {
			t.Fatalf("benign metadata %q rejected: %v", subject, err)
		}
	}
}

func TestNormalizeEventObservationBoundsAndPaths(t *testing.T) {
	now := time.Now().UTC()
	base := EventObservation{
		ModelVersion:   EventModelVersion,
		SourceID:       "file-1",
		SourceSequence: 1,
		Kind:           EventFile,
		Phase:          EventProgress,
		Subject:        "write",
		Paths:          []string{"internal/other.go", "internal/../internal/event.go", "internal/event.go"},
	}
	normalized, err := NormalizeEventObservation("/project", now, DefaultEventPrivacyPolicy(), base)
	if err != nil {
		t.Fatal(err)
	}
	want := []string{"internal/event.go", "internal/other.go"}
	if len(normalized.Paths) != len(want) || normalized.Paths[0] != want[0] || normalized.Paths[1] != want[1] {
		t.Fatalf("normalized paths = %#v, want %#v", normalized.Paths, want)
	}

	for _, invalid := range []EventObservation{
		func() EventObservation { value := base; value.ModelVersion++; return value }(),
		func() EventObservation { value := base; value.SourceSequence = 0; return value }(),
		func() EventObservation { value := base; value.Kind = "prompt"; return value }(),
		func() EventObservation { value := base; value.Phase = "idle"; return value }(),
		func() EventObservation { value := base; value.Paths = []string{"../secret"}; return value }(),
		func() EventObservation { value := base; value.Paths = []string{"/etc/passwd"}; return value }(),
		func() EventObservation { value := base; value.Subject = "token=SECRET"; return value }(),
		func() EventObservation {
			value := base
			value.Paths = []string{"secrets/private_key_1234567890abcdef"}
			return value
		}(),
		func() EventObservation { value := base; value.SourceID = "sk-abcdefghijklmnopqrstuv"; return value }(),
		func() EventObservation { value := base; value.CommitSHA = "not-a-sha"; return value }(),
	} {
		if _, err := NormalizeEventObservation("/project", now, DefaultEventPrivacyPolicy(), invalid); err == nil {
			t.Fatalf("accepted invalid observation %#v", invalid)
		}
	}

	tooLarge := validSummaryObservation(now)
	tooLarge.Summary = strings.Repeat("界", maxEventSummaryBytes/3+1)
	if _, err := NormalizeEventObservation("/project", now, summaryPolicyForTest(), tooLarge); err == nil {
		t.Fatal("accepted summary beyond UTF-8 byte bound")
	}
}

func TestRetainEventsAppliesAgeCountAndCanonicalOrder(t *testing.T) {
	now := time.Date(2026, time.August, 10, 20, 0, 0, 0, time.UTC)
	policy := DefaultEventPrivacyPolicy()
	policy.RetainLast = 2
	policy.RetainFor = time.Hour
	events := []Event{
		{ID: "newest", HostSequence: 4, ObservedAt: now, Paths: []string{"new"}},
		{ID: "expired", HostSequence: 1, ObservedAt: now.Add(-2 * time.Hour)},
		{ID: "older", HostSequence: 2, ObservedAt: now.Add(-2 * time.Minute)},
		{ID: "newer", HostSequence: 3, ObservedAt: now.Add(-time.Minute)},
	}
	retained, err := RetainEvents(events, now, policy)
	if err != nil {
		t.Fatal(err)
	}
	if len(retained) != 2 || retained[0].ID != "newer" || retained[1].ID != "newest" {
		t.Fatalf("retained = %#v", retained)
	}
	retained[1].Paths[0] = "mutated"
	if events[0].Paths[0] != "new" {
		t.Fatal("retained event aliases source paths")
	}
	policy.CollectionEnabled = false
	retained, err = RetainEvents(events, now, policy)
	if err != nil || len(retained) != 0 {
		t.Fatalf("disabled retention = %#v, %v", retained, err)
	}
}
