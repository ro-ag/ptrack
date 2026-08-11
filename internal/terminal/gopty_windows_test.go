//go:build windows

package terminal

import (
	"bytes"
	"encoding/base64"
	"encoding/binary"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"
	"unicode/utf16"

	"golang.org/x/sys/windows"
)

func TestNativePTYInteractiveUnicodeResizeAndExit(t *testing.T) {
	session := newSession(StartRequest{
		Executable: nativeWindowsPowerShell(t),
		Args:       []string{"-NoLogo", "-NoProfile", "-NoExit"},
		Env:        os.Environ(),
		CWD:        t.TempDir(),
		Rows:       24,
		Columns:    80,
	}, sessionDependencies{factory: GoPTYFactory{}})
	if err := session.start(); err != nil {
		t.Fatalf("start native PTY: %v", err)
	}
	t.Cleanup(func() { _ = session.Close(true) })
	startup, output, err := session.attachOutput()
	if err != nil {
		t.Fatalf("attach native PTY: %v", err)
	}
	if err := session.Resize(42, 132); err != nil {
		t.Fatalf("resize native PTY: %v", err)
	}
	const fixture = "PTRACK_NATIVE café 日本語 🚀"
	command := "[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); " +
		"$s=$Host.UI.RawUI.WindowSize; " +
		"Write-Output ('PTRACK_SIZE {0} {1}' -f $s.Height,$s.Width); " +
		"Write-Output '" + fixture + "'; exit 7\r\n"
	if err := session.WriteInput([]byte(command)); err != nil {
		t.Fatalf("write native PTY input: %v", err)
	}
	collected := append([]byte(nil), startup...)
	for {
		select {
		case chunk, ok := <-output:
			if ok {
				collected = append(collected, chunk...)
				continue
			}
			result := <-session.ExitResults()
			if result.Err != nil || result.State != SessionExited || result.ExitCode != 7 {
				t.Fatalf("native PTY exit = %#v", result)
			}
			if !bytes.Contains(collected, []byte(fixture)) {
				t.Fatal("native PTY output omitted the Unicode fixture")
			}
			if !bytes.Contains(collected, []byte("PTRACK_SIZE 42 132")) {
				t.Fatalf("native PTY did not report the resized dimensions: %q", collected)
			}
			return
		case <-time.After(10 * time.Second):
			t.Fatal("timed out waiting for native PTY interaction")
		}
	}
}

func TestNativePTYForceCloseKillsDescendantJob(t *testing.T) {
	for _, pid := range startNativeWindowsPTYDescendants(t, false) {
		assertWindowsProcessExited(t, pid)
	}
}

func TestNativePTYNaturalExitKillsRemainingDescendantJob(t *testing.T) {
	for _, pid := range startNativeWindowsPTYDescendants(t, true) {
		assertWindowsProcessExited(t, pid)
	}
}

func startNativeWindowsPTYDescendants(t *testing.T, rootExits bool) [2]int {
	t.Helper()
	pidFile := filepath.Join(t.TempDir(), "descendants.pid")
	rootAction := "Wait-Process -Id $p.Id"
	if rootExits {
		rootAction = "exit 0"
	}
	powerShell := nativeWindowsPowerShell(t)
	childScript := `$grand=Start-Process -PassThru -NoNewWindow ping.exe ` +
		`-ArgumentList '-t','127.0.0.1'; ` +
		`Set-Content -Path $env:PIDFILE -Value @($PID,$grand.Id); ` +
		`Wait-Process -Id $grand.Id`
	script := `$p=Start-Process -PassThru -NoNewWindow -FilePath $env:POWERSHELL_EXE ` +
		`-ArgumentList '-NoLogo','-NoProfile','-EncodedCommand','` +
		encodePowerShellCommand(childScript) + `'; ` + rootAction
	session := newSession(StartRequest{
		Executable: powerShell,
		Args:       []string{"-NoLogo", "-NoProfile", "-Command", script},
		Env: append(
			os.Environ(),
			"PIDFILE="+pidFile,
			"POWERSHELL_EXE="+powerShell,
		),
		CWD:     t.TempDir(),
		Rows:    24,
		Columns: 80,
	}, sessionDependencies{factory: GoPTYFactory{}})
	if err := session.start(); err != nil {
		t.Fatalf("start descendant PTY: %v", err)
	}
	_, output, err := session.attachOutput()
	if err != nil {
		t.Fatalf("attach descendant PTY: %v", err)
	}
	go func() {
		for range output {
		}
	}()
	pids := waitForNativeWindowsPIDFile(t, pidFile)
	t.Cleanup(func() {
		for _, pid := range pids {
			terminateWindowsProcess(pid)
		}
	})
	if rootExits {
		select {
		case result := <-session.ExitResults():
			if result.Err != nil || result.State != SessionExited || result.ExitCode != 0 {
				t.Fatalf("natural descendant session exit = %#v", result)
			}
		case <-time.After(10 * time.Second):
			_ = session.Close(true)
			t.Fatal("timed out waiting for natural root exit")
		}
	} else if err := session.Close(true); err != nil {
		t.Fatalf("force-close descendant PTY: %v", err)
	}
	return pids
}

func encodePowerShellCommand(script string) string {
	units := utf16.Encode([]rune(script))
	encoded := make([]byte, len(units)*2)
	for index, unit := range units {
		binary.LittleEndian.PutUint16(encoded[index*2:], unit)
	}
	return base64.StdEncoding.EncodeToString(encoded)
}

func nativeWindowsPowerShell(t *testing.T) string {
	t.Helper()
	executable, err := exec.LookPath("powershell.exe")
	if err != nil {
		t.Fatalf("resolve PowerShell: %v", err)
	}
	return executable
}

func waitForNativeWindowsPIDFile(t *testing.T, path string) [2]int {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		contents, err := os.ReadFile(path)
		if err == nil {
			fields := strings.Fields(string(contents))
			if len(fields) == 2 {
				child, childErr := strconv.Atoi(fields[0])
				grandchild, grandchildErr := strconv.Atoi(fields[1])
				if childErr == nil && grandchildErr == nil && child > 0 && grandchild > 0 {
					return [2]int{child, grandchild}
				}
			}
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal("timed out waiting for descendant PID")
	return [2]int{}
}

func assertWindowsProcessExited(t *testing.T, pid int) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		handle, err := windows.OpenProcess(windows.SYNCHRONIZE, false, uint32(pid))
		if errors.Is(err, windows.ERROR_INVALID_PARAMETER) {
			return
		}
		if err == nil {
			status, waitErr := windows.WaitForSingleObject(handle, 0)
			_ = windows.CloseHandle(handle)
			if waitErr == nil && status == windows.WAIT_OBJECT_0 {
				return
			}
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("descendant process %d remained after terminal cleanup", pid)
}

func terminateWindowsProcess(pid int) {
	handle, err := windows.OpenProcess(windows.PROCESS_TERMINATE, false, uint32(pid))
	if err != nil {
		return
	}
	_ = windows.TerminateProcess(handle, 1)
	_ = windows.CloseHandle(handle)
}
