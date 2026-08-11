//go:build darwin || dragonfly || freebsd || linux || netbsd || openbsd || solaris

package terminal

import (
	"bytes"
	"errors"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"golang.org/x/sys/unix"
)

func TestNativePTYInteractiveUnicodeResizeAndExit(t *testing.T) {
	session := newSession(StartRequest{
		Executable: "/bin/sh",
		Env:        append(os.Environ(), "TERM=xterm-256color"),
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
	if err := session.WriteInput([]byte(
		"printf 'PTRACK_SIZE '; stty size; printf '" + fixture + "\\n'; exit 7\n",
	)); err != nil {
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
		case <-time.After(5 * time.Second):
			t.Fatal("timed out waiting for native PTY interaction")
		}
	}
}

func TestNativePTYForceCloseKillsDescendantProcessGroup(t *testing.T) {
	for _, pid := range startNativePTYDescendants(t, false) {
		assertUnixProcessExited(t, pid)
	}
}

func TestNativePTYNaturalExitKillsRemainingDescendantProcessGroup(t *testing.T) {
	for _, pid := range startNativePTYDescendants(t, true) {
		assertUnixProcessExited(t, pid)
	}
}

func startNativePTYDescendants(t *testing.T, rootExits bool) [2]int {
	t.Helper()
	pidFile := filepath.Join(t.TempDir(), "descendants.pid")
	rootAction := `wait "$descendant"`
	if rootExits {
		rootAction = `while [ ! -s "$PIDFILE" ]; do :; done; exit 0`
	}
	session := newSession(StartRequest{
		Executable: "/bin/sh",
		Args: []string{
			"-c",
			`sh -c 'trap "" HUP TERM; sleep 600 & grandchild=$!; ` +
				`printf "%s\n%s\n" "$$" "$grandchild" > "$PIDFILE"; wait "$grandchild"' & ` +
				`descendant=$!; ` + rootAction,
		},
		Env:     append(os.Environ(), "PIDFILE="+pidFile),
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
	pids := waitForNativePIDFile(t, pidFile)
	t.Cleanup(func() {
		for _, pid := range pids {
			_ = unix.Kill(pid, unix.SIGKILL)
		}
	})
	if rootExits {
		select {
		case result := <-session.ExitResults():
			if result.Err != nil || result.State != SessionExited || result.ExitCode != 0 {
				t.Fatalf("natural descendant session exit = %#v", result)
			}
		case <-time.After(5 * time.Second):
			_ = session.Close(true)
			t.Fatal("timed out waiting for natural root exit")
		}
	} else if err := session.Close(true); err != nil {
		t.Fatalf("force-close descendant PTY: %v", err)
	}
	return pids
}

func waitForNativePIDFile(t *testing.T, path string) [2]int {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
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
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("timed out waiting for descendant PID")
	return [2]int{}
}

func assertUnixProcessExited(t *testing.T, pid int) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		err := unix.Kill(pid, 0)
		if errors.Is(err, unix.ESRCH) {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatalf("descendant process %d remained after terminal cleanup", pid)
}

func TestNormalizePTYReadErrorTreatsEIOAsEOF(t *testing.T) {
	if err := normalizePTYReadError(unix.EIO); err != io.EOF {
		t.Fatalf("normalize EIO = %v, want EOF", err)
	}
}

func TestSessionNaturalExitDrainsPTYOutput(t *testing.T) {
	const outputBytes = 1024 * 1024
	session := newSession(StartRequest{
		Executable: "/bin/sh",
		Args: []string{
			"-c",
			"dd if=/dev/zero bs=1024 count=1024 2>/dev/null; printf FINAL",
		},
		Env:     os.Environ(),
		CWD:     t.TempDir(),
		Rows:    24,
		Columns: 80,
	}, sessionDependencies{factory: GoPTYFactory{}})
	if err := session.start(); err != nil {
		t.Fatalf("start session: %v", err)
	}
	startup, output, err := session.attachOutput()
	if err != nil {
		t.Fatalf("attach output: %v", err)
	}

	collected := append([]byte(nil), startup...)
	timeout := time.After(5 * time.Second)
	for {
		select {
		case chunk, ok := <-output:
			if !ok {
				if len(collected) != outputBytes+len("FINAL") {
					t.Fatalf("output bytes = %d, want %d", len(collected), outputBytes+len("FINAL"))
				}
				if !bytes.HasSuffix(collected, []byte("FINAL")) {
					t.Fatal("terminal output is missing the final process bytes")
				}
				result := <-session.ExitResults()
				if result.Err != nil || result.State != SessionExited {
					t.Fatalf("exit result = %#v", result)
				}
				return
			}
			collected = append(collected, chunk...)
		case <-timeout:
			_ = session.Close(true)
			t.Fatal("timed out draining terminal output")
		}
	}
}
