package capability

import (
	"bytes"
	"context"
	"errors"
	"os"
	"os/exec"
	"sync"
)

// ProcessSpec describes one direct executable invocation. Args are never
// interpreted by a local shell.
type ProcessSpec struct {
	Name           string
	Args           []string
	Env            []string
	MaxOutputBytes int64
	Interactive    bool
}

// ProcessResult contains bounded subprocess output.
type ProcessResult struct {
	ExitCode  int
	Stdout    string
	Stderr    string
	Truncated bool
}

// ProcessRunner is injected into Git/SSH executors for deterministic tests.
type ProcessRunner interface {
	Run(context.Context, ProcessSpec) (ProcessResult, error)
}

// ExecProcessRunner invokes host executables directly.
type ExecProcessRunner struct{}

func (ExecProcessRunner) Run(ctx context.Context, spec ProcessSpec) (ProcessResult, error) {
	command := exec.CommandContext(ctx, spec.Name, spec.Args...)
	command.Env = append(os.Environ(), spec.Env...)
	if spec.Interactive {
		command.Stdin = os.Stdin
		command.Stdout = os.Stdout
		command.Stderr = os.Stderr
		err := command.Run()
		return ProcessResult{ExitCode: exitCode(err)}, err
	}
	budget := newBoundedProcessBudget(spec.MaxOutputBytes)
	stdout := newBoundedProcessBuffer(budget)
	stderr := newBoundedProcessBuffer(budget)
	command.Stdout = stdout
	command.Stderr = stderr
	err := command.Run()
	return ProcessResult{
		ExitCode: exitCode(err), Stdout: stdout.String(), Stderr: stderr.String(),
		Truncated: budget.Truncated(),
	}, err
}

func exitCode(err error) int {
	if err == nil {
		return 0
	}
	var exitError *exec.ExitError
	if errors.As(err, &exitError) {
		return exitError.ExitCode()
	}
	return -1
}

type boundedProcessBuffer struct {
	mu     sync.Mutex
	buffer bytes.Buffer
	budget *boundedProcessBudget
}

type boundedProcessBudget struct {
	mu        sync.Mutex
	remaining int64
	truncated bool
}

func newBoundedProcessBudget(maximum int64) *boundedProcessBudget {
	if maximum < 1 {
		maximum = defaultOutputBytes
	}
	return &boundedProcessBudget{remaining: maximum}
}

func newBoundedProcessBuffer(budget *boundedProcessBudget) *boundedProcessBuffer {
	return &boundedProcessBuffer{budget: budget}
}

func (b *boundedProcessBudget) take(size int) int {
	b.mu.Lock()
	defer b.mu.Unlock()
	if int64(size) > b.remaining {
		size = int(b.remaining)
		b.truncated = true
	}
	b.remaining -= int64(size)
	return size
}

func (b *boundedProcessBudget) Truncated() bool {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.truncated
}

func (b *boundedProcessBuffer) Write(data []byte) (int, error) {
	original := len(data)
	data = data[:b.budget.take(len(data))]
	if len(data) > 0 {
		b.mu.Lock()
		defer b.mu.Unlock()
		_, _ = b.buffer.Write(data)
	}
	return original, nil
}

func (b *boundedProcessBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buffer.String()
}
