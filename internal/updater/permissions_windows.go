//go:build windows

package updater

import (
	"errors"
	"fmt"
	"os"

	"golang.org/x/sys/windows"
)

func preparePrivateDir(path string) error {
	if err := os.MkdirAll(path, 0o700); err != nil {
		return fmt.Errorf("create update directory: %w", err)
	}
	return securePrivatePath(path, true)
}

func securePrivatePath(path string, directory bool) error {
	handle, info, err := openUpdateHandle(path, directory, windows.READ_CONTROL|windows.WRITE_DAC)
	if err != nil {
		return err
	}
	defer windows.CloseHandle(handle)
	if info.FileAttributes&windows.FILE_ATTRIBUTE_REPARSE_POINT != 0 {
		return errors.New("update path is a reparse point")
	}
	return protectCurrentUserHandle(handle, directory)
}

// validatePrivatePath opens the path without traversing reparse points, then
// reapplies the protected current-user-only DACL through that same handle.
func validatePrivatePath(path string, directory bool) error {
	handle, info, err := openUpdateHandle(path, directory, windows.READ_CONTROL|windows.WRITE_DAC)
	if err != nil {
		return err
	}
	defer windows.CloseHandle(handle)
	if info.FileAttributes&windows.FILE_ATTRIBUTE_REPARSE_POINT != 0 {
		return errors.New("update path is a reparse point")
	}
	return protectCurrentUserHandle(handle, directory)
}

func openPrivateRegular(path string) (*os.File, error) {
	handle, info, err := openUpdateHandle(path, false, windows.GENERIC_READ|windows.READ_CONTROL|windows.WRITE_DAC)
	if err != nil {
		return nil, err
	}
	if info.FileAttributes&windows.FILE_ATTRIBUTE_REPARSE_POINT != 0 {
		_ = windows.CloseHandle(handle)
		return nil, errors.New("update path is a reparse point")
	}
	if err := protectCurrentUserHandle(handle, false); err != nil {
		_ = windows.CloseHandle(handle)
		return nil, err
	}
	return os.NewFile(uintptr(handle), path), nil
}

func openUpdateHandle(path string, directory bool, access uint32) (windows.Handle, windows.ByHandleFileInformation, error) {
	path16, err := windows.UTF16PtrFromString(path)
	if err != nil {
		return windows.InvalidHandle, windows.ByHandleFileInformation{}, err
	}
	flags := uint32(windows.FILE_FLAG_OPEN_REPARSE_POINT)
	if directory {
		flags |= windows.FILE_FLAG_BACKUP_SEMANTICS
	}
	handle, err := windows.CreateFile(
		path16,
		access,
		windows.FILE_SHARE_READ,
		nil,
		windows.OPEN_EXISTING,
		flags,
		0,
	)
	if err != nil {
		return windows.InvalidHandle, windows.ByHandleFileInformation{}, err
	}
	var info windows.ByHandleFileInformation
	if err := windows.GetFileInformationByHandle(handle, &info); err != nil {
		_ = windows.CloseHandle(handle)
		return windows.InvalidHandle, windows.ByHandleFileInformation{}, err
	}
	isDirectory := info.FileAttributes&windows.FILE_ATTRIBUTE_DIRECTORY != 0
	if isDirectory != directory {
		_ = windows.CloseHandle(handle)
		return windows.InvalidHandle, windows.ByHandleFileInformation{}, errors.New("update path type mismatch")
	}
	return handle, info, nil
}

func protectCurrentUserHandle(handle windows.Handle, directory bool) error {
	token, err := windows.OpenCurrentProcessToken()
	if err != nil {
		return fmt.Errorf("open process token for update ACL: %w", err)
	}
	defer token.Close()
	user, err := token.GetTokenUser()
	if err != nil {
		return fmt.Errorf("read process user for update ACL: %w", err)
	}
	acl, err := windows.ACLFromEntries([]windows.EXPLICIT_ACCESS{{
		AccessPermissions: windows.GENERIC_ALL,
		AccessMode:        windows.SET_ACCESS,
		Inheritance: func() uint32 {
			if directory {
				return windows.SUB_CONTAINERS_AND_OBJECTS_INHERIT
			}
			return windows.NO_INHERITANCE
		}(),
		Trustee: windows.TRUSTEE{
			TrusteeForm:  windows.TRUSTEE_IS_SID,
			TrusteeType:  windows.TRUSTEE_IS_USER,
			TrusteeValue: windows.TrusteeValueFromSID(user.User.Sid),
		},
	}}, nil)
	if err != nil {
		return fmt.Errorf("build private update ACL: %w", err)
	}
	if err := windows.SetSecurityInfo(
		handle,
		windows.SE_FILE_OBJECT,
		windows.DACL_SECURITY_INFORMATION|windows.PROTECTED_DACL_SECURITY_INFORMATION,
		nil,
		nil,
		acl,
		nil,
	); err != nil {
		return fmt.Errorf("secure update handle ACL: %w", err)
	}
	return nil
}
