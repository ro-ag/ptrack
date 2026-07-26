//go:build darwin || dragonfly || freebsd || linux || netbsd || openbsd || solaris

package terminal

import (
	"bytes"
	"io"
	"os"
	"testing"
	"time"

	"golang.org/x/sys/unix"
)

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
