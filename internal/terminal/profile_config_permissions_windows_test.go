//go:build windows

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
	if info.IsDir() {
		t.Fatal("profile config is a directory")
	}
}
