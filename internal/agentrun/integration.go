package agentrun

import (
	"context"
	"crypto/subtle"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

const (
	maxIntegrationBodyBytes = 16 * 1024
	integrationReadTimeout  = 5 * time.Second
	integrationWriteTimeout = 5 * time.Second
	launchedEventBindWait   = 2 * time.Second
)

type IntegrationConfig struct {
	GlobalHome  string
	ProjectRoot string
	Generation  uint64
	// RuntimeChanged is host-owned presentation invalidation. It conveys no
	// run data or authority and is invoked only after an accepted mutation.
	RuntimeChanged func()
}

type IntegrationDescriptor struct {
	ProjectRoot       string `json:"projectRoot"`
	URL               string `json:"url"`
	Generation        uint64 `json:"generation"`
	RegistrationToken string `json:"registrationToken"`
	// PID is the process hosting the integration server. Consumers use it for
	// a fast staleness check: after a crash the descriptor file can outlive
	// its server, and a dead PID means "do not dial, wait for a fresh
	// descriptor" instead of discovering connection-refused on a dead port.
	PID int `json:"pid"`
}

var (
	// ErrDescriptorNotFound is returned by ReadIntegrationDescriptor when no
	// integration server has published a descriptor for the project.
	ErrDescriptorNotFound = errors.New("AgentRun descriptor not found")
	// ErrDescriptorStale is returned by ReadIntegrationDescriptor when the
	// descriptor's owning process is gone (for example after a crash), so the
	// advertised URL would refuse connections.
	ErrDescriptorStale = errors.New("AgentRun descriptor is stale")
)

// ReadIntegrationDescriptor loads the descriptor an integration server
// published for the given project and verifies its owning process is still
// alive. It is the documented recovery path for consumers: read, check for
// ErrDescriptorStale (or ErrDescriptorNotFound), and only then dial the URL.
// PID reuse can theoretically fool the liveness check; the registration
// token and generation stay authoritative once a connection is made.
func ReadIntegrationDescriptor(
	globalHome string,
	projectRoot string,
) (IntegrationDescriptor, error) {
	runtimeDir, err := RuntimeDir(globalHome, projectRoot)
	if err != nil {
		return IntegrationDescriptor{}, err
	}
	path := filepath.Join(runtimeDir, "agent-registry.json")
	contents, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return IntegrationDescriptor{}, ErrDescriptorNotFound
	}
	if err != nil {
		return IntegrationDescriptor{}, fmt.Errorf("read AgentRun descriptor: %w", err)
	}
	var descriptor IntegrationDescriptor
	if err := json.Unmarshal(contents, &descriptor); err != nil {
		return IntegrationDescriptor{}, fmt.Errorf("decode AgentRun descriptor: %w", err)
	}
	if !ProcessAlive(descriptor.PID) {
		return IntegrationDescriptor{}, fmt.Errorf(
			"%w: owning process %d is not running", ErrDescriptorStale, descriptor.PID)
	}
	return descriptor, nil
}

type IntegrationServer struct {
	registry       *Registry
	projectRoot    string
	generation     uint64
	token          string
	listener       net.Listener
	httpServer     *http.Server
	descriptorPath string
	runtimeChanged func()

	serveDone chan struct{}
	serveErr  error

	shutdownOnce sync.Once
	shutdownDone chan struct{}
	shutdownErr  error
}

func StartIntegrationServer(
	registry *Registry,
	config IntegrationConfig,
) (*IntegrationServer, error) {
	if registry == nil {
		return nil, errors.New("AgentRun registry is required")
	}
	projectRoot, err := filepath.Abs(config.ProjectRoot)
	if err != nil {
		return nil, fmt.Errorf("resolve AgentRun project root: %w", err)
	}
	globalHome, err := filepath.Abs(config.GlobalHome)
	if err != nil {
		return nil, fmt.Errorf("resolve AgentRun runtime home: %w", err)
	}
	token, err := randomOpaqueValue()
	if err != nil {
		return nil, err
	}
	listener, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		return nil, fmt.Errorf("listen for AgentRun integration: %w", err)
	}
	server := &IntegrationServer{
		registry:       registry,
		projectRoot:    filepath.Clean(projectRoot),
		generation:     config.Generation,
		token:          token,
		listener:       listener,
		runtimeChanged: config.RuntimeChanged,
		serveDone:      make(chan struct{}),
		shutdownDone:   make(chan struct{}),
	}
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/runs/register", server.handleRegister)
	mux.HandleFunc("/v1/runs/", server.handleRun)
	mux.HandleFunc("/v1/events", server.handleLaunchedEvent)
	server.httpServer = &http.Server{
		Handler:           mux,
		ReadHeaderTimeout: integrationReadTimeout,
		ReadTimeout:       integrationReadTimeout,
		WriteTimeout:      integrationWriteTimeout,
		IdleTimeout:       15 * time.Second,
	}
	descriptorPath, err := writeIntegrationDescriptor(globalHome, IntegrationDescriptor{
		ProjectRoot:       server.projectRoot,
		URL:               "http://" + listener.Addr().String(),
		Generation:        config.Generation,
		RegistrationToken: token,
		PID:               os.Getpid(),
	})
	if err != nil {
		_ = listener.Close()
		return nil, err
	}
	server.descriptorPath = descriptorPath
	go server.serve()
	return server, nil
}

func (s *IntegrationServer) DescriptorPath() string {
	return s.descriptorPath
}

// EventEndpoint is the run-ID-free telemetry endpoint injected only into
// host-launched agent processes. Its token remains unusable until the host
// binds the successful launch to an AgentRun.
func (s *IntegrationServer) EventEndpoint() string {
	return "http://" + s.listener.Addr().String() + "/v1/events"
}

func (s *IntegrationServer) serve() {
	defer close(s.serveDone)
	err := s.httpServer.Serve(s.listener)
	if err != nil && !errors.Is(err, http.ErrServerClosed) &&
		!errors.Is(err, net.ErrClosed) {
		s.serveErr = err
	}
}

func (s *IntegrationServer) handleRegister(
	response http.ResponseWriter,
	request *http.Request,
) {
	if !s.acceptRequest(response, request, s.token) {
		return
	}
	var registration struct {
		Profile  string `json:"profile"`
		Provider string `json:"provider"`
		PID      int    `json:"pid"`
		CWD      string `json:"cwd"`
	}
	if !decodeIntegrationJSON(response, request, &registration) {
		return
	}
	lease, err := s.registry.RegisterExternal(Registration{
		Profile:  registration.Profile,
		Provider: registration.Provider,
		PID:      registration.PID,
		CWD:      registration.CWD,
	})
	if err != nil {
		http.Error(response, "AgentRun registration rejected", http.StatusBadRequest)
		return
	}
	s.notifyRuntimeChanged()
	response.Header().Set("Content-Type", "application/json")
	response.WriteHeader(http.StatusCreated)
	_ = json.NewEncoder(response).Encode(struct {
		ID         string `json:"id"`
		LeaseToken string `json:"leaseToken"`
	}{
		ID:         lease.Run.ID,
		LeaseToken: lease.LeaseToken,
	})
}

func (s *IntegrationServer) handleRun(
	response http.ResponseWriter,
	request *http.Request,
) {
	path := strings.TrimPrefix(request.URL.Path, "/v1/runs/")
	id, action, found := strings.Cut(path, "/")
	if !found || id == "" || strings.Contains(action, "/") {
		http.NotFound(response, request)
		return
	}
	token := bearerToken(request.Header.Get("Authorization"))
	if token == "" {
		http.Error(response, "AgentRun lease rejected", http.StatusUnauthorized)
		return
	}
	if request.Method != http.MethodPost || request.Header.Get("Origin") != "" {
		http.Error(response, "AgentRun request rejected", http.StatusForbidden)
		return
	}
	switch action {
	case "heartbeat":
		if err := s.registry.Heartbeat(id, token); err != nil {
			http.Error(response, "AgentRun lease rejected", http.StatusUnauthorized)
			return
		}
		s.notifyRuntimeChanged()
	case "exit":
		var result struct {
			Code   int    `json:"code"`
			Result string `json:"result"`
		}
		if !decodeIntegrationJSON(response, request, &result) {
			return
		}
		if err := s.registry.ExitExternal(id, token, result.Code, result.Result); err != nil {
			http.Error(response, "AgentRun lease rejected", http.StatusUnauthorized)
			return
		}
		s.notifyRuntimeChanged()
	case "events":
		if err := s.registry.AuthenticateEventLease(id, token); err != nil {
			http.Error(response, "AgentRun lease rejected", http.StatusUnauthorized)
			return
		}
		var providerEvent ProviderEvent
		if !decodeIntegrationJSON(response, request, &providerEvent) {
			return
		}
		event, err := s.registry.RecordProviderEvent(id, token, providerEvent)
		if err != nil {
			if errors.Is(err, ErrInvalidLease) || errors.Is(err, ErrRunNotFound) {
				http.Error(response, "AgentRun lease rejected", http.StatusUnauthorized)
			} else {
				http.Error(response, "AgentRun event rejected", http.StatusBadRequest)
			}
			return
		}
		s.notifyRuntimeChanged()
		writeEventReceipt(response, event)
		return
	default:
		http.NotFound(response, request)
		return
	}
	response.WriteHeader(http.StatusNoContent)
}

func (s *IntegrationServer) handleLaunchedEvent(
	response http.ResponseWriter,
	request *http.Request,
) {
	if request.Method != http.MethodPost || request.Header.Get("Origin") != "" {
		http.Error(response, "AgentRun request rejected", http.StatusForbidden)
		return
	}
	token := bearerToken(request.Header.Get("Authorization"))
	bindContext, cancelBind := context.WithTimeout(request.Context(), launchedEventBindWait)
	defer cancelBind()
	if token == "" || s.registry.AwaitLaunchedEventToken(bindContext, token) != nil {
		http.Error(response, "AgentRun event token rejected", http.StatusUnauthorized)
		return
	}
	var providerEvent ProviderEvent
	if !decodeIntegrationJSON(response, request, &providerEvent) {
		return
	}
	event, err := s.registry.RecordLaunchedProviderEvent(token, providerEvent)
	if err != nil {
		if errors.Is(err, ErrInvalidEventToken) || errors.Is(err, ErrRunNotFound) {
			http.Error(response, "AgentRun event token rejected", http.StatusUnauthorized)
		} else {
			http.Error(response, "AgentRun event rejected", http.StatusBadRequest)
		}
		return
	}
	s.notifyRuntimeChanged()
	writeEventReceipt(response, event)
}

func (s *IntegrationServer) notifyRuntimeChanged() {
	if s.runtimeChanged != nil {
		s.runtimeChanged()
	}
}

func writeEventReceipt(response http.ResponseWriter, event Event) {
	response.Header().Set("Content-Type", "application/json")
	response.WriteHeader(http.StatusCreated)
	_ = json.NewEncoder(response).Encode(struct {
		ID           string    `json:"id"`
		HostSequence uint64    `json:"hostSequence"`
		ObservedAt   time.Time `json:"observedAt"`
	}{
		ID:           event.ID,
		HostSequence: event.HostSequence,
		ObservedAt:   event.ObservedAt,
	})
}

func (s *IntegrationServer) acceptRequest(
	response http.ResponseWriter,
	request *http.Request,
	expectedToken string,
) bool {
	if request.Method != http.MethodPost || request.Header.Get("Origin") != "" {
		http.Error(response, "AgentRun request rejected", http.StatusForbidden)
		return false
	}
	token := bearerToken(request.Header.Get("Authorization"))
	if token == "" || subtle.ConstantTimeCompare([]byte(token), []byte(expectedToken)) != 1 {
		http.Error(response, "AgentRun request rejected", http.StatusUnauthorized)
		return false
	}
	return true
}

func bearerToken(authorization string) string {
	const prefix = "Bearer "
	if !strings.HasPrefix(authorization, prefix) {
		return ""
	}
	return strings.TrimSpace(strings.TrimPrefix(authorization, prefix))
}

func decodeIntegrationJSON(
	response http.ResponseWriter,
	request *http.Request,
	target any,
) bool {
	reader := io.LimitReader(request.Body, maxIntegrationBodyBytes+1)
	body, err := io.ReadAll(reader)
	if err != nil {
		http.Error(response, "invalid AgentRun request", http.StatusBadRequest)
		return false
	}
	if len(body) > maxIntegrationBodyBytes {
		http.Error(response, "AgentRun request too large", http.StatusRequestEntityTooLarge)
		return false
	}
	decoder := json.NewDecoder(strings.NewReader(string(body)))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(target); err != nil {
		http.Error(response, "invalid AgentRun request", http.StatusBadRequest)
		return false
	}
	if err := decoder.Decode(&struct{}{}); err != io.EOF {
		http.Error(response, "invalid AgentRun request", http.StatusBadRequest)
		return false
	}
	return true
}

func writeIntegrationDescriptor(
	globalHome string,
	descriptor IntegrationDescriptor,
) (string, error) {
	runtimeDir, err := RuntimeDir(globalHome, descriptor.ProjectRoot)
	if err != nil {
		return "", err
	}
	if err := preparePrivateRuntimeDir(runtimeDir); err != nil {
		return "", err
	}
	unlock, err := lockPrivateDescriptor(runtimeDir)
	if err != nil {
		return "", err
	}
	defer func() { _ = unlock() }()
	path := filepath.Join(runtimeDir, "agent-registry.json")
	tempToken, err := randomOpaqueValue()
	if err != nil {
		return "", err
	}
	tempPath := filepath.Join(runtimeDir, ".agent-registry-"+tempToken)
	file, err := openPrivateDescriptor(tempPath)
	if err != nil {
		return "", fmt.Errorf("create AgentRun descriptor: %w", err)
	}
	encodeErr := json.NewEncoder(file).Encode(descriptor)
	syncErr := file.Sync()
	closeErr := file.Close()
	if err := errors.Join(encodeErr, syncErr, closeErr); err != nil {
		_ = os.Remove(tempPath)
		return "", fmt.Errorf("write AgentRun descriptor: %w", err)
	}
	if err := replacePrivateDescriptor(tempPath, path); err != nil {
		_ = os.Remove(tempPath)
		return "", fmt.Errorf("publish AgentRun descriptor: %w", err)
	}
	if err := securePublishedDescriptor(path); err != nil {
		_ = os.Remove(path)
		return "", err
	}
	return path, nil
}

func (s *IntegrationServer) Shutdown(ctx context.Context) error {
	s.shutdownOnce.Do(func() {
		go s.runShutdown()
	})
	select {
	case <-s.shutdownDone:
		return s.shutdownErr
	case <-ctx.Done():
		return ctx.Err()
	}
}

func (s *IntegrationServer) runShutdown() {
	shutdownCtx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	shutdownErr := s.httpServer.Shutdown(shutdownCtx)
	cancel()
	closeErr := s.listener.Close()
	if errors.Is(closeErr, net.ErrClosed) {
		closeErr = nil
	}
	<-s.serveDone
	removeErr := removeOwnedIntegrationDescriptor(
		s.descriptorPath,
		s.token,
		s.generation,
	)
	s.shutdownErr = errors.Join(shutdownErr, closeErr, s.serveErr, removeErr)
	close(s.shutdownDone)
}

func removeOwnedIntegrationDescriptor(
	path string,
	token string,
	generation uint64,
) error {
	unlock, err := lockPrivateDescriptor(filepath.Dir(path))
	if err != nil {
		return err
	}
	defer func() { _ = unlock() }()
	contents, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("read AgentRun descriptor for cleanup: %w", err)
	}
	var descriptor IntegrationDescriptor
	if err := json.Unmarshal(contents, &descriptor); err != nil {
		// A descriptor not written by this server is never ours to remove.
		return nil
	}
	if descriptor.Generation != generation ||
		subtle.ConstantTimeCompare(
			[]byte(descriptor.RegistrationToken),
			[]byte(token),
		) != 1 {
		return nil
	}
	if err := os.Remove(path); err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return nil
		}
		return fmt.Errorf("remove AgentRun descriptor: %w", err)
	}
	return nil
}
