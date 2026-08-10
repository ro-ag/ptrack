package capability

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
)

// GitRequest exposes fixed operation fields instead of arbitrary Git argv.
type GitRequest struct {
	Operation string `json:"operation"`
	Branch    string `json:"branch,omitempty"`
	Refspec   string `json:"refspec,omitempty"`
	Force     bool   `json:"force,omitempty"`
}

// GitResult contains bounded transient process output.
type GitResult struct {
	ExitCode   int    `json:"exit_code"`
	Stdout     string `json:"stdout,omitempty"`
	Stderr     string `json:"stderr,omitempty"`
	Diagnostic string `json:"diagnostic"`
}

// GitExecutor performs fixed Git operations after re-reading repository and
// remote identity. An SSH remote additionally requires a matching SSH grant.
type GitExecutor struct {
	Runner   ProcessRunner
	Recorder Recorder
	Now      func() time.Time
}

// Execute performs one authorized Git operation in the canonical project
// repository. sshCapability is required only for SSH remotes.
func (e GitExecutor) Execute(
	ctx context.Context,
	gitCapability model.Capability,
	sshCapability *model.Capability,
	agentProfile, projectRoot string,
	request GitRequest,
) (result GitResult, retErr error) {
	runner := e.Runner
	if runner == nil {
		runner = ExecProcessRunner{}
	}
	now := time.Now
	if e.Now != nil {
		now = e.Now
	}
	canonicalRoot, err := filepath.EvalSymlinks(projectRoot)
	if err != nil {
		return result, ErrDenied{Reason: "Git project root cannot be canonicalized"}
	}
	canonicalRoot, err = filepath.Abs(canonicalRoot)
	if err != nil {
		return result, ErrDenied{Reason: "Git project root cannot be canonicalized"}
	}

	limitsPreview, err := Normalize(gitCapability)
	if err != nil || limitsPreview.Capability.Kind != model.CapabilityGit {
		return result, ErrDenied{Reason: "stored Git capability is invalid"}
	}
	limits := limitsPreview.Capability.Limits
	metadataCtx, cancelMetadata := context.WithTimeout(ctx, time.Duration(limits.TimeoutSeconds)*time.Second)
	defer cancelMetadata()
	rootResult, err := runner.Run(metadataCtx, gitProcess(canonicalRoot, limits.MaxOutputBytes, "rev-parse", "--show-toplevel"))
	if err != nil || rootResult.Truncated {
		return result, ErrDenied{Reason: "Git repository identity could not be verified"}
	}
	actualRoot, err := filepath.EvalSymlinks(strings.TrimSpace(rootResult.Stdout))
	if err != nil {
		return result, ErrDenied{Reason: "Git repository identity could not be verified"}
	}
	actualRoot, _ = filepath.Abs(actualRoot)
	if actualRoot != canonicalRoot {
		return result, ErrDenied{Reason: "Git repository root does not match the project"}
	}

	remoteName := limitsPreview.Capability.Git.RemoteName
	remoteResult, err := runner.Run(metadataCtx, gitProcess(canonicalRoot, limits.MaxOutputBytes, "config", "--get-all", "remote."+remoteName+".url"))
	if err != nil || remoteResult.Truncated {
		return result, ErrDenied{Reason: "Git remote could not be verified"}
	}
	remoteURLs := strings.Split(strings.TrimSuffix(strings.ReplaceAll(remoteResult.Stdout, "\r\n", "\n"), "\n"), "\n")
	if len(remoteURLs) != 1 || strings.TrimSpace(remoteURLs[0]) == "" || remoteURLs[0] != strings.TrimSpace(remoteURLs[0]) {
		return result, ErrDenied{Reason: "Git remote is invalid"}
	}
	remoteURL := remoteURLs[0]
	overrideResult, overrideErr := runner.Run(metadataCtx, gitProcess(
		canonicalRoot, limits.MaxOutputBytes, "config", "--get-regexp",
		`^remote\.`+regexp.QuoteMeta(remoteName)+`\.(pushurl|uploadpack|receivepack)$`,
	))
	if overrideErr != nil && overrideResult.ExitCode != 1 {
		return result, ErrDenied{Reason: "Git remote override policy could not be verified"}
	}
	if overrideResult.Truncated || strings.TrimSpace(overrideResult.Stdout) != "" {
		return result, ErrDenied{Reason: "Git remote overrides make the approved operation ambiguous"}
	}
	rewriteResult, rewriteErr := runner.Run(metadataCtx, gitProcess(canonicalRoot, limits.MaxOutputBytes, "config", "--get-regexp", `^url\..*\.(insteadOf|pushInsteadOf)$`))
	if rewriteErr != nil && rewriteResult.ExitCode != 1 {
		return result, ErrDenied{Reason: "Git URL rewrite policy could not be verified"}
	}
	if rewriteResult.Truncated || strings.TrimSpace(rewriteResult.Stdout) != "" {
		return result, ErrDenied{Reason: "Git URL rewrite rules make the approved remote ambiguous"}
	}

	normalized, err := AuthorizeGit(gitCapability, agentProfile, now(), GitAuthorization{
		Operation: request.Operation, RemoteName: remoteName, RemoteURL: remoteURL,
		Branch: request.Branch, Refspec: request.Refspec, Force: request.Force,
	})
	if err != nil {
		return result, err
	}

	env := []string{"LC_ALL=C", "LANG=C", "GIT_TERMINAL_PROMPT=0", "GCM_INTERACTIVE=Never"}
	cleanupSSH := func() {}
	if isSSHRemote(normalized.Git.RemoteURL) {
		if sshCapability == nil {
			return result, ErrDenied{Reason: "Git-over-SSH requires a separate SSH grant"}
		}
		approvedSSH, authErr := AuthorizeSSH(*sshCapability, agentProfile, now(), SSHGit, "")
		if authErr != nil || !gitRemoteMatchesSSH(normalized.Git.RemoteURL, approvedSSH.SSH) {
			return result, ErrDenied{Reason: "Git remote does not match the approved SSH host identity"}
		}
		directory, knownHosts, writeErr := writePinnedKnownHosts(approvedSSH.SSH)
		if writeErr != nil {
			return result, fmt.Errorf("prepare Git SSH host key: %w", writeErr)
		}
		cleanupSSH = func() { _ = os.RemoveAll(directory) }
		env = append(env,
			"GIT_SSH_VARIANT=ssh",
			"GIT_SSH_COMMAND="+gitSSHCommand(approvedSSH.SSH, knownHosts),
		)
	}
	defer cleanupSSH()

	hooksDir, err := os.MkdirTemp("", "ptrack-empty-hooks-")
	if err != nil {
		return result, err
	}
	defer os.RemoveAll(hooksDir)
	pinnedRemote, err := gitPinnedRemoteAlias()
	if err != nil {
		return result, err
	}
	operationSpec, err := buildGitOperation(normalized, canonicalRoot, hooksDir, env, request, pinnedRemote)
	if err != nil {
		return result, err
	}
	start := time.Now()
	defer func() {
		class := ClassifyGitError(retErr, result.Stderr)
		auditErr := e.Recorder.Record(context.Background(), normalized, AuditEvent{
			Operation: request.Operation, Target: remoteName,
			Success: retErr == nil, ErrorClass: class, Duration: time.Since(start),
			ResponseBytes: int64(len(result.Stdout) + len(result.Stderr)),
		})
		if auditErr != nil && retErr == nil {
			retErr = fmt.Errorf("record capability audit: %w", auditErr)
		}
	}()

	operationCtx, cancel := context.WithTimeout(ctx, time.Duration(normalized.Limits.TimeoutSeconds)*time.Second)
	defer cancel()
	processResult, runErr := runner.Run(operationCtx, operationSpec)
	result = GitResult{ExitCode: processResult.ExitCode, Stdout: processResult.Stdout, Stderr: processResult.Stderr}
	if processResult.Truncated {
		result.Diagnostic = "output-limit"
		return result, outputLimitError{}
	}
	if runErr != nil {
		result.Diagnostic = ClassifyGitError(runErr, processResult.Stderr)
		return result, gitExecutionError{Class: result.Diagnostic}
	}
	result.Diagnostic = "none"
	return result, nil
}

func gitProcess(root string, maximum int64, args ...string) ProcessSpec {
	return ProcessSpec{
		Name: "git", Args: append([]string{"-C", root}, args...),
		Env:            []string{"LC_ALL=C", "LANG=C", "GIT_TERMINAL_PROMPT=0", "GCM_INTERACTIVE=Never"},
		MaxOutputBytes: maximum,
	}
}

func buildGitOperation(
	capability model.Capability,
	root, hooksDir string,
	env []string,
	request GitRequest,
	pinnedRemote string,
) (ProcessSpec, error) {
	args := []string{
		"-C", root,
		"-c", "core.hooksPath=" + hooksDir,
		"-c", "protocol.allow=never",
		"-c", "protocol.ext.allow=never",
		"-c", "submodule.recurse=false",
		"-c", "fetch.recurseSubmodules=false",
		"-c", "push.recurseSubmodules=no",
	}
	scope := capability.Git
	if isSSHRemote(scope.RemoteURL) {
		args = append(args, "-c", "protocol.ssh.allow=always")
	} else {
		args = append(args, "-c", "protocol.https.allow=always")
	}
	// The repository remains writable by the launched agent. Pass a fresh,
	// unguessable transport alias instead of a mutable remote name or the raw
	// URL. Command-scope exact rewrites resolve the alias once to the approved
	// URL, so pre-existing project rewrites for that URL are never applied.
	args = append(args,
		"-c", "url."+scope.RemoteURL+".insteadOf="+pinnedRemote,
		"-c", "url."+scope.RemoteURL+".pushInsteadOf="+pinnedRemote,
	)
	remote := pinnedRemote
	switch request.Operation {
	case "status":
		args = append(args, "status", "--short", "--branch")
	case "fetch":
		args = append(args, "fetch", "--no-recurse-submodules")
		if scope.AllowTags {
			args = append(args, "--tags")
		} else {
			args = append(args, "--no-tags")
		}
		args = append(args, "--", remote)
		if request.Refspec != "" {
			args = append(args, request.Refspec)
		} else {
			args = append(args, "refs/heads/"+request.Branch)
		}
	case "pull":
		args = append(args, "pull", "--ff-only", "--no-rebase", "--no-recurse-submodules")
		if !scope.AllowTags {
			args = append(args, "--no-tags")
		}
		args = append(args, "--", remote, "refs/heads/"+request.Branch)
	case "push":
		args = append(args, "push")
		if request.Force {
			args = append(args, "--force-with-lease")
		}
		args = append(args, "--", remote)
		if request.Refspec != "" {
			args = append(args, request.Refspec)
		} else {
			args = append(args, "refs/heads/"+request.Branch+":refs/heads/"+request.Branch)
		}
	case "ls-remote":
		args = append(args, "ls-remote", "--", remote)
		if request.Branch != "" {
			args = append(args, "refs/heads/"+request.Branch)
		}
	default:
		return ProcessSpec{}, ErrDenied{Reason: "unsupported Git operation"}
	}
	return ProcessSpec{Name: "git", Args: args, Env: env, MaxOutputBytes: capability.Limits.MaxOutputBytes}, nil
}

func gitPinnedRemoteAlias() (string, error) {
	value := make([]byte, 24)
	if _, err := rand.Read(value); err != nil {
		return "", err
	}
	return "ptrack-approved-" + hex.EncodeToString(value) + "://remote", nil
}

func isSSHRemote(remote string) bool {
	return strings.HasPrefix(remote, "ssh://") || !strings.Contains(remote, "://")
}

func gitRemoteMatchesSSH(remote string, scope *model.SSHScope) bool {
	user, host, port, err := gitSSHIdentity(remote)
	return err == nil && user == scope.User && host == scope.Host && port == scope.Port
}

func gitSSHIdentity(remote string) (string, string, uint16, error) {
	if strings.HasPrefix(remote, "ssh://") {
		u, err := url.Parse(remote)
		if err != nil || u.User == nil {
			return "", "", 0, errors.New("SSH Git remote must include a user")
		}
		port := uint16(22)
		if u.Port() != "" {
			parsed, parseErr := strconv.Atoi(u.Port())
			if parseErr != nil || parsed < 1 || parsed > 65535 {
				return "", "", 0, errors.New("invalid SSH Git port")
			}
			port = uint16(parsed)
		}
		return u.User.Username(), u.Hostname(), port, nil
	}
	colon := strings.Index(remote, ":")
	if colon <= 0 {
		return "", "", 0, errors.New("invalid SCP-style Git remote")
	}
	at := strings.LastIndex(remote[:colon], "@")
	if at <= 0 {
		return "", "", 0, errors.New("SCP-style Git remote must include a user")
	}
	return remote[:at], remote[at+1 : colon], 22, nil
}

func gitSSHCommand(scope *model.SSHScope, knownHosts string) string {
	args := sshBaseArgs(scope, knownHosts)
	parts := []string{"ssh"}
	for _, arg := range args {
		parts = append(parts, shellQuote(arg))
	}
	return strings.Join(parts, " ")
}

func shellQuote(value string) string {
	return "'" + strings.ReplaceAll(value, "'", `'\''`) + "'"
}

type gitExecutionError struct{ Class string }

func (e gitExecutionError) Error() string { return "Git operation failed: " + e.Class }

// ClassifyGitError returns stable diagnostics without persisting raw stderr.
func ClassifyGitError(err error, stderr string) string {
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
	if errors.Is(err, context.DeadlineExceeded) {
		return "timeout"
	}
	if errors.Is(err, context.Canceled) {
		return "cancelled"
	}
	lower := strings.ToLower(stderr)
	switch {
	case strings.Contains(lower, "could not resolve host"), strings.Contains(lower, "could not resolve hostname"):
		return "dns"
	case strings.Contains(lower, "ssl certificate problem"), strings.Contains(lower, "certificate verify failed"):
		return "tls"
	case strings.Contains(lower, "host key verification failed"):
		return "host-key"
	case strings.Contains(lower, "authentication failed"), strings.Contains(lower, "could not read username"), strings.Contains(lower, "permission denied (publickey"):
		return "authentication"
	case strings.Contains(lower, "protected branch"), strings.Contains(lower, "remote rejected"), strings.Contains(lower, "repository not found"), strings.Contains(lower, "permission to"):
		return "remote-policy"
	case strings.Contains(lower, "connection refused"), strings.Contains(lower, "no route to host"), strings.Contains(lower, "failed to connect"):
		return "routing"
	default:
		return ClassifyConnectionError(err)
	}
}
