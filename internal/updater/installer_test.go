package updater

import (
	"context"
	"errors"
	"runtime"
	"strings"
	"testing"
)

func TestInstallerRejectsMismatchedHostAndMissingDependencies(t *testing.T) {
	t.Parallel()
	installer := NewInstaller()
	if _, err := installer.Apply(context.Background(), StagedUpdate{GOOS: "not-" + runtimeGOOS()}); !errors.Is(err, ErrInstallRefused) {
		t.Fatalf("mismatched host error = %v, want ErrInstallRefused", err)
	}
	if _, err := (&Installer{}).Apply(context.Background(), StagedUpdate{}); !errors.Is(err, ErrInstallRefused) {
		t.Fatalf("missing dependency error = %v, want ErrInstallRefused", err)
	}
}

func TestBoundedBufferReportsFullWritesAndCapsOutput(t *testing.T) {
	t.Parallel()
	buffer := &boundedBuffer{limit: 8}
	input := []byte(strings.Repeat("x", 64))
	if written, err := buffer.Write(input); err != nil || written != len(input) {
		t.Fatalf("Write = %d, %v", written, err)
	}
	if got := len(buffer.Bytes()); got != 8 {
		t.Fatalf("buffer length = %d, want 8", got)
	}
}

func runtimeGOOS() string {
	// Kept as a function so the mismatched-target assertion is readable in
	// cross-compiled test binaries.
	return runtime.GOOS
}
