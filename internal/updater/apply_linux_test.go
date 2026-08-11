//go:build linux

package updater

import (
	"bytes"
	"context"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

func TestLinuxApplyAtomicallyReplacesAndVerifiesUserOwnedBinary(t *testing.T) {
	t.Parallel()
	stage, original, target := linuxApplyFixture(t)
	installer := &Installer{
		currentExecutable: func() (string, error) { return target, nil },
		run: func(_ context.Context, name string, args ...string) ([]byte, error) {
			if name != target || len(args) != 1 || args[0] != "version" {
				t.Fatalf("verify command = %q %v", name, args)
			}
			return []byte("ptrack " + stage.Version + "\n"), nil
		},
	}
	result, err := installer.Apply(context.Background(), stage)
	if err != nil {
		t.Fatal(err)
	}
	if result.Action != ApplyInstalled || !result.RestartRequired || result.ManualInstall {
		t.Fatalf("result = %#v", result)
	}
	installed, err := os.ReadFile(target)
	if err != nil || !bytes.Equal(installed, mustRead(t, stage.PayloadPath)) || bytes.Equal(installed, original) {
		t.Fatalf("installed payload mismatch: %v", err)
	}
	assertNoInstallTemps(t, filepath.Dir(target))
}

func TestLinuxApplyRollsBackFailedReplacementVerification(t *testing.T) {
	t.Parallel()
	stage, original, target := linuxApplyFixture(t)
	installer := &Installer{
		currentExecutable: func() (string, error) { return target, nil },
		run:               func(context.Context, string, ...string) ([]byte, error) { return nil, errors.New("cannot run") },
	}
	if _, err := installer.Apply(context.Background(), stage); !errors.Is(err, ErrInstallRefused) {
		t.Fatalf("error = %v, want ErrInstallRefused", err)
	}
	if got := mustRead(t, target); !bytes.Equal(got, original) {
		t.Fatal("original executable was not restored")
	}
	assertNoInstallTemps(t, filepath.Dir(target))
}

func TestLinuxApplyRefusesUnsafeOwnershipAndModes(t *testing.T) {
	t.Parallel()
	for _, mode := range []os.FileMode{0o4755, 0o775, 0o757} {
		stage, _, target := linuxApplyFixture(t)
		if err := os.Chmod(target, mode); err != nil {
			t.Fatal(err)
		}
		installer := &Installer{
			currentExecutable: func() (string, error) { return target, nil },
			run:               func(context.Context, string, ...string) ([]byte, error) { return nil, nil },
		}
		if _, err := installer.Apply(context.Background(), stage); !errors.Is(err, ErrInstallRefused) {
			t.Fatalf("mode %o: error = %v, want ErrInstallRefused", mode, err)
		}
	}
}

func TestRecoverPendingLinuxApplyCleansVerifiedSwap(t *testing.T) {
	t.Parallel()
	stage, _, targetPath := linuxApplyFixture(t)
	target, err := canonicalLinuxTarget(targetPath)
	if err != nil {
		t.Fatal(err)
	}
	backup, err := reserveSiblingPath(filepath.Dir(targetPath), ".ptrack-backup-*")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Link(targetPath, backup); err != nil {
		t.Fatal(err)
	}
	journal := linuxApplyJournal{
		Version: stage.Version, StageRoot: stage.Root, Target: targetPath, Backup: backup,
		OriginalDev: target.dev, OriginalIno: target.ino,
		PayloadSHA256: stage.PayloadSHA256, PayloadSizeBytes: stage.PayloadSizeBytes,
	}
	journalPath := linuxApplyJournalPath(stage.Root, targetPath)
	if err := writeLinuxApplyJournal(journalPath, journal); err != nil {
		t.Fatal(err)
	}
	replacement := targetPath + ".new"
	if err := os.WriteFile(replacement, mustRead(t, stage.PayloadPath), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Rename(replacement, targetPath); err != nil {
		t.Fatal(err)
	}
	recovered, err := recoverPendingApply(context.Background(), stage.Root, func() (string, error) { return targetPath, nil })
	if err != nil || !recovered {
		t.Fatalf("RecoverPendingApply = %t, %v", recovered, err)
	}
	if _, err := os.Lstat(backup); !os.IsNotExist(err) {
		t.Fatalf("backup still exists: %v", err)
	}
	if _, err := os.Lstat(journalPath); !os.IsNotExist(err) {
		t.Fatalf("journal still exists: %v", err)
	}
}

func TestRecoverPendingLinuxApplyRejectsTargetAsBackup(t *testing.T) {
	t.Parallel()
	stage, _, targetPath := linuxApplyFixture(t)
	target, err := canonicalLinuxTarget(targetPath)
	if err != nil {
		t.Fatal(err)
	}
	journal := linuxApplyJournal{
		Version: stage.Version, StageRoot: stage.Root, Target: targetPath, Backup: targetPath,
		OriginalDev: target.dev, OriginalIno: target.ino,
		PayloadSHA256: stage.PayloadSHA256, PayloadSizeBytes: stage.PayloadSizeBytes,
	}
	journalPath := linuxApplyJournalPath(stage.Root, targetPath)
	if err := writeLinuxApplyJournal(journalPath, journal); err != nil {
		t.Fatal(err)
	}
	if _, err := recoverPendingApply(context.Background(), stage.Root, func() (string, error) { return targetPath, nil }); !errors.Is(err, ErrInstallRefused) {
		t.Fatalf("error = %v, want ErrInstallRefused", err)
	}
	if _, err := os.Stat(targetPath); err != nil {
		t.Fatalf("target was removed: %v", err)
	}
}

func TestRecoverPendingLinuxApplyIdentifiesAnotherVerifiedStage(t *testing.T) {
	t.Parallel()
	stage, _, targetPath := linuxApplyFixture(t)
	target, err := canonicalLinuxTarget(targetPath)
	if err != nil {
		t.Fatal(err)
	}
	backup, err := reserveSiblingPath(filepath.Dir(targetPath), ".ptrack-backup-*")
	if err != nil {
		t.Fatal(err)
	}
	journal := linuxApplyJournal{
		Version: stage.Version, StageRoot: filepath.Join(filepath.Dir(stage.Root), ".stage-other"),
		Target: targetPath, Backup: backup, OriginalDev: target.dev, OriginalIno: target.ino,
		PayloadSHA256: stage.PayloadSHA256, PayloadSizeBytes: stage.PayloadSizeBytes,
	}
	journalPath := linuxApplyJournalPath(stage.Root, targetPath)
	if err := writeLinuxApplyJournal(journalPath, journal); err != nil {
		t.Fatal(err)
	}
	if _, err := recoverPendingApply(context.Background(), stage.Root, func() (string, error) { return targetPath, nil }); !errors.Is(err, ErrPendingStageMismatch) {
		t.Fatalf("error = %v, want ErrPendingStageMismatch", err)
	}
	if _, err := os.Stat(journalPath); err != nil {
		t.Fatalf("journal was not preserved: %v", err)
	}
}

func TestLinuxApplyLockIsTargetScopedAndExclusive(t *testing.T) {
	t.Parallel()
	stage, _, targetPath := linuxApplyFixture(t)
	release, err := acquireLinuxApplyLock(stage.Root, targetPath)
	if err != nil {
		t.Fatal(err)
	}
	defer release()
	if _, err := acquireLinuxApplyLock(stage.Root, targetPath); !errors.Is(err, ErrInstallRefused) {
		t.Fatalf("second lock error = %v, want ErrInstallRefused", err)
	}
}

func linuxApplyFixture(t *testing.T) (StagedUpdate, []byte, string) {
	t.Helper()
	targetSpec := Target{GOOS: "linux", GOARCH: runtime.GOARCH}
	newPayload := append(fakeELF(t, runtime.GOARCH), byte(2))
	client, candidate, _ := stageFixture(t, targetSpec, tarRelease(t, targetSpec, newPayload))
	stage, err := client.Stage(context.Background(), candidate, targetSpec, t.TempDir(), nil)
	if err != nil {
		t.Fatal(err)
	}
	installDir := t.TempDir()
	if err := os.Chmod(installDir, 0o700); err != nil {
		t.Fatal(err)
	}
	target := filepath.Join(installDir, "ptrack")
	original := append(fakeELF(t, runtime.GOARCH), byte(1))
	if err := os.WriteFile(target, original, 0o755); err != nil {
		t.Fatal(err)
	}
	return stage, original, target
}

func assertNoInstallTemps(t *testing.T, directory string) {
	t.Helper()
	entries, err := os.ReadDir(directory)
	if err != nil {
		t.Fatal(err)
	}
	for _, entry := range entries {
		if entry.Name() != "ptrack" {
			t.Fatalf("unexpected installation residue %q", entry.Name())
		}
	}
}

func mustRead(t *testing.T, path string) []byte {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return data
}
