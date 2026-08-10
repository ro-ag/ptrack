package store

import (
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
)

func TestCapabilityLifecycleAndAuditRetention(t *testing.T) {
	s := openTemp(t)
	created, err := s.AddCapability(model.Capability{
		Name:         "API",
		Kind:         model.CapabilityHTTP,
		AgentProfile: "agent-codex",
		HTTP: &model.HTTPScope{
			BaseURL: "https://example.com/api/",
			Methods: []string{"GET"},
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	if created.ID == 0 || created.ModelVersion != model.CapabilityModelVersion || created.Revision != 1 {
		t.Fatalf("unexpected capability identity/version: %+v", created)
	}

	created.Enabled = true
	created.Name = "API read"
	if err := s.UpdateCapability(created); err != nil {
		t.Fatal(err)
	}
	got, err := s.GetCapability(created.ID)
	if err != nil {
		t.Fatal(err)
	}
	if !got.Enabled || got.Name != "API read" || got.Revision != 2 || !got.CreatedAt.Equal(created.CreatedAt) {
		t.Errorf("updated capability = %+v", got)
	}
	listed, err := s.ListCapabilities()
	if err != nil || len(listed) != 1 {
		t.Fatalf("ListCapabilities = %+v, %v", listed, err)
	}

	for i := 0; i < 3; i++ {
		if _, err := s.AddCapabilityAudit(model.CapabilityAudit{
			CapabilityID: created.ID,
			Operation:    "GET",
			Success:      i == 2,
		}); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.PruneCapabilityAudits(created.ID, 2); err != nil {
		t.Fatal(err)
	}
	audits, err := s.ListCapabilityAudits(created.ID, 0)
	if err != nil || len(audits) != 2 || !audits[0].Success {
		t.Fatalf("audits = %+v, %v", audits, err)
	}

	if err := s.DeleteCapability(created.ID); err != nil {
		t.Fatal(err)
	}
	if _, err := s.GetCapability(created.ID); err != ErrNotFound {
		t.Fatalf("deleted capability error = %v", err)
	}
	audits, err = s.ListCapabilityAudits(created.ID, 0)
	if err != nil || len(audits) != 2 {
		t.Fatalf("retained audits = %+v, %v", audits, err)
	}
}

func TestCapabilityMaterialEditRevokesApproval(t *testing.T) {
	s := openTemp(t)
	created, err := s.AddCapability(model.Capability{
		Name: "repo", Kind: model.CapabilityGit, AgentProfile: "agent-codex", Enabled: true,
		Git: &model.GitScope{RemoteName: "origin", RemoteURL: "https://example.com/repo.git"},
	})
	if err != nil {
		t.Fatal(err)
	}
	created.Git.RemoteURL = "https://example.com/other.git"
	if err := s.UpdateCapability(created); err != nil {
		t.Fatal(err)
	}
	got, err := s.GetCapability(created.ID)
	if err != nil {
		t.Fatal(err)
	}
	if got.Enabled || !got.ApprovedAt.IsZero() || !got.ExpiresAt.IsZero() {
		t.Fatalf("material edit did not revoke approval: %+v", got)
	}
}

func TestCapabilityAuditPolicyEditRevokesApproval(t *testing.T) {
	s := openTemp(t)
	created, err := s.AddCapability(model.Capability{
		Name: "api", Kind: model.CapabilityHTTP, AgentProfile: "agent-codex", Enabled: true,
		Audit: model.CapabilityAuditPolicy{Enabled: true, RetainLast: 100},
		HTTP:  &model.HTTPScope{BaseURL: "https://example.com", Methods: []string{"GET"}},
	})
	if err != nil {
		t.Fatal(err)
	}
	created.Audit.Enabled = false
	if err := s.UpdateCapability(created); err != nil {
		t.Fatal(err)
	}
	got, err := s.GetCapability(created.ID)
	if err != nil {
		t.Fatal(err)
	}
	if got.Enabled || !got.ApprovedAt.IsZero() || !got.ExpiresAt.IsZero() {
		t.Fatalf("audit edit retained approval: %+v", got)
	}
}

func TestCapabilityBucketsCreatedByMigration(t *testing.T) {
	s := openTemp(t)
	if _, err := s.AddCapability(model.Capability{Name: "x"}); err != nil {
		t.Errorf("capabilities bucket unusable: %v", err)
	}
	if _, err := s.AddCapabilityAudit(model.CapabilityAudit{}); err != nil {
		t.Errorf("capability audit bucket unusable: %v", err)
	}
}
