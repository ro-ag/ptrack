//go:build windows

package agentrun

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"golang.org/x/sys/windows"
)

func preparePrivateRuntimeDir(path string) error {
	if err := os.MkdirAll(path, 0o700); err != nil {
		return fmt.Errorf("create AgentRun runtime directory: %w", err)
	}
	return protectCurrentUser(path, windows.SUB_CONTAINERS_AND_OBJECTS_INHERIT)
}

func openPrivateDescriptor(path string) (*os.File, error) {
	return os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
}

func replacePrivateDescriptor(tempPath, path string) error {
	from, err := windows.UTF16PtrFromString(tempPath)
	if err != nil {
		return err
	}
	to, err := windows.UTF16PtrFromString(path)
	if err != nil {
		return err
	}
	return windows.MoveFileEx(
		from,
		to,
		windows.MOVEFILE_REPLACE_EXISTING|windows.MOVEFILE_WRITE_THROUGH,
	)
}

func lockPrivateDescriptor(runtimeDir string) (func() error, error) {
	lockPath := filepath.Join(runtimeDir, ".agent-registry.lock")
	file, err := os.OpenFile(lockPath, os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, fmt.Errorf("open AgentRun descriptor lock: %w", err)
	}
	var overlapped windows.Overlapped
	if err := windows.LockFileEx(
		windows.Handle(file.Fd()),
		windows.LOCKFILE_EXCLUSIVE_LOCK,
		0,
		1,
		0,
		&overlapped,
	); err != nil {
		_ = file.Close()
		return nil, fmt.Errorf("lock AgentRun descriptor: %w", err)
	}
	return func() error {
		unlockErr := windows.UnlockFileEx(
			windows.Handle(file.Fd()),
			0,
			1,
			0,
			&overlapped,
		)
		return errors.Join(unlockErr, file.Close())
	}, nil
}

func securePublishedDescriptor(path string) error {
	return protectCurrentUser(path, windows.NO_INHERITANCE)
}

// protectCurrentUser installs a protected DACL containing only the process
// user's SID. This remains private even when PTRACK_HOME is a shared path.
func protectCurrentUser(path string, inheritance uint32) error {
	token, err := windows.OpenCurrentProcessToken()
	if err != nil {
		return fmt.Errorf("open process token for AgentRun ACL: %w", err)
	}
	defer token.Close()
	user, err := token.GetTokenUser()
	if err != nil {
		return fmt.Errorf("read process user for AgentRun ACL: %w", err)
	}
	acl, err := windows.ACLFromEntries([]windows.EXPLICIT_ACCESS{{
		AccessPermissions: windows.GENERIC_ALL,
		AccessMode:        windows.SET_ACCESS,
		Inheritance:       inheritance,
		Trustee: windows.TRUSTEE{
			TrusteeForm:  windows.TRUSTEE_IS_SID,
			TrusteeType:  windows.TRUSTEE_IS_USER,
			TrusteeValue: windows.TrusteeValueFromSID(user.User.Sid),
		},
	}}, nil)
	if err != nil {
		return fmt.Errorf("build private AgentRun ACL: %w", err)
	}
	if err := windows.SetNamedSecurityInfo(
		path,
		windows.SE_FILE_OBJECT,
		windows.DACL_SECURITY_INFORMATION|
			windows.PROTECTED_DACL_SECURITY_INFORMATION,
		nil,
		nil,
		acl,
		nil,
	); err != nil {
		return fmt.Errorf("secure AgentRun path ACL: %w", err)
	}
	return nil
}
