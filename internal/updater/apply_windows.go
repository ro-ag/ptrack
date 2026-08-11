//go:build windows

package updater

import (
	"context"
	"fmt"
	"path/filepath"

	"golang.org/x/sys/windows"
)

func (i *Installer) applyPlatform(ctx context.Context, stage StagedUpdate) (ApplyResult, error) {
	if stage.Kind != StageWindowsZIP {
		return ApplyResult{}, fmt.Errorf("%w: Windows requires the verified ZIP", ErrInstallRefused)
	}
	windowsDirectory, err := windows.GetWindowsDirectory()
	if err != nil {
		return ApplyResult{}, fmt.Errorf("%w: locate Windows directory", ErrInstallRefused)
	}
	explorer := filepath.Join(windowsDirectory, "explorer.exe")
	if _, err := i.run(ctx, explorer, "/select,"+stage.AssetPath); err != nil {
		return ApplyResult{}, fmt.Errorf("%w: could not reveal the verified Windows archive", ErrInstallRefused)
	}
	return ApplyResult{Version: stage.Version, Action: ApplyRevealedArchive, ManualInstall: true}, nil
}
