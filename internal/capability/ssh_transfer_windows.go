//go:build windows

package capability

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"io"
	"os"
	"path/filepath"
	"strings"
	"unsafe"

	"golang.org/x/sys/windows"
)

type fileRenameInformation struct {
	ReplaceIfExists uint32
	RootDirectory   windows.Handle
	FileNameLength  uint32
	FileName        [1]uint16
}

func installStagedDownload(canonicalProject, destination, stagedPath string, maximum int64) error {
	relative, err := filepath.Rel(canonicalProject, destination)
	if err != nil || relative == "." || relative == ".." || strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
		return ErrDenied{Reason: "download destination escapes the project"}
	}
	parts := strings.Split(relative, string(filepath.Separator))
	parent, err := openWindowsProjectDirectory(0, windowsNTPath(canonicalProject))
	if err != nil {
		return err
	}
	for _, component := range parts[:len(parts)-1] {
		next, openErr := openWindowsProjectDirectory(parent, component)
		_ = windows.CloseHandle(parent)
		if openErr != nil {
			return ErrDenied{Reason: "download destination parent is not a stable project directory"}
		}
		parent = next
	}
	defer windows.CloseHandle(parent)

	stagedUTF16, err := windows.UTF16PtrFromString(stagedPath)
	if err != nil {
		return err
	}
	sourceHandle, err := windows.CreateFile(
		stagedUTF16,
		windows.GENERIC_READ,
		windows.FILE_SHARE_READ,
		nil,
		windows.OPEN_EXISTING,
		windows.FILE_FLAG_OPEN_REPARSE_POINT|windows.FILE_FLAG_SEQUENTIAL_SCAN,
		0,
	)
	if err != nil {
		return ErrDenied{Reason: "download staging file is invalid"}
	}
	var sourceInfo windows.ByHandleFileInformation
	if err := windows.GetFileInformationByHandle(sourceHandle, &sourceInfo); err != nil ||
		sourceInfo.FileAttributes&(windows.FILE_ATTRIBUTE_REPARSE_POINT|windows.FILE_ATTRIBUTE_DIRECTORY) != 0 {
		_ = windows.CloseHandle(sourceHandle)
		return ErrDenied{Reason: "download staging file is invalid"}
	}
	source := os.NewFile(uintptr(sourceHandle), stagedPath)
	defer source.Close()

	temporaryName, err := randomTransferNameWindows()
	if err != nil {
		return err
	}
	temporaryHandle, err := createWindowsProjectFile(parent, temporaryName)
	if err != nil {
		return err
	}
	temporary := os.NewFile(uintptr(temporaryHandle), temporaryName)
	removeTemporary := true
	defer func() {
		if removeTemporary {
			markWindowsFileForDeletion(temporaryHandle)
		}
		_ = temporary.Close()
	}()
	written, copyErr := io.Copy(temporary, io.LimitReader(source, maximum+1))
	syncErr := temporary.Sync()
	if err := errors.Join(copyErr, syncErr); err != nil {
		return err
	}
	if written > maximum {
		return responseLimitError{}
	}
	if err := renameWindowsProjectFile(temporaryHandle, parent, parts[len(parts)-1]); err != nil {
		return err
	}
	removeTemporary = false
	return nil
}

func windowsNTPath(path string) string {
	if strings.HasPrefix(path, `\\`) {
		return `\??\UNC\` + strings.TrimPrefix(path, `\\`)
	}
	return `\??\` + path
}

func openWindowsProjectDirectory(root windows.Handle, name string) (windows.Handle, error) {
	objectName, err := windows.NewNTUnicodeString(name)
	if err != nil {
		return 0, err
	}
	attributes := &windows.OBJECT_ATTRIBUTES{
		RootDirectory: root,
		ObjectName:    objectName,
		Attributes:    windows.OBJ_DONT_REPARSE,
	}
	attributes.Length = uint32(unsafe.Sizeof(*attributes))
	var handle windows.Handle
	var status windows.IO_STATUS_BLOCK
	var allocation int64
	err = windows.NtCreateFile(
		&handle,
		windows.FILE_GENERIC_READ|windows.SYNCHRONIZE,
		attributes,
		&status,
		&allocation,
		0,
		windows.FILE_SHARE_READ|windows.FILE_SHARE_WRITE|windows.FILE_SHARE_DELETE,
		windows.FILE_OPEN,
		windows.FILE_DIRECTORY_FILE|windows.FILE_OPEN_REPARSE_POINT|windows.FILE_SYNCHRONOUS_IO_NONALERT,
		0,
		0,
	)
	if err != nil {
		return 0, err
	}
	var info windows.ByHandleFileInformation
	if err := windows.GetFileInformationByHandle(handle, &info); err != nil || info.FileAttributes&windows.FILE_ATTRIBUTE_REPARSE_POINT != 0 {
		_ = windows.CloseHandle(handle)
		if err != nil {
			return 0, err
		}
		return 0, ErrDenied{Reason: "download destination parent contains a reparse point"}
	}
	return handle, nil
}

func createWindowsProjectFile(parent windows.Handle, name string) (windows.Handle, error) {
	objectName, err := windows.NewNTUnicodeString(name)
	if err != nil {
		return 0, err
	}
	attributes := &windows.OBJECT_ATTRIBUTES{
		RootDirectory: parent,
		ObjectName:    objectName,
		Attributes:    windows.OBJ_DONT_REPARSE,
	}
	attributes.Length = uint32(unsafe.Sizeof(*attributes))
	var handle windows.Handle
	var status windows.IO_STATUS_BLOCK
	var allocation int64
	err = windows.NtCreateFile(
		&handle,
		windows.FILE_GENERIC_READ|windows.FILE_GENERIC_WRITE|windows.DELETE|windows.SYNCHRONIZE,
		attributes,
		&status,
		&allocation,
		windows.FILE_ATTRIBUTE_TEMPORARY,
		windows.FILE_SHARE_READ|windows.FILE_SHARE_WRITE|windows.FILE_SHARE_DELETE,
		windows.FILE_CREATE,
		windows.FILE_NON_DIRECTORY_FILE|windows.FILE_OPEN_REPARSE_POINT|windows.FILE_SYNCHRONOUS_IO_NONALERT,
		0,
		0,
	)
	return handle, err
}

func renameWindowsProjectFile(handle, parent windows.Handle, name string) error {
	utf16Name, err := windows.UTF16FromString(name)
	if err != nil {
		return err
	}
	nameBytes := (len(utf16Name) - 1) * 2
	var layout fileRenameInformation
	buffer := make([]byte, int(unsafe.Offsetof(layout.FileName))+nameBytes)
	info := (*fileRenameInformation)(unsafe.Pointer(&buffer[0]))
	info.ReplaceIfExists = windows.FILE_RENAME_REPLACE_IF_EXISTS | windows.FILE_RENAME_POSIX_SEMANTICS
	info.RootDirectory = parent
	info.FileNameLength = uint32(nameBytes)
	copy((*[windows.MAX_LONG_PATH]uint16)(unsafe.Pointer(&info.FileName[0]))[:nameBytes/2:nameBytes/2], utf16Name)
	var status windows.IO_STATUS_BLOCK
	return windows.NtSetInformationFile(handle, &status, &buffer[0], uint32(len(buffer)), windows.FileRenameInformation)
}

func markWindowsFileForDeletion(handle windows.Handle) {
	deleteFile := byte(1)
	var status windows.IO_STATUS_BLOCK
	_ = windows.NtSetInformationFile(handle, &status, &deleteFile, 1, windows.FileDispositionInformation)
}

func randomTransferNameWindows() (string, error) {
	value := make([]byte, 16)
	if _, err := rand.Read(value); err != nil {
		return "", err
	}
	return ".ptrack-download-" + hex.EncodeToString(value), nil
}
