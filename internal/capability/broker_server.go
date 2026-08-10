package capability

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
)

const (
	brokerDescriptorName = "capability-broker.json"
	maxBrokerBodyBytes   = 48 << 20
)

// BrokerDescriptor lets a token-bearing child process locate its project host
// broker. It deliberately contains no registration or authorization secret.
type BrokerDescriptor struct {
	Version     int    `json:"version"`
	ProjectRoot string `json:"project_root"`
	Generation  uint64 `json:"generation"`
	URL         string `json:"url"`
	PID         int    `json:"pid"`
}

// BrokerServerConfig identifies one generation-scoped project broker.
type BrokerServerConfig struct {
	GlobalHome  string
	ProjectRoot string
	DBPath      string
	Generation  uint64
}

// BrokerServer hosts the capability broker on authenticated loopback HTTP.
type BrokerServer struct {
	Broker         *Broker
	globalHome     string
	projectRoot    string
	descriptor     BrokerDescriptor
	descriptorPath string
	listener       net.Listener
	server         *http.Server

	shutdownOnce sync.Once
	shutdownErr  error
	serveMu      sync.Mutex
	serveErr     error
}

// StartBrokerServer starts and publishes a generation-scoped host broker.
func StartBrokerServer(config BrokerServerConfig) (*BrokerServer, error) {
	broker, err := NewBroker(config.ProjectRoot, config.DBPath, config.Generation)
	if err != nil {
		return nil, err
	}
	listener, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		_ = broker.Shutdown(context.Background())
		return nil, err
	}
	server := &BrokerServer{
		Broker: broker, globalHome: config.GlobalHome, projectRoot: broker.projectRoot, listener: listener,
	}
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/tools/call", server.handleCall)
	mux.HandleFunc("/v1/tools/list", server.handleList)
	server.server = &http.Server{
		Handler: mux, ReadHeaderTimeout: 5 * time.Second, ReadTimeout: 5 * time.Minute,
		WriteTimeout: 5 * time.Minute, IdleTimeout: 15 * time.Second,
	}
	descriptor := BrokerDescriptor{
		Version: 1, ProjectRoot: broker.projectRoot, Generation: config.Generation,
		URL: "http://" + listener.Addr().String(), PID: os.Getpid(),
	}
	descriptorPath, err := agentrun.PublishRuntimeJSON(config.GlobalHome, broker.projectRoot, brokerDescriptorName, descriptor)
	if err != nil {
		_ = listener.Close()
		_ = broker.Shutdown(context.Background())
		return nil, err
	}
	server.descriptor = descriptor
	server.descriptorPath = descriptorPath
	go func() {
		err := server.server.Serve(listener)
		if err != nil && !errors.Is(err, http.ErrServerClosed) && !errors.Is(err, net.ErrClosed) {
			server.serveMu.Lock()
			server.serveErr = err
			server.serveMu.Unlock()
		}
	}()
	return server, nil
}

// Descriptor returns the published locator for tests and clients.
func (s *BrokerServer) Descriptor() BrokerDescriptor {
	return s.descriptor
}

func (s *BrokerServer) handleList(response http.ResponseWriter, request *http.Request) {
	if _, _, ok := s.accept(response, request); !ok {
		return
	}
	writeBrokerJSON(response, http.StatusOK, map[string]any{"tools": ToolDefinitions()})
}

func (s *BrokerServer) handleCall(response http.ResponseWriter, request *http.Request) {
	token, ctx, ok := s.accept(response, request)
	if !ok {
		return
	}
	request.Body = http.MaxBytesReader(response, request.Body, maxBrokerBodyBytes)
	decoder := json.NewDecoder(request.Body)
	decoder.DisallowUnknownFields()
	var call ToolCall
	if err := decoder.Decode(&call); err != nil {
		http.Error(response, "capability request is invalid", http.StatusBadRequest)
		return
	}
	var trailing any
	if err := decoder.Decode(&trailing); !errors.Is(err, io.EOF) {
		http.Error(response, "capability request is invalid", http.StatusBadRequest)
		return
	}
	result, err := s.Broker.Call(ctx, token, call)
	if err != nil {
		writeBrokerJSON(response, http.StatusForbidden, map[string]any{"error": err.Error()})
		return
	}
	writeBrokerJSON(response, http.StatusOK, map[string]any{"result": result})
}

func (s *BrokerServer) accept(response http.ResponseWriter, request *http.Request) (string, context.Context, bool) {
	if request.Method != http.MethodPost || request.Header.Get("Origin") != "" {
		http.Error(response, "capability request rejected", http.StatusForbidden)
		return "", nil, false
	}
	token := strings.TrimSpace(strings.TrimPrefix(request.Header.Get("Authorization"), "Bearer "))
	if token == "" {
		http.Error(response, "capability session rejected", http.StatusUnauthorized)
		return "", nil, false
	}
	if _, _, err := s.Broker.authenticate(token); err != nil {
		http.Error(response, "capability session rejected", http.StatusUnauthorized)
		return "", nil, false
	}
	return token, request.Context(), true
}

func writeBrokerJSON(response http.ResponseWriter, status int, value any) {
	response.Header().Set("Content-Type", "application/json")
	response.WriteHeader(status)
	_ = json.NewEncoder(response).Encode(value)
}

// Shutdown invalidates sessions, stops the listener, and removes the locator.
func (s *BrokerServer) Shutdown(ctx context.Context) error {
	s.shutdownOnce.Do(func() {
		s.serveMu.Lock()
		serveErr := s.serveErr
		s.serveMu.Unlock()
		brokerErr := s.Broker.Shutdown(ctx)
		serverErr := s.server.Shutdown(ctx)
		removeErr := agentrun.RemoveRuntimeJSONIfEqual(s.globalHome, s.projectRoot, brokerDescriptorName, s.descriptor)
		s.shutdownErr = errors.Join(serveErr, brokerErr, serverErr, removeErr)
	})
	return s.shutdownErr
}

// ReadBrokerDescriptor locates and validates the active project broker.
func ReadBrokerDescriptor(globalHome, projectRoot string) (BrokerDescriptor, error) {
	canonicalRoot, err := filepath.EvalSymlinks(projectRoot)
	if err != nil {
		return BrokerDescriptor{}, err
	}
	canonicalRoot, err = filepath.Abs(canonicalRoot)
	if err != nil {
		return BrokerDescriptor{}, err
	}
	directory, err := agentrun.RuntimeDir(globalHome, canonicalRoot)
	if err != nil {
		return BrokerDescriptor{}, err
	}
	contents, err := os.ReadFile(filepath.Join(directory, brokerDescriptorName))
	if err != nil {
		return BrokerDescriptor{}, err
	}
	var descriptor BrokerDescriptor
	if err := json.Unmarshal(contents, &descriptor); err != nil {
		return BrokerDescriptor{}, err
	}
	if descriptor.Version != 1 || descriptor.ProjectRoot != canonicalRoot || !agentrun.ProcessAlive(descriptor.PID) {
		return BrokerDescriptor{}, errors.New("capability broker descriptor is stale or belongs to another project")
	}
	u, err := url.Parse(descriptor.URL)
	if err != nil || u.Scheme != "http" || u.User != nil || u.Path != "" || net.ParseIP(u.Hostname()) == nil || !net.ParseIP(u.Hostname()).IsLoopback() {
		return BrokerDescriptor{}, errors.New("capability broker descriptor URL is invalid")
	}
	return descriptor, nil
}

// BrokerClient forwards token-authenticated tool calls to the active host.
type BrokerClient struct {
	Descriptor BrokerDescriptor
	HTTPClient *http.Client
}

// Call forwards one tool call and returns its raw structured result.
func (c BrokerClient) Call(ctx context.Context, token string, call ToolCall) (json.RawMessage, error) {
	body, err := json.Marshal(call)
	if err != nil {
		return nil, err
	}
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, c.Descriptor.URL+"/v1/tools/call", bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	request.Header.Set("Authorization", "Bearer "+token)
	request.Header.Set("Content-Type", "application/json")
	client := c.HTTPClient
	if client == nil {
		client = &http.Client{}
	}
	response, err := client.Do(request)
	if err != nil {
		return nil, err
	}
	defer response.Body.Close()
	contents, err := io.ReadAll(io.LimitReader(response.Body, maxBrokerBodyBytes+1))
	if err != nil {
		return nil, err
	}
	if len(contents) > maxBrokerBodyBytes {
		return nil, errors.New("capability broker response is too large")
	}
	var envelope struct {
		Result json.RawMessage `json:"result"`
		Error  string          `json:"error"`
	}
	if err := json.Unmarshal(contents, &envelope); err != nil {
		return nil, err
	}
	if response.StatusCode != http.StatusOK {
		if envelope.Error == "" {
			envelope.Error = "capability broker rejected the request"
		}
		return nil, errors.New(envelope.Error)
	}
	return envelope.Result, nil
}

// ClientForProject loads the active descriptor for a canonical project.
func ClientForProject(globalHome, projectRoot string) (BrokerClient, error) {
	descriptor, err := ReadBrokerDescriptor(globalHome, projectRoot)
	if err != nil {
		return BrokerClient{}, err
	}
	return BrokerClient{Descriptor: descriptor}, nil
}

// ValidateSessionEnvironment ensures a CLI bridge is using the project and
// generation injected by the host that launched it.
func ValidateSessionEnvironment(descriptor BrokerDescriptor) error {
	if expected := os.Getenv("PTRACK_CAPABILITY_PROJECT"); expected != "" {
		root, err := filepath.EvalSymlinks(expected)
		if err != nil {
			return err
		}
		root, _ = filepath.Abs(root)
		if root != descriptor.ProjectRoot {
			return fmt.Errorf("capability broker project does not match the launched session")
		}
	}
	if expected := os.Getenv("PTRACK_CAPABILITY_GENERATION"); expected != "" && expected != fmt.Sprint(descriptor.Generation) {
		return fmt.Errorf("capability broker generation does not match the launched session")
	}
	return nil
}
