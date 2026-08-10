package capability

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
)

type queuedProcessRunner struct {
	specs   []ProcessSpec
	results []ProcessResult
	errs    []error
}

func (r *queuedProcessRunner) Run(_ context.Context, spec ProcessSpec) (ProcessResult, error) {
	r.specs = append(r.specs, spec)
	index := len(r.specs) - 1
	if index >= len(r.results) {
		return ProcessResult{}, nil
	}
	var err error
	if index < len(r.errs) {
		err = r.errs[index]
	}
	return r.results[index], err
}

func approvedGit(t *testing.T, remoteURL string, operations []string) (model.Capability, time.Time) {
	t.Helper()
	return approvedCapability(t, model.Capability{
		Name: "git", Kind: model.CapabilityGit, AgentProfile: "agent-codex",
		Git: &model.GitScope{
			RemoteName: "origin", RemoteURL: remoteURL, Operations: operations,
			Branches: []string{"main"}, Refspecs: []string{"main:main"},
		},
	})
}

func gitRunnerFor(project, remote string) *queuedProcessRunner {
	return &queuedProcessRunner{results: []ProcessResult{
		{Stdout: project + "\n"},
		{Stdout: remote + "\n"},
		{ExitCode: 1},
		{ExitCode: 1},
		{Stdout: "ok"},
	}, errs: []error{nil, nil, errors.New("exit status 1"), errors.New("exit status 1"), nil}}
}

func TestGitExecutorUsesFreshExactRemoteAndFixedFetchArgs(t *testing.T) {
	project := t.TempDir()
	capability, now := approvedGit(t, "https://example.com/repo.git", []string{"fetch"})
	runner := gitRunnerFor(project, "https://example.com/repo.git")
	executor := GitExecutor{Runner: runner, Now: func() time.Time { return now }}
	result, err := executor.Execute(context.Background(), capability, nil, "agent-codex", project, GitRequest{Operation: "fetch", Branch: "main"})
	if err != nil || result.Stdout != "ok" {
		t.Fatalf("result=%+v err=%v", result, err)
	}
	operation := runner.specs[len(runner.specs)-1]
	joined := strings.Join(operation.Args, " ")
	if !strings.Contains(joined, "fetch --no-recurse-submodules --no-tags -- ptrack-approved-") ||
		!strings.Contains(joined, "://remote refs/heads/main") ||
		!strings.Contains(joined, "protocol.allow=never") ||
		!strings.Contains(joined, "url.https://example.com/repo.git.insteadOf=ptrack-approved-") ||
		!strings.Contains(joined, "core.hooksPath=") {
		t.Fatalf("fetch argv = %v", operation.Args)
	}
	if !contains(operation.Env, "GIT_TERMINAL_PROMPT=0") {
		t.Fatalf("fetch env = %v", operation.Env)
	}
}

func TestGitExecutorPinsPushToExactApprovedHeadAndURL(t *testing.T) {
	project := t.TempDir()
	capability, now := approvedGit(t, "https://example.com/repo.git", []string{"push"})
	runner := gitRunnerFor(project, "https://example.com/repo.git")
	executor := GitExecutor{Runner: runner, Now: func() time.Time { return now }}
	if _, err := executor.Execute(context.Background(), capability, nil, "agent-codex", project, GitRequest{
		Operation: "push", Branch: "main",
	}); err != nil {
		t.Fatal(err)
	}
	joined := strings.Join(runner.specs[len(runner.specs)-1].Args, " ")
	if !strings.Contains(joined, "push -- ptrack-approved-") ||
		!strings.Contains(joined, "://remote refs/heads/main:refs/heads/main") ||
		strings.Contains(joined, " -- origin ") {
		t.Fatalf("push argv = %v", runner.specs[len(runner.specs)-1].Args)
	}
}

func TestGitExecutorDeniesChangedRemoteAndURLRewritesBeforeOperation(t *testing.T) {
	project := t.TempDir()
	capability, now := approvedGit(t, "https://example.com/repo.git", []string{"fetch"})
	changed := gitRunnerFor(project, "https://evil.example/repo.git")
	executor := GitExecutor{Runner: changed, Now: func() time.Time { return now }}
	if _, err := executor.Execute(context.Background(), capability, nil, "agent-codex", project, GitRequest{Operation: "fetch", Branch: "main"}); err == nil {
		t.Fatal("changed remote authorized")
	}
	if len(changed.specs) != 4 {
		t.Fatalf("unexpected operation ran: %d specs", len(changed.specs))
	}

	rewritten := gitRunnerFor(project, "https://example.com/repo.git")
	rewritten.results[3] = ProcessResult{Stdout: "url.https://evil/.insteadOf https://example.com/\n"}
	rewritten.errs[3] = nil
	executor.Runner = rewritten
	if _, err := executor.Execute(context.Background(), capability, nil, "agent-codex", project, GitRequest{Operation: "fetch", Branch: "main"}); err == nil {
		t.Fatal("URL rewrite authorized")
	}
	if len(rewritten.specs) != 4 {
		t.Fatalf("operation ran after rewrite: %d specs", len(rewritten.specs))
	}
}

func TestGitExecutorRejectsMultipleURLsAndRemoteCommandOverrides(t *testing.T) {
	project := t.TempDir()
	capability, now := approvedGit(t, "https://example.com/repo.git", []string{"fetch", "push"})
	executor := GitExecutor{Now: func() time.Time { return now }}

	multiple := gitRunnerFor(project, "https://example.com/repo.git\nhttps://evil.example/repo.git")
	executor.Runner = multiple
	if _, err := executor.Execute(context.Background(), capability, nil, "agent-codex", project, GitRequest{Operation: "push", Branch: "main"}); err == nil {
		t.Fatal("multiple remote URLs authorized")
	}
	if len(multiple.specs) != 2 {
		t.Fatalf("multiple URLs ran %d commands", len(multiple.specs))
	}

	for _, override := range []string{
		"remote.origin.pushurl https://evil.example/repo.git\n",
		"remote.origin.uploadpack /tmp/helper\n",
		"remote.origin.receivepack /tmp/helper\n",
	} {
		runner := gitRunnerFor(project, "https://example.com/repo.git")
		runner.results[2] = ProcessResult{Stdout: override}
		runner.errs[2] = nil
		executor.Runner = runner
		if _, err := executor.Execute(context.Background(), capability, nil, "agent-codex", project, GitRequest{Operation: "fetch", Branch: "main"}); err == nil {
			t.Fatalf("remote override authorized: %q", override)
		}
		if len(runner.specs) != 3 {
			t.Fatalf("override ran %d commands", len(runner.specs))
		}
	}
}

func TestGitOverSSHRequiresMatchingPinnedSSHGrant(t *testing.T) {
	project := t.TempDir()
	gitCapability, now := approvedGit(t, "git@example.com:org/repo.git", []string{"fetch"})
	key := "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA=="
	sshCapability, _ := approvedSSH(t, &model.SSHScope{Host: "example.com", User: "git", HostKey: key, AllowGit: true})

	runner := gitRunnerFor(project, "git@example.com:org/repo.git")
	executor := GitExecutor{Runner: runner, Now: func() time.Time { return now }}
	if _, err := executor.Execute(context.Background(), gitCapability, nil, "agent-codex", project, GitRequest{Operation: "fetch", Branch: "main"}); err == nil {
		t.Fatal("Git-over-SSH ran without SSH grant")
	}

	runner = gitRunnerFor(project, "git@example.com:org/repo.git")
	executor.Runner = runner
	if _, err := executor.Execute(context.Background(), gitCapability, &sshCapability, "agent-codex", project, GitRequest{Operation: "fetch", Branch: "main"}); err != nil {
		t.Fatal(err)
	}
	operation := runner.specs[len(runner.specs)-1]
	env := strings.Join(operation.Env, "\n")
	if !strings.Contains(env, "GIT_SSH_COMMAND=ssh") || !strings.Contains(env, "StrictHostKeyChecking=yes") || !strings.Contains(env, "UserKnownHostsFile=") {
		t.Fatalf("Git SSH env = %v", operation.Env)
	}
}

func TestGitExecutorRejectsSymlinkedProjectIdentity(t *testing.T) {
	project := t.TempDir()
	realRepo := filepath.Join(project, "real")
	if err := os.Mkdir(realRepo, 0o755); err != nil {
		t.Fatal(err)
	}
	alias := filepath.Join(project, "alias")
	if err := os.Symlink(realRepo, alias); err != nil {
		t.Fatal(err)
	}
	capability, now := approvedGit(t, "https://example.com/repo.git", []string{"status"})
	runner := gitRunnerFor(realRepo, "https://example.com/repo.git")
	executor := GitExecutor{Runner: runner, Now: func() time.Time { return now }}
	if _, err := executor.Execute(context.Background(), capability, nil, "agent-codex", alias, GitRequest{Operation: "status"}); err != nil {
		t.Fatal(err)
	}
}

func TestClassifyGitError(t *testing.T) {
	for diagnostic, want := range map[string]string{
		"fatal: unable to access: Could not resolve host":     "dns",
		"SSL certificate problem: unable to get local issuer": "tls",
		"remote: protected branch hook declined":              "remote-policy",
		"fatal: could not read Username":                      "authentication",
	} {
		if got := ClassifyGitError(errors.New("exit status 1"), diagnostic); got != want {
			t.Errorf("%q => %q want %q", diagnostic, got, want)
		}
	}
}
