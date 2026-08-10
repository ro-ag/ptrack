package capability

import (
	"strings"
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
)

func TestNormalizeHTTPDisplaysCanonicalEffectiveScope(t *testing.T) {
	preview, err := Normalize(model.Capability{
		Name: " API ", Kind: model.CapabilityHTTP, AgentProfile: "agent-codex",
		Audit: model.CapabilityAuditPolicy{Enabled: true},
		HTTP: &model.HTTPScope{
			BaseURL:      "HTTPS://Example.COM:443/api/./v1/",
			Methods:      []string{"get", "HEAD", "get"},
			PathPrefixes: []string{"/api/v1/users", "/api/v1"},
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	if preview.Capability.HTTP.BaseURL != "https://example.com/api/v1" {
		t.Errorf("base URL = %q", preview.Capability.HTTP.BaseURL)
	}
	if got := strings.Join(preview.Capability.HTTP.Methods, ","); got != "GET,HEAD" {
		t.Errorf("methods = %q", got)
	}
	if preview.ScopeDigest == "" || !strings.Contains(preview.EffectiveScope, "paths=/api/v1,/api/v1/users") {
		t.Errorf("preview = %+v", preview)
	}
	for _, field := range []string{"profile=agent-codex", "approval_duration_seconds=3600", "max_request_bytes=", "max_redirects=0", "audit enabled=true"} {
		if !strings.Contains(preview.EffectiveScope, field) {
			t.Errorf("effective scope missing %q: %s", field, preview.EffectiveScope)
		}
	}
}

func TestNormalizePreservesExplicitZeroRedirectLimit(t *testing.T) {
	preview, err := Normalize(model.Capability{
		Name: "no redirects", Kind: model.CapabilityHTTP, AgentProfile: "agent-codex",
		Limits: model.CapabilityLimits{MaxRedirects: 0},
		HTTP:   &model.HTTPScope{BaseURL: "https://example.com", Methods: []string{"GET"}},
	})
	if err != nil {
		t.Fatal(err)
	}
	if preview.Capability.Limits.MaxRedirects != 0 || !strings.Contains(preview.EffectiveScope, "max_redirects=0") {
		t.Fatalf("preview = %+v", preview)
	}
}

func TestNormalizeRejectsCredentialsAndAmbiguousHTTPPaths(t *testing.T) {
	for _, rawURL := range []string{
		"https://token@example.com/api",
		"https://example.com/api/%2e%2e/admin",
		"https://example.com/api%2fadmin",
	} {
		_, err := Normalize(model.Capability{
			Name: "bad", Kind: model.CapabilityHTTP, AgentProfile: "agent-codex",
			HTTP: &model.HTTPScope{BaseURL: rawURL, Methods: []string{"GET"}},
		})
		if err == nil {
			t.Errorf("Normalize(%q) unexpectedly succeeded", rawURL)
		}
	}
}

func TestNormalizeHTTPPathUsesSegmentBoundaries(t *testing.T) {
	_, err := Normalize(model.Capability{
		Name: "bad path", Kind: model.CapabilityHTTP, AgentProfile: "agent-codex",
		HTTP: &model.HTTPScope{BaseURL: "https://example.com/api", Methods: []string{"GET"}, PathPrefixes: []string{"/apix"}},
	})
	if err == nil {
		t.Fatal("expected /apix to be outside /api")
	}
}

func TestNormalizeGitRejectsCredentialsAndRiskyRefspecs(t *testing.T) {
	base := model.Capability{
		Name: "repo", Kind: model.CapabilityGit, AgentProfile: "agent-codex",
		Git: &model.GitScope{RemoteName: "origin", RemoteURL: "https://example.com/repo.git", Operations: []string{"push"}},
	}
	credentialed := base
	credentialed.Git = &model.GitScope{RemoteName: "origin", RemoteURL: "https://token@example.com/repo.git", Operations: []string{"fetch"}}
	if _, err := Normalize(credentialed); err == nil {
		t.Fatal("credentialed Git URL accepted")
	}
	forced := base
	forced.Git = &model.GitScope{RemoteName: "origin", RemoteURL: "git@example.com:org/repo.git", Operations: []string{"push"}, Refspecs: []string{"+main:main"}}
	if _, err := Normalize(forced); err == nil {
		t.Fatal("force refspec accepted without grant")
	}
}

func TestNormalizeGitRejectsBranchForceTagAndPseudorefBypasses(t *testing.T) {
	for _, branch := range []string{"+main", "refs/tags/v1", "HEAD", "@", "FETCH_HEAD"} {
		_, err := Normalize(model.Capability{
			Name: "repo", Kind: model.CapabilityGit, AgentProfile: "agent-codex",
			Git: &model.GitScope{
				RemoteName: "origin", RemoteURL: "https://example.com/repo.git",
				Operations: []string{"push"}, Branches: []string{branch},
			},
		})
		if err == nil {
			t.Errorf("branch bypass %q was accepted", branch)
		}
	}
}

func TestNormalizeGitEffectiveScopeDisplaysEveryWriteGrant(t *testing.T) {
	preview, err := Normalize(model.Capability{
		Name: "repo", Kind: model.CapabilityGit, AgentProfile: "agent-codex",
		Git: &model.GitScope{
			RemoteName: "origin", RemoteURL: "https://example.com/repo.git",
			Operations: []string{"push"}, Branches: []string{"main"},
			Refspecs: []string{"main:main"}, AllowForcePush: true,
			AllowDeleteRefs: true, AllowTags: true,
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	for _, field := range []string{`refspecs=["refs/heads/main:refs/heads/main"]`, "allow_tags=true", "allow_force_with_lease=true", "allow_delete_refs=true"} {
		if !strings.Contains(preview.EffectiveScope, field) {
			t.Errorf("effective scope missing %q: %s", field, preview.EffectiveScope)
		}
	}
}

func TestNormalizeGitRejectsUnconditionalForceAndUnsupportedRefNamespaces(t *testing.T) {
	for _, refspec := range []string{"+main:main", "refs/notes/review:refs/notes/review"} {
		_, err := Normalize(model.Capability{
			Name: "repo", Kind: model.CapabilityGit, AgentProfile: "agent-codex",
			Git: &model.GitScope{
				RemoteName: "origin", RemoteURL: "https://example.com/repo.git",
				Operations: []string{"push"}, Branches: []string{"main"},
				Refspecs: []string{refspec}, AllowForcePush: true,
			},
		})
		if err == nil {
			t.Errorf("ambiguous refspec %q was accepted", refspec)
		}
	}
}

func TestNormalizeSSHSeparatesHighRiskGrants(t *testing.T) {
	key := "ssh-ed25519 " + "QUJDREVGR0hJSktMTU5PUA=="
	preview, err := Normalize(model.Capability{
		Name: "deploy", Kind: model.CapabilitySSH, AgentProfile: "agent-codex",
		SSH: &model.SSHScope{
			Host: "EXAMPLE.com.", User: "deploy", HostKey: key,
			AllowUpload: true, UploadRoots: []string{"dist"}, UploadRemoteRoots: []string{"/srv/app"},
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	if preview.Capability.SSH.Host != "example.com" || preview.Capability.SSH.Port != 22 {
		t.Errorf("SSH scope = %+v", preview.Capability.SSH)
	}
	if !strings.Contains(preview.EffectiveScope, `grants=["upload"]`) || strings.Contains(preview.EffectiveScope, `"interactive-shell"`) {
		t.Errorf("effective scope = %q", preview.EffectiveScope)
	}
}

func TestNormalizeSSHEffectiveScopeDisplaysExactHighRiskTargets(t *testing.T) {
	preview, err := Normalize(model.Capability{
		Name: "deploy", Kind: model.CapabilitySSH, AgentProfile: "agent-codex",
		SSH: &model.SSHScope{
			Alias: "prod", Host: "example.com", User: "deploy",
			HostKey:        "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA==",
			RemoteCommands: []string{"systemctl status app"},
			AllowUpload:    true, UploadRoots: []string{"dist"}, UploadRemoteRoots: []string{"/srv/app"},
			LocalForwardTargets: []string{"db.internal:5432"},
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	for _, field := range []string{`alias="prod"`, `commands=["systemctl status app"]`, `upload_local_roots=["dist"]`, `upload_remote_roots=["/srv/app"]`, `local_forward_targets=["db.internal:5432"]`} {
		if !strings.Contains(preview.EffectiveScope, field) {
			t.Errorf("effective scope missing %q: %s", field, preview.EffectiveScope)
		}
	}
}

func TestNormalizeSSHRejectsTraversalAndUnpairedTransferGrant(t *testing.T) {
	key := "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA=="
	for _, scope := range []*model.SSHScope{
		{Host: "example.com", User: "deploy", HostKey: key, AllowUpload: true, UploadRoots: []string{"../secret"}, UploadRemoteRoots: []string{"/srv/app"}},
		{Host: "example.com", User: "deploy", HostKey: key, UploadRoots: []string{"dist"}, UploadRemoteRoots: []string{"/srv/app"}},
	} {
		_, err := Normalize(model.Capability{Name: "bad", Kind: model.CapabilitySSH, AgentProfile: "agent-codex", SSH: scope})
		if err == nil {
			t.Errorf("scope %+v unexpectedly accepted", scope)
		}
	}
}

func TestNormalizeSSHRejectsUnavailableInteractiveShellGrant(t *testing.T) {
	_, err := Normalize(model.Capability{
		Name: "shell", Kind: model.CapabilitySSH, AgentProfile: "agent-codex",
		SSH: &model.SSHScope{
			Host: "example.com", User: "deploy", HostKey: "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA==",
			AllowInteractiveShell: true,
		},
	})
	if err == nil || !strings.Contains(err.Error(), "unavailable") {
		t.Fatalf("interactive-shell grant error = %v", err)
	}
}
