//go:build !windows

package store

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"golang.org/x/sys/unix"
)

const migrationPartialCreateAttempts = 128

type migrationOutputDirectory struct {
	directory    *os.File
	originalPath string
	finalName    string
}

func migrationOutputSupported() error { return nil }

func openMigrationOutputDirectory(outputPath string) (*migrationOutputDirectory, error) {
	if filepath.Clean(outputPath) != outputPath {
		return nil, errors.New("output path must be clean")
	}
	finalName := filepath.Base(outputPath)
	if finalName == "" || finalName == "." || finalName == string(filepath.Separator) ||
		strings.ContainsRune(finalName, filepath.Separator) {
		return nil, errors.New("output filename must be one non-empty path component")
	}
	parentPath := filepath.Dir(outputPath)
	preOpenInfo, err := os.Lstat(parentPath)
	if err != nil {
		return nil, fmt.Errorf("inspect output parent: %w", err)
	}
	if preOpenInfo.Mode()&os.ModeSymlink != 0 || !preOpenInfo.IsDir() {
		return nil, errors.New("output parent must be an existing non-symlink directory")
	}

	descriptor, err := unix.Open(parentPath, unix.O_RDONLY|unix.O_DIRECTORY|unix.O_NOFOLLOW|unix.O_CLOEXEC, 0)
	if err != nil {
		return nil, fmt.Errorf("open output parent without following links: %w", err)
	}
	directory := os.NewFile(uintptr(descriptor), parentPath)
	if directory == nil {
		_ = unix.Close(descriptor)
		return nil, errors.New("adopt output parent descriptor")
	}
	openedInfo, err := directory.Stat()
	if err != nil {
		_ = directory.Close()
		return nil, fmt.Errorf("inspect opened output parent: %w", err)
	}
	postOpenInfo, err := os.Lstat(parentPath)
	if err != nil {
		_ = directory.Close()
		return nil, fmt.Errorf("reinspect output parent after open: %w", err)
	}
	if !openedInfo.IsDir() || postOpenInfo.Mode()&os.ModeSymlink != 0 || !postOpenInfo.IsDir() ||
		!os.SameFile(preOpenInfo, openedInfo) || !os.SameFile(preOpenInfo, postOpenInfo) {
		_ = directory.Close()
		return nil, errors.New("output parent changed while it was being opened")
	}

	handle := &migrationOutputDirectory{directory: directory, originalPath: parentPath, finalName: finalName}
	if err := handle.requireFinalAbsent(); err != nil {
		_ = directory.Close()
		return nil, err
	}
	return handle, nil
}

func (d *migrationOutputDirectory) requireFinalAbsent() error {
	var stat unix.Stat_t
	err := unix.Fstatat(int(d.directory.Fd()), d.finalName, &stat, unix.AT_SYMLINK_NOFOLLOW)
	if err == nil {
		return errors.New("output path already exists")
	}
	if !errors.Is(err, unix.ENOENT) {
		return fmt.Errorf("inspect output through parent descriptor: %w", err)
	}
	return nil
}

func (d *migrationOutputDirectory) createPartial() (*os.File, string, string, error) {
	for range migrationPartialCreateAttempts {
		var random [16]byte
		if _, err := rand.Read(random[:]); err != nil {
			return nil, "", "", fmt.Errorf("generate private partial name: %w", err)
		}
		name := ".ptrack-migrate-" + hex.EncodeToString(random[:]) + ".partial"
		descriptor, err := unix.Openat(
			int(d.directory.Fd()),
			name,
			unix.O_WRONLY|unix.O_CREAT|unix.O_EXCL|unix.O_NOFOLLOW|unix.O_CLOEXEC,
			0o600,
		)
		if errors.Is(err, unix.EEXIST) {
			continue
		}
		displayPath := filepath.Join(d.originalPath, name)
		if err != nil {
			return nil, name, displayPath, err
		}
		if err := unix.Fchmod(descriptor, 0o600); err != nil {
			_ = unix.Close(descriptor)
			return nil, name, displayPath, fmt.Errorf("set private partial mode: %w", err)
		}
		file := os.NewFile(uintptr(descriptor), displayPath)
		if file == nil {
			_ = unix.Close(descriptor)
			return nil, name, displayPath, errors.New("adopt partial output descriptor")
		}
		return file, name, displayPath, nil
	}
	return nil, "", "", fmt.Errorf("could not allocate a unique partial name after %d attempts", migrationPartialCreateAttempts)
}

func (d *migrationOutputDirectory) publish(partialName string) error {
	directoryFD := int(d.directory.Fd())
	if err := unix.Linkat(directoryFD, partialName, directoryFD, d.finalName, 0); err != nil {
		return fmt.Errorf("publish output without clobber: %w", err)
	}
	if err := unix.Fsync(directoryFD); err != nil {
		_ = unix.Unlinkat(directoryFD, d.finalName, 0)
		_ = unix.Fsync(directoryFD)
		return fmt.Errorf("sync published output directory: %w", err)
	}
	if err := unix.Unlinkat(directoryFD, partialName, 0); err != nil {
		_ = unix.Unlinkat(directoryFD, d.finalName, 0)
		_ = unix.Fsync(directoryFD)
		return fmt.Errorf("remove published partial: %w", err)
	}
	if err := unix.Fsync(directoryFD); err != nil {
		if linkErr := unix.Linkat(directoryFD, d.finalName, directoryFD, partialName, 0); linkErr == nil {
			_ = unix.Unlinkat(directoryFD, d.finalName, 0)
			_ = unix.Fsync(directoryFD)
		}
		return fmt.Errorf("sync partial removal: %w", err)
	}
	return nil
}

func (d *migrationOutputDirectory) close() {
	_ = d.directory.Close()
}
