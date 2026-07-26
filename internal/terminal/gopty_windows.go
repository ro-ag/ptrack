//go:build windows

package terminal

import (
	"errors"
	"os"

	gopty "github.com/aymanbagabas/go-pty"
)

func preparePTYAfterStart(gopty.Pty) error {
	return nil
}

func closePlatformPTY(terminalPTY gopty.Pty) error {
	return terminalPTY.Close()
}

func normalizePTYReadError(err error) error {
	return err
}

func terminateProcess(_ *os.Process, closePTY func() error) error {
	// Closing a pseudoconsole sends CTRL_CLOSE_EVENT to attached clients.
	return closePTY()
}

func killProcess(process *os.Process) error {
	err := process.Kill()
	if errors.Is(err, os.ErrProcessDone) {
		return nil
	}
	return err
}
