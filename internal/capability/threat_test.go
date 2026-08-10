package capability

import (
	"context"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
)

func TestThreatHTTPEncodedPathEscapesAreDenied(t *testing.T) {
	capability, now := approvedHTTP(t, "https://example.com/api", []string{"/api"}, 1024, 1)
	for _, requestURL := range []string{
		"https://example.com/api/%2e%2e/admin",
		"https://example.com/api%2fadmin",
		"https://example.com/api/%252e%252e/admin",
	} {
		if _, _, err := AuthorizeHTTP(capability, "agent-codex", now, "GET", requestURL, 0); err == nil {
			t.Errorf("encoded escape %q was authorized", requestURL)
		}
	}
}

func TestThreatUnapprovedSSHMetacharactersNeverReachProcessRunner(t *testing.T) {
	capability, now := approvedSSH(t, &model.SSHScope{
		Host: "example.com", User: "deploy",
		HostKey:        "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA==",
		RemoteCommands: []string{"uptime"},
	})
	runner := &recordingProcessRunner{}
	executor := SSHExecutor{Runner: runner, Now: func() time.Time { return now }}
	_, err := executor.Execute(context.Background(), capability, "agent-codex", t.TempDir(), SSHRequest{
		Operation: SSHRemoteCommand,
		Command:   "uptime; touch /tmp/ptrack-injection-$(id)\nwhoami",
	})
	if err == nil {
		t.Fatal("unapproved metacharacter command was authorized")
	}
	if runner.spec.Name != "" {
		t.Fatalf("process runner invoked for denied command: %+v", runner.spec)
	}
}
