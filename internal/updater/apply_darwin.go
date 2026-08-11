//go:build darwin

package updater

import (
	"context"
	"fmt"
)

const darwinPublisherRequirement = `anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = "3CAJR4ZDMQ"`

func (i *Installer) applyPlatform(ctx context.Context, stage StagedUpdate) (ApplyResult, error) {
	if stage.Kind != StageDarwinDMG {
		return ApplyResult{}, fmt.Errorf("%w: macOS requires the whole signed DMG", ErrInstallRefused)
	}
	commands := []struct {
		name string
		args []string
	}{
		{name: "/usr/bin/hdiutil", args: []string{"verify", stage.AssetPath}},
		{name: "/usr/bin/codesign", args: []string{"--verify", "--strict", "--verbose=2", "-R=" + darwinPublisherRequirement, stage.AssetPath}},
		{name: "/usr/sbin/spctl", args: []string{"--assess", "--type", "open", "--context", "context:primary-signature", stage.AssetPath}},
	}
	for _, command := range commands {
		if _, err := i.run(ctx, command.name, command.args...); err != nil {
			return ApplyResult{}, fmt.Errorf("%w: native macOS verification failed", ErrInstallRefused)
		}
	}
	if _, err := i.run(ctx, "/usr/bin/open", stage.AssetPath); err != nil {
		return ApplyResult{}, fmt.Errorf("%w: could not open the verified macOS installer", ErrInstallRefused)
	}
	return ApplyResult{Version: stage.Version, Action: ApplyOpenedInstaller, ManualInstall: true}, nil
}
