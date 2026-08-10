package capability

import (
	"context"
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
)

func approvedHTTP(t *testing.T, baseURL string, paths []string, limit int64, redirects int) (model.Capability, time.Time) {
	t.Helper()
	draft := model.Capability{
		Name: "http", Kind: model.CapabilityHTTP, AgentProfile: "agent-codex",
		Limits: model.CapabilityLimits{MaxResponseBytes: limit, MaxRedirects: redirects},
		HTTP:   &model.HTTPScope{BaseURL: baseURL, Methods: []string{"GET"}, PathPrefixes: paths},
	}
	return approvedCapability(t, draft)
}

func TestHTTPExecutorFollowsConfinedRedirectAndStripsSecrets(t *testing.T) {
	var redirectedAuthorization string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/start" {
			http.Redirect(w, r, "/api/final", http.StatusFound)
			return
		}
		redirectedAuthorization = r.Header.Get("Authorization")
		_, _ = w.Write([]byte("ok"))
	}))
	defer server.Close()
	capability, now := approvedHTTP(t, server.URL+"/api", []string{"/api"}, 1024, 2)
	executor := HTTPExecutor{Now: func() time.Time { return now }}
	response, err := executor.Execute(context.Background(), capability, "agent-codex", HTTPRequest{
		Method: "GET", URL: server.URL + "/api/start", Headers: map[string][]string{"Authorization": {"Bearer secret"}},
	})
	if err != nil {
		t.Fatal(err)
	}
	if string(response.Body) != "ok" || response.Redirects != 1 || redirectedAuthorization != "" {
		t.Fatalf("response=%+v redirected authorization=%q", response, redirectedAuthorization)
	}
}

func TestHTTPExecutorRejectsRedirectEscapeBeforeSecondRequest(t *testing.T) {
	escaped := false
	outside := httptest.NewServer(http.HandlerFunc(func(http.ResponseWriter, *http.Request) { escaped = true }))
	defer outside.Close()
	inside := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, outside.URL+"/admin", http.StatusFound)
	}))
	defer inside.Close()
	capability, now := approvedHTTP(t, inside.URL, []string{"/"}, 1024, 2)
	executor := HTTPExecutor{Now: func() time.Time { return now }}
	_, err := executor.Execute(context.Background(), capability, "agent-codex", HTTPRequest{Method: "GET", URL: inside.URL})
	if err == nil || escaped || ClassifyConnectionError(err) != "denied" {
		t.Fatalf("err=%v escaped=%v class=%s", err, escaped, ClassifyConnectionError(err))
	}
}

func TestHTTPExecutorRejectsFirstRedirectWhenLimitIsZero(t *testing.T) {
	reached := false
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/final" {
			reached = true
			_, _ = w.Write([]byte("ok"))
			return
		}
		http.Redirect(w, r, "/final", http.StatusFound)
	}))
	defer server.Close()
	capability, now := approvedHTTP(t, server.URL, []string{"/"}, 1024, 0)
	executor := HTTPExecutor{Now: func() time.Time { return now }}
	_, err := executor.Execute(context.Background(), capability, "agent-codex", HTTPRequest{Method: "GET", URL: server.URL})
	if err == nil || reached || ClassifyConnectionError(err) != "denied" {
		t.Fatalf("err=%v reached=%v class=%s", err, reached, ClassifyConnectionError(err))
	}
}

func TestHTTPExecutorEnforcesResponseLimitAndTimeout(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/slow" {
			<-r.Context().Done()
			return
		}
		_, _ = w.Write([]byte(strings.Repeat("x", 20)))
	}))
	defer server.Close()
	capability, now := approvedHTTP(t, server.URL, []string{"/"}, 8, 1)
	executor := HTTPExecutor{Now: func() time.Time { return now }}
	_, err := executor.Execute(context.Background(), capability, "agent-codex", HTTPRequest{Method: "GET", URL: server.URL + "/large"})
	if ClassifyConnectionError(err) != "response-limit" {
		t.Fatalf("large response error=%v class=%s", err, ClassifyConnectionError(err))
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	_, err = executor.Execute(ctx, capability, "agent-codex", HTTPRequest{Method: "GET", URL: server.URL + "/slow"})
	if ClassifyConnectionError(err) != "timeout" {
		t.Fatalf("timeout error=%v class=%s", err, ClassifyConnectionError(err))
	}
}

func TestHTTPDiagnosticsRedactProxyCredentials(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { _, _ = w.Write([]byte("ok")) }))
	defer server.Close()
	capability, now := approvedHTTP(t, server.URL, []string{"/"}, 1024, 1)
	proxyURL, _ := url.Parse("http://user:secret@proxy.example:8080")
	executor := HTTPExecutor{
		Now:       func() time.Time { return now },
		Proxy:     func(*http.Request) (*url.URL, error) { return proxyURL, nil },
		Transport: roundTripperFunc(func(request *http.Request) (*http.Response, error) { return http.DefaultTransport.RoundTrip(request) }),
	}
	response, err := executor.Execute(context.Background(), capability, "agent-codex", HTTPRequest{Method: "GET", URL: server.URL})
	if err != nil {
		t.Fatal(err)
	}
	if response.Diagnostics.Proxy != "http://proxy.example:8080" || response.Diagnostics.CAStore != "system" {
		t.Fatalf("diagnostics = %+v", response.Diagnostics)
	}
}

func TestClassifyConnectionError(t *testing.T) {
	cases := []struct {
		err  error
		want string
	}{
		{&net.DNSError{Err: "missing", Name: "x"}, "dns"},
		{context.DeadlineExceeded, "timeout"},
		{context.Canceled, "cancelled"},
		{ErrDenied{Reason: "no"}, "denied"},
		{errors.New("opaque"), "transport"},
	}
	for _, tc := range cases {
		if got := ClassifyConnectionError(tc.err); got != tc.want {
			t.Errorf("ClassifyConnectionError(%v)=%q want %q", tc.err, got, tc.want)
		}
	}
}

type roundTripperFunc func(*http.Request) (*http.Response, error)

func (fn roundTripperFunc) RoundTrip(request *http.Request) (*http.Response, error) {
	return fn(request)
}
