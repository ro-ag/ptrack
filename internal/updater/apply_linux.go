//go:build linux

package updater

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"

	"golang.org/x/sys/unix"
)

func (i *Installer) applyPlatform(ctx context.Context, stage StagedUpdate) (ApplyResult, error) {
	if stage.Kind != StageLinuxBinary {
		return ApplyResult{}, fmt.Errorf("%w: Linux requires the verified native archive", ErrInstallRefused)
	}
	executable, err := i.currentExecutable()
	if err != nil {
		return ApplyResult{}, fmt.Errorf("%w: locate current executable", ErrInstallRefused)
	}
	target, err := canonicalLinuxTarget(executable)
	if err != nil {
		return ApplyResult{}, err
	}
	directory := filepath.Dir(target.path)
	releaseLock, err := acquireLinuxApplyLock(stage.Root, target.path)
	if err != nil {
		return ApplyResult{}, err
	}
	defer releaseLock()
	journalPath := linuxApplyJournalPath(stage.Root, target.path)
	if _, err := os.Lstat(journalPath); err == nil {
		return ApplyResult{}, fmt.Errorf("%w: a prior installation requires recovery", ErrInstallRefused)
	} else if !os.IsNotExist(err) {
		return ApplyResult{}, fmt.Errorf("%w: inspect pending installation", ErrInstallRefused)
	}
	candidate, err := os.CreateTemp(directory, ".ptrack-update-*")
	if err != nil {
		return ApplyResult{}, fmt.Errorf("%w: installation directory is not writable", ErrInstallRefused)
	}
	candidatePath := candidate.Name()
	keepCandidate := false
	defer func() {
		_ = candidate.Close()
		if !keepCandidate {
			_ = os.Remove(candidatePath)
		}
	}()

	payload, err := openPrivateRegular(stage.PayloadPath)
	if err != nil {
		return ApplyResult{}, fmt.Errorf("%w: reopen verified payload", ErrInstallRefused)
	}
	hash := sha256.New()
	copied, copyErr := io.Copy(
		io.MultiWriter(candidate, hash),
		io.LimitReader(&contextReader{ctx: ctx, reader: payload}, stage.PayloadSizeBytes+1),
	)
	closePayloadErr := payload.Close()
	if copyErr != nil || closePayloadErr != nil || copied != stage.PayloadSizeBytes ||
		hex.EncodeToString(hash.Sum(nil)) != stage.PayloadSHA256 {
		return ApplyResult{}, fmt.Errorf("%w: copy verified payload", ErrInstallRefused)
	}
	mode := target.mode.Perm()
	if mode&0o111 == 0 || mode&0o022 != 0 {
		return ApplyResult{}, fmt.Errorf("%w: current executable mode is unsafe", ErrInstallRefused)
	}
	if err := candidate.Chmod(mode); err != nil {
		return ApplyResult{}, fmt.Errorf("%w: set replacement permissions", ErrInstallRefused)
	}
	if err := candidate.Sync(); err != nil {
		return ApplyResult{}, fmt.Errorf("%w: sync replacement", ErrInstallRefused)
	}
	if err := candidate.Close(); err != nil {
		return ApplyResult{}, fmt.Errorf("%w: close replacement", ErrInstallRefused)
	}
	if err := target.unchanged(); err != nil {
		return ApplyResult{}, err
	}

	backup, err := reserveSiblingPath(directory, ".ptrack-backup-*")
	if err != nil {
		return ApplyResult{}, fmt.Errorf("%w: reserve rollback path", ErrInstallRefused)
	}
	if err := os.Link(target.path, backup); err != nil {
		return ApplyResult{}, fmt.Errorf("%w: create rollback link", ErrInstallRefused)
	}
	if err := target.verifyBackup(backup); err != nil {
		_ = os.Remove(backup)
		return ApplyResult{}, err
	}
	journal := linuxApplyJournal{
		Version: stage.Version, StageRoot: stage.Root, Target: target.path, Backup: backup,
		OriginalDev: target.dev, OriginalIno: target.ino,
		PayloadSHA256: stage.PayloadSHA256, PayloadSizeBytes: stage.PayloadSizeBytes,
	}
	if err := writeLinuxApplyJournal(journalPath, journal); err != nil {
		_ = os.Remove(backup)
		return ApplyResult{}, fmt.Errorf("%w: persist rollback journal", ErrInstallRefused)
	}
	if err := os.Rename(candidatePath, target.path); err != nil {
		_ = os.Remove(backup)
		_ = removeLinuxApplyJournal(journalPath)
		return ApplyResult{}, fmt.Errorf("%w: atomically replace executable", ErrInstallRefused)
	}
	keepCandidate = true
	if err := syncDirectory(directory); err != nil {
		if rollbackErr := rollbackLinux(target.path, backup, directory); rollbackErr != nil {
			return ApplyResult{}, fmt.Errorf("%w: replacement sync and rollback failed", ErrInstallRefused)
		}
		_ = removeLinuxApplyJournal(journalPath)
		return ApplyResult{}, fmt.Errorf("%w: replacement sync failed and was rolled back", ErrInstallRefused)
	}

	output, verifyErr := i.run(ctx, target.path, "version")
	if verifyErr != nil || strings.TrimSpace(string(output)) != "ptrack "+stage.Version {
		if rollbackErr := rollbackLinux(target.path, backup, directory); rollbackErr != nil {
			return ApplyResult{}, fmt.Errorf("%w: replacement verification and rollback failed", ErrInstallRefused)
		}
		_ = removeLinuxApplyJournal(journalPath)
		return ApplyResult{}, fmt.Errorf("%w: replacement verification failed and was rolled back", ErrInstallRefused)
	}
	if err := os.Remove(backup); err != nil {
		return ApplyResult{Version: stage.Version, Action: ApplyInstalled, RestartRequired: true, CleanupPending: true}, nil
	}
	if err := syncDirectory(directory); err != nil || removeLinuxApplyJournal(journalPath) != nil {
		return ApplyResult{Version: stage.Version, Action: ApplyInstalled, RestartRequired: true, CleanupPending: true}, nil
	}
	return ApplyResult{Version: stage.Version, Action: ApplyInstalled, RestartRequired: true}, nil
}

// RecoverPendingApply resolves a crash-interrupted Linux replacement. It only
// cleans a verified new target or an unchanged original; ambiguous state is
// preserved for manual recovery.
func RecoverPendingApply(ctx context.Context, stageRoot string) (bool, error) {
	return recoverPendingApply(ctx, stageRoot, os.Executable)
}

func recoverPendingApply(ctx context.Context, stageRoot string, currentExecutable func() (string, error)) (bool, error) {
	stage, err := LoadStageContext(ctx, stageRoot)
	if err != nil {
		return false, err
	}
	if stage.Kind != StageLinuxBinary || runtime.GOOS != "linux" || runtime.GOARCH != stage.GOARCH {
		return false, fmt.Errorf("%w: pending apply does not match this host", ErrInstallRefused)
	}
	executable, err := currentExecutable()
	if err != nil {
		return false, fmt.Errorf("%w: locate current executable", ErrInstallRefused)
	}
	current, err := canonicalLinuxTarget(executable)
	if err != nil {
		return false, err
	}
	releaseLock, err := acquireLinuxApplyLock(stage.Root, current.path)
	if err != nil {
		return false, err
	}
	defer releaseLock()
	journalPath := linuxApplyJournalPath(stage.Root, current.path)
	data, err := readPrivateFile(ctx, journalPath, 4096)
	if os.IsNotExist(err) {
		return false, nil
	}
	if err != nil {
		return false, fmt.Errorf("%w: read pending apply", ErrInstallRefused)
	}
	var journal linuxApplyJournal
	if err := json.Unmarshal(data, &journal); err != nil {
		return false, fmt.Errorf("%w: invalid pending apply journal", ErrInstallRefused)
	}
	if journal.StageRoot != stage.Root {
		return false, ErrPendingStageMismatch
	}
	if journal.Version != stage.Version || journal.Target != current.path ||
		journal.PayloadSHA256 != stage.PayloadSHA256 || journal.PayloadSizeBytes != stage.PayloadSizeBytes ||
		!validLinuxJournalPaths(journal) {
		return false, fmt.Errorf("%w: invalid pending apply journal", ErrInstallRefused)
	}
	targetInfo, err := os.Lstat(journal.Target)
	if err != nil || !targetInfo.Mode().IsRegular() || targetInfo.Mode()&os.ModeSymlink != 0 {
		return false, fmt.Errorf("%w: pending target is unsafe", ErrInstallRefused)
	}
	targetStat, ok := targetInfo.Sys().(*syscall.Stat_t)
	if !ok {
		return false, fmt.Errorf("%w: inspect pending target", ErrInstallRefused)
	}
	if uint64(targetStat.Dev) == journal.OriginalDev && targetStat.Ino == journal.OriginalIno {
		if err := removeVerifiedBackup(journal); err != nil {
			return false, err
		}
		return true, removeLinuxApplyJournal(journalPath)
	}
	digest, size, err := hashLinuxTarget(ctx, journal.Target, stage.PayloadSizeBytes)
	if err != nil || digest != stage.PayloadSHA256 || size != stage.PayloadSizeBytes {
		return false, fmt.Errorf("%w: pending target is neither original nor verified update", ErrInstallRefused)
	}
	if err := removeVerifiedBackup(journal); err != nil {
		return false, err
	}
	return true, removeLinuxApplyJournal(journalPath)
}

type linuxApplyJournal struct {
	Version          string `json:"version"`
	StageRoot        string `json:"stage_root"`
	Target           string `json:"target"`
	Backup           string `json:"backup"`
	OriginalDev      uint64 `json:"original_dev"`
	OriginalIno      uint64 `json:"original_ino"`
	PayloadSHA256    string `json:"payload_sha256"`
	PayloadSizeBytes int64  `json:"payload_size_bytes"`
}

type linuxTarget struct {
	path string
	mode os.FileMode
	dev  uint64
	ino  uint64
}

func canonicalLinuxTarget(executable string) (linuxTarget, error) {
	absolute, err := filepath.Abs(executable)
	if err != nil {
		return linuxTarget{}, fmt.Errorf("%w: resolve executable path", ErrInstallRefused)
	}
	canonical, err := filepath.EvalSymlinks(absolute)
	if err != nil || !filepath.IsAbs(canonical) {
		return linuxTarget{}, fmt.Errorf("%w: resolve executable symlink", ErrInstallRefused)
	}
	file, err := openLinuxTarget(canonical)
	if err != nil {
		return linuxTarget{}, err
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil {
		return linuxTarget{}, fmt.Errorf("%w: inspect executable", ErrInstallRefused)
	}
	stat, ok := info.Sys().(*syscall.Stat_t)
	if !ok || stat.Uid != uint32(os.Geteuid()) || info.Mode()&(os.ModeSetuid|os.ModeSetgid) != 0 {
		return linuxTarget{}, fmt.Errorf("%w: executable is not safely user-owned", ErrInstallRefused)
	}
	parent, err := os.Stat(filepath.Dir(canonical))
	if err != nil {
		return linuxTarget{}, fmt.Errorf("%w: inspect installation directory", ErrInstallRefused)
	}
	parentStat, ok := parent.Sys().(*syscall.Stat_t)
	if !ok || parentStat.Uid != uint32(os.Geteuid()) || parent.Mode().Perm()&0o022 != 0 {
		return linuxTarget{}, fmt.Errorf("%w: installation directory is not safely user-owned", ErrInstallRefused)
	}
	return linuxTarget{path: canonical, mode: info.Mode(), dev: uint64(stat.Dev), ino: stat.Ino}, nil
}

func openLinuxTarget(path string) (*os.File, error) {
	fd, err := unix.Open(path, unix.O_RDONLY|unix.O_CLOEXEC|unix.O_NOFOLLOW, 0)
	if err != nil {
		return nil, fmt.Errorf("%w: open current executable", ErrInstallRefused)
	}
	file := os.NewFile(uintptr(fd), path)
	if file == nil {
		_ = unix.Close(fd)
		return nil, fmt.Errorf("%w: open current executable", ErrInstallRefused)
	}
	info, statErr := file.Stat()
	if statErr != nil || !info.Mode().IsRegular() {
		_ = file.Close()
		return nil, fmt.Errorf("%w: current executable is not regular", ErrInstallRefused)
	}
	return file, nil
}

func (target linuxTarget) unchanged() error {
	info, err := os.Lstat(target.path)
	if err != nil || !info.Mode().IsRegular() || info.Mode()&os.ModeSymlink != 0 {
		return fmt.Errorf("%w: executable changed before replacement", ErrInstallRefused)
	}
	stat, ok := info.Sys().(*syscall.Stat_t)
	if !ok || uint64(stat.Dev) != target.dev || stat.Ino != target.ino {
		return fmt.Errorf("%w: executable changed before replacement", ErrInstallRefused)
	}
	return nil
}

func (target linuxTarget) verifyBackup(path string) error {
	info, err := os.Lstat(path)
	if err != nil || !info.Mode().IsRegular() || info.Mode()&os.ModeSymlink != 0 {
		return fmt.Errorf("%w: rollback link is unsafe", ErrInstallRefused)
	}
	stat, ok := info.Sys().(*syscall.Stat_t)
	if !ok || uint64(stat.Dev) != target.dev || stat.Ino != target.ino {
		return fmt.Errorf("%w: rollback link identity mismatch", ErrInstallRefused)
	}
	return nil
}

func reserveSiblingPath(directory, pattern string) (string, error) {
	file, err := os.CreateTemp(directory, pattern)
	if err != nil {
		return "", err
	}
	path := file.Name()
	if closeErr := file.Close(); closeErr != nil {
		_ = os.Remove(path)
		return "", closeErr
	}
	if err := os.Remove(path); err != nil {
		return "", err
	}
	return path, nil
}

func rollbackLinux(target, backup, directory string) error {
	if err := os.Rename(backup, target); err != nil {
		return err
	}
	return syncDirectory(directory)
}

func writeLinuxApplyJournal(path string, journal linuxApplyJournal) error {
	data, err := json.Marshal(journal)
	if err != nil {
		return err
	}
	data = append(data, '\n')
	directory := filepath.Dir(path)
	temp, err := os.CreateTemp(directory, ".pending-apply-*")
	if err != nil {
		return err
	}
	tempPath := temp.Name()
	keep := false
	defer func() {
		_ = temp.Close()
		if !keep {
			_ = os.Remove(tempPath)
		}
	}()
	if _, err := temp.Write(data); err != nil {
		return err
	}
	if err := temp.Sync(); err != nil {
		return err
	}
	if err := temp.Close(); err != nil {
		return err
	}
	if err := securePrivatePath(tempPath, false); err != nil {
		return err
	}
	if err := os.Rename(tempPath, path); err != nil {
		return err
	}
	keep = true
	return syncDirectory(directory)
}

func removeLinuxApplyJournal(path string) error {
	err := os.Remove(path)
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil {
		return err
	}
	return syncDirectory(filepath.Dir(path))
}

func validLinuxJournalPaths(journal linuxApplyJournal) bool {
	if !filepath.IsAbs(journal.StageRoot) || !filepath.IsAbs(journal.Target) || !filepath.IsAbs(journal.Backup) ||
		journal.Target == journal.Backup ||
		filepath.Dir(journal.Target) != filepath.Dir(journal.Backup) {
		return false
	}
	name := filepath.Base(journal.Backup)
	return strings.HasPrefix(name, ".ptrack-backup-") && name != ".ptrack-backup-"
}

func linuxApplyJournalPath(stageRoot, target string) string {
	return filepath.Join(filepath.Dir(stageRoot), ".pending-apply-"+linuxTargetKey(target)+".json")
}

func acquireLinuxApplyLock(stageRoot, target string) (func(), error) {
	base := filepath.Dir(stageRoot)
	if err := validatePrivatePath(base, true); err != nil {
		return nil, fmt.Errorf("%w: unsafe update lock directory", ErrInstallRefused)
	}
	path := filepath.Join(base, ".apply-lock-"+linuxTargetKey(target))
	fd, err := unix.Open(path, unix.O_CREAT|unix.O_RDWR|unix.O_CLOEXEC|unix.O_NOFOLLOW, 0o600)
	if err != nil {
		return nil, fmt.Errorf("%w: open installation lock", ErrInstallRefused)
	}
	file := os.NewFile(uintptr(fd), path)
	if file == nil {
		_ = unix.Close(fd)
		return nil, fmt.Errorf("%w: open installation lock", ErrInstallRefused)
	}
	info, err := file.Stat()
	if err != nil || !info.Mode().IsRegular() {
		_ = file.Close()
		return nil, fmt.Errorf("%w: unsafe installation lock", ErrInstallRefused)
	}
	if err := file.Chmod(0o600); err != nil {
		_ = file.Close()
		return nil, fmt.Errorf("%w: protect installation lock", ErrInstallRefused)
	}
	if err := unix.Flock(fd, unix.LOCK_EX|unix.LOCK_NB); err != nil {
		_ = file.Close()
		return nil, fmt.Errorf("%w: another installation is active", ErrInstallRefused)
	}
	return func() {
		_ = unix.Flock(fd, unix.LOCK_UN)
		_ = file.Close()
	}, nil
}

func linuxTargetKey(target string) string {
	digest := sha256.Sum256([]byte(target))
	return hex.EncodeToString(digest[:16])
}

func removeVerifiedBackup(journal linuxApplyJournal) error {
	info, err := os.Lstat(journal.Backup)
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil || !info.Mode().IsRegular() || info.Mode()&os.ModeSymlink != 0 {
		return fmt.Errorf("%w: pending rollback link is unsafe", ErrInstallRefused)
	}
	stat, ok := info.Sys().(*syscall.Stat_t)
	if !ok || uint64(stat.Dev) != journal.OriginalDev || stat.Ino != journal.OriginalIno {
		return fmt.Errorf("%w: pending rollback link identity mismatch", ErrInstallRefused)
	}
	if err := os.Remove(journal.Backup); err != nil {
		return fmt.Errorf("%w: remove pending rollback link", ErrInstallRefused)
	}
	return syncDirectory(filepath.Dir(journal.Backup))
}

func hashLinuxTarget(ctx context.Context, path string, limit int64) (string, int64, error) {
	file, err := openLinuxTarget(path)
	if err != nil {
		return "", 0, err
	}
	defer file.Close()
	hash := sha256.New()
	size, err := io.Copy(hash, io.LimitReader(&contextReader{ctx: ctx, reader: file}, limit+1))
	if err != nil {
		return "", 0, err
	}
	return hex.EncodeToString(hash.Sum(nil)), size, nil
}

func syncDirectory(path string) error {
	directory, err := os.Open(path)
	if err != nil {
		return err
	}
	defer directory.Close()
	return directory.Sync()
}
