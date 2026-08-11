//go:build windows

package updater

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"golang.org/x/sys/windows"
)

func TestWindowsApplyRevealsVerifiedArchiveWithoutReplacingRunningExecutable(t *testing.T) {
	t.Parallel()
	target := Target{GOOS: "windows", GOARCH: runtime.GOARCH}
	client, candidate, _ := stageFixture(t, target, zipRelease(t, target, fakePE(t, runtime.GOARCH)))
	stage, err := client.Stage(context.Background(), candidate, target, t.TempDir(), nil)
	if err != nil {
		t.Fatal(err)
	}
	var name string
	var args []string
	installer := &Installer{
		currentExecutable: os.Executable,
		run: func(_ context.Context, command string, commandArgs ...string) ([]byte, error) {
			name, args = command, append([]string(nil), commandArgs...)
			return nil, nil
		},
	}
	result, err := installer.Apply(context.Background(), stage)
	if err != nil {
		t.Fatal(err)
	}
	windowsDirectory, err := windows.GetWindowsDirectory()
	if err != nil {
		t.Fatal(err)
	}
	explorer := filepath.Join(windowsDirectory, "explorer.exe")
	if _, err := os.Stat(explorer); err != nil {
		t.Fatalf("canonical Explorer is unavailable: %v", err)
	}
	if name != explorer || len(args) != 1 || args[0] != "/select,"+stage.AssetPath {
		t.Fatalf("command = %q %v", name, args)
	}
	if result.Action != ApplyRevealedArchive || !result.ManualInstall || result.RestartRequired {
		t.Fatalf("result = %#v", result)
	}
}
