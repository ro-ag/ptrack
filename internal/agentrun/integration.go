package agentrun

import (
	"context"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
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
)

type IntegrationConfig struct {
	GlobalHome  string
	ProjectRoot string
	Generation  uint64
}

type IntegrationDescriptor struct {
	ProjectRoot       string `json:"projectRoot"`
	URL               string `json:"url"`
	Generation        uint64 `json:"generation"`
	RegistrationToken string `json:"registrationToken"`
}

type IntegrationServer struct {
	registry       *Registry
	projectRoot    string
	generation     uint64
	token          string
	listener       net.Listener
	httpServer     *http.Server
	descriptorPath string

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
		registry:     registry,
		projectRoot:  filepath.Clean(projectRoot),
		generation:   config.Generation,
		token:        token,
		listener:     listener,
		serveDone:    make(chan struct{}),
		shutdownDone: make(chan struct{}),
	}
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/runs/register", server.handleRegister)
	mux.HandleFunc("/v1/runs/", server.handleRun)
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
		PlanID   uint64 `json:"planId"`
		TaskID   uint64 `json:"taskId"`
		CWD      string `json:"cwd"`
	}
	if !decodeIntegrationJSON(response, request, &registration) {
		return
	}
	lease, err := s.registry.RegisterExternal(Registration{
		Profile:  registration.Profile,
		Provider: registration.Provider,
		PID:      registration.PID,
		PlanID:   registration.PlanID,
		TaskID:   registration.TaskID,
		CWD:      registration.CWD,
	})
	if err != nil {
		http.Error(response, "AgentRun registration rejected", http.StatusBadRequest)
		return
	}
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
	default:
		http.NotFound(response, request)
		return
	}
	response.WriteHeader(http.StatusNoContent)
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
	hash := sha256.Sum256([]byte(descriptor.ProjectRoot))
	runtimeDir := filepath.Join(
		globalHome,
		"runtime",
		hex.EncodeToString(hash[:]),
	)
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
