//go:build !windows

package terminal

import (
	"os"
	"testing"
)

func assertProfileConfigPrivate(t *testing.T, path string) {
	t.Helper()
	info, err := os.Stat(path)
	if err != nil {
		t.Fatalf("stat profile config: %v", err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("profile config permissions = %o, want 600", info.Mode().Perm())
	}
}
