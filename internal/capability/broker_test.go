package capability

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

func brokerFixture(t *testing.T) (*BrokerServer, model.Capability, string, string) {
	t.Helper()
	project := t.TempDir()
	dbPath := filepath.Join(project, ".ptrack", "ptrack.db")
	if err := os.MkdirAll(filepath.Dir(dbPath), 0o755); err != nil {
		t.Fatal(err)
	}
	draft := model.Capability{
		Name: "api", Kind: model.CapabilityHTTP, AgentProfile: "agent-codex",
		ApprovalDurationSeconds: 30 * 24 * 3600,
		Audit:                   model.CapabilityAuditPolicy{Enabled: true, RetainLast: 10},
		HTTP:                    &model.HTTPScope{BaseURL: "https://example.com/api", Methods: []string{"GET"}},
	}
	approved, _ := approvedCapability(t, draft)
	s, err := store.Open(dbPath)
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
	server, err := StartBrokerServer(BrokerServerConfig{
		GlobalHome: globalHome, ProjectRoot: project, DBPath: dbPath, Generation: 7,
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		ctx, cancel := context.WithTimeout(context.Background(), time.Second)
		defer cancel()
		_ = server.Shutdown(ctx)
	})
	server.Broker.HTTP.Now = func() time.Time { return approved.ApprovedAt.Add(time.Hour) }
	return server, approved, project, globalHome
}

func brokerHTTPCall(t *testing.T, capabilityID uint64) ToolCall {
	t.Helper()
	arguments, err := json.Marshal(map[string]any{
		"capability_id": capabilityID,
		"request":       map[string]any{"method": "GET", "url": "https://example.com/api/items"},
	})
	if err != nil {
		t.Fatal(err)
	}
	return ToolCall{Name: ToolHTTPRequest, Arguments: arguments}
}

func TestBrokerServerUsesHostMintedImmutableProfile(t *testing.T) {
	server, capability, _, _ := brokerFixture(t)
	server.Broker.HTTP.Transport = roundTripperFunc(func(request *http.Request) (*http.Response, error) {
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
	client := BrokerClient{Descriptor: server.Descriptor()}
	result, err := client.Call(context.Background(), token, brokerHTTPCall(t, capability.ID))
	if err != nil || !bytes.Contains(result, []byte(`"status_code":200`)) {
		t.Fatalf("result=%s err=%v", result, err)
	}
	if _, err := client.Call(context.Background(), "not-a-token", brokerHTTPCall(t, capability.ID)); err == nil {
		t.Fatal("unknown token accepted")
	}
	claudeToken, err := server.Broker.IssueSessionToken("agent-claude")
	if err != nil {
		t.Fatal(err)
	}
	if err := server.Broker.BindSession(claudeToken, "terminal-2"); err != nil {
		t.Fatal(err)
	}
	if _, err := client.Call(context.Background(), claudeToken, brokerHTTPCall(t, capability.ID)); err == nil || !strings.Contains(err.Error(), "profile") {
		t.Fatalf("profile impersonation error = %v", err)
	}
	server.Broker.RevokeSession("terminal-1")
	if _, err := client.Call(context.Background(), token, brokerHTTPCall(t, capability.ID)); err == nil {
		t.Fatal("revoked terminal token accepted")
	}
}

func TestBrokerRejectsUnauthenticatedToolDiscovery(t *testing.T) {
	server, _, _, _ := brokerFixture(t)
	request, err := http.NewRequest(http.MethodPost, server.Descriptor().URL+"/v1/tools/list", nil)
	if err != nil {
		t.Fatal(err)
	}
	request.Header.Set("Authorization", "Bearer unknown-session")
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusUnauthorized {
		t.Fatalf("tools/list status = %d, want %d", response.StatusCode, http.StatusUnauthorized)
	}
}

func TestBrokerEnforcesConcurrencyBeforeStartingTransport(t *testing.T) {
	server, capability, _, _ := brokerFixture(t)
	started := make(chan struct{})
	release := make(chan struct{})
	var calls int
	var callsMu sync.Mutex
	server.Broker.HTTP.Transport = roundTripperFunc(func(request *http.Request) (*http.Response, error) {
		callsMu.Lock()
		calls++
		current := calls
		callsMu.Unlock()
		if current == 1 {
			close(started)
		}
		select {
		case <-release:
		case <-request.Context().Done():
			return nil, request.Context().Err()
		}
		return &http.Response{
			StatusCode: http.StatusOK, Status: "200 OK", Header: make(http.Header),
			Body: io.NopCloser(strings.NewReader("ok")), Request: request,
		}, nil
	})
	token, _ := server.Broker.IssueSessionToken("agent-codex")
	if err := server.Broker.BindSession(token, "terminal-1"); err != nil {
		t.Fatal(err)
	}
	client := BrokerClient{Descriptor: server.Descriptor()}
	first := make(chan error, 1)
	go func() {
		_, err := client.Call(context.Background(), token, brokerHTTPCall(t, capability.ID))
		first <- err
	}()
	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("first transport did not start")
	}
	if _, err := client.Call(context.Background(), token, brokerHTTPCall(t, capability.ID)); err == nil || !strings.Contains(err.Error(), "concurrency") {
		t.Fatalf("second call error = %v", err)
	}
	close(release)
	if err := <-first; err != nil {
		t.Fatalf("first call: %v", err)
	}
	callsMu.Lock()
	defer callsMu.Unlock()
	if calls != 1 {
		t.Fatalf("transport calls = %d, want 1", calls)
	}
}

func TestBrokerTokenCannotCrossProjectBoundary(t *testing.T) {
	first, _, _, _ := brokerFixture(t)
	second, capability, _, _ := brokerFixture(t)
	token, _ := first.Broker.IssueSessionToken("agent-codex")
	if err := first.Broker.BindSession(token, "terminal-1"); err != nil {
		t.Fatal(err)
	}
	client := BrokerClient{Descriptor: second.Descriptor()}
	if _, err := client.Call(context.Background(), token, brokerHTTPCall(t, capability.ID)); err == nil {
		t.Fatal("project A token was accepted by project B broker")
	}
}

func TestBrokerRevocationCancelsInFlightCall(t *testing.T) {
	server, capability, _, _ := brokerFixture(t)
	started := make(chan struct{})
	server.Broker.HTTP.Transport = roundTripperFunc(func(request *http.Request) (*http.Response, error) {
		close(started)
		<-request.Context().Done()
		return nil, request.Context().Err()
	})
	token, _ := server.Broker.IssueSessionToken("agent-codex")
	if err := server.Broker.BindSession(token, "terminal-1"); err != nil {
		t.Fatal(err)
	}
	client := BrokerClient{Descriptor: server.Descriptor()}
	done := make(chan error, 1)
	go func() {
		_, err := client.Call(context.Background(), token, brokerHTTPCall(t, capability.ID))
		done <- err
	}()
	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("broker call did not start")
	}
	server.Broker.RevokeSession("terminal-1")
	select {
	case err := <-done:
		if err == nil {
			t.Fatal("revoked in-flight call succeeded")
		}
	case <-time.After(time.Second):
		t.Fatal("revoked in-flight call was not cancelled")
	}
}

func TestBrokerDoesNotHoldProjectStoreDuringInFlightCall(t *testing.T) {
	server, capability, _, _ := brokerFixture(t)
	started := make(chan struct{})
	server.Broker.HTTP.Transport = roundTripperFunc(func(request *http.Request) (*http.Response, error) {
		close(started)
		<-request.Context().Done()
		return nil, request.Context().Err()
	})
	token, _ := server.Broker.IssueSessionToken("agent-codex")
	if err := server.Broker.BindSession(token, "terminal-1"); err != nil {
		t.Fatal(err)
	}
	client := BrokerClient{Descriptor: server.Descriptor()}
	callDone := make(chan error, 1)
	go func() {
		_, err := client.Call(context.Background(), token, brokerHTTPCall(t, capability.ID))
		callDone <- err
	}()
	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("broker call did not start")
	}

	updated := make(chan error, 1)
	go func() {
		s, err := store.Open(server.Broker.dbPath)
		if err == nil {
			stored, getErr := s.GetCapability(capability.ID)
			err = getErr
			if err == nil {
				err = s.UpdateCapability(Disable(stored))
			}
			err = errors.Join(err, s.Close())
		}
		if err == nil {
			server.Broker.RevokeCapability(capability.ID)
		}
		updated <- err
	}()
	select {
	case err := <-updated:
		if err != nil {
			t.Fatalf("disable capability: %v", err)
		}
	case <-time.After(time.Second):
		t.Fatal("capability update blocked behind in-flight broker call")
	}
	select {
	case err := <-callDone:
		if err == nil {
			t.Fatal("disabled in-flight call succeeded")
		}
	case <-time.After(time.Second):
		t.Fatal("disable did not cancel the in-flight call")
	}
}

func TestBrokerDescriptorAndTokensAreGenerationScoped(t *testing.T) {
	server, _, project, globalHome := brokerFixture(t)
	token, _ := server.Broker.IssueSessionToken("agent-codex")
	if err := server.Broker.BindSession(token, "terminal-1"); err != nil {
		t.Fatal(err)
	}
	descriptor, err := ReadBrokerDescriptor(globalHome, project)
	canonicalProject, _ := filepath.EvalSymlinks(project)
	if err != nil || descriptor.Generation != 7 || descriptor.ProjectRoot != canonicalProject {
		t.Fatalf("descriptor=%+v err=%v", descriptor, err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if err := server.Shutdown(ctx); err != nil {
		t.Fatal(err)
	}
	if _, err := ReadBrokerDescriptor(globalHome, project); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("descriptor after shutdown error=%v", err)
	}

	second, err := StartBrokerServer(BrokerServerConfig{
		GlobalHome: globalHome, ProjectRoot: project, DBPath: filepath.Join(project, ".ptrack", "ptrack.db"), Generation: 8,
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = second.Shutdown(context.Background()) })
	client := BrokerClient{Descriptor: second.Descriptor()}
	if _, err := client.Call(context.Background(), token, ToolCall{Name: ToolHTTPRequest, Arguments: json.RawMessage(`{}`)}); err == nil {
		t.Fatal("previous-generation token replayed")
	}
}

func TestOlderBrokerCannotRemoveReplacementDescriptor(t *testing.T) {
	first, _, project, globalHome := brokerFixture(t)
	second, err := StartBrokerServer(BrokerServerConfig{
		GlobalHome: globalHome, ProjectRoot: project,
		DBPath: filepath.Join(project, ".ptrack", "ptrack.db"), Generation: 8,
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = second.Shutdown(context.Background()) })

	if err := first.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}
	descriptor, err := ReadBrokerDescriptor(globalHome, project)
	if err != nil {
		t.Fatalf("replacement descriptor was removed: %v", err)
	}
	if descriptor != second.Descriptor() {
		t.Fatalf("descriptor = %+v, want %+v", descriptor, second.Descriptor())
	}
}
