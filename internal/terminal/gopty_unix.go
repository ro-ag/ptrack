//go:build darwin || dragonfly || freebsd || linux || netbsd || openbsd || solaris

package terminal

import (
	"errors"
	"io"
	"os"
	"sync"

	gopty "github.com/aymanbagabas/go-pty"
	"golang.org/x/sys/unix"
)

type platformProcessState struct {
	resource *unixProcessResource
}

type unixProcessResource struct {
	mu             sync.Mutex
	processGroupID int
}

func preparePTYBeforeStart(*gopty.Cmd) (platformProcessState, error) {
	return platformProcessState{resource: &unixProcessResource{}}, nil
}

func preparePTYAfterStart(
	terminalPTY gopty.Pty,
	process *os.Process,
	platform platformProcessState,
) (platformProcessState, error) {
	if platform.resource == nil {
		platform.resource = &unixProcessResource{}
	}
	platform.resource.processGroupID = process.Pid
	unixPTY, ok := terminalPTY.(gopty.UnixPty)
	if !ok {
		return platform, nil
	}
	return platform, ignoreClosedFile(unixPTY.Slave().Close())
}

func closePlatformProcess(platform *platformProcessState) error {
	if platform == nil || platform.resource == nil {
		return nil
	}
	resource := platform.resource
	resource.mu.Lock()
	defer resource.mu.Unlock()
	return killUnixProcessGroup(resource)
}

func killUnixProcessGroup(resource *unixProcessResource) error {
	if resource.processGroupID <= 0 {
		return nil
	}
	err := unix.Kill(-resource.processGroupID, unix.SIGKILL)
	if err == nil || errors.Is(err, unix.ESRCH) {
		resource.processGroupID = 0
		return nil
	}
	return err
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

func terminateProcess(process *os.Process, platform *platformProcessState, _ func() error) error {
	processGroupID := process.Pid
	var resource *unixProcessResource
	if platform != nil {
		resource = platform.resource
	}
	if resource != nil {
		resource.mu.Lock()
		defer resource.mu.Unlock()
		if resource.processGroupID <= 0 {
			return nil
		}
		processGroupID = resource.processGroupID
	}
	err := unix.Kill(-processGroupID, unix.SIGTERM)
	if errors.Is(err, unix.ESRCH) {
		return nil
	}
	return err
}

func killProcess(process *os.Process, platform *platformProcessState) error {
	if platform != nil && platform.resource != nil {
		resource := platform.resource
		resource.mu.Lock()
		defer resource.mu.Unlock()
		return killUnixProcessGroup(resource)
	}
	err := unix.Kill(-process.Pid, unix.SIGKILL)
	if errors.Is(err, unix.ESRCH) {
		return nil
	}
	return err
}
