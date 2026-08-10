package cli

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/capability"
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

func TestCapabilityCallUsesActiveHostBroker(t *testing.T) {
	project := t.TempDir()
	metadata := filepath.Join(project, ".ptrack")
	if err := os.Mkdir(metadata, 0o755); err != nil {
		t.Fatal(err)
	}
	dbPath := filepath.Join(metadata, "ptrack.db")
	s, err := store.Open(dbPath)
	if err != nil {
		t.Fatal(err)
	}
	draft := model.Capability{
		Name: "api", Kind: model.CapabilityHTTP, AgentProfile: "agent-codex",
		ApprovalDurationSeconds: 3600,
		HTTP:                    &model.HTTPScope{BaseURL: "https://example.com/api", Methods: []string{"GET"}},
	}
	preview, err := capability.Normalize(draft)
	if err != nil {
		t.Fatal(err)
	}
	approved, err := capability.Approve(preview.Capability, preview.ScopeDigest, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	approved, err = s.AddCapability(approved)
	if err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	globalHome := t.TempDir()
	server, err := capability.StartBrokerServer(capability.BrokerServerConfig{
		GlobalHome: globalHome, ProjectRoot: project, DBPath: dbPath, Generation: 3,
	})
	if err != nil {
		t.Fatal(err)
	}
	defer server.Shutdown(context.Background())
	server.Broker.HTTP.Transport = cliRoundTripper(func(request *http.Request) (*http.Response, error) {
		return &http.Response{
			StatusCode: http.StatusOK, Status: "200 OK", Header: make(http.Header),
			Body: io.NopCloser(strings.NewReader("ok")), Request: request,
		}, nil
	})
	token, err := server.Broker.IssueSessionToken("agent-codex")
	if err != nil {
		t.Fatal(err)
	}
	if err := server.Broker.BindSession(token, "terminal-1"); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PTRACK_HOME", globalHome)
	t.Setenv("PTRACK_CAPABILITY_TOKEN", token)
	t.Setenv("PTRACK_CAPABILITY_PROJECT", project)
	t.Setenv("PTRACK_CAPABILITY_GENERATION", "3")
	previous, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Chdir(project); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chdir(previous) })
	output, err := runCmd(t,
		"capability", "call", capability.ToolHTTPRequest,
		"--arguments", `{"capability_id":`+fmt.Sprint(approved.ID)+`,"request":{"method":"GET","url":"https://example.com/api/items"}}`,
	)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(output, `"status_code":200`) {
		t.Fatalf("output = %s", output)
	}
}

type cliRoundTripper func(*http.Request) (*http.Response, error)

func (fn cliRoundTripper) RoundTrip(request *http.Request) (*http.Response, error) {
	return fn(request)
}
