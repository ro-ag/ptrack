//go:build aix || darwin || dragonfly || freebsd || linux || netbsd || openbsd || solaris

package capability

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"io"
	"os"
	"path/filepath"
	"strings"

	"golang.org/x/sys/unix"
)

// installStagedDownload walks from an already canonical project directory
// using no-follow directory descriptors, writes a sibling temporary file, and
// atomically renames it into place. No attacker-controlled path is resolved
// again after the parent descriptor is opened.
func installStagedDownload(canonicalProject, destination, stagedPath string, maximum int64) error {
	relative, err := filepath.Rel(canonicalProject, destination)
	if err != nil || relative == "." || relative == ".." || strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
		return ErrDenied{Reason: "download destination escapes the project"}
	}
	parts := strings.Split(relative, string(filepath.Separator))
	parentFD, err := unix.Open(canonicalProject, unix.O_RDONLY|unix.O_DIRECTORY|unix.O_CLOEXEC|unix.O_NOFOLLOW, 0)
	if err != nil {
		return err
	}
	for _, component := range parts[:len(parts)-1] {
		nextFD, openErr := unix.Openat(parentFD, component, unix.O_RDONLY|unix.O_DIRECTORY|unix.O_CLOEXEC|unix.O_NOFOLLOW, 0)
		_ = unix.Close(parentFD)
		if openErr != nil {
			return ErrDenied{Reason: "download destination parent is not a stable project directory"}
		}
		parentFD = nextFD
	}
	defer unix.Close(parentFD)

	sourceFD, err := unix.Open(stagedPath, unix.O_RDONLY|unix.O_CLOEXEC|unix.O_NOFOLLOW, 0)
	if err != nil {
		return ErrDenied{Reason: "download staging file is invalid"}
	}
	var sourceStat unix.Stat_t
	if err := unix.Fstat(sourceFD, &sourceStat); err != nil || sourceStat.Mode&unix.S_IFMT != unix.S_IFREG {
		_ = unix.Close(sourceFD)
		return ErrDenied{Reason: "download staging file is invalid"}
	}
	source := os.NewFile(uintptr(sourceFD), stagedPath)
	defer source.Close()

	temporaryName, err := randomTransferName()
	if err != nil {
		return err
	}
	temporaryFD, err := unix.Openat(
		parentFD,
		temporaryName,
		unix.O_WRONLY|unix.O_CREAT|unix.O_EXCL|unix.O_CLOEXEC|unix.O_NOFOLLOW,
		0o600,
	)
	if err != nil {
		return err
	}
	removeTemporary := true
	defer func() {
		if removeTemporary {
			_ = unix.Unlinkat(parentFD, temporaryName, 0)
		}
	}()
	temporary := os.NewFile(uintptr(temporaryFD), temporaryName)
	written, copyErr := io.Copy(temporary, io.LimitReader(source, maximum+1))
	syncErr := temporary.Sync()
	closeErr := temporary.Close()
	if err := errors.Join(copyErr, syncErr, closeErr); err != nil {
		return err
	}
	if written > maximum {
		return responseLimitError{}
	}
	if err := unix.Renameat(parentFD, temporaryName, parentFD, parts[len(parts)-1]); err != nil {
		return err
	}
	removeTemporary = false
	return unix.Fsync(parentFD)
}

func randomTransferName() (string, error) {
	value := make([]byte, 16)
	if _, err := rand.Read(value); err != nil {
		return "", err
	}
	return ".ptrack-download-" + hex.EncodeToString(value), nil
}
