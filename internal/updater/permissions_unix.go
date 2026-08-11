//go:build !windows

package updater

import (
	"errors"
	"fmt"
	"os"

	"golang.org/x/sys/unix"
)

func preparePrivateDir(path string) error {
	if err := os.MkdirAll(path, 0o700); err != nil {
		return fmt.Errorf("create update directory: %w", err)
	}
	return securePrivatePath(path, true)
}

func securePrivatePath(path string, directory bool) error {
	info, err := os.Lstat(path)
	if err != nil {
		return err
	}
	if info.Mode()&os.ModeSymlink != 0 || (directory && !info.IsDir()) || (!directory && !info.Mode().IsRegular()) {
		return errors.New("update path has an unsafe type")
	}
	mode := os.FileMode(0o600)
	if directory {
		mode = 0o700
	}
	if err := os.Chmod(path, mode); err != nil {
		return err
	}
	return validatePrivatePath(path, directory)
}

func validatePrivatePath(path string, directory bool) error {
	info, err := os.Lstat(path)
	if err != nil {
		return err
	}
	if info.Mode()&os.ModeSymlink != 0 || info.Mode().Perm()&0o077 != 0 {
		return errors.New("update path is not private")
	}
	if directory && !info.IsDir() {
		return errors.New("update path is not a directory")
	}
	if !directory && !info.Mode().IsRegular() {
		return errors.New("update path is not a regular file")
	}
	return nil
}

func openPrivateRegular(path string) (*os.File, error) {
	fd, err := unix.Open(path, unix.O_RDONLY|unix.O_CLOEXEC|unix.O_NOFOLLOW, 0)
	if err != nil {
		return nil, err
	}
	file := os.NewFile(uintptr(fd), path)
	if file == nil {
		_ = unix.Close(fd)
		return nil, errors.New("open update file")
	}
	info, err := file.Stat()
	if err != nil || !info.Mode().IsRegular() || info.Mode().Perm()&0o077 != 0 {
		_ = file.Close()
		return nil, errors.New("update file is not a private regular file")
	}
	return file, nil
}
