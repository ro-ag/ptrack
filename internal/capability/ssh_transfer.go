package capability

import (
	"errors"
	"io"
	"os"
	"path/filepath"
)

func stageSSHUpload(
	projectRoot, requested string,
	approvedRoots []string,
	maximum int64,
) (string, func(), error) {
	sourcePath, err := ResolveProjectPath(projectRoot, requested, approvedRoots, true)
	if err != nil {
		return "", func() {}, ErrDenied{Reason: "upload path is outside approved roots"}
	}
	source, err := os.Open(sourcePath)
	if err != nil {
		return "", func() {}, err
	}
	cleanupSource := true
	defer func() {
		if cleanupSource {
			_ = source.Close()
		}
	}()
	sourceInfo, err := source.Stat()
	if err != nil || !sourceInfo.Mode().IsRegular() {
		return "", func() {}, ErrDenied{Reason: "upload source must be a regular file"}
	}
	if err := verifyProjectFileIdentity(projectRoot, requested, approvedRoots, sourceInfo); err != nil {
		return "", func() {}, err
	}

	directory, err := os.MkdirTemp("", "ptrack-upload-")
	if err != nil {
		return "", func() {}, err
	}
	cleanup := func() { _ = os.RemoveAll(directory) }
	if err := os.Chmod(directory, 0o700); err != nil {
		cleanup()
		return "", func() {}, err
	}
	stagedPath := filepath.Join(directory, filepath.Base(sourcePath))
	staged, err := os.OpenFile(stagedPath, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o600)
	if err != nil {
		cleanup()
		return "", func() {}, err
	}
	written, copyErr := io.Copy(staged, io.LimitReader(source, maximum+1))
	syncErr := staged.Sync()
	closeErr := staged.Close()
	if err := errors.Join(copyErr, syncErr, closeErr); err != nil {
		cleanup()
		return "", func() {}, err
	}
	if written > maximum {
		cleanup()
		return "", func() {}, requestLimitError{}
	}
	if err := verifyProjectFileIdentity(projectRoot, requested, approvedRoots, sourceInfo); err != nil {
		cleanup()
		return "", func() {}, err
	}
	if err := source.Close(); err != nil {
		cleanup()
		return "", func() {}, err
	}
	cleanupSource = false
	if err := os.Chmod(stagedPath, 0o400); err != nil {
		cleanup()
		return "", func() {}, err
	}
	return stagedPath, cleanup, nil
}

func verifyProjectFileIdentity(
	projectRoot, requested string,
	approvedRoots []string,
	expected os.FileInfo,
) error {
	current, err := ResolveProjectPath(projectRoot, requested, approvedRoots, true)
	if err != nil {
		return ErrDenied{Reason: "upload source changed during verification"}
	}
	currentInfo, err := os.Stat(current)
	if err != nil || !os.SameFile(expected, currentInfo) {
		return ErrDenied{Reason: "upload source changed during verification"}
	}
	return nil
}

func stageSSHDownload(
	projectRoot, requested string,
	approvedRoots []string,
	maximum int64,
) (func([]byte) error, func(), error) {
	destination, err := ResolveProjectPath(projectRoot, requested, approvedRoots, false)
	if err != nil {
		return nil, func() {}, ErrDenied{Reason: "download path is outside approved roots"}
	}
	canonicalProject, err := filepath.EvalSymlinks(projectRoot)
	if err != nil {
		return nil, func() {}, ErrDenied{Reason: "project root cannot be canonicalized"}
	}
	canonicalProject, err = filepath.Abs(canonicalProject)
	if err != nil {
		return nil, func() {}, ErrDenied{Reason: "project root cannot be canonicalized"}
	}
	directory, err := os.MkdirTemp("", "ptrack-download-")
	if err != nil {
		return nil, func() {}, err
	}
	cleanup := func() { _ = os.RemoveAll(directory) }
	if err := os.Chmod(directory, 0o700); err != nil {
		cleanup()
		return nil, func() {}, err
	}
	stagedPath := filepath.Join(directory, "payload")
	complete := func(payload []byte) error {
		if int64(len(payload)) > maximum {
			return responseLimitError{}
		}
		staged, openErr := os.OpenFile(stagedPath, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o600)
		if openErr != nil {
			return openErr
		}
		_, writeErr := staged.Write(payload)
		syncErr := staged.Sync()
		closeErr := staged.Close()
		if err := errors.Join(writeErr, syncErr, closeErr); err != nil {
			return err
		}
		currentDestination, resolveErr := ResolveProjectPath(projectRoot, requested, approvedRoots, false)
		if resolveErr != nil || currentDestination != destination {
			return ErrDenied{Reason: "download destination changed during transfer"}
		}
		return installStagedDownload(canonicalProject, destination, stagedPath, maximum)
	}
	return complete, cleanup, nil
}

type requestLimitError struct{}

func (requestLimitError) Error() string { return "transfer request exceeds its byte limit" }
