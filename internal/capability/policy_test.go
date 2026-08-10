package capability

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
)

func approvedSSH(t *testing.T, scope *model.SSHScope) (model.Capability, time.Time) {
	t.Helper()
	now := time.Date(2026, 8, 9, 12, 0, 0, 0, time.UTC)
	draft := model.Capability{
		Name: "ssh", Kind: model.CapabilitySSH, AgentProfile: "agent-codex",
		SSH: scope,
	}
	preview, err := Normalize(draft)
	if err != nil {
		t.Fatal(err)
	}
	approved, err := Approve(preview.Capability, preview.ScopeDigest, now)
	if err != nil {
		t.Fatal(err)
	}
	return approved, now
}

func approvedCapability(t *testing.T, draft model.Capability) (model.Capability, time.Time) {
	t.Helper()
	now := time.Date(2026, 8, 9, 12, 0, 0, 0, time.UTC)
	preview, err := Normalize(draft)
	if err != nil {
		t.Fatal(err)
	}
	approved, err := Approve(preview.Capability, preview.ScopeDigest, now)
	if err != nil {
		t.Fatal(err)
	}
	return approved, now
}

func TestAuthorizeRejectsStaleProfileDisabledAndExpiredGrants(t *testing.T) {
	key := "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA=="
	capability, now := approvedSSH(t, &model.SSHScope{Host: "example.com", User: "deploy", HostKey: key, AllowGit: true})

	for name, mutate := range map[string]func(*model.Capability){
		"profile":  func(c *model.Capability) {},
		"disabled": func(c *model.Capability) { c.Enabled = false },
		"expired":  func(c *model.Capability) { c.ExpiresAt = now },
		"stale":    func(c *model.Capability) { c.SSH.Host = "other.example.com" },
	} {
		t.Run(name, func(t *testing.T) {
			candidate := capability
			copyScope := *capability.SSH
			candidate.SSH = &copyScope
			mutate(&candidate)
			profile := "agent-codex"
			if name == "profile" {
				profile = "agent-claude"
			}
			if _, err := Authorize(candidate, profile, now); err == nil {
				t.Fatal("authorization unexpectedly succeeded")
			}
		})
	}
}

func TestSSHGrantsDoNotCompose(t *testing.T) {
	key := "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA=="
	operations := []SSHOperation{
		SSHGit, SSHRemoteCommand, SSHUpload, SSHDownload,
		SSHLocalForward, SSHRemoteForward,
	}
	for _, approvedOperation := range operations {
		t.Run(string(approvedOperation), func(t *testing.T) {
			scope := &model.SSHScope{Host: "example.com", User: "deploy", HostKey: key}
			switch approvedOperation {
			case SSHGit:
				scope.AllowGit = true
			case SSHRemoteCommand:
				scope.RemoteCommands = []string{"uptime"}
			case SSHUpload:
				scope.AllowUpload, scope.UploadRoots, scope.UploadRemoteRoots = true, []string{"dist"}, []string{"/srv/app"}
			case SSHDownload:
				scope.AllowDownload, scope.DownloadRoots, scope.DownloadRemoteRoots = true, []string{"artifacts"}, []string{"/srv/artifacts"}
			case SSHLocalForward:
				scope.LocalForwardTargets = []string{"db.internal:5432"}
			case SSHRemoteForward:
				scope.RemoteForwardTargets = []string{"localhost:8080"}
			}
			capability, now := approvedSSH(t, scope)
			for _, operation := range operations {
				value := ""
				if operation == SSHRemoteCommand {
					value = "uptime"
				}
				if operation == SSHLocalForward {
					value = "db.internal:5432"
				}
				if operation == SSHRemoteForward {
					value = "localhost:8080"
				}
				_, err := AuthorizeSSH(capability, "agent-codex", now, operation, value)
				if operation == approvedOperation && err != nil {
					t.Errorf("approved %s denied: %v", operation, err)
				}
				if operation != approvedOperation && err == nil {
					t.Errorf("%s grant implied %s", approvedOperation, operation)
				}
			}
		})
	}
}

func TestApproveRequiresExactPreviewDigest(t *testing.T) {
	draft := model.Capability{
		Name: "api", Kind: model.CapabilityHTTP, AgentProfile: "agent-codex",
		HTTP: &model.HTTPScope{BaseURL: "https://example.com/api", Methods: []string{"GET"}},
	}
	preview, err := Normalize(draft)
	if err != nil {
		t.Fatal(err)
	}
	preview.Capability.HTTP.PathPrefixes = []string{"/api/admin"}
	_, err = Approve(preview.Capability, preview.ScopeDigest, time.Now())
	if err == nil {
		t.Fatal("changed scope accepted with stale preview digest")
	}
	var denied ErrDenied
	if _, err := Authorize(preview.Capability, "agent-codex", time.Now()); !errors.As(err, &denied) {
		t.Fatalf("Authorize error = %v", err)
	}
}

func TestHTTPWriteMethodAndPathRequireExactApproval(t *testing.T) {
	capability, now := approvedCapability(t, model.Capability{
		Name: "api", Kind: model.CapabilityHTTP, AgentProfile: "agent-codex",
		HTTP: &model.HTTPScope{BaseURL: "https://example.com/api", Methods: []string{"GET"}, PathPrefixes: []string{"/api/v1"}},
	})
	if _, _, err := AuthorizeHTTP(capability, "agent-codex", now, "GET", "https://example.com/api/v1/items?token=transient", 0); err != nil {
		t.Fatal(err)
	}
	for _, request := range []struct{ method, url string }{
		{"POST", "https://example.com/api/v1/items"},
		{"GET", "https://example.com/api/v10/items"},
		{"GET", "https://other.example.com/api/v1/items"},
	} {
		if _, _, err := AuthorizeHTTP(capability, "agent-codex", now, request.method, request.url, 0); err == nil {
			t.Errorf("%s %s unexpectedly authorized", request.method, request.url)
		}
	}
}

func TestGitReadGrantDoesNotImplyWriteAndRemoteMustStayExact(t *testing.T) {
	capability, now := approvedCapability(t, model.Capability{
		Name: "repo", Kind: model.CapabilityGit, AgentProfile: "agent-codex",
		Git: &model.GitScope{
			RemoteName: "origin", RemoteURL: "https://example.com/repo.git",
			Operations: []string{"status", "fetch"}, Branches: []string{"main"},
		},
	})
	if _, err := AuthorizeGit(capability, "agent-codex", now, GitAuthorization{
		Operation: "fetch", RemoteName: "origin", RemoteURL: "https://example.com/repo.git", Branch: "main",
	}); err != nil {
		t.Fatal(err)
	}
	for _, request := range []GitAuthorization{
		{Operation: "push", RemoteName: "origin", RemoteURL: "https://example.com/repo.git", Branch: "main"},
		{Operation: "fetch", RemoteName: "origin", RemoteURL: "https://evil.example/repo.git", Branch: "main"},
		{Operation: "fetch", RemoteName: "origin", RemoteURL: "https://example.com/repo.git", Branch: "release"},
	} {
		if _, err := AuthorizeGit(capability, "agent-codex", now, request); err == nil {
			t.Errorf("request %+v unexpectedly authorized", request)
		}
	}
}

func TestResolveProjectPathRejectsSymlinkEscapes(t *testing.T) {
	project := t.TempDir()
	outside := t.TempDir()
	if err := os.Mkdir(filepath.Join(project, "dist"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(project, "dist", "app.js"), []byte("ok"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(project, "dist", "escape")); err != nil {
		t.Fatal(err)
	}
	if _, err := ResolveProjectPath(project, "dist/app.js", []string{"dist"}, true); err != nil {
		t.Fatal(err)
	}
	if _, err := ResolveProjectPath(project, "dist/escape/secret", []string{"dist"}, false); err == nil {
		t.Fatal("symlinked-parent escape authorized")
	}
}
