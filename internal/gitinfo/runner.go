package gitinfo

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"sync"
	"time"
)

const (
	defaultCommandTimeout = 3 * time.Second
	defaultOutputLimit    = 4 * 1024 * 1024
)

var (
	ErrOutputLimit    = errors.New("git command output limit exceeded")
	ErrCommandTimeout = errors.New("git command timed out")
	ErrCommandFailed  = errors.New("git command failed")
)

type Runner interface {
	Output(ctx context.Context, root string, args ...string) ([]byte, error)
}

type ExecRunner struct {
	GitPath        string
	Timeout        time.Duration
	MaxOutputBytes int
	newCommand     func(context.Context, string, ...string) *exec.Cmd
}

func (r ExecRunner) Output(
	ctx context.Context,
	root string,
	args ...string,
) ([]byte, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	timeout := r.Timeout
	if timeout <= 0 {
		timeout = defaultCommandTimeout
	}
	maxOutput := r.MaxOutputBytes
	if maxOutput <= 0 {
		maxOutput = defaultOutputLimit
	}
	gitPath := r.GitPath
	if gitPath == "" {
		gitPath = "git"
	}
	newCommand := r.newCommand
	if newCommand == nil {
		newCommand = exec.CommandContext
	}

	commandCtx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	command := newCommand(commandCtx, gitPath, gitCommandArgs(root, args)...)
	command.Env = gitEnvironment(os.Environ())
	output := newCommandOutput(maxOutput)
	command.Stdout = output.stdoutWriter()
	command.Stderr = output.stderrWriter()
	runErr := command.Run()
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	if errors.Is(commandCtx.Err(), context.DeadlineExceeded) {
		return nil, ErrCommandTimeout
	}
	if output.exceededLimit() {
		return nil, ErrOutputLimit
	}
	if runErr != nil {
		return nil, fmt.Errorf("%w: %v", ErrCommandFailed, runErr)
	}
	return output.stdoutBytes(), nil
}

func gitCommandArgs(root string, args []string) []string {
	commandArgs := make([]string, 0, len(args)+3)
	commandArgs = append(commandArgs, "--no-optional-locks", "-C", root)
	commandArgs = append(commandArgs, args...)
	return commandArgs
}

func gitEnvironment(source []string) []string {
	overrides := map[string]string{
		"LANG":                "C",
		"LC_ALL":              "C",
		"GIT_OPTIONAL_LOCKS":  "0",
		"GIT_PAGER":           "cat",
		"GIT_TERMINAL_PROMPT": "0",
	}
	environment := make([]string, 0, len(source)+len(overrides))
	for _, entry := range source {
		key, _, found := strings.Cut(entry, "=")
		if !found {
			continue
		}
		overridden := false
		for overrideKey := range overrides {
			if strings.EqualFold(key, overrideKey) {
				overridden = true
				break
			}
		}
		if !overridden {
			environment = append(environment, entry)
		}
	}
	for _, key := range []string{
		"LANG",
		"LC_ALL",
		"GIT_OPTIONAL_LOCKS",
		"GIT_PAGER",
		"GIT_TERMINAL_PROMPT",
	} {
		environment = append(environment, key+"="+overrides[key])
	}
	return environment
}

type commandOutput struct {
	mu        sync.Mutex
	remaining int
	exceeded  bool
	stdout    bytes.Buffer
	stderr    bytes.Buffer
}

type commandOutputWriter struct {
	output *commandOutput
	stdout bool
}

func newCommandOutput(limit int) *commandOutput {
	return &commandOutput{remaining: limit}
}

func (o *commandOutput) stdoutWriter() commandOutputWriter {
	return commandOutputWriter{output: o, stdout: true}
}

func (o *commandOutput) stderrWriter() commandOutputWriter {
	return commandOutputWriter{output: o}
}

func (w commandOutputWriter) Write(input []byte) (int, error) {
	w.output.mu.Lock()
	defer w.output.mu.Unlock()
	accepted := min(len(input), max(0, w.output.remaining))
	if accepted > 0 {
		if w.stdout {
			_, _ = w.output.stdout.Write(input[:accepted])
		} else {
			_, _ = w.output.stderr.Write(input[:accepted])
		}
		w.output.remaining -= accepted
	}
	if accepted < len(input) {
		w.output.exceeded = true
	}
	// Consume the write after the cap so the child cannot make memory grow.
	return len(input), nil
}

func (o *commandOutput) exceededLimit() bool {
	o.mu.Lock()
	defer o.mu.Unlock()
	return o.exceeded
}

func (o *commandOutput) stdoutBytes() []byte {
	o.mu.Lock()
	defer o.mu.Unlock()
	return append([]byte(nil), o.stdout.Bytes()...)
}
