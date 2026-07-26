package gitinfo

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"reflect"
	"strings"
	"testing"
	"time"
)

func TestGitCommandArgsAreReadOnlyAndRootScoped(t *testing.T) {
	got := gitCommandArgs("/project", []string{"status", "--porcelain=v2"})
	want := []string{
		"--no-optional-locks",
		"-C", "/project",
		"status", "--porcelain=v2",
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("gitCommandArgs = %#v want %#v", got, want)
	}
}

func TestGitEnvironmentDisablesWritesPagerAndPrompts(t *testing.T) {
	env := gitEnvironment([]string{
		"PATH=/bin",
		"LANG=fr_FR",
		"GIT_OPTIONAL_LOCKS=1",
		"GIT_PAGER=less",
	})
	joined := strings.Join(env, "\n")
	for _, want := range []string{
		"LANG=C",
		"LC_ALL=C",
		"GIT_OPTIONAL_LOCKS=0",
		"GIT_PAGER=cat",
		"GIT_TERMINAL_PROMPT=0",
	} {
		if !strings.Contains(joined, want) {
			t.Fatalf("environment missing %q:\n%s", want, joined)
		}
	}
	if strings.Contains(joined, "LANG=fr_FR") || strings.Contains(joined, "GIT_OPTIONAL_LOCKS=1") {
		t.Fatalf("environment retained overridden values:\n%s", joined)
	}
}

func TestExecRunnerBoundsOutput(t *testing.T) {
	runner := ExecRunner{
		MaxOutputBytes: 8,
		newCommand: func(ctx context.Context, _ string, _ ...string) *exec.Cmd {
			return exec.CommandContext(ctx, os.Args[0], "-test.run=TestGitInfoHelperProcess", "--", "output")
		},
	}
	_, err := runner.Output(context.Background(), "/project", "status")
	if !errors.Is(err, ErrOutputLimit) {
		t.Fatalf("Output error = %v, want ErrOutputLimit", err)
	}
}

func TestExecRunnerHonorsCancellationAndTimeout(t *testing.T) {
	runner := ExecRunner{
		Timeout: 20 * time.Millisecond,
		newCommand: func(ctx context.Context, _ string, _ ...string) *exec.Cmd {
			return exec.CommandContext(ctx, os.Args[0], "-test.run=TestGitInfoHelperProcess", "--", "block")
		},
	}
	_, err := runner.Output(context.Background(), "/project", "status")
	if !errors.Is(err, ErrCommandTimeout) {
		t.Fatalf("timeout error = %v, want ErrCommandTimeout", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	_, err = runner.Output(ctx, "/project", "status")
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("cancel error = %v, want context.Canceled", err)
	}
}

func TestGitInfoHelperProcess(t *testing.T) {
	if len(os.Args) < 2 || os.Args[len(os.Args)-2] != "--" {
		return
	}
	switch os.Args[len(os.Args)-1] {
	case "output":
		_, _ = os.Stdout.WriteString("0123456789abcdef")
		os.Exit(0)
	case "block":
		time.Sleep(time.Minute)
	}
}
