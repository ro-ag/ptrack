//go:build darwin || dragonfly || freebsd || linux || netbsd || openbsd || solaris

package terminal

import (
	"errors"
	"io"
	"os"

	gopty "github.com/aymanbagabas/go-pty"
	"golang.org/x/sys/unix"
)

func preparePTYAfterStart(terminalPTY gopty.Pty) error {
	unixPTY, ok := terminalPTY.(gopty.UnixPty)
	if !ok {
		return nil
	}
	return ignoreClosedFile(unixPTY.Slave().Close())
}

func closePlatformPTY(terminalPTY gopty.Pty) error {
	unixPTY, ok := terminalPTY.(gopty.UnixPty)
	if !ok {
		return terminalPTY.Close()
	}
	return errors.Join(
		ignoreClosedFile(unixPTY.Master().Close()),
		ignoreClosedFile(unixPTY.Slave().Close()),
	)
}

func ignoreClosedFile(err error) error {
	if errors.Is(err, os.ErrClosed) {
		return nil
	}
	return err
}

func normalizePTYReadError(err error) error {
	if errors.Is(err, unix.EIO) {
		return io.EOF
	}
	return err
}

func terminateProcess(process *os.Process, _ func() error) error {
	err := unix.Kill(-process.Pid, unix.SIGTERM)
	if errors.Is(err, unix.ESRCH) {
		return nil
	}
	return err
}

func killProcess(process *os.Process) error {
	err := unix.Kill(-process.Pid, unix.SIGKILL)
	if errors.Is(err, unix.ESRCH) {
		return nil
	}
	return err
}
