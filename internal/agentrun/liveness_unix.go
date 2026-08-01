//go:build !windows

package agentrun

import (
	"errors"

	"golang.org/x/sys/unix"
)

// ProcessAlive reports whether a process with the given PID currently exists.
// Signal 0 performs no signalling; EPERM means the process exists but belongs
// to another user, which still counts as alive. PID reuse can make a dead
// owner look alive, so treat this as a fast staleness check, not proof of
// identity — the descriptor's generation and token remain the authority.
func ProcessAlive(pid int) bool {
	if pid <= 0 {
		return false
	}
	err := unix.Kill(pid, 0)
	return err == nil || errors.Is(err, unix.EPERM)
}
