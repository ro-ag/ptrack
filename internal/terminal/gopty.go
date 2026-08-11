package terminal

import (
	"errors"
	"fmt"
	"os"
	"sync"
	"time"

	gopty "github.com/aymanbagabas/go-pty"
)

// GoPTYFactory creates the platform PTY implementation supplied by go-pty.
type GoPTYFactory struct{}

func (GoPTYFactory) Start(request StartRequest) (PTYProcess, error) {
	terminalPTY, err := gopty.New()
	if err != nil {
		return nil, fmt.Errorf("create PTY: %w", err)
	}
	cleanup := func(startErr error) (PTYProcess, error) {
		return nil, errors.Join(startErr, terminalPTY.Close())
	}

	if err := terminalPTY.Resize(request.Columns, request.Rows); err != nil {
		return cleanup(fmt.Errorf("resize PTY: %w", err))
	}

	command := terminalPTY.Command(request.Executable, request.Args...)
	command.Dir = request.CWD
	command.Env = append([]string(nil), request.Env...)
	platform, err := preparePTYBeforeStart(command)
	if err != nil {
		return cleanup(fmt.Errorf("prepare PTY process: %w", err))
	}
	if err := command.Start(); err != nil {
		return cleanup(errors.Join(
			fmt.Errorf("start PTY process: %w", err),
			closePlatformProcess(&platform),
		))
	}

	platform, err = preparePTYAfterStart(terminalPTY, command.Process, platform)
	process := &goPTYProcess{pty: terminalPTY, command: command, platform: platform}
	if err != nil {
		return nil, errors.Join(
			fmt.Errorf("prepare PTY after start: %w", err),
			process.cleanupFailedStart(),
		)
	}
	return process, nil
}

const failedPTYStartWaitTimeout = 5 * time.Second

type goPTYProcess struct {
	pty      gopty.Pty
	command  *gopty.Cmd
	platform platformProcessState

	waitOnce sync.Once
	waitCode int
	waitErr  error

	ptyCloseOnce      sync.Once
	ptyCloseErr       error
	platformCloseOnce sync.Once
	platformCloseErr  error
}

func (p *goPTYProcess) PID() int {
	if p.command.Process == nil {
		return 0
	}
	return p.command.Process.Pid
}

func (p *goPTYProcess) Read(buffer []byte) (int, error) {
	read, err := p.pty.Read(buffer)
	return read, normalizePTYReadError(err)
}

func (p *goPTYProcess) Write(buffer []byte) (int, error) {
	return p.pty.Write(buffer)
}

func (p *goPTYProcess) Resize(rows, columns int) error {
	return p.pty.Resize(columns, rows)
}

func (p *goPTYProcess) Wait() (int, error) {
	p.waitOnce.Do(func() {
		p.waitErr = p.command.Wait()
		if p.command.ProcessState != nil {
			p.waitCode = p.command.ProcessState.ExitCode()
			// A nonzero process status is an ordinary terminal exit. Preserve
			// only wait failures which did not yield process state.
			p.waitErr = nil
		} else {
			p.waitCode = -1
		}
	})
	return p.waitCode, p.waitErr
}

func (p *goPTYProcess) Terminate() error {
	if p.command.Process == nil {
		return os.ErrProcessDone
	}
	return terminateProcess(p.command.Process, &p.platform, p.closePTY)
}

func (p *goPTYProcess) Kill() error {
	if p.command.Process == nil {
		return os.ErrProcessDone
	}
	return killProcess(p.command.Process, &p.platform)
}

func (p *goPTYProcess) Close() error {
	p.platformCloseOnce.Do(func() {
		p.platformCloseErr = closePlatformProcess(&p.platform)
	})
	return errors.Join(p.closePTY(), p.platformCloseErr)

}

func (p *goPTYProcess) cleanupFailedStart() error {
	killErr := p.Kill()
	closeErr := p.Close()
	waitDone := make(chan error, 1)
	go func() {
		_, err := p.Wait()
		waitDone <- err
	}()
	select {
	case waitErr := <-waitDone:
		return errors.Join(killErr, closeErr, waitErr)
	case <-time.After(failedPTYStartWaitTimeout):
		return errors.Join(
			killErr,
			closeErr,
			fmt.Errorf("timed out after %s waiting for failed PTY process cleanup", failedPTYStartWaitTimeout),
		)
	}
}

func (p *goPTYProcess) closePTY() error {
	p.ptyCloseOnce.Do(func() {
		p.ptyCloseErr = closePlatformPTY(p.pty)
	})
	return p.ptyCloseErr
}
