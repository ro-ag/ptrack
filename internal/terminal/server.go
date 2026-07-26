package terminal

import (
	"context"
	"crypto/subtle"
	"errors"
	"fmt"
	"net"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/gorilla/websocket"
)

const (
	streamPathPrefix = "/terminal/"
	streamPongWait   = 60 * time.Second
	streamPingEvery  = 25 * time.Second
	streamWriteWait  = 10 * time.Second
)

type streamServer struct {
	manager    *Manager
	listener   net.Listener
	httpServer *http.Server
	upgrader   websocket.Upgrader

	ctx    context.Context
	cancel context.CancelFunc

	mu       sync.Mutex
	conns    map[*websocket.Conn]struct{}
	stopping bool
	handlers sync.WaitGroup

	serveDone chan struct{}
	serveErr  error

	shutdownOnce sync.Once
	shutdownDone chan struct{}
	shutdownErr  error
}

func newStreamServer(manager *Manager) (*streamServer, error) {
	listener, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		return nil, fmt.Errorf("listen for terminal streams: %w", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	server := &streamServer{
		manager:      manager,
		listener:     listener,
		ctx:          ctx,
		cancel:       cancel,
		conns:        make(map[*websocket.Conn]struct{}),
		serveDone:    make(chan struct{}),
		shutdownDone: make(chan struct{}),
	}
	server.upgrader = websocket.Upgrader{
		HandshakeTimeout: streamWriteWait,
		CheckOrigin:      func(request *http.Request) bool { return allowedStreamOrigin(request.Header.Get("Origin")) },
	}
	mux := http.NewServeMux()
	mux.HandleFunc(streamPathPrefix, server.handle)
	server.httpServer = &http.Server{
		Handler:           mux,
		ReadHeaderTimeout: 5 * time.Second,
	}
	go server.serve()
	return server, nil
}

func (s *streamServer) serve() {
	defer close(s.serveDone)
	err := s.httpServer.Serve(s.listener)
	if err != nil && !errors.Is(err, http.ErrServerClosed) && !errors.Is(err, net.ErrClosed) {
		s.serveErr = err
	}
}

func (s *streamServer) sessionURL(session *Session) string {
	return (&url.URL{
		Scheme:   "ws",
		Host:     s.listener.Addr().String(),
		Path:     streamPathPrefix + session.ID(),
		RawQuery: url.Values{"token": []string{session.StreamToken()}}.Encode(),
	}).String()
}

func (s *streamServer) handle(response http.ResponseWriter, request *http.Request) {
	if !s.beginHandler() {
		http.Error(response, "terminal stream server is stopping", http.StatusServiceUnavailable)
		return
	}
	var (
		connection *websocket.Conn
		sessionID  string
		claimed    bool
	)
	defer func() {
		if connection != nil {
			s.unregister(connection)
			_ = connection.Close()
		}
		if claimed {
			_ = s.manager.CloseSession(sessionID, false)
		}
		s.handlers.Done()
	}()

	if request.Method != http.MethodGet || !allowedStreamOrigin(request.Header.Get("Origin")) {
		http.Error(response, "terminal stream rejected", http.StatusForbidden)
		return
	}
	sessionID = strings.TrimPrefix(request.URL.Path, streamPathPrefix)
	if sessionID == "" || strings.Contains(sessionID, "/") {
		http.NotFound(response, request)
		return
	}
	session, err := s.manager.Get(sessionID)
	if err != nil {
		http.NotFound(response, request)
		return
	}
	token := request.URL.Query().Get("token")
	if token == "" || subtle.ConstantTimeCompare([]byte(token), []byte(session.StreamToken())) != 1 {
		http.Error(response, "terminal stream rejected", http.StatusUnauthorized)
		return
	}
	startup, live, err := session.attachOutput()
	if err != nil {
		http.Error(response, "terminal stream unavailable", http.StatusConflict)
		return
	}
	claimed = true

	connection, err = s.upgrader.Upgrade(response, request, nil)
	if err != nil {
		return
	}
	s.register(connection)
	s.runConnection(connection, session, startup, live)
}

func (s *streamServer) runConnection(
	connection *websocket.Conn,
	session *Session,
	startup []byte,
	live <-chan []byte,
) {
	ctx, cancel := context.WithCancel(s.ctx)
	defer cancel()

	ledger := newFlowLedger(outputWindowBytes)
	results := make(chan error, 2)
	go func() {
		results <- writeTerminalStream(ctx, connection, ledger, startup, live)
	}()
	go func() {
		results <- readTerminalStream(connection, session, ledger)
	}()
	<-results
	cancel()
	_ = connection.Close()
	_ = s.manager.CloseSession(session.ID(), false)
	<-results
}

func writeTerminalStream(
	ctx context.Context,
	connection *websocket.Conn,
	ledger *flowLedger,
	startup []byte,
	live <-chan []byte,
) error {
	for _, chunk := range splitOutput(startup) {
		if err := ledger.reservePending(ctx, len(chunk)); err != nil {
			return err
		}
		if err := ledger.commit(len(chunk), func() error {
			return writeStreamMessage(connection, websocket.BinaryMessage, chunk)
		}); err != nil {
			return err
		}
	}

	ping := time.NewTicker(streamPingEvery)
	defer ping.Stop()
	for {
		if err := reserveOutputWithPing(ctx, connection, ledger, ping); err != nil {
			return err
		}
		select {
		case chunk, ok := <-live:
			if !ok {
				ledger.release(outputChunkBytes)
				_ = connection.SetWriteDeadline(time.Now().Add(streamWriteWait))
				return connection.WriteControl(
					websocket.CloseMessage,
					websocket.FormatCloseMessage(websocket.CloseNormalClosure, ""),
					time.Now().Add(streamWriteWait),
				)
			}
			if len(chunk) == 0 || len(chunk) > outputChunkBytes {
				ledger.release(outputChunkBytes)
				return errors.New("invalid terminal output chunk")
			}
			ledger.release(outputChunkBytes - len(chunk))
			if err := ledger.commit(len(chunk), func() error {
				return writeStreamMessage(connection, websocket.BinaryMessage, chunk)
			}); err != nil {
				return err
			}
		case <-ping.C:
			ledger.release(outputChunkBytes)
			if err := connection.SetWriteDeadline(time.Now().Add(streamWriteWait)); err != nil {
				return err
			}
			if err := connection.WriteMessage(websocket.PingMessage, nil); err != nil {
				return err
			}
		case <-ctx.Done():
			ledger.release(outputChunkBytes)
			return ctx.Err()
		}
	}
}

func reserveOutputWithPing(
	ctx context.Context,
	connection *websocket.Conn,
	ledger *flowLedger,
	ping *time.Ticker,
) error {
	for {
		if ledger.tryReservePending(outputChunkBytes) {
			return nil
		}
		select {
		case <-ledger.changed:
		case <-ping.C:
			if err := connection.SetWriteDeadline(time.Now().Add(streamWriteWait)); err != nil {
				return err
			}
			if err := connection.WriteMessage(websocket.PingMessage, nil); err != nil {
				return err
			}
		case <-ctx.Done():
			return ctx.Err()
		}
	}
}

func readTerminalStream(
	connection *websocket.Conn,
	session *Session,
	ledger *flowLedger,
) error {
	connection.SetReadLimit(maxInputFrameBytes)
	if err := connection.SetReadDeadline(time.Now().Add(streamPongWait)); err != nil {
		return err
	}
	connection.SetPongHandler(func(string) error {
		return connection.SetReadDeadline(time.Now().Add(streamPongWait))
	})

	for {
		messageType, payload, err := connection.ReadMessage()
		if err != nil {
			return err
		}
		switch messageType {
		case websocket.BinaryMessage:
			if len(payload) == 0 || len(payload) > maxInputFrameBytes {
				return errors.New("invalid terminal input frame size")
			}
			if err := session.WriteInput(payload); err != nil {
				return err
			}
		case websocket.TextMessage:
			acknowledged, err := parseACKControl(payload)
			if err != nil {
				return err
			}
			if err := ledger.acknowledge(acknowledged); err != nil {
				return err
			}
		default:
			return errors.New("unsupported terminal stream frame")
		}
	}
}

func writeStreamMessage(connection *websocket.Conn, messageType int, payload []byte) error {
	if err := connection.SetWriteDeadline(time.Now().Add(streamWriteWait)); err != nil {
		return err
	}
	return connection.WriteMessage(messageType, payload)
}

func allowedStreamOrigin(rawOrigin string) bool {
	if rawOrigin == "" || rawOrigin == "null" {
		return false
	}
	origin, err := url.Parse(rawOrigin)
	if err != nil || origin.User != nil {
		return false
	}
	if origin.Scheme == "wails" && origin.Host == "wails" {
		return true
	}
	if (origin.Scheme == "http" || origin.Scheme == "https") && origin.Hostname() == "wails.localhost" {
		return true
	}
	if origin.Scheme != "http" && origin.Scheme != "https" {
		return false
	}
	host := origin.Hostname()
	if strings.EqualFold(host, "localhost") {
		return true
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}

func (s *streamServer) register(connection *websocket.Conn) {
	s.mu.Lock()
	s.conns[connection] = struct{}{}
	s.mu.Unlock()
}

func (s *streamServer) beginHandler() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.stopping {
		return false
	}
	s.handlers.Add(1)
	return true
}

func (s *streamServer) unregister(connection *websocket.Conn) {
	s.mu.Lock()
	delete(s.conns, connection)
	s.mu.Unlock()
}

func (s *streamServer) Shutdown() error {
	s.shutdownOnce.Do(func() {
		s.cancel()
		s.mu.Lock()
		s.stopping = true
		connections := make([]*websocket.Conn, 0, len(s.conns))
		for connection := range s.conns {
			connections = append(connections, connection)
		}
		s.mu.Unlock()
		for _, connection := range connections {
			_ = connection.Close()
		}
		closeErr := s.httpServer.Close()
		s.handlers.Wait()
		<-s.serveDone
		s.shutdownErr = errors.Join(closeErr, s.serveErr)
		close(s.shutdownDone)
	})
	<-s.shutdownDone
	return s.shutdownErr
}
