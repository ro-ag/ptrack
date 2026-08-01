//go:build windows

package agentrun

import (
	"golang.org/x/sys/windows"
)

// stillActive is the exit code Windows reports for a running process.
const stillActive = 259

// ProcessAlive reports whether a process with the given PID currently exists
// and has not exited. PID reuse can make a dead owner look alive, so treat
// this as a fast staleness check, not proof of identity — the descriptor's
// generation and token remain the authority.
func ProcessAlive(pid int) bool {
	if pid <= 0 {
		return false
	}
	handle, err := windows.OpenProcess(
		windows.PROCESS_QUERY_LIMITED_INFORMATION, false, uint32(pid))
	if err != nil {
		return false
	}
	defer func() { _ = windows.CloseHandle(handle) }()
	var code uint32
	if err := windows.GetExitCodeProcess(handle, &code); err != nil {
		return false
	}
	return code == stillActive
}
