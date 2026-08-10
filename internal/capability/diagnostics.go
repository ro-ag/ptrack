package capability

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/http"
	"os"
	"strings"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
)

// VPNState reports observable host interface state without guessing whether a
// route requires a particular corporate VPN.
type VPNState string

const (
	VPNActive   VPNState = "active"
	VPNInactive VPNState = "inactive"
	VPNUnknown  VPNState = "unknown"
)

// ConnectionDiagnostic is a stable, sanitized connection-test result.
type ConnectionDiagnostic struct {
	Kind       model.CapabilityKind `json:"kind"`
	Success    bool                 `json:"success"`
	Stage      string               `json:"stage"`
	Class      string               `json:"class"`
	Message    string               `json:"message"`
	VPN        VPNState             `json:"vpn"`
	Proxy      string               `json:"proxy,omitempty"`
	CAStore    string               `json:"ca_store,omitempty"`
	StatusCode int                  `json:"status_code,omitempty"`
}

// VPNUnavailableError is used only when the host has positive evidence that
// a required VPN route/policy is unavailable.
type VPNUnavailableError struct{}

func (VPNUnavailableError) Error() string { return "required VPN route is unavailable" }

// ConnectionTester runs non-mutating tests against normalized drafts. A human
// Settings action is the approval boundary for these pre-enable probes.
type ConnectionTester struct {
	HTTP      HTTPExecutor
	GitRunner ProcessRunner
	SSHRunner ProcessRunner
	DetectVPN func() VPNState
	Now       func() time.Time
}

// TestHTTP tests the approved base origin without headers or a request body.
func (t ConnectionTester) TestHTTP(ctx context.Context, draft model.Capability) ConnectionDiagnostic {
	now := t.now()
	preview, err := Normalize(draft)
	if err != nil || preview.Capability.Kind != model.CapabilityHTTP {
		return diagnosticFor(model.CapabilityHTTP, errOrInvalid(err), "", 0, t.vpn())
	}
	probe := preview.Capability
	probe, err = Approve(probe, preview.ScopeDigest, now)
	if err != nil {
		return diagnosticFor(model.CapabilityHTTP, err, "", 0, t.vpn())
	}
	method := ""
	for _, candidate := range []string{http.MethodHead, http.MethodGet, http.MethodOptions} {
		if contains(probe.HTTP.Methods, candidate) {
			method = candidate
			break
		}
	}
	if method == "" {
		return diagnosticFor(
			model.CapabilityHTTP,
			ErrDenied{Reason: "HTTP connection tests require an approved HEAD, GET, or OPTIONS method"},
			"",
			0,
			t.vpn(),
		)
	}
	executor := t.HTTP
	executor.Now = func() time.Time { return now }
	response, err := executor.Execute(ctx, probe, probe.AgentProfile, HTTPRequest{Method: method, URL: probe.HTTP.BaseURL})
	if err != nil {
		diagnostic := diagnosticFor(model.CapabilityHTTP, err, "", response.StatusCode, t.vpn())
		diagnostic.Proxy, diagnostic.CAStore = response.Diagnostics.Proxy, response.Diagnostics.CAStore
		return diagnostic
	}
	diagnostic := diagnosticFor(model.CapabilityHTTP, httpStatusError(response.StatusCode), "", response.StatusCode, t.vpn())
	diagnostic.Proxy, diagnostic.CAStore = response.Diagnostics.Proxy, response.Diagnostics.CAStore
	return diagnostic
}

// TestGit performs ls-remote through the fixed Git executor. It never fetches
// objects or mutates the working tree.
func (t ConnectionTester) TestGit(
	ctx context.Context,
	draft model.Capability,
	sshDraft *model.Capability,
	projectRoot string,
) ConnectionDiagnostic {
	now := t.now()
	preview, err := Normalize(draft)
	if err != nil || preview.Capability.Kind != model.CapabilityGit {
		return diagnosticFor(model.CapabilityGit, errOrInvalid(err), "", 0, t.vpn())
	}
	probe := preview.Capability
	if !contains(probe.Git.Operations, "ls-remote") {
		probe.Git.Operations = append(probe.Git.Operations, "ls-remote")
		preview, err = Normalize(probe)
		if err != nil {
			return diagnosticFor(model.CapabilityGit, err, "", 0, t.vpn())
		}
		probe = preview.Capability
	}
	probe, err = Approve(probe, probe.ScopeDigest, now)
	if err != nil {
		return diagnosticFor(model.CapabilityGit, err, "", 0, t.vpn())
	}
	var approvedSSH *model.Capability
	if sshDraft != nil {
		sshPreview, normalizeErr := Normalize(*sshDraft)
		if normalizeErr != nil {
			return diagnosticFor(model.CapabilityGit, normalizeErr, "", 0, t.vpn())
		}
		sshProbe := sshPreview.Capability
		sshProbe, normalizeErr = Approve(sshProbe, sshProbe.ScopeDigest, now)
		if normalizeErr != nil {
			return diagnosticFor(model.CapabilityGit, normalizeErr, "", 0, t.vpn())
		}
		approvedSSH = &sshProbe
	}
	executor := GitExecutor{Runner: t.GitRunner, Now: func() time.Time { return now }}
	result, err := executor.Execute(ctx, probe, approvedSSH, probe.AgentProfile, projectRoot, GitRequest{Operation: "ls-remote"})
	return diagnosticFor(model.CapabilityGit, err, result.Stderr, 0, t.vpn())
}

// TestSSH authenticates with the pinned host key and runs the fixed command
// "true". No draft remote-command grant is widened or persisted.
func (t ConnectionTester) TestSSH(ctx context.Context, draft model.Capability) ConnectionDiagnostic {
	preview, err := Normalize(draft)
	if err != nil || preview.Capability.Kind != model.CapabilitySSH {
		return diagnosticFor(model.CapabilitySSH, errOrInvalid(err), "", 0, t.vpn())
	}
	probe := preview.Capability
	directory, knownHosts, err := writePinnedKnownHosts(probe.SSH)
	if err != nil {
		return diagnosticFor(model.CapabilitySSH, err, "", 0, t.vpn())
	}
	defer os.RemoveAll(directory)
	runner := t.SSHRunner
	if runner == nil {
		runner = ExecProcessRunner{}
	}
	args := append(sshBaseArgs(probe.SSH, knownHosts), "-T", probe.SSH.User+"@"+probe.SSH.Host, "true")
	timeoutCtx, cancel := context.WithTimeout(ctx, time.Duration(probe.Limits.TimeoutSeconds)*time.Second)
	defer cancel()
	result, runErr := runner.Run(timeoutCtx, ProcessSpec{
		Name: "ssh", Args: args, Env: []string{"LC_ALL=C", "LANG=C"}, MaxOutputBytes: probe.Limits.MaxOutputBytes,
	})
	if result.Truncated {
		runErr = outputLimitError{}
	}
	return diagnosticFor(model.CapabilitySSH, runErr, result.Stderr, 0, t.vpn())
}

func (t ConnectionTester) now() time.Time {
	if t.Now != nil {
		return t.Now()
	}
	return time.Now()
}

func (t ConnectionTester) vpn() VPNState {
	if t.DetectVPN != nil {
		return t.DetectVPN()
	}
	return DetectVPNState()
}

// DetectVPNState reports whether a commonly named tunnel interface is up.
func DetectVPNState() VPNState {
	interfaces, err := net.Interfaces()
	if err != nil {
		return VPNUnknown
	}
	for _, networkInterface := range interfaces {
		name := strings.ToLower(networkInterface.Name)
		if networkInterface.Flags&net.FlagUp != 0 && (strings.HasPrefix(name, "utun") || strings.HasPrefix(name, "tun") || strings.HasPrefix(name, "tap") || strings.HasPrefix(name, "wg") || strings.HasPrefix(name, "ppp")) {
			return VPNActive
		}
	}
	return VPNInactive
}

type remotePolicyStatusError struct{ status int }

func (e remotePolicyStatusError) Error() string {
	return fmt.Sprintf("remote policy returned HTTP %d", e.status)
}

func httpStatusError(status int) error {
	if status == 0 || status < 400 {
		return nil
	}
	if status == http.StatusProxyAuthRequired {
		return proxyPolicyError{}
	}
	return remotePolicyStatusError{status: status}
}

func diagnosticFor(kind model.CapabilityKind, err error, stderr string, status int, vpn VPNState) ConnectionDiagnostic {
	class := "none"
	if err != nil {
		var vpnUnavailable VPNUnavailableError
		var remotePolicy remotePolicyStatusError
		switch {
		case errors.As(err, &vpnUnavailable):
			class = "vpn"
		case errors.As(err, &remotePolicy):
			class = "remote-policy"
		case kind == model.CapabilitySSH:
			class = ClassifySSHError(err, stderr)
		case kind == model.CapabilityGit:
			class = ClassifyGitError(err, stderr)
		default:
			class = ClassifyConnectionError(err)
		}
	}
	stage := map[string]string{
		"none": "complete", "denied": "policy", "dns": "dns", "routing": "routing", "vpn": "vpn",
		"proxy": "proxy", "tls": "tls", "host-key": "host-key", "authentication": "authentication",
		"sandbox": "sandbox", "remote-policy": "remote-policy", "timeout": "connect",
		"request-limit": "request", "response-limit": "response", "output-limit": "response",
		"cancelled": "cancelled", "transport": "transport", "internal": "internal",
	}[class]
	if stage == "" {
		stage = "transport"
	}
	messages := map[string]string{
		"none":           "Connection test succeeded.",
		"denied":         "The capability policy rejected the test.",
		"dns":            "The host name could not be resolved.",
		"routing":        "No usable route to the host was available.",
		"vpn":            "A required VPN route or policy was unavailable.",
		"proxy":          "The current proxy rejected or could not authenticate the request.",
		"tls":            "TLS certificate or handshake validation failed with the system CA store.",
		"host-key":       "The SSH host key did not match the pinned key.",
		"authentication": "Host authentication failed using current credential helpers or ssh-agent.",
		"sandbox":        "The host sandbox or local permissions blocked the operation.",
		"remote-policy":  "The remote service was reached but rejected the operation.",
		"timeout":        "The connection test timed out.",
		"output-limit":   "The connection test exceeded its output limit.",
		"cancelled":      "The connection test was cancelled.",
		"transport":      "The connection failed for an unclassified transport reason.",
		"internal":       "The connection test failed internally.",
	}
	message := messages[class]
	if message == "" {
		message = messages["transport"]
	}
	return ConnectionDiagnostic{
		Kind: kind, Success: class == "none", Stage: stage, Class: class,
		Message: message, VPN: vpn, StatusCode: status,
	}
}

func errOrInvalid(err error) error {
	if err != nil {
		return err
	}
	return ErrDenied{Reason: "capability kind does not match the connection test"}
}
