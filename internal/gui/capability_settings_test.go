package gui

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

func capabilitySettingsApp(t *testing.T) *App {
	t.Helper()
	root := t.TempDir()
	if err := os.Mkdir(filepath.Join(root, ".ptrack"), 0o755); err != nil {
		t.Fatal(err)
	}
	dbPath := filepath.Join(root, ".ptrack", "ptrack.db")
	s, err := store.Open(dbPath)
	if err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	app, err := newAppWithTerminal(dbPath, 0, nil, nil)
	if err != nil {
		t.Fatal(err)
	}
	return app
}

func TestCapabilitySettingsLifecycleRequiresPreviewDigest(t *testing.T) {
	app := capabilitySettingsApp(t)
	draft := model.Capability{
		Name: "api", Kind: model.CapabilityHTTP, AgentProfile: "agent-codex",
		Audit: model.CapabilityAuditPolicy{Enabled: true},
		HTTP:  &model.HTTPScope{BaseURL: "https://example.com/api", Methods: []string{"GET"}},
	}
	preview, err := app.PreviewCapabilityV2(1, draft)
	if err != nil || preview.View.EffectiveScope == "" || preview.View.Capability.ScopeDigest == "" {
		t.Fatalf("preview=%+v err=%v", preview, err)
	}
	saved, err := app.SaveCapabilityV2(1, preview.View.Capability)
	if err != nil || saved.View.State != "disabled" || saved.View.Capability.ID == 0 {
		t.Fatalf("saved=%+v err=%v", saved, err)
	}
	if _, err := app.EnableCapabilityV2(1, saved.View.Capability.ID, "stale"); err == nil {
		t.Fatal("stale digest enabled capability")
	}
	enabled, err := app.EnableCapabilityV2(1, saved.View.Capability.ID, saved.View.Capability.ScopeDigest)
	if err != nil || enabled.View.State != "enabled" {
		t.Fatalf("enabled=%+v err=%v", enabled, err)
	}
	expired, err := app.ExpireCapabilityV2(1, saved.View.Capability.ID)
	if err != nil || expired.View.State != "expired" {
		t.Fatalf("expired=%+v err=%v", expired, err)
	}
	disabled, err := app.DisableCapabilityV2(1, saved.View.Capability.ID)
	if err != nil || disabled.View.State != "disabled" {
		t.Fatalf("disabled=%+v err=%v", disabled, err)
	}
	if _, err := app.RemoveCapabilityV2(1, saved.View.Capability.ID); err != nil {
		t.Fatal(err)
	}
	settings, err := app.GetCapabilitiesV2(1)
	if err != nil || len(settings.Capabilities) != 0 {
		t.Fatalf("settings=%+v err=%v", settings, err)
	}
}

func TestCapabilityMaterialEditDisablesApproval(t *testing.T) {
	app := capabilitySettingsApp(t)
	draft := model.Capability{
		Name: "api", Kind: model.CapabilityHTTP, AgentProfile: "agent-codex",
		HTTP: &model.HTTPScope{BaseURL: "https://example.com/api", Methods: []string{"GET"}},
	}
	saved, err := app.SaveCapabilityV2(1, draft)
	if err != nil {
		t.Fatal(err)
	}
	enabled, err := app.EnableCapabilityV2(1, saved.View.Capability.ID, saved.View.Capability.ScopeDigest)
	if err != nil {
		t.Fatal(err)
	}
	edited := enabled.View.Capability
	edited.HTTP.PathPrefixes = []string{"/api/admin"}
	updated, err := app.SaveCapabilityV2(1, edited)
	if err != nil {
		t.Fatal(err)
	}
	if updated.View.State != "disabled" || !updated.View.Capability.ApprovedAt.IsZero() {
		t.Fatalf("material edit retained approval: %+v", updated.View)
	}
}
