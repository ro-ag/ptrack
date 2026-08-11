package gui

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	wailsruntime "github.com/wailsapp/wails/v2/pkg/runtime"
)

// Managed block markers in ~/.zprofile. Everything between them belongs to
// p-track and is rewritten or detected as a whole, so repeated installs are
// idempotent and never duplicate the PATH entry.
const (
	shellPathMarkerBegin = "# >>> ptrack cli >>>"
	shellPathMarkerEnd   = "# <<< ptrack cli <<<"
)

// InstallShellCommand makes the `ptrack` CLI available in new terminal
// sessions by appending the app's own binary directory to PATH in
// ~/.zprofile. Safe to invoke repeatedly: an existing managed block is left
// untouched. Reports the outcome with a native dialog.
func (a *App) InstallShellCommand() {
	ctx, release, ok := a.acquireRuntimeCall()
	if !ok {
		return
	}
	defer release()
	binDir, err := cliBinaryDir()
	if err != nil {
		a.shellCommandDialog(ctx, "Shell Command", err.Error())
		return
	}
	profile, err := zprofilePath()
	if err != nil {
		a.shellCommandDialog(ctx, "Shell Command", err.Error())
		return
	}
	changed, err := ensureShellPath(profile, binDir)
	switch {
	case err != nil:
		a.shellCommandDialog(ctx, "Shell Command", err.Error())
	case changed:
		a.shellCommandDialog(ctx, "Shell Command",
			fmt.Sprintf("Added to PATH in %s:\n\n%s\n\nOpen a new terminal window, then run `ptrack`.", profile, binDir))
	default:
		a.shellCommandDialog(ctx, "Shell Command",
			fmt.Sprintf("Already on PATH via %s:\n\n%s", profile, binDir))
	}
}

func (a *App) shellCommandDialog(ctx context.Context, title, message string) {
	wailsruntime.MessageDialog(ctx, wailsruntime.MessageDialogOptions{
		Type:    wailsruntime.InfoDialog,
		Title:   title,
		Message: message,
	})
}

// cliBinaryDir resolves the directory holding the running ptrack binary —
// Contents/MacOS inside the app bundle, or the build directory in dev.
func cliBinaryDir() (string, error) {
	exe, err := os.Executable()
	if err != nil {
		return "", fmt.Errorf("cannot locate the ptrack binary: %w", err)
	}
	return filepath.Dir(exe), nil
}

func zprofilePath() (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("cannot locate your home directory: %w", err)
	}
	return filepath.Join(home, ".zprofile"), nil
}

// ensureShellPath appends the managed PATH block to the profile unless it is
// already there. Reports whether the file was modified.
func ensureShellPath(profile, binDir string) (bool, error) {
	data, err := os.ReadFile(profile)
	if err != nil && !os.IsNotExist(err) {
		return false, fmt.Errorf("cannot read %s: %w", profile, err)
	}
	if strings.Contains(string(data), shellPathMarkerBegin) {
		return false, nil
	}
	var block strings.Builder
	if len(data) > 0 && !strings.HasSuffix(string(data), "\n") {
		block.WriteString("\n")
	}
	block.WriteString(shellPathMarkerBegin + "\n")
	block.WriteString("# Added by p-track: makes the `ptrack` CLI available in new terminal sessions.\n")
	block.WriteString(fmt.Sprintf("export PATH=\"$PATH:%s\"\n", binDir))
	block.WriteString(shellPathMarkerEnd + "\n")
	file, err := os.OpenFile(profile, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return false, fmt.Errorf("cannot update %s: %w", profile, err)
	}
	defer file.Close()
	if _, err := file.WriteString(block.String()); err != nil {
		return false, fmt.Errorf("cannot update %s: %w", profile, err)
	}
	return true, nil
}
