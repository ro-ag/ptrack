package terminal

import (
	"bytes"
	"errors"
	"io"
	"sync"
)

type fakePTYFactory struct {
	mu      sync.Mutex
	starts  []StartRequest
	process *fakePTYProcess
	err     error
}

func (f *fakePTYFactory) Start(request StartRequest) (PTYProcess, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.starts = append(f.starts, cloneStartRequest(request))
	if f.err != nil {
		return nil, f.err
	}
	if f.process == nil {
		f.process = newFakePTYProcess()
	}
	return f.process, nil
}

func (f *fakePTYFactory) lastStart() StartRequest {
	f.mu.Lock()
	defer f.mu.Unlock()
	if len(f.starts) == 0 {
		return StartRequest{}
	}
	return cloneStartRequest(f.starts[len(f.starts)-1])
}

type fakePTYProcess struct {
	mu             sync.Mutex
	output         *bytes.Reader
	input          bytes.Buffer
	resizes        [][2]int
	waitCode       int
	waitErr        error
	waitCalls      int
	terminateCalls int
	killCalls      int
	closeCalls     int
	closed         bool
}

func (p *fakePTYProcess) PID() int { return 31337 }

func newFakePTYProcess() *fakePTYProcess {
	return &fakePTYProcess{output: bytes.NewReader(nil)}
}

func (p *fakePTYProcess) Read(buffer []byte) (int, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.output == nil {
		return 0, io.EOF
	}
	return p.output.Read(buffer)
}

func (p *fakePTYProcess) Write(buffer []byte) (int, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.closed {
		return 0, errors.New("fake PTY is closed")
	}
	return p.input.Write(buffer)
}

func (p *fakePTYProcess) Resize(rows, columns int) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.resizes = append(p.resizes, [2]int{rows, columns})
	return nil
}

func (p *fakePTYProcess) Wait() (int, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.waitCalls++
	return p.waitCode, p.waitErr
}

func (p *fakePTYProcess) Terminate() error {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.terminateCalls++
	return nil
}

func (p *fakePTYProcess) Kill() error {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.killCalls++
	return nil
}

func (p *fakePTYProcess) Close() error {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.closeCalls++
	p.closed = true
	return nil
}
