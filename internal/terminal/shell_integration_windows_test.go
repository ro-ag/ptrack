//go:build windows

package terminal

import "testing"

func TestShellIntegrationIsNotInjectedIntoWindowsProfiles(t *testing.T) {
	owner, err := newShellIntegrationOwner(map[string]Profile{
		"shell": {
			ID: "shell", Name: "Shell", Kind: ProfileShell, Executable: `C:\\Windows\\System32\\cmd.exe`,
		},
	})
	if err != nil {
		t.Fatalf("newShellIntegrationOwner: %v", err)
	}
	if owner != nil {
		t.Fatal("Windows profile unexpectedly received Unix shell integration")
	}
}
