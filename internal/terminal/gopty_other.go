//go:build !darwin && !dragonfly && !freebsd && !linux && !netbsd && !openbsd && !solaris && !windows

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

func terminateProcess(process *os.Process, _ func() error) error {
	err := process.Signal(os.Interrupt)
	if errors.Is(err, os.ErrProcessDone) {
		return nil
	}
	return err
}

func killProcess(process *os.Process) error {
	err := process.Kill()
	if errors.Is(err, os.ErrProcessDone) {
		return nil
	}
	return err
}
