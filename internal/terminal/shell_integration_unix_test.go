//go:build darwin || dragonfly || freebsd || linux || netbsd || openbsd || solaris

package terminal

import (
	"bytes"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"
)

func TestShellIntegrationEmitsAuthenticatedLifecycleFromNativePTY(t *testing.T) {
	for _, shell := range []string{"zsh", "bash"} {
		executable, err := exec.LookPath(shell)
		if err != nil {
			t.Logf("%s is unavailable; skipping native integration case", shell)
			continue
		}
		t.Run(shell, func(t *testing.T) {
			testNativeShellIntegration(t, executable)
		})
	}
}

func testNativeShellIntegration(t *testing.T, executable string) {
	t.Helper()
	home := t.TempDir()
	const prompt = "PTRACK_NATIVE_PROMPT> "
	startupFile := ".zshrc"
	profileArgs := []string{"-l"}
	if filepath.Base(executable) == "bash" {
		startupFile = ".bashrc"
		profileArgs = []string{"-i"}
	} else if err := os.WriteFile(
		filepath.Join(home, ".zprofile"),
		[]byte("ZDOTDIR='"+filepath.Join(home, "redirected")+"'\n"),
		0o600,
	); err != nil {
		t.Fatalf("write native zsh profile fixture: %v", err)
	}
	if err := os.WriteFile(filepath.Join(home, startupFile), []byte("PS1='"+prompt+"'\n"), 0o600); err != nil {
		t.Fatalf("write native shell startup fixture: %v", err)
	}
	workingDirectory := filepath.Join(home, "semi; control\a space é")
	if err := os.Mkdir(workingDirectory, 0o700); err != nil {
		t.Fatalf("create native shell working directory: %v", err)
	}
	canonicalWorkingDirectory, err := filepath.EvalSymlinks(workingDirectory)
	if err != nil {
		t.Fatalf("canonicalize native shell working directory: %v", err)
	}
	profile := Profile{
		ID:         "native-shell",
		Name:       filepath.Base(executable),
		Kind:       ProfileShell,
		Executable: executable,
		Args:       profileArgs,
	}
	owner, err := newShellIntegrationOwner(map[string]Profile{profile.ID: profile})
	if err != nil {
		t.Fatalf("create shell integration: %v", err)
	}
	t.Cleanup(func() {
		if closeErr := owner.Close(); closeErr != nil {
			t.Errorf("close shell integration: %v", closeErr)
		}
	})

	const nonce = "native-pty-test-nonce"
	args, environment, descriptor := owner.prepare(profile, []string{
		"HOME=" + home,
		"PATH=" + os.Getenv("PATH"),
		"TERM=xterm-256color",
	}, nonce)
	if descriptor.Quality != ShellIntegrationRich || descriptor.Nonce != nonce {
		t.Fatalf("native descriptor = %#v", descriptor)
	}
	session := newSession(StartRequest{
		Executable: executable,
		Args:       args,
		Env:        environment,
		CWD:        workingDirectory,
		Rows:       24,
		Columns:    80,
	}, sessionDependencies{factory: GoPTYFactory{}})
	if err := session.start(); err != nil {
		t.Fatalf("start native shell: %v", err)
	}
	t.Cleanup(func() { _ = session.Close(true) })
	startup, output, err := session.attachOutput()
	if err != nil {
		t.Fatalf("attach native shell output: %v", err)
	}

	collected := append([]byte(nil), startup...)
	wantPrompt := []byte("\x1b]633;A;" + nonce + "\x07")
	wantEditing := []byte("\x1b]633;B;" + nonce + "\x07")
	wantExecuting := []byte("\x1b]633;C;" + nonce + "\x07")
	wantCompleted := []byte("\x1b]633;D;1;" + nonce + "\x07")
	wantAdvisory := [][]byte{
		[]byte("\x1b]133;A\x07"),
		[]byte("\x1b]133;B\x07"),
		[]byte("\x1b]133;C\x07"),
		[]byte("\x1b]133;D;1\x07"),
	}
	encodedCWD := strings.ReplaceAll(url.PathEscape(canonicalWorkingDirectory), "%2F", "/")
	wantAdvisoryCWD := []byte("\x1b]7;file://" + encodedCWD + "\x07")
	wantCWD := []byte("\x1b]633;P;Cwd=file://" + encodedCWD + ";" + nonce + "\x07")
	wroteCommand := false
	timeout := time.NewTimer(5 * time.Second)
	defer timeout.Stop()
	for {
		if !wroteCommand && bytes.Contains(collected, wantPrompt) && bytes.Contains(collected, wantEditing) {
			if err := session.WriteInput([]byte("false\n")); err != nil {
				t.Fatalf("write native shell input: %v", err)
			}
			wroteCommand = true
		}
		if wroteCommand && bytes.Contains(collected, wantExecuting) && bytes.Contains(collected, wantCompleted) {
			for _, marker := range wantAdvisory {
				if !bytes.Contains(collected, marker) {
					t.Fatalf("native integration omitted advisory marker %q", marker)
				}
			}
			if !bytes.Contains(collected, wantAdvisoryCWD) {
				t.Fatal("native integration omitted the advisory OSC 7 working directory")
			}
			if bytes.Contains(collected, []byte("\x1b]633;E;")) {
				t.Fatal("native integration emitted a command-text marker")
			}
			indices := []int{
				bytes.Index(collected, wantCWD),
				bytes.Index(collected, wantPrompt),
				bytes.Index(collected, []byte(prompt)),
				bytes.Index(collected, wantEditing),
				bytes.Index(collected, wantExecuting),
				bytes.Index(collected, wantCompleted),
			}
			for index, position := range indices {
				if position < 0 || index > 0 && position <= indices[index-1] {
					if index == 0 {
						prefix := []byte("\x1b]633;P;Cwd=")
						start := bytes.Index(collected, prefix)
						if start >= 0 {
							end := bytes.IndexByte(collected[start:], '\a')
							if end > 0 {
								actual := collected[start : start+end+1]
								mismatch := 0
								for mismatch < len(actual) && mismatch < len(wantCWD) && actual[mismatch] == wantCWD[mismatch] {
									mismatch++
								}
								var actualByte, expectedByte byte
								if mismatch < len(actual) {
									actualByte = actual[mismatch]
								}
								if mismatch < len(wantCWD) {
									expectedByte = wantCWD[mismatch]
								}
								t.Fatalf("native CWD marker mismatch (actual=%d expected=%d first-difference=%d actual-byte=%02x expected-byte=%02x)", len(actual), len(wantCWD), mismatch, actualByte, expectedByte)
							}
						}
					}
					t.Fatalf("native shell marker order is invalid at step %d", index)
				}
			}
			return
		}
		select {
		case chunk, ok := <-output:
			if !ok {
				t.Fatal("native shell exited before emitting the lifecycle markers")
			}
			collected = append(collected, chunk...)
			if len(collected) > 256*1024 {
				t.Fatal("native shell startup exceeded the bounded test capture")
			}
		case <-timeout.C:
			t.Fatalf(
				"timed out waiting for native shell lifecycle markers (prompt=%t editing=%t executing=%t completed=%t)",
				bytes.Contains(collected, wantPrompt),
				bytes.Contains(collected, wantEditing),
				bytes.Contains(collected, wantExecuting),
				bytes.Contains(collected, wantCompleted),
			)
		}
	}
}

func TestNativeBashLoginShellKeepsLoginIdentityAndLogout(t *testing.T) {
	executable, err := exec.LookPath("bash")
	if err != nil {
		t.Skip("bash is unavailable")
	}
	home := t.TempDir()
	logoutPath := filepath.Join(home, "logout-ran")
	if err := os.WriteFile(
		filepath.Join(home, ".bash_profile"),
		[]byte("PS1='PTRACK_LOGIN> '\n"),
		0o600,
	); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(
		filepath.Join(home, ".bash_logout"),
		[]byte("printf logout > '"+logoutPath+"'\n"),
		0o600,
	); err != nil {
		t.Fatal(err)
	}
	owner := shellIntegrationOwnerForTest(t)
	t.Cleanup(func() { _ = owner.Close() })
	profile := Profile{Kind: ProfileShell, Executable: executable, Args: []string{"-l"}}
	baseEnvironment := []string{"HOME=" + home, "PATH=" + os.Getenv("PATH"), "TERM=xterm-256color"}
	args, environment, descriptor := owner.prepare(profile, baseEnvironment, "unused-nonce")
	if descriptor.Quality != ShellIntegrationNone || !reflect.DeepEqual(args, profile.Args) ||
		!reflect.DeepEqual(environment, baseEnvironment) {
		t.Fatalf("login bash integration = args %v env %v descriptor %#v", args, environment, descriptor)
	}
	session := newSession(StartRequest{
		Executable: executable,
		Args:       args,
		Env:        environment,
		CWD:        home,
		Rows:       24,
		Columns:    80,
	}, sessionDependencies{factory: GoPTYFactory{}})
	if err := session.start(); err != nil {
		t.Fatalf("start login bash: %v", err)
	}
	t.Cleanup(func() { _ = session.Close(true) })
	startup, output, err := session.attachOutput()
	if err != nil {
		t.Fatalf("attach login bash: %v", err)
	}
	if err := session.WriteInput([]byte("shopt -q login_shell && printf 'PTRACK_LOGIN=1\\n' || printf 'PTRACK_LOGIN=0\\n'; exit\n")); err != nil {
		t.Fatalf("write login bash input: %v", err)
	}
	collected := append([]byte(nil), startup...)
	deadline := time.NewTimer(5 * time.Second)
	defer deadline.Stop()
	for {
		select {
		case chunk, ok := <-output:
			if ok {
				collected = append(collected, chunk...)
				continue
			}
			if !bytes.Contains(collected, []byte("PTRACK_LOGIN=1")) ||
				bytes.Contains(collected, []byte("\x1b]633;")) {
				t.Fatalf("login bash output = %q", collected)
			}
			if contents, readErr := os.ReadFile(logoutPath); readErr != nil || string(contents) != "logout" {
				t.Fatalf("bash logout result = %q, %v", contents, readErr)
			}
			return
		case <-deadline.C:
			t.Fatal("timed out waiting for login bash exit")
		}
	}
}
