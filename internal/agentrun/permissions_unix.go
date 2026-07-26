//go:build !windows

package agentrun

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"golang.org/x/sys/unix"
)

func preparePrivateRuntimeDir(path string) error {
	if err := os.MkdirAll(path, 0o700); err != nil {
		return fmt.Errorf("create private AgentRun runtime directory: %w", err)
	}
	if err := os.Chmod(path, 0o700); err != nil {
		return fmt.Errorf("secure AgentRun runtime directory: %w", err)
	}
	info, err := os.Stat(path)
	if err != nil {
		return err
	}
	if info.Mode().Perm()&0o077 != 0 {
		return errors.New("AgentRun runtime directory is not private")
	}
	return nil
}

func openPrivateDescriptor(path string) (*os.File, error) {
	return os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
}

func replacePrivateDescriptor(tempPath, path string) error {
	return os.Rename(tempPath, path)
}

func lockPrivateDescriptor(runtimeDir string) (func() error, error) {
	lockPath := filepath.Join(runtimeDir, ".agent-registry.lock")
	file, err := os.OpenFile(lockPath, os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, fmt.Errorf("open AgentRun descriptor lock: %w", err)
	}
	if err := unix.Flock(int(file.Fd()), unix.LOCK_EX); err != nil {
		_ = file.Close()
		return nil, fmt.Errorf("lock AgentRun descriptor: %w", err)
	}
	return func() error {
		unlockErr := unix.Flock(int(file.Fd()), unix.LOCK_UN)
		return errors.Join(unlockErr, file.Close())
	}, nil
}

func securePublishedDescriptor(path string) error {
	if err := os.Chmod(path, 0o600); err != nil {
		return fmt.Errorf("secure AgentRun descriptor: %w", err)
	}
	info, err := os.Stat(path)
	if err != nil {
		return err
	}
	if info.Mode().Perm()&0o077 != 0 {
		return errors.New("AgentRun descriptor is not private")
	}
	return nil
}
