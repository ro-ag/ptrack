//go:build !windows

package terminal

import (
	"context"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"
)

func TestShellIntegrationOwnerCreatesPrivateBoundedHooksAndCleansUp(t *testing.T) {
	owner, err := newShellIntegrationOwner(map[string]Profile{
		"shell": {
			ID: "shell", Name: "Shell", Kind: ProfileShell,
			Executable: filepath.Join(string(filepath.Separator), "bin", "zsh"),
		},
	})
	if err != nil {
		t.Fatalf("newShellIntegrationOwner: %v", err)
	}
	if owner == nil {
		t.Fatal("supported shell did not create an integration owner")
	}
	directory := owner.directory
	info, err := os.Stat(directory)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm()&0o077 != 0 {
		t.Fatalf("integration directory permissions = %o", info.Mode().Perm())
	}
	entries, err := os.ReadDir(directory)
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 6 {
		t.Fatalf("integration files = %d, want 6", len(entries))
	}
	for _, entry := range entries {
		fileInfo, statErr := entry.Info()
		if statErr != nil {
			t.Fatal(statErr)
		}
		if fileInfo.Size() <= 0 || fileInfo.Size() > 16*1024 || fileInfo.Mode().Perm() != 0o600 {
			t.Fatalf("integration file %s metadata = size %d mode %o", entry.Name(), fileInfo.Size(), fileInfo.Mode().Perm())
		}
	}
	if err := owner.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}
	if _, err := os.Stat(directory); !os.IsNotExist(err) {
		t.Fatalf("integration directory remains after close: %v", err)
	}
}

func TestZshIntegrationPreservesInteractiveLaunchAndSourcesUserStartup(t *testing.T) {
	owner := shellIntegrationOwnerForTest(t)
	defer owner.Close()
	profile := Profile{Kind: ProfileShell, Executable: "/bin/zsh", Args: []string{"-l"}}
	baseEnvironment := []string{"HOME=/users/test", "PATH=/bin"}
	originalArgs := append([]string(nil), profile.Args...)
	originalEnvironment := append([]string(nil), baseEnvironment...)

	args, environment, descriptor := owner.prepare(profile, baseEnvironment, "nonce-value")
	if descriptor.Quality != ShellIntegrationRich || descriptor.Nonce != "nonce-value" {
		t.Fatalf("descriptor = %#v", descriptor)
	}
	if !reflect.DeepEqual(args, []string{"-l"}) ||
		environmentValue(environment, "ZDOTDIR") != owner.directory ||
		environmentValue(environment, shellIntegrationOriginalZDOTDIR) != "/users/test" ||
		environmentValue(environment, shellIntegrationNonceEnvironment) != "nonce-value" {
		t.Fatalf("zsh launch args=%v environment=%v", args, environment)
	}
	if !reflect.DeepEqual(profile.Args, originalArgs) || !reflect.DeepEqual(baseEnvironment, originalEnvironment) {
		t.Fatal("zsh integration mutated caller data")
	}
	zshrc, err := os.ReadFile(filepath.Join(owner.directory, ".zshrc"))
	if err != nil {
		t.Fatal(err)
	}
	content := string(zshrc)
	if strings.Index(content, `source "${PTRACK_SHELL_ORIGINAL_ZDOTDIR_V1}/.zshrc"`) >
		strings.Index(content, "add-zsh-hook precmd") {
		t.Fatal("zsh hooks installed before the user startup file")
	}
	for _, expected := range []string{"133;A", "133;B", "133;C", "133;D", "633;A", "633;B", "633;C", "633;D", "]7;file://", "633;P;Cwd=", "unset PTRACK_SHELL_INTEGRATION_NONCE_V1"} {
		if !strings.Contains(content, expected) {
			t.Fatalf("zsh integration missing %q", expected)
		}
	}
	if strings.Contains(content, "633;E") {
		t.Fatal("zsh integration emits command text marker")
	}
}

func TestBashIntegrationUsesPrivateInitFileAndPreservesUserPromptCommand(t *testing.T) {
	owner := shellIntegrationOwnerForTest(t)
	defer owner.Close()
	profile := Profile{Kind: ProfileShell, Executable: "/bin/bash", Args: []string{"-i"}}
	baseEnvironment := []string{"HOME=/users/test", "PROMPT_COMMAND=user_prompt"}
	args, environment, descriptor := owner.prepare(profile, baseEnvironment, "nonce-value")
	if descriptor.Quality != ShellIntegrationRich ||
		!reflect.DeepEqual(args, []string{"--init-file", owner.bashRegular, "-i"}) ||
		environmentValue(environment, shellIntegrationNonceEnvironment) != "nonce-value" {
		t.Fatalf("bash launch args=%v environment=%v descriptor=%#v", args, environment, descriptor)
	}
	contents, err := os.ReadFile(owner.bashRegular)
	if err != nil {
		t.Fatal(err)
	}
	content := string(contents)
	for _, expected := range []string{"~/.bashrc", "__ptrack_original_prompt_command", "__ptrack_original_debug_trap", "133;A", "133;B", "133;C", "133;D", "633;A", "633;B", "633;C", "633;D", "unset PTRACK_SHELL_INTEGRATION_NONCE_V1"} {
		if !strings.Contains(content, expected) {
			t.Fatalf("bash integration missing %q", expected)
		}
	}
	if strings.Contains(content, "633;E") {
		t.Fatal("bash integration emits command text marker")
	}
}

func TestBashLoginShellDegradesWithoutChangingLaunchSemantics(t *testing.T) {
	owner := shellIntegrationOwnerForTest(t)
	defer owner.Close()
	profile := Profile{Kind: ProfileShell, Executable: "/bin/bash", Args: []string{"--login"}}
	baseEnvironment := []string{"HOME=/users/test", "PROMPT_COMMAND=user_prompt"}

	args, environment, descriptor := owner.prepare(profile, baseEnvironment, "nonce-value")
	if descriptor.Quality != ShellIntegrationNone || descriptor.Nonce != "" ||
		!reflect.DeepEqual(args, profile.Args) || !reflect.DeepEqual(environment, baseEnvironment) {
		t.Fatalf("login bash was rewritten: args=%v environment=%v descriptor=%#v", args, environment, descriptor)
	}
}

func TestShellIntegrationDegradesUnknownAgentAndCommandLaunchesWithoutMutation(t *testing.T) {
	owner := shellIntegrationOwnerForTest(t)
	defer owner.Close()
	tests := []Profile{
		{Kind: ProfileAgent, Executable: "/bin/zsh", Args: []string{"-l"}},
		{Kind: ProfileShell, Executable: "/bin/zsh", Args: []string{"-c", "echo hidden"}},
		{Kind: ProfileShell, Executable: "/bin/bash", Args: []string{"-l"}},
		{Kind: ProfileShell, Executable: "/bin/bash", Args: []string{"script.sh"}},
		{Kind: ProfileShell, Executable: "/bin/fish", Args: []string{"-l"}},
	}
	for _, profile := range tests {
		baseEnvironment := []string{"HOME=/users/test"}
		args, environment, descriptor := owner.prepare(profile, baseEnvironment, "nonce")
		if descriptor.Quality != ShellIntegrationNone || descriptor.Nonce != "" ||
			!reflect.DeepEqual(args, profile.Args) || !reflect.DeepEqual(environment, baseEnvironment) {
			t.Fatalf("unsupported launch was rewritten: profile=%#v args=%v env=%v descriptor=%#v", profile, args, environment, descriptor)
		}
	}
}

func TestManagerKeepsShellHooksUntilShutdown(t *testing.T) {
	factory := &fakePTYFactory{}
	manager, err := NewManager(t.TempDir(), []Profile{{
		ID: "shell", Name: "Shell", Kind: ProfileShell, Executable: "/bin/zsh",
	}}, factory)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	if manager.shells == nil {
		t.Fatal("manager did not create shell hooks")
	}
	directory := manager.shells.directory
	session, err := manager.Create("shell", "", 24, 80)
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	if err := manager.CloseSession(session.ID(), true); err != nil {
		t.Fatalf("CloseSession: %v", err)
	}
	if _, err := os.Stat(directory); err != nil {
		t.Fatalf("hooks removed before manager shutdown: %v", err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if err := manager.Shutdown(ctx); err != nil {
		t.Fatalf("Shutdown: %v", err)
	}
	if _, err := os.Stat(directory); !os.IsNotExist(err) {
		t.Fatalf("hooks remain after manager shutdown: %v", err)
	}
}

func shellIntegrationOwnerForTest(t *testing.T) *shellIntegrationOwner {
	t.Helper()
	owner, err := newShellIntegrationOwner(map[string]Profile{
		"zsh":  {ID: "zsh", Name: "zsh", Kind: ProfileShell, Executable: "/bin/zsh"},
		"bash": {ID: "bash", Name: "bash", Kind: ProfileShell, Executable: "/bin/bash"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if owner == nil {
		t.Fatal("shell integration owner is nil")
	}
	return owner
}
