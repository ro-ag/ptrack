package capability

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
)

// SSHRequest is a typed SSH operation. Only the fields required by Operation
// are used; no raw argv surface is exposed.
type SSHRequest struct {
	Operation     SSHOperation `json:"operation"`
	Command       string       `json:"command,omitempty"`
	LocalPath     string       `json:"local_path,omitempty"`
	RemotePath    string       `json:"remote_path,omitempty"`
	ForwardTarget string       `json:"forward_target,omitempty"`
	ListenPort    int          `json:"listen_port,omitempty"`
}

// SSHResult returns bounded transient output and a stable diagnostic class.
type SSHResult struct {
	ExitCode   int    `json:"exit_code"`
	Stdout     string `json:"stdout,omitempty"`
	Stderr     string `json:"stderr,omitempty"`
	Diagnostic string `json:"diagnostic"`
}

// SSHExecutor uses host OpenSSH/scp and the current ssh-agent. It never stores
// private keys or enables password/keyboard-interactive fallback.
type SSHExecutor struct {
	Runner   ProcessRunner
	Recorder Recorder
	Now      func() time.Time
}

// Execute performs one separately authorized SSH operation.
func (e SSHExecutor) Execute(
	ctx context.Context,
	capability model.Capability,
	agentProfile, projectRoot string,
	request SSHRequest,
) (result SSHResult, retErr error) {
	now := time.Now
	if e.Now != nil {
		now = e.Now
	}
	value := request.Command
	if request.Operation == SSHLocalForward || request.Operation == SSHRemoteForward {
		value = request.ForwardTarget
	}
	normalized, err := AuthorizeSSH(capability, agentProfile, now(), request.Operation, value)
	if err != nil {
		return result, err
	}
	runner := e.Runner
	if runner == nil {
		runner = ExecProcessRunner{}
	}
	knownHostsDir, knownHostsPath, err := writePinnedKnownHosts(normalized.SSH)
	if err != nil {
		return result, fmt.Errorf("prepare pinned host key: %w", err)
	}
	defer os.RemoveAll(knownHostsDir)

	plan, err := buildSSHProcess(normalized, projectRoot, knownHostsPath, request)
	if err != nil {
		return result, err
	}
	defer plan.cleanup()
	var transferBytes int64
	start := time.Now()
	defer func() {
		class := ClassifySSHError(retErr, result.Stderr)
		auditErr := e.Recorder.Record(context.Background(), normalized, AuditEvent{
			Operation: string(request.Operation), Target: netJoinAuditTarget(normalized.SSH.Host, normalized.SSH.Port),
			Success: retErr == nil, ErrorClass: class, Duration: time.Since(start),
			ResponseBytes: transferBytes + int64(len(result.Stdout)+len(result.Stderr)),
		})
		if auditErr != nil && retErr == nil {
			retErr = fmt.Errorf("record capability audit: %w", auditErr)
		}
	}()

	timeoutCtx, cancel := context.WithTimeout(ctx, time.Duration(normalized.Limits.TimeoutSeconds)*time.Second)
	defer cancel()
	processResult, runErr := runner.Run(timeoutCtx, plan.spec)
	if request.Operation == SSHDownload {
		transferBytes = int64(len(processResult.Stdout))
	}
	result = SSHResult{
		ExitCode: processResult.ExitCode, Stdout: processResult.Stdout, Stderr: processResult.Stderr,
	}
	if request.Operation == SSHDownload {
		// Download payload is written to the approved project destination; it
		// is not duplicated into the broker response.
		result.Stdout = ""
	}
	if processResult.Truncated {
		if request.Operation == SSHDownload {
			result.Diagnostic = "response-limit"
			return result, responseLimitError{}
		}
		result.Diagnostic = "output-limit"
		return result, outputLimitError{}
	}
	if runErr != nil {
		result.Diagnostic = ClassifySSHError(runErr, processResult.Stderr)
		return result, sshExecutionError{Class: result.Diagnostic}
	}
	if err := plan.complete(processResult); err != nil {
		result.Diagnostic = ClassifySSHError(err, "")
		return result, err
	}
	result.Diagnostic = "none"
	return result, nil
}

type sshProcessPlan struct {
	spec     ProcessSpec
	complete func(ProcessResult) error
	cleanup  func()
}

func buildSSHProcess(capability model.Capability, projectRoot, knownHostsPath string, request SSHRequest) (sshProcessPlan, error) {
	scope := capability.SSH
	base := sshBaseArgs(scope, knownHostsPath)
	target := scope.User + "@" + scope.Host
	spec := ProcessSpec{
		Name: "ssh", Env: []string{"LC_ALL=C", "LANG=C"}, MaxOutputBytes: capability.Limits.MaxOutputBytes,
	}
	plan := sshProcessPlan{spec: spec, complete: func(ProcessResult) error { return nil }, cleanup: func() {}}
	switch request.Operation {
	case SSHGit:
		return sshProcessPlan{}, ErrDenied{Reason: "Git-over-SSH must be invoked through the Git capability intersection"}
	case SSHRemoteCommand:
		spec.Args = append(base, "-T", target, request.Command)
	case SSHUpload:
		if !anyRemotePathWithin(scope.UploadRemoteRoots, request.RemotePath) {
			return sshProcessPlan{}, ErrDenied{Reason: "upload path is outside approved roots"}
		}
		local, cleanup, err := stageSSHUpload(
			projectRoot, request.LocalPath, scope.UploadRoots, capability.Limits.MaxRequestBytes,
		)
		if err != nil {
			return sshProcessPlan{}, err
		}
		plan.cleanup = cleanup
		spec.Name = "scp"
		spec.Args = append(scpBaseArgs(scope, knownHostsPath), "--", local, target+":"+request.RemotePath)
	case SSHDownload:
		if !anyRemotePathWithin(scope.DownloadRemoteRoots, request.RemotePath) {
			return sshProcessPlan{}, ErrDenied{Reason: "download path is outside approved roots"}
		}
		complete, cleanup, err := stageSSHDownload(
			projectRoot, request.LocalPath, scope.DownloadRoots, capability.Limits.MaxResponseBytes,
		)
		if err != nil {
			return sshProcessPlan{}, err
		}
		plan.complete = func(processResult ProcessResult) error {
			return complete([]byte(processResult.Stdout))
		}
		plan.cleanup = cleanup
		// Remote paths contain only normalized shell-inert characters. Streaming
		// through bounded stdout prevents a hostile server from filling local
		// disk before the response limit is enforced.
		spec.MaxOutputBytes = capability.Limits.MaxResponseBytes
		spec.Args = append(base, "-T", target, "cat -- "+request.RemotePath)
	case SSHInteractiveShell:
		return sshProcessPlan{}, ErrDenied{Reason: "interactive SSH shells are unavailable through the capability broker transport"}
	case SSHLocalForward, SSHRemoteForward:
		if request.ListenPort < 1 || request.ListenPort > 65535 {
			return sshProcessPlan{}, ErrDenied{Reason: "forward listen port is invalid"}
		}
		flag := "-L"
		if request.Operation == SSHRemoteForward {
			flag = "-R"
		}
		forward := "127.0.0.1:" + strconv.Itoa(request.ListenPort) + ":" + request.ForwardTarget
		spec.Args = append(base, "-o", "ClearAllForwardings=no", "-o", "ExitOnForwardFailure=yes", "-N", flag, forward, target)
	default:
		return sshProcessPlan{}, ErrDenied{Reason: "unsupported SSH operation"}
	}
	plan.spec = spec
	return plan, nil
}

func sshBaseArgs(scope *model.SSHScope, knownHostsPath string) []string {
	return []string{
		"-F", os.DevNull,
		"-o", "BatchMode=yes",
		"-o", "PasswordAuthentication=no",
		"-o", "KbdInteractiveAuthentication=no",
		"-o", "StrictHostKeyChecking=yes",
		"-o", "UserKnownHostsFile=" + knownHostsPath,
		"-o", "GlobalKnownHostsFile=" + os.DevNull,
		"-o", "PermitLocalCommand=no",
		"-o", "ClearAllForwardings=yes",
		"-p", strconv.Itoa(int(scope.Port)),
	}
}

func scpBaseArgs(scope *model.SSHScope, knownHostsPath string) []string {
	args := sshBaseArgs(scope, knownHostsPath)
	for index := range args {
		if args[index] == "-p" {
			args[index] = "-P"
		}
	}
	return args
}

func writePinnedKnownHosts(scope *model.SSHScope) (string, string, error) {
	directory, err := os.MkdirTemp("", "ptrack-known-hosts-")
	if err != nil {
		return "", "", err
	}
	if err := os.Chmod(directory, 0o700); err != nil {
		_ = os.RemoveAll(directory)
		return "", "", err
	}
	host := scope.Host
	if scope.Port != 22 {
		host = "[" + host + "]:" + strconv.Itoa(int(scope.Port))
	}
	path := filepath.Join(directory, "known_hosts")
	if err := os.WriteFile(path, []byte(host+" "+scope.HostKey+"\n"), 0o600); err != nil {
		_ = os.RemoveAll(directory)
		return "", "", err
	}
	return directory, path, nil
}

func anyRemotePathWithin(roots []string, candidate string) bool {
	for _, root := range roots {
		if remotePathWithin(root, candidate) {
			return true
		}
	}
	return false
}

func netJoinAuditTarget(host string, port uint16) string {
	if strings.Contains(host, ":") {
		host = "[" + host + "]"
	}
	return host + ":" + strconv.Itoa(int(port))
}

type sshExecutionError struct{ Class string }

func (e sshExecutionError) Error() string { return "SSH operation failed: " + e.Class }

type outputLimitError struct{}

func (outputLimitError) Error() string { return "process output exceeds its byte limit" }

// ClassifySSHError maps OpenSSH's stable C-locale diagnostics without
// persisting raw stderr. Unknown diagnostics stay generic.
func ClassifySSHError(err error, stderr string) string {
	if err == nil {
		return "none"
	}
	var denied ErrDenied
	if errors.As(err, &denied) {
		return "denied"
	}
	var limit outputLimitError
	if errors.As(err, &limit) {
		return "output-limit"
	}
	var requestLimit requestLimitError
	if errors.As(err, &requestLimit) {
		return "request-limit"
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return "timeout"
	}
	if errors.Is(err, context.Canceled) {
		return "cancelled"
	}
	lower := strings.ToLower(stderr)
	switch {
	case strings.Contains(lower, "could not resolve hostname"):
		return "dns"
	case strings.Contains(lower, "host key verification failed"), strings.Contains(lower, "remote host identification has changed"):
		return "host-key"
	case strings.Contains(lower, "permission denied"), strings.Contains(lower, "no more authentication methods"):
		return "authentication"
	case strings.Contains(lower, "network is unreachable"), strings.Contains(lower, "no route to host"), strings.Contains(lower, "connection refused"):
		return "routing"
	case strings.Contains(lower, "operation timed out"), strings.Contains(lower, "connection timed out"):
		return "timeout"
	case strings.Contains(lower, "administratively prohibited"), strings.Contains(lower, "not allowed"):
		return "remote-policy"
	case strings.Contains(lower, "operation not permitted"):
		return "sandbox"
	default:
		return ClassifyConnectionError(err)
	}
}
