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

type recordingProcessRunner struct {
	spec    ProcessSpec
	result  ProcessResult
	err     error
	inspect func(ProcessSpec)
}

func (r *recordingProcessRunner) Run(_ context.Context, spec ProcessSpec) (ProcessResult, error) {
	r.spec = spec
	if r.inspect != nil {
		r.inspect(spec)
	}
	return r.result, r.err
}

func TestSSHCommandUsesPinnedKeyAgentOnlyAndExactCommand(t *testing.T) {
	key := "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA=="
	capability, now := approvedSSH(t, &model.SSHScope{
		Host: "example.com", Port: 2222, User: "deploy", HostKey: key,
		RemoteCommands: []string{"printf safe"},
	})
	runner := &recordingProcessRunner{result: ProcessResult{Stdout: "safe"}}
	executor := SSHExecutor{Runner: runner, Now: func() time.Time { return now }}
	result, err := executor.Execute(context.Background(), capability, "agent-codex", t.TempDir(), SSHRequest{
		Operation: SSHRemoteCommand, Command: "printf safe",
	})
	if err != nil || result.Stdout != "safe" {
		t.Fatalf("result=%+v err=%v", result, err)
	}
	joined := strings.Join(runner.spec.Args, " ")
	for _, required := range []string{"BatchMode=yes", "PasswordAuthentication=no", "StrictHostKeyChecking=yes", "UserKnownHostsFile=", "-p 2222", "deploy@example.com", "printf safe"} {
		if !strings.Contains(joined, required) {
			t.Errorf("argv missing %q: %v", required, runner.spec.Args)
		}
	}
	if strings.Contains(joined, "IdentityFile") || runner.spec.Name != "ssh" {
		t.Errorf("unexpected process: %+v", runner.spec)
	}
}

func TestSSHUploadConfinesLocalRemotePathsAndUsesDirectSCPArgs(t *testing.T) {
	project := t.TempDir()
	if err := os.Mkdir(filepath.Join(project, "dist"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(project, "dist", "app.js"), []byte("x"), 0o600); err != nil {
		t.Fatal(err)
	}
	key := "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA=="
	capability, now := approvedSSH(t, &model.SSHScope{
		Host: "example.com", User: "deploy", HostKey: key,
		AllowUpload: true, UploadRoots: []string{"dist"}, UploadRemoteRoots: []string{"/srv/app"},
	})
	var stagedSource string
	runner := &recordingProcessRunner{inspect: func(spec ProcessSpec) {
		separator := indexOf(spec.Args, "--")
		if separator < 0 || separator+1 >= len(spec.Args) {
			t.Fatalf("scp args = %v", spec.Args)
		}
		stagedSource = spec.Args[separator+1]
		contents, readErr := os.ReadFile(stagedSource)
		if readErr != nil || string(contents) != "x" {
			t.Fatalf("staged upload = %q, %v", contents, readErr)
		}
		if stagedSource == filepath.Join(project, "dist", "app.js") {
			t.Fatal("scp received the mutable project pathname")
		}
	}}
	executor := SSHExecutor{Runner: runner, Now: func() time.Time { return now }}
	_, err := executor.Execute(context.Background(), capability, "agent-codex", project, SSHRequest{
		Operation: SSHUpload, LocalPath: "dist/app.js", RemotePath: "/srv/app/app.js",
	})
	if err != nil {
		t.Fatal(err)
	}
	if runner.spec.Name != "scp" || !contains(runner.spec.Args, "--") || !contains(runner.spec.Args, "deploy@example.com:/srv/app/app.js") {
		t.Fatalf("scp spec = %+v", runner.spec)
	}
	if _, err := os.Stat(stagedSource); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("private upload staging file remains: %v", err)
	}
	if _, err := executor.Execute(context.Background(), capability, "agent-codex", project, SSHRequest{
		Operation: SSHUpload, LocalPath: "dist/app.js", RemotePath: "/etc/passwd",
	}); err == nil {
		t.Fatal("remote upload escape authorized")
	}
}

func TestSSHTransfersEnforceByteLimitsAndInstallDownloadsSafely(t *testing.T) {
	project := t.TempDir()
	if err := os.MkdirAll(filepath.Join(project, "dist"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(project, "dist", "large.bin"), []byte("too-large"), 0o600); err != nil {
		t.Fatal(err)
	}
	key := "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA=="
	upload, now := approvedCapability(t, model.Capability{
		Name: "upload", Kind: model.CapabilitySSH, AgentProfile: "agent-codex",
		Limits: model.CapabilityLimits{MaxRequestBytes: 4},
		SSH: &model.SSHScope{
			Host: "example.com", User: "deploy", HostKey: key,
			AllowUpload: true, UploadRoots: []string{"dist"}, UploadRemoteRoots: []string{"/srv/app"},
		},
	})
	executor := SSHExecutor{Runner: &recordingProcessRunner{}, Now: func() time.Time { return now }}
	if _, err := executor.Execute(context.Background(), upload, "agent-codex", project, SSHRequest{
		Operation: SSHUpload, LocalPath: "dist/large.bin", RemotePath: "/srv/app/large.bin",
	}); ClassifySSHError(err, "") != "request-limit" {
		t.Fatalf("oversized upload error = %v", err)
	}

	if err := os.MkdirAll(filepath.Join(project, "artifacts"), 0o755); err != nil {
		t.Fatal(err)
	}
	download, _ := approvedCapability(t, model.Capability{
		Name: "download", Kind: model.CapabilitySSH, AgentProfile: "agent-codex",
		Limits: model.CapabilityLimits{MaxResponseBytes: 16},
		SSH: &model.SSHScope{
			Host: "example.com", User: "deploy", HostKey: key,
			AllowDownload: true, DownloadRoots: []string{"artifacts"}, DownloadRemoteRoots: []string{"/srv/releases"},
		},
	})
	runner := &recordingProcessRunner{result: ProcessResult{Stdout: "release"}}
	executor.Runner = runner
	if _, err := executor.Execute(context.Background(), download, "agent-codex", project, SSHRequest{
		Operation: SSHDownload, LocalPath: "artifacts/release.txt", RemotePath: "/srv/releases/release.txt",
	}); err != nil {
		t.Fatal(err)
	}
	contents, err := os.ReadFile(filepath.Join(project, "artifacts", "release.txt"))
	if err != nil || string(contents) != "release" {
		t.Fatalf("installed download = %q, %v", contents, err)
	}
	if runner.spec.Name != "ssh" || runner.spec.MaxOutputBytes != 16 ||
		!contains(runner.spec.Args, "cat -- /srv/releases/release.txt") {
		t.Fatalf("bounded download spec = %+v", runner.spec)
	}
	runner.result = ProcessResult{Stdout: strings.Repeat("x", 16), Truncated: true}
	if result, err := executor.Execute(context.Background(), download, "agent-codex", project, SSHRequest{
		Operation: SSHDownload, LocalPath: "artifacts/oversized.txt", RemotePath: "/srv/releases/oversized.txt",
	}); err == nil || result.Stdout != "" {
		t.Fatalf("unbounded download result=%+v err=%v", result, err)
	}
}

func TestSSHDownloadRejectsDestinationSymlinkSwapAfterTransfer(t *testing.T) {
	project := t.TempDir()
	outside := t.TempDir()
	artifacts := filepath.Join(project, "artifacts")
	if err := os.Mkdir(artifacts, 0o755); err != nil {
		t.Fatal(err)
	}
	key := "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA=="
	capability, now := approvedSSH(t, &model.SSHScope{
		Host: "example.com", User: "deploy", HostKey: key,
		AllowDownload: true, DownloadRoots: []string{"artifacts"}, DownloadRemoteRoots: []string{"/srv/releases"},
	})
	runner := &recordingProcessRunner{result: ProcessResult{Stdout: "secret"}, inspect: func(spec ProcessSpec) {
		if err := os.Rename(artifacts, filepath.Join(project, "artifacts-original")); err != nil {
			t.Fatal(err)
		}
		if err := os.Symlink(outside, artifacts); err != nil {
			t.Fatal(err)
		}
	}}
	executor := SSHExecutor{Runner: runner, Now: func() time.Time { return now }}
	_, err := executor.Execute(context.Background(), capability, "agent-codex", project, SSHRequest{
		Operation: SSHDownload, LocalPath: "artifacts/release.txt", RemotePath: "/srv/releases/release.txt",
	})
	if ClassifySSHError(err, "") != "denied" {
		t.Fatalf("symlink swap error = %v", err)
	}
	if _, statErr := os.Stat(filepath.Join(outside, "release.txt")); !errors.Is(statErr, os.ErrNotExist) {
		t.Fatalf("download escaped project: %v", statErr)
	}
}

func indexOf(values []string, wanted string) int {
	for index, value := range values {
		if value == wanted {
			return index
		}
	}
	return -1
}

func TestSSHForwardingIsLoopbackAndDirectionScoped(t *testing.T) {
	key := "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA=="
	capability, now := approvedSSH(t, &model.SSHScope{
		Host: "example.com", User: "deploy", HostKey: key,
		LocalForwardTargets: []string{"db.internal:5432"},
	})
	runner := &recordingProcessRunner{}
	executor := SSHExecutor{Runner: runner, Now: func() time.Time { return now }}
	_, err := executor.Execute(context.Background(), capability, "agent-codex", t.TempDir(), SSHRequest{
		Operation: SSHLocalForward, ForwardTarget: "db.internal:5432", ListenPort: 15432,
	})
	if err != nil {
		t.Fatal(err)
	}
	joined := strings.Join(runner.spec.Args, " ")
	if !strings.Contains(joined, "-L 127.0.0.1:15432:db.internal:5432") {
		t.Fatalf("forward argv = %v", runner.spec.Args)
	}
	if _, err := executor.Execute(context.Background(), capability, "agent-codex", t.TempDir(), SSHRequest{
		Operation: SSHRemoteForward, ForwardTarget: "db.internal:5432", ListenPort: 15432,
	}); err == nil {
		t.Fatal("local forward approval implied remote forwarding")
	}
}

func TestClassifySSHErrorUsesSanitizedClasses(t *testing.T) {
	for diagnostic, want := range map[string]string{
		"ssh: Could not resolve hostname x":                "dns",
		"Host key verification failed":                     "host-key",
		"Permission denied (publickey)":                    "authentication",
		"channel open failed: administratively prohibited": "remote-policy",
	} {
		if got := ClassifySSHError(errors.New("exit status 255"), diagnostic); got != want {
			t.Errorf("%q => %q want %q", diagnostic, got, want)
		}
	}
}
