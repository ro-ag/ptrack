package capability

import (
	"context"
	"crypto/x509"
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"syscall"
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
)

func TestDiagnosticClassesAreDistinctAndSanitized(t *testing.T) {
	tests := []struct {
		name   string
		kind   model.CapabilityKind
		err    error
		stderr string
		want   string
	}{
		{"dns", model.CapabilityHTTP, &net.DNSError{Err: "missing"}, "", "dns"},
		{"routing", model.CapabilityHTTP, syscall.EHOSTUNREACH, "", "routing"},
		{"vpn", model.CapabilityHTTP, VPNUnavailableError{}, "", "vpn"},
		{"proxy", model.CapabilityHTTP, proxyPolicyError{}, "", "proxy"},
		{"tls", model.CapabilityHTTP, x509.UnknownAuthorityError{}, "", "tls"},
		{"host-key", model.CapabilitySSH, errors.New("exit"), "Host key verification failed", "host-key"},
		{"authentication", model.CapabilitySSH, errors.New("exit"), "Permission denied (publickey)", "authentication"},
		{"sandbox", model.CapabilityHTTP, os.ErrPermission, "", "sandbox"},
		{"remote-policy", model.CapabilityHTTP, remotePolicyStatusError{status: 403}, "", "remote-policy"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			diagnostic := diagnosticFor(test.kind, test.err, test.stderr, 0, VPNActive)
			if diagnostic.Class != test.want || diagnostic.Stage == "" || diagnostic.Message == "" || diagnostic.VPN != VPNActive {
				t.Fatalf("diagnostic = %+v", diagnostic)
			}
		})
	}
}

func TestHTTPConnectionTestReportsProxyCAAndRemotePolicy(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, "denied", http.StatusForbidden)
	}))
	defer server.Close()
	tester := ConnectionTester{DetectVPN: func() VPNState { return VPNInactive }}
	diagnostic := tester.TestHTTP(context.Background(), model.Capability{
		Name: "api", Kind: model.CapabilityHTTP, AgentProfile: "agent-codex",
		Audit: model.CapabilityAuditPolicy{Enabled: true, RetainLast: 20},
		HTTP:  &model.HTTPScope{BaseURL: server.URL, Methods: []string{"GET"}},
	})
	if diagnostic.Class != "remote-policy" || diagnostic.StatusCode != 403 || diagnostic.CAStore != "system" || diagnostic.Proxy == "" {
		t.Fatalf("diagnostic = %+v", diagnostic)
	}
}

func TestHTTPConnectionTestNeverUsesMutatingMethod(t *testing.T) {
	requested := false
	server := httptest.NewServer(http.HandlerFunc(func(http.ResponseWriter, *http.Request) {
		requested = true
	}))
	defer server.Close()
	diagnostic := (ConnectionTester{}).TestHTTP(context.Background(), model.Capability{
		Name: "write-only", Kind: model.CapabilityHTTP, AgentProfile: "agent-codex",
		HTTP: &model.HTTPScope{BaseURL: server.URL, Methods: []string{"POST", "DELETE"}},
	})
	if diagnostic.Class != "denied" || requested {
		t.Fatalf("diagnostic=%+v requested=%v", diagnostic, requested)
	}
}

func TestSSHConnectionTestUsesPinnedKeyAndClassifiesAuthentication(t *testing.T) {
	runner := &recordingProcessRunner{
		result: ProcessResult{ExitCode: 255, Stderr: "Permission denied (publickey)"},
		err:    errors.New("exit status 255"),
	}
	tester := ConnectionTester{SSHRunner: runner, DetectVPN: func() VPNState { return VPNActive }}
	diagnostic := tester.TestSSH(context.Background(), model.Capability{
		Name: "ssh", Kind: model.CapabilitySSH, AgentProfile: "agent-codex",
		SSH: &model.SSHScope{
			Host: "example.com", User: "deploy", HostKey: "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA==",
			AllowGit: true,
		},
	})
	if diagnostic.Class != "authentication" || !contains(runner.spec.Args, "StrictHostKeyChecking=yes") || !contains(runner.spec.Args, "true") {
		t.Fatalf("diagnostic=%+v spec=%+v", diagnostic, runner.spec)
	}
}
