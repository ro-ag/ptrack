package updater

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"runtime"
)

var (
	ErrInstallRefused       = errors.New("update installation refused")
	ErrPendingStageMismatch = errors.New("pending update belongs to another verified stage")
)

// ApplyAction describes the bounded host action completed for an update.
type ApplyAction string

const (
	ApplyInstalled       ApplyAction = "installed-restart-required"
	ApplyOpenedInstaller ApplyAction = "opened-native-installer"
	ApplyRevealedArchive ApplyAction = "revealed-verified-archive"
)

// ApplyResult contains no filesystem paths or remote URLs.
type ApplyResult struct {
	Version         string      `json:"version"`
	Action          ApplyAction `json:"action"`
	RestartRequired bool        `json:"restartRequired"`
	ManualInstall   bool        `json:"manualInstall"`
	CleanupPending  bool        `json:"cleanupPending"`
}

type commandRunner func(context.Context, string, ...string) ([]byte, error)

// Installer applies or hands off a verified stage using fixed host commands.
type Installer struct {
	currentExecutable func() (string, error)
	run               commandRunner
}

// NewInstaller returns the production platform installer.
func NewInstaller() *Installer {
	return &Installer{currentExecutable: os.Executable, run: runBoundedCommand}
}

// Apply revalidates the stage and dispatches only to the running host target.
func (i *Installer) Apply(ctx context.Context, stage StagedUpdate) (ApplyResult, error) {
	if i == nil || i.currentExecutable == nil || i.run == nil {
		return ApplyResult{}, fmt.Errorf("%w: installer is not configured", ErrInstallRefused)
	}
	if stage.GOOS != runtime.GOOS || stage.GOARCH != runtime.GOARCH {
		return ApplyResult{}, fmt.Errorf("%w: stage target does not match this host", ErrInstallRefused)
	}
	if err := ValidateStageContext(ctx, stage); err != nil {
		return ApplyResult{}, err
	}
	return i.applyPlatform(ctx, stage)
}

func runBoundedCommand(ctx context.Context, name string, args ...string) ([]byte, error) {
	command := exec.CommandContext(ctx, name, args...)
	output := &boundedBuffer{limit: 4096}
	command.Stdout = output
	command.Stderr = output
	err := command.Run()
	if ctx.Err() != nil {
		return nil, ctx.Err()
	}
	return output.Bytes(), err
}

type boundedBuffer struct {
	buffer bytes.Buffer
	limit  int
}

func (b *boundedBuffer) Write(data []byte) (int, error) {
	original := len(data)
	remaining := b.limit - b.buffer.Len()
	if remaining > 0 {
		if len(data) > remaining {
			data = data[:remaining]
		}
		_, _ = b.buffer.Write(data)
	}
	return original, nil
}

func (b *boundedBuffer) Bytes() []byte { return b.buffer.Bytes() }
