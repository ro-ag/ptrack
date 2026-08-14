//go:build windows

package main

import "testing"

func requirePrivateTestPath(t *testing.T, path string, directory bool) {
	t.Helper()
	if err := protectPrivatePath(path, directory); err != nil {
		t.Fatal(err)
	}
}
