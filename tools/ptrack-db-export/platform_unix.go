//go:build !windows

package main

import (
	"errors"
	"os"
	"syscall"

	"golang.org/x/sys/unix"
)

func migrationOutputSupported() error { return nil }

func createPrivateExportDirectory(path string) error { return os.Mkdir(path, 0o700) }

func createPrivateExportFile(path string) (*os.File, error) {
	return os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
}

func openLegacyExportSource(path string) (*os.File, error) {
	fd, err := unix.Open(path, unix.O_RDONLY|unix.O_CLOEXEC|unix.O_NOFOLLOW, 0)
	if err != nil {
		return nil, err
	}
	return os.NewFile(uintptr(fd), path), nil
}

func protectPrivatePath(path string, directory bool) error {
	mode := os.FileMode(0o600)
	if directory {
		mode = 0o700
	}
	return os.Chmod(path, mode)
}

func requirePrivateExportPath(path string, directory bool) error {
	info, err := os.Lstat(path)
	if err != nil {
		return err
	}
	if info.Mode()&os.ModeSymlink != 0 || info.IsDir() != directory || info.Mode().Perm()&0o077 != 0 {
		return errors.New("path type or permissions are unsafe")
	}
	return nil
}

func sourceDeviceInode(_ *os.File, info os.FileInfo) (uint64, uint64, error) {
	stat, ok := info.Sys().(*syscall.Stat_t)
	if !ok {
		return 0, 0, errors.New("platform does not expose device and inode")
	}
	return uint64(stat.Dev), uint64(stat.Ino), nil
}

func syncDirectory(path string) error {
	directory, err := os.Open(path)
	if err != nil {
		return err
	}
	err = directory.Sync()
	if closeErr := directory.Close(); err == nil {
		err = closeErr
	}
	return err
}
