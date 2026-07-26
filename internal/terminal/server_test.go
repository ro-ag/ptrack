package terminal

import (
	"bytes"
	"context"
	"errors"
	"io"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/gorilla/websocket"
)

const (
	testACKWindow       = 512 * 1024
	testOutputChunkSize = 64 * 1024
	testMaxOutputFrame  = 100 * 1024
	testMaxClientFrame  = 64 * 1024
	testStreamDeadline  = 2 * time.Second
)

func TestStreamServerUsesOneOSAssignedLoopbackListenerPerManager(t *testing.T) {
	firstProcess := newServerFakeProcess()
	secondProcess := newServerFakeProcess()
	manager, sessions := newServerManager(t, firstProcess, secondProcess)

	firstURL := streamURLForTest(t, manager, sessions[0].ID())
	secondURL := streamURLForTest(t, manager, sessions[1].ID())
	firstParsed := parseStreamURL(t, firstURL)
	secondParsed := parseStreamURL(t, secondURL)

	if firstParsed.Scheme != "ws" {
		t.Fatalf("stream scheme = %q, want ws", firstParsed.Scheme)
	}
	host, portText, err := net.SplitHostPort(firstParsed.Host)
	if err != nil {
		t.Fatalf("split stream host %q: %v", firstParsed.Host, err)
	}
	if host != "127.0.0.1" {
		t.Fatalf("listener host = %q, want 127.0.0.1", host)
	}
	port, err := strconv.Atoi(portText)
	if err != nil || port <= 0 {
		t.Fatalf("OS-assigned listener port = %q, want positive port", portText)
	}
	if secondParsed.Host != firstParsed.Host {
		t.Fatalf("sessions use different listeners: %q and %q",
			firstParsed.Host, secondParsed.Host)
	}
	if firstParsed.Query().Get("token") == "" || secondParsed.Query().Get("token") == "" {
		t.Fatal("stream URL is missing its one-session token")
	}
	if !strings.Contains(firstParsed.Path, sessions[0].ID()) ||
		!strings.Contains(secondParsed.Path, sessions[1].ID()) {
		t.Fatalf("stream paths do not contain opaque session IDs: %q and %q",
			firstParsed.Path, secondParsed.Path)
	}
}

func TestStreamServerOriginPolicy(t *testing.T) {
	accepted := []string{
		"wails://wails",
		"http://wails.localhost",
		"https://wails.localhost",
		"http://localhost:5173",
		"https://localhost:34115",
		"http://127.0.0.1:5173",
		"http://[::1]:5173",
	}
	for _, origin := range accepted {
		t.Run("accept "+origin, func(t *testing.T) {
			fixture := newServerFixture(t)
			connection := dialStreamForTest(t, fixture.streamURL, origin)
			_ = connection.Close()
		})
	}

	rejected := []struct {
		name   string
		origin string
	}{
		{name: "missing origin"},
		{name: "opaque origin", origin: "null"},
		{name: "remote origin", origin: "https://evil.example"},
		{name: "lookalike Wails host", origin: "http://wails.localhost.evil.example"},
		{name: "non-loopback Vite host", origin: "http://192.0.2.10:5173"},
		{name: "file origin", origin: "file:///tmp/frontend"},
	}
	for _, test := range rejected {
		t.Run("reject "+test.name, func(t *testing.T) {
			fixture := newServerFixture(t)
			assertUpgradeRejected(t, fixture.streamURL, test.origin)
		})
	}
}

func TestStreamServerRejectsMissingWrongOrUnknownAuthentication(t *testing.T) {
	t.Run("missing token", func(t *testing.T) {
		fixture := newServerFixture(t)
		parsed := parseStreamURL(t, fixture.streamURL)
		query := parsed.Query()
		query.Del("token")
		parsed.RawQuery = query.Encode()
		assertUpgradeRejected(t, parsed.String(), "wails://wails")
	})

	t.Run("wrong token", func(t *testing.T) {
		fixture := newServerFixture(t)
		parsed := parseStreamURL(t, fixture.streamURL)
		query := parsed.Query()
		query.Set("token", "wrong-token")
		parsed.RawQuery = query.Encode()
		assertUpgradeRejected(t, parsed.String(), "wails://wails")
	})

	t.Run("unknown session", func(t *testing.T) {
		fixture := newServerFixture(t)
		parsed := parseStreamURL(t, fixture.streamURL)
		parsed.Path = strings.Replace(
			parsed.Path,
			fixture.session.ID(),
			"unknown-session",
			1,
		)
		assertUpgradeRejected(t, parsed.String(), "wails://wails")
	})
}

func TestStreamServerRejectsClosedSessionAndItsExpiredURL(t *testing.T) {
	fixture := newServerFixture(t)
	staleURL := fixture.streamURL

	if err := fixture.manager.CloseSession(fixture.session.ID(), true); err != nil {
		t.Fatalf("CloseSession: %v", err)
	}
	if _, err := fixture.manager.Get(fixture.session.ID()); !errors.Is(err, ErrSessionNotFound) {
		t.Fatalf("Get closed session error = %v, want %v", err, ErrSessionNotFound)
	}
	assertUpgradeRejected(t, staleURL, "wails://wails")
}

func TestStreamServerRejectsSecondAttachment(t *testing.T) {
	fixture := newServerFixture(t)
	first := dialStreamForTest(t, fixture.streamURL, "wails://wails")
	defer first.Close()

	assertUpgradeRejected(t, fixture.streamURL, "wails://wails")
}

func TestStreamServerCarriesOnlyBinaryPTYInputAndOutput(t *testing.T) {
	fixture := newServerFixture(t)
	connection := dialStreamForTest(t, fixture.streamURL, "wails://wails")
	defer connection.Close()

	input := []byte{0, 1, 2, '\r', '\n', 0xff}
	if err := connection.WriteMessage(websocket.BinaryMessage, input); err != nil {
		t.Fatalf("write binary input: %v", err)
	}
	if got := awaitBytes(t, fixture.process.inputWrites, "PTY input"); !bytes.Equal(got, input) {
		t.Fatalf("PTY input = %v, want %v", got, input)
	}

	output := []byte("prompt: \x1b[32mready\x1b[0m\r\n")
	fixture.process.queueOutput(t, output)
	messageType, got, err := readStreamMessage(connection)
	if err != nil {
		t.Fatalf("read PTY output: %v", err)
	}
	if messageType != websocket.BinaryMessage {
		t.Fatalf("PTY output message type = %d, want binary", messageType)
	}
	if !bytes.Equal(got, output) {
		t.Fatalf("PTY output = %q, want %q", got, output)
	}
}

func TestStreamServerRejectsMalformedControlAndOversizedFrames(t *testing.T) {
	tests := []struct {
		name        string
		messageType int
		payload     []byte
	}{
		{
			name:        "invalid JSON ACK",
			messageType: websocket.TextMessage,
			payload:     []byte(`{"type":"ack",`),
		},
		{
			name:        "zero ACK",
			messageType: websocket.TextMessage,
			payload:     []byte(`{"type":"ack","bytes":0}`),
		},
		{
			name:        "negative ACK",
			messageType: websocket.TextMessage,
			payload:     []byte(`{"type":"ack","bytes":-1}`),
		},
		{
			name:        "ACK beyond bytes sent",
			messageType: websocket.TextMessage,
			payload:     []byte(`{"type":"ack","bytes":1}`),
		},
		{
			name:        "unknown control type",
			messageType: websocket.TextMessage,
			payload:     []byte(`{"type":"detach","bytes":1}`),
		},
		{
			name:        "oversized text frame",
			messageType: websocket.TextMessage,
			payload:     bytes.Repeat([]byte("x"), testMaxClientFrame+1),
		},
		{
			name:        "oversized binary frame",
			messageType: websocket.BinaryMessage,
			payload:     bytes.Repeat([]byte("x"), testMaxClientFrame+1),
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			fixture := newServerFixture(t)
			connection := dialStreamForTest(t, fixture.streamURL, "wails://wails")
			defer connection.Close()

			if err := connection.WriteMessage(test.messageType, test.payload); err != nil {
				t.Fatalf("write invalid frame: %v", err)
			}
			assertConnectionClosed(t, connection)
			awaitSignal(t, fixture.process.terminateStarted, "session close after protocol error")
		})
	}
}

func TestStreamDisconnectClosesSessionAndPreventsReconnect(t *testing.T) {
	fixture := newServerFixture(t)
	staleURL := fixture.streamURL
	connection := dialStreamForTest(t, staleURL, "wails://wails")

	if err := connection.Close(); err != nil {
		t.Fatalf("abrupt stream close: %v", err)
	}
	awaitSignal(t, fixture.process.terminateStarted, "session close after disconnect")
	if _, err := fixture.manager.Get(fixture.session.ID()); !errors.Is(err, ErrSessionNotFound) {
		t.Fatalf("Get disconnected session error = %v, want %v", err, ErrSessionNotFound)
	}
	assertUpgradeRejected(t, staleURL, "wails://wails")
}

func TestManagerShutdownClosesActiveStreamsAndListener(t *testing.T) {
	fixture := newServerFixture(t)
	connection := dialStreamForTest(t, fixture.streamURL, "wails://wails")
	defer connection.Close()

	ctx, cancel := context.WithTimeout(context.Background(), testStreamDeadline)
	defer cancel()
	if err := fixture.manager.Shutdown(ctx); err != nil {
		t.Fatalf("Shutdown: %v", err)
	}
	assertConnectionClosed(t, connection)
	assertDialFails(t, fixture.streamURL, "wails://wails")
}

func TestManagerShutdownUnblocksAStalledPTYInputWrite(t *testing.T) {
	process := &blockedWriteServerProcess{
		serverFakeProcess: newServerFakeProcess(),
		writeStarted:      make(chan struct{}),
	}
	manager := newManagerForTest(
		t,
		t.TempDir(),
		newManagerFakeFactory(managerStartOutcome{process: process}),
	)
	session, err := manager.Create("agent", "", 24, 80)
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	cleanupManager(t, manager, process.managerFakeProcess)
	awaitSignal(t, process.readStarted, "PTY output read loop")
	awaitSignal(t, process.waitStarted, "PTY wait loop")

	connection := dialStreamForTest(
		t,
		streamURLForTest(t, manager, session.ID()),
		"wails://wails",
	)
	defer connection.Close()
	if err := connection.WriteMessage(websocket.BinaryMessage, []byte("blocked input")); err != nil {
		t.Fatalf("write input: %v", err)
	}
	awaitSignal(t, process.writeStarted, "blocked PTY input write")

	ctx, cancel := context.WithTimeout(context.Background(), testStreamDeadline)
	defer cancel()
	if err := manager.Shutdown(ctx); err != nil {
		t.Fatalf("Shutdown with stalled PTY write: %v", err)
	}
}

func TestStreamServerCapsUnacknowledgedOutputAndResumesAfterACK(t *testing.T) {
	fixture := newServerFixture(t)
	connection := dialStreamForTest(t, fixture.streamURL, "wails://wails")
	defer connection.Close()
	frames := readStreamFrames(connection)

	chunk := bytes.Repeat([]byte("o"), testOutputChunkSize)
	for index := 0; index < testACKWindow/testOutputChunkSize+4; index++ {
		fixture.process.queueOutput(t, chunk)
	}

	sentLimit := testACKWindow - testOutputChunkSize
	receivedBeforeACK := readFrameBytes(t, frames, sentLimit)
	if receivedBeforeACK != sentLimit {
		t.Fatalf("bytes sent before ACK = %d, want %d", receivedBeforeACK, sentLimit)
	}

	readBeforeACK := 0
	for readBeforeACK < testACKWindow {
		readBeforeACK += awaitInt(t, fixture.process.readBytes, "PTY read before ACK")
	}
	if readBeforeACK != testACKWindow {
		t.Fatalf("queued plus sent-unacknowledged bytes = %d, exceeds %d",
			readBeforeACK, testACKWindow)
	}
	assertNoIntBeforeDeadline(
		t,
		fixture.process.readBytes,
		150*time.Millisecond,
		"PTY read beyond ACK window",
	)
	assertNoFrameBeforeDeadline(t, frames, 150*time.Millisecond, "output beyond send ledger")

	ack := []byte(`{"type":"ack","bytes":65536}`)
	if err := connection.WriteMessage(websocket.TextMessage, ack); err != nil {
		t.Fatalf("write ACK: %v", err)
	}
	if got := readFrameBytes(t, frames, testOutputChunkSize); got != testOutputChunkSize {
		t.Fatalf("bytes sent after ACK = %d, want %d", got, testOutputChunkSize)
	}
	if resumed := awaitInt(t, fixture.process.readBytes, "PTY read after ACK"); resumed <= 0 {
		t.Fatalf("resumed PTY read bytes = %d, want positive", resumed)
	}
}

type serverFixture struct {
	manager   *Manager
	session   *Session
	process   *serverFakeProcess
	streamURL string
}

func newServerFixture(t *testing.T) serverFixture {
	t.Helper()
	process := newServerFakeProcess()
	manager, sessions := newServerManager(t, process)
	return serverFixture{
		manager:   manager,
		session:   sessions[0],
		process:   process,
		streamURL: streamURLForTest(t, manager, sessions[0].ID()),
	}
}

func newServerManager(
	t *testing.T,
	processes ...*serverFakeProcess,
) (*Manager, []*Session) {
	t.Helper()
	outcomes := make([]managerStartOutcome, 0, len(processes))
	for _, process := range processes {
		outcomes = append(outcomes, managerStartOutcome{process: process})
	}
	projectRoot := t.TempDir()
	manager := newManagerForTest(t, projectRoot, newManagerFakeFactory(outcomes...))
	sessions := make([]*Session, 0, len(processes))
	for _, process := range processes {
		session, err := manager.Create("agent", "", 24, 80)
		if err != nil {
			t.Fatalf("Create: %v", err)
		}
		sessions = append(sessions, session)
		awaitSignal(t, process.readStarted, "PTY output read loop")
		awaitSignal(t, process.waitStarted, "PTY wait loop")
	}
	baseProcesses := make([]*managerFakeProcess, 0, len(processes))
	for _, process := range processes {
		baseProcesses = append(baseProcesses, process.managerFakeProcess)
	}
	cleanupManager(t, manager, baseProcesses...)
	return manager, sessions
}

func streamURLForTest(t *testing.T, manager *Manager, sessionID string) string {
	t.Helper()
	rawURL, err := manager.StreamURL(sessionID)
	if err != nil {
		t.Fatalf("StreamURL: %v", err)
	}
	return rawURL
}

func parseStreamURL(t *testing.T, rawURL string) *url.URL {
	t.Helper()
	parsed, err := url.Parse(rawURL)
	if err != nil {
		t.Fatalf("parse stream URL %q: %v", rawURL, err)
	}
	return parsed
}

func dialStreamForTest(t *testing.T, rawURL, origin string) *websocket.Conn {
	t.Helper()
	connection, response, err := dialStream(rawURL, origin)
	if err != nil {
		if response != nil && response.Body != nil {
			_ = response.Body.Close()
		}
		t.Fatalf("dial stream: %v", err)
	}
	return connection
}

func dialStream(
	rawURL string,
	origin string,
) (*websocket.Conn, *http.Response, error) {
	headers := make(http.Header)
	if origin != "" {
		headers.Set("Origin", origin)
	}
	dialer := websocket.Dialer{
		HandshakeTimeout: testStreamDeadline,
	}
	return dialer.Dial(rawURL, headers)
}

func assertUpgradeRejected(t *testing.T, rawURL, origin string) {
	t.Helper()
	connection, response, err := dialStream(rawURL, origin)
	if connection != nil {
		_ = connection.Close()
	}
	if err == nil {
		t.Fatal("WebSocket upgrade succeeded, want rejection")
	}
	if response == nil {
		t.Fatalf("upgrade failed without an HTTP response: %v", err)
	}
	defer response.Body.Close()
	if response.StatusCode == http.StatusSwitchingProtocols {
		t.Fatalf("upgrade response status = %d, want rejection", response.StatusCode)
	}
}

func assertDialFails(t *testing.T, rawURL, origin string) {
	t.Helper()
	connection, response, err := dialStream(rawURL, origin)
	if connection != nil {
		_ = connection.Close()
	}
	if response != nil && response.Body != nil {
		_ = response.Body.Close()
	}
	if err == nil {
		t.Fatal("dial succeeded after server shutdown")
	}
}

func readStreamMessage(connection *websocket.Conn) (int, []byte, error) {
	if err := connection.SetReadDeadline(time.Now().Add(testStreamDeadline)); err != nil {
		return 0, nil, err
	}
	return connection.ReadMessage()
}

func readStreamBytes(t *testing.T, connection *websocket.Conn, want int) int {
	t.Helper()
	total := 0
	for total < want {
		messageType, output, err := readStreamMessage(connection)
		if err != nil {
			t.Fatalf("read stream output after %d bytes: %v", total, err)
		}
		if messageType != websocket.BinaryMessage {
			t.Fatalf("stream output type = %d, want binary", messageType)
		}
		if len(output) == 0 || len(output) > testMaxOutputFrame {
			t.Fatalf("stream output frame size = %d, want 1..%d",
				len(output), testMaxOutputFrame)
		}
		total += len(output)
		if total > want {
			t.Fatalf("stream sent %d bytes, crossing requested boundary %d", total, want)
		}
	}
	return total
}

type streamFrame struct {
	messageType int
	output      []byte
	err         error
}

func readStreamFrames(connection *websocket.Conn) <-chan streamFrame {
	frames := make(chan streamFrame, 16)
	go func() {
		defer close(frames)
		for {
			messageType, output, err := connection.ReadMessage()
			frames <- streamFrame{messageType: messageType, output: output, err: err}
			if err != nil {
				return
			}
		}
	}()
	return frames
}

func readFrameBytes(t *testing.T, frames <-chan streamFrame, want int) int {
	t.Helper()
	total := 0
	for total < want {
		select {
		case frame := <-frames:
			if frame.err != nil {
				t.Fatalf("read stream output after %d bytes: %v", total, frame.err)
			}
			if frame.messageType != websocket.BinaryMessage {
				t.Fatalf("stream output type = %d, want binary", frame.messageType)
			}
			if len(frame.output) == 0 || len(frame.output) > testMaxOutputFrame {
				t.Fatalf("stream output frame size = %d, want 1..%d",
					len(frame.output), testMaxOutputFrame)
			}
			total += len(frame.output)
			if total > want {
				t.Fatalf("stream sent %d bytes, crossing requested boundary %d", total, want)
			}
		case <-time.After(testStreamDeadline):
			t.Fatalf("timed out after %d of %d stream bytes", total, want)
		}
	}
	return total
}

func assertNoFrameBeforeDeadline(
	t *testing.T,
	frames <-chan streamFrame,
	deadline time.Duration,
	description string,
) {
	t.Helper()
	select {
	case frame := <-frames:
		if frame.err != nil {
			t.Fatalf("%s: stream closed: %v", description, frame.err)
		}
		t.Fatalf("%s: unexpectedly received %d bytes", description, len(frame.output))
	case <-time.After(deadline):
	}
}

func assertConnectionClosed(t *testing.T, connection *websocket.Conn) {
	t.Helper()
	if err := connection.SetReadDeadline(time.Now().Add(testStreamDeadline)); err != nil {
		t.Fatalf("set close deadline: %v", err)
	}
	for {
		if _, _, err := connection.ReadMessage(); err != nil {
			return
		}
	}
}

func awaitBytes(t *testing.T, values <-chan []byte, description string) []byte {
	t.Helper()
	select {
	case value := <-values:
		return value
	case <-time.After(testStreamDeadline):
		t.Fatalf("timed out waiting for %s", description)
		return nil
	}
}

func awaitInt(t *testing.T, values <-chan int, description string) int {
	t.Helper()
	select {
	case value := <-values:
		return value
	case <-time.After(testStreamDeadline):
		t.Fatalf("timed out waiting for %s", description)
		return 0
	}
}

func assertNoIntBeforeDeadline(
	t *testing.T,
	values <-chan int,
	deadline time.Duration,
	description string,
) {
	t.Helper()
	select {
	case value := <-values:
		t.Fatalf("%s: unexpectedly read %d bytes", description, value)
	case <-time.After(deadline):
	}
}

type serverFakeProcess struct {
	*managerFakeProcess

	output      chan []byte
	inputWrites chan []byte
	readBytes   chan int
	done        chan struct{}
	doneOnce    sync.Once
}

type blockedWriteServerProcess struct {
	*serverFakeProcess
	writeStarted chan struct{}
	writeOnce    sync.Once
}

func (p *blockedWriteServerProcess) Write([]byte) (int, error) {
	p.writeOnce.Do(func() {
		close(p.writeStarted)
	})
	<-p.done
	return 0, io.EOF
}

func newServerFakeProcess() *serverFakeProcess {
	return &serverFakeProcess{
		managerFakeProcess: newManagerFakeProcess(),
		output:             make(chan []byte, 32),
		inputWrites:        make(chan []byte, 16),
		readBytes:          make(chan int, 32),
		done:               make(chan struct{}),
	}
}

func (p *serverFakeProcess) Read(buffer []byte) (int, error) {
	p.readStartOnce.Do(func() {
		close(p.readStarted)
	})
	select {
	case output := <-p.output:
		if len(output) > len(buffer) {
			return 0, errors.New("server fake output exceeds PTY read buffer")
		}
		n := copy(buffer, output)
		select {
		case p.readBytes <- n:
		case <-p.done:
			return 0, io.EOF
		case <-p.readClosed:
			return 0, io.EOF
		}
		return n, nil
	case <-p.done:
		return 0, io.EOF
	case <-p.readClosed:
		return 0, io.EOF
	}
}

func (p *serverFakeProcess) Write(input []byte) (int, error) {
	copyOfInput := append([]byte(nil), input...)
	p.fakePTYProcess.mu.Lock()
	if p.fakePTYProcess.closed {
		p.fakePTYProcess.mu.Unlock()
		return 0, errors.New("server fake PTY is closed")
	}
	_, _ = p.fakePTYProcess.input.Write(copyOfInput)
	p.fakePTYProcess.mu.Unlock()
	select {
	case p.inputWrites <- copyOfInput:
		return len(input), nil
	case <-p.done:
		return 0, io.EOF
	}
}

func (p *serverFakeProcess) Close() error {
	p.doneOnce.Do(func() {
		close(p.done)
	})
	return p.managerFakeProcess.Close()
}

func (p *serverFakeProcess) queueOutput(t *testing.T, output []byte) {
	t.Helper()
	copyOfOutput := append([]byte(nil), output...)
	select {
	case p.output <- copyOfOutput:
	case <-time.After(testStreamDeadline):
		t.Fatal("timed out queueing fake PTY output")
	}
}
