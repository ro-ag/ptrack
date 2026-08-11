//go:build !darwin && !dragonfly && !freebsd && !linux && !netbsd && !openbsd && !solaris && !windows

package terminal

import (
	"errors"
	"os"

	gopty "github.com/aymanbagabas/go-pty"
)

type platformProcessState struct{}

func preparePTYBeforeStart(*gopty.Cmd) (platformProcessState, error) {
	return platformProcessState{}, nil
}

func preparePTYAfterStart(
	_ gopty.Pty,
	_ *os.Process,
	platform platformProcessState,
) (platformProcessState, error) {
	return platform, nil
}

func closePlatformProcess(*platformProcessState) error {
	return nil
}

func closePlatformPTY(terminalPTY gopty.Pty) error {
	return terminalPTY.Close()
}

func normalizePTYReadError(err error) error {
	return err
}

func terminateProcess(process *os.Process, _ *platformProcessState, _ func() error) error {
	err := process.Signal(os.Interrupt)
	if errors.Is(err, os.ErrProcessDone) {
		return nil
	}
	return err
}

func killProcess(process *os.Process, _ *platformProcessState) error {
	err := process.Kill()
	if errors.Is(err, os.ErrProcessDone) {
		return nil
	}
	return err
}
