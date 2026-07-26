package terminal

import "io"

// StartRequest contains the already-validated process launch parameters passed
// to a PTYFactory. Rows and Columns describe the initial terminal dimensions.
type StartRequest struct {
	Executable string
	Args       []string
	Env        []string
	CWD        string
	Rows       int
	Columns    int
}

// PTYFactory creates processes attached to a pseudo-terminal.
type PTYFactory interface {
	Start(StartRequest) (PTYProcess, error)
}

// PTYProcess is the process and pseudo-terminal pair owned by one Session.
type PTYProcess interface {
	io.ReadWriteCloser
	Resize(rows, columns int) error
	Wait() (exitCode int, err error)
	Terminate() error
	Kill() error
}

func cloneStartRequest(request StartRequest) StartRequest {
	request.Args = append([]string(nil), request.Args...)
	request.Env = append([]string(nil), request.Env...)
	return request
}
