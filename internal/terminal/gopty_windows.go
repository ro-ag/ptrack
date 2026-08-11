//go:build windows

package terminal

import (
	"errors"
	"os"
	"sync"
	"syscall"
	"unsafe"

	gopty "github.com/aymanbagabas/go-pty"
	"golang.org/x/sys/windows"
)

type platformProcessState struct {
	resource *windowsProcessResource
}

type windowsProcessResource struct {
	mu       sync.Mutex
	job      windows.Handle
	assigned bool
}

func preparePTYBeforeStart(command *gopty.Cmd) (platformProcessState, error) {
	job, err := windows.CreateJobObject(nil, nil)
	if err != nil {
		return platformProcessState{}, err
	}
	cleanup := func(setupErr error) (platformProcessState, error) {
		return platformProcessState{}, errors.Join(setupErr, windows.CloseHandle(job))
	}
	limits := windows.JOBOBJECT_EXTENDED_LIMIT_INFORMATION{}
	limits.BasicLimitInformation.LimitFlags = windows.JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
	if _, err := windows.SetInformationJobObject(
		job,
		windows.JobObjectExtendedLimitInformation,
		uintptr(unsafe.Pointer(&limits)),
		uint32(unsafe.Sizeof(limits)),
	); err != nil {
		return cleanup(err)
	}
	if command.SysProcAttr == nil {
		command.SysProcAttr = &syscall.SysProcAttr{}
	}
	command.SysProcAttr.CreationFlags |= windows.CREATE_SUSPENDED
	return platformProcessState{resource: &windowsProcessResource{job: job}}, nil
}

func preparePTYAfterStart(
	_ gopty.Pty,
	process *os.Process,
	platform platformProcessState,
) (platformProcessState, error) {
	resource := platform.resource
	if resource == nil {
		return platform, errors.New("PTY process has no Windows Job Object")
	}
	resource.mu.Lock()
	defer resource.mu.Unlock()
	job := resource.job
	cleanup := func(setupErr error) (platformProcessState, error) {
		return platform, setupErr
	}
	processHandle, err := windows.OpenProcess(
		windows.PROCESS_SET_QUOTA|windows.PROCESS_TERMINATE,
		false,
		uint32(process.Pid),
	)
	if err != nil {
		return cleanup(err)
	}
	assignErr := windows.AssignProcessToJobObject(job, processHandle)
	if assignErr == nil {
		resource.assigned = true
	}
	closeErr := windows.CloseHandle(processHandle)
	if assignErr != nil || closeErr != nil {
		return cleanup(errors.Join(assignErr, closeErr))
	}
	if err := resumeWindowsProcess(uint32(process.Pid)); err != nil {
		return cleanup(err)
	}
	return platform, nil
}

func resumeWindowsProcess(processID uint32) error {
	snapshot, err := windows.CreateToolhelp32Snapshot(windows.TH32CS_SNAPTHREAD, 0)
	if err != nil {
		return err
	}
	defer windows.CloseHandle(snapshot) //nolint:errcheck -- preserve the primary resume error
	entry := windows.ThreadEntry32{Size: uint32(unsafe.Sizeof(windows.ThreadEntry32{}))}
	if err := windows.Thread32First(snapshot, &entry); err != nil {
		return err
	}
	for {
		if entry.OwnerProcessID == processID {
			thread, openErr := windows.OpenThread(
				windows.THREAD_SUSPEND_RESUME,
				false,
				entry.ThreadID,
			)
			if openErr != nil {
				return openErr
			}
			_, resumeErr := windows.ResumeThread(thread)
			return errors.Join(resumeErr, windows.CloseHandle(thread))
		}
		err := windows.Thread32Next(snapshot, &entry)
		if errors.Is(err, windows.ERROR_NO_MORE_FILES) {
			return errors.New("suspended PTY process has no primary thread")
		}
		if err != nil {
			return err
		}
	}
}

func closePlatformProcess(platform *platformProcessState) error {
	if platform == nil || platform.resource == nil {
		return nil
	}
	resource := platform.resource
	resource.mu.Lock()
	defer resource.mu.Unlock()
	if resource.job == 0 {
		return nil
	}
	err := windows.CloseHandle(resource.job)
	resource.job = 0
	resource.assigned = false
	if errors.Is(err, windows.ERROR_INVALID_HANDLE) {
		return nil
	}
	return err
}

func closePlatformPTY(terminalPTY gopty.Pty) error {
	return terminalPTY.Close()
}

func normalizePTYReadError(err error) error {
	return err
}

func terminateProcess(_ *os.Process, _ *platformProcessState, closePTY func() error) error {
	// Closing a pseudoconsole sends CTRL_CLOSE_EVENT to attached clients.
	return closePTY()
}

func killProcess(process *os.Process, platform *platformProcessState) error {
	if platform != nil && platform.resource != nil {
		resource := platform.resource
		resource.mu.Lock()
		defer resource.mu.Unlock()
		if resource.job != 0 && resource.assigned {
			err := windows.TerminateJobObject(resource.job, 1)
			if errors.Is(err, windows.ERROR_INVALID_HANDLE) {
				return nil
			}
			return err
		}
	}
	err := process.Kill()
	if errors.Is(err, os.ErrProcessDone) {
		return nil
	}
	return err
}
