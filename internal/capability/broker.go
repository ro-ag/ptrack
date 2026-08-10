package capability

import (
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"path/filepath"
	"sync"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

const (
	ToolHTTPRequest = "ptrack_http_request"
	ToolGit         = "ptrack_git"
	ToolSSH         = "ptrack_ssh"
)

// ToolDefinition is the provider-compatible MCP tool description.
type ToolDefinition struct {
	Name        string         `json:"name"`
	Title       string         `json:"title,omitempty"`
	Description string         `json:"description"`
	InputSchema map[string]any `json:"inputSchema"`
	Annotations map[string]any `json:"annotations,omitempty"`
}

// ToolCall is one typed broker dispatch request.
type ToolCall struct {
	Name      string          `json:"name"`
	Arguments json.RawMessage `json:"arguments"`
}

// ToolDefinitions returns the stable provider-facing capability tools.
func ToolDefinitions() []ToolDefinition {
	object := func(properties map[string]any, required ...string) map[string]any {
		return map[string]any{"type": "object", "properties": properties, "additionalProperties": false, "required": required}
	}
	id := map[string]any{"type": "integer", "minimum": 1}
	text := map[string]any{"type": "string"}
	requestObject := func(properties map[string]any, required ...string) map[string]any {
		return object(properties, required...)
	}
	annotations := map[string]any{"destructiveHint": true, "openWorldHint": true}
	return []ToolDefinition{
		{
			Name: ToolHTTPRequest, Title: "p-track HTTP capability",
			Description: "Make an explicitly approved bounded HTTP request",
			InputSchema: object(map[string]any{
				"capability_id": id,
				"request": requestObject(map[string]any{
					"method": text, "url": text,
					"headers": map[string]any{"type": "object", "additionalProperties": map[string]any{"type": "array", "items": text}},
					"body":    map[string]any{"type": "string", "contentEncoding": "base64"},
				}, "method", "url"),
			}, "capability_id", "request"),
			Annotations: annotations,
		},
		{
			Name: ToolGit, Title: "p-track Git capability",
			Description: "Run an explicitly approved fixed Git operation",
			InputSchema: object(map[string]any{
				"capability_id": id, "ssh_capability_id": id,
				"request": requestObject(map[string]any{
					"operation": text, "branch": text, "refspec": text, "force": map[string]any{"type": "boolean"},
				}, "operation"),
			}, "capability_id", "request"),
			Annotations: annotations,
		},
		{
			Name: ToolSSH, Title: "p-track SSH capability",
			Description: "Run an explicitly approved fixed SSH operation",
			InputSchema: object(map[string]any{
				"capability_id": id,
				"request": requestObject(map[string]any{
					"operation": text, "command": text, "local_path": text, "remote_path": text,
					"forward_target": text, "listen_port": map[string]any{"type": "integer", "minimum": 1, "maximum": 65535},
				}, "operation"),
			}, "capability_id", "request"),
			Annotations: annotations,
		},
	}
}

// SessionIdentity is minted by the host and immutable for a terminal run.
type SessionIdentity struct {
	Profile     string
	ProjectRoot string
	Generation  uint64
	SessionID   string
}

type sessionGrant struct {
	identity SessionIdentity
	ctx      context.Context
	cancel   context.CancelFunc
}

// Broker authenticates host-minted terminal sessions and dispatches all tools
// through one policy/executor path.
type Broker struct {
	projectRoot string
	dbPath      string
	generation  uint64

	ctx    context.Context
	cancel context.CancelFunc

	mu         sync.Mutex
	sessions   map[[32]byte]*sessionGrant
	active     map[uint64]map[uint64]context.CancelFunc
	nextActive uint64

	HTTP HTTPExecutor
	Git  GitExecutor
	SSH  SSHExecutor
}

// NewBroker creates a generation-bound broker for one canonical project.
func NewBroker(projectRoot, dbPath string, generation uint64) (*Broker, error) {
	root, err := filepath.EvalSymlinks(projectRoot)
	if err != nil {
		return nil, err
	}
	root, err = filepath.Abs(root)
	if err != nil {
		return nil, err
	}
	ctx, cancel := context.WithCancel(context.Background())
	return &Broker{
		projectRoot: root, dbPath: dbPath, generation: generation,
		ctx: ctx, cancel: cancel, sessions: make(map[[32]byte]*sessionGrant),
		active: make(map[uint64]map[uint64]context.CancelFunc),
	}, nil
}

// IssueSessionToken mints an opaque token bound to one immutable profile,
// project, and generation. The raw token is returned once for environment
// injection and is never persisted.
func (b *Broker) IssueSessionToken(profile string) (string, error) {
	profile, err := normalizeProfile(profile)
	if err != nil {
		return "", err
	}
	raw := make([]byte, 32)
	if _, err := rand.Read(raw); err != nil {
		return "", err
	}
	token := base64.RawURLEncoding.EncodeToString(raw)
	hash := sha256.Sum256([]byte(token))
	ctx, cancel := context.WithCancel(b.ctx)
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.ctx.Err() != nil {
		cancel()
		return "", errors.New("capability broker is closed")
	}
	b.sessions[hash] = &sessionGrant{
		identity: SessionIdentity{Profile: profile, ProjectRoot: b.projectRoot, Generation: b.generation},
		ctx:      ctx, cancel: cancel,
	}
	return token, nil
}

// BindSession attaches the host terminal identity after process launch.
func (b *Broker) BindSession(token, sessionID string) error {
	hash := sha256.Sum256([]byte(token))
	b.mu.Lock()
	defer b.mu.Unlock()
	grant := b.sessions[hash]
	if grant == nil || grant.identity.SessionID != "" || sessionID == "" {
		return errors.New("capability session token cannot be bound")
	}
	grant.identity.SessionID = sessionID
	return nil
}

// RevokeToken invalidates an unbound or failed launch token.
func (b *Broker) RevokeToken(token string) {
	hash := sha256.Sum256([]byte(token))
	b.mu.Lock()
	if grant := b.sessions[hash]; grant != nil {
		grant.cancel()
		delete(b.sessions, hash)
	}
	b.mu.Unlock()
}

// RevokeSession invalidates a terminal token and cancels its in-flight calls.
func (b *Broker) RevokeSession(sessionID string) {
	b.mu.Lock()
	for hash, grant := range b.sessions {
		if grant.identity.SessionID == sessionID {
			grant.cancel()
			delete(b.sessions, hash)
		}
	}
	b.mu.Unlock()
}

// RevokeCapability cancels in-flight operations after disable/edit/removal.
func (b *Broker) RevokeCapability(capabilityID uint64) {
	b.mu.Lock()
	for _, cancel := range b.active[capabilityID] {
		cancel()
	}
	delete(b.active, capabilityID)
	b.mu.Unlock()
}

// Call authenticates and dispatches one provider tool call.
func (b *Broker) Call(ctx context.Context, token string, call ToolCall) (any, error) {
	identity, sessionCtx, err := b.authenticate(token)
	if err != nil {
		return nil, err
	}
	callCtx, cancel := context.WithCancel(ctx)
	stopBroker := context.AfterFunc(b.ctx, cancel)
	stopSession := context.AfterFunc(sessionCtx, cancel)
	defer func() {
		stopBroker()
		stopSession()
		cancel()
	}()

	switch call.Name {
	case ToolHTTPRequest:
		var arguments struct {
			CapabilityID uint64      `json:"capability_id"`
			Request      HTTPRequest `json:"request"`
		}
		if err := decodeToolArguments(call.Arguments, &arguments); err != nil {
			return nil, err
		}
		loaded, err := loadBrokerCapabilities(b.dbPath, arguments.CapabilityID)
		if err != nil {
			return nil, err
		}
		capability := loaded[arguments.CapabilityID]
		release, err := b.track(arguments.CapabilityID, capability.Limits.MaxConcurrent, cancel)
		if err != nil {
			return nil, err
		}
		defer release()
		loaded, err = loadBrokerCapabilities(b.dbPath, arguments.CapabilityID)
		if err != nil {
			return nil, err
		}
		capability = loaded[arguments.CapabilityID]
		executor := b.HTTP
		executor.Recorder = Recorder{Store: brokerAuditStore{dbPath: b.dbPath}}
		return executor.Execute(callCtx, capability, identity.Profile, arguments.Request)
	case ToolGit:
		var arguments struct {
			CapabilityID    uint64     `json:"capability_id"`
			SSHCapabilityID uint64     `json:"ssh_capability_id,omitempty"`
			Request         GitRequest `json:"request"`
		}
		if err := decodeToolArguments(call.Arguments, &arguments); err != nil {
			return nil, err
		}
		ids := []uint64{arguments.CapabilityID}
		if arguments.SSHCapabilityID != 0 {
			ids = append(ids, arguments.SSHCapabilityID)
		}
		loaded, err := loadBrokerCapabilities(b.dbPath, ids...)
		if err != nil {
			return nil, err
		}
		capability := loaded[arguments.CapabilityID]
		var sshCapability *model.Capability
		if arguments.SSHCapabilityID != 0 {
			ssh := loaded[arguments.SSHCapabilityID]
			sshCapability = &ssh
		}
		release, err := b.track(arguments.CapabilityID, capability.Limits.MaxConcurrent, cancel)
		if err != nil {
			return nil, err
		}
		defer release()
		if sshCapability != nil {
			releaseSSH, trackErr := b.track(arguments.SSHCapabilityID, sshCapability.Limits.MaxConcurrent, cancel)
			if trackErr != nil {
				return nil, trackErr
			}
			defer releaseSSH()
		}
		loaded, err = loadBrokerCapabilities(b.dbPath, ids...)
		if err != nil {
			return nil, err
		}
		capability = loaded[arguments.CapabilityID]
		if arguments.SSHCapabilityID != 0 {
			ssh := loaded[arguments.SSHCapabilityID]
			sshCapability = &ssh
		}
		executor := b.Git
		executor.Recorder = Recorder{Store: brokerAuditStore{dbPath: b.dbPath}}
		return executor.Execute(callCtx, capability, sshCapability, identity.Profile, identity.ProjectRoot, arguments.Request)
	case ToolSSH:
		var arguments struct {
			CapabilityID uint64     `json:"capability_id"`
			Request      SSHRequest `json:"request"`
		}
		if err := decodeToolArguments(call.Arguments, &arguments); err != nil {
			return nil, err
		}
		if arguments.Request.Operation == SSHInteractiveShell {
			return nil, ErrDenied{Reason: "interactive SSH is unavailable over the MCP transport"}
		}
		loaded, err := loadBrokerCapabilities(b.dbPath, arguments.CapabilityID)
		if err != nil {
			return nil, err
		}
		capability := loaded[arguments.CapabilityID]
		release, err := b.track(arguments.CapabilityID, capability.Limits.MaxConcurrent, cancel)
		if err != nil {
			return nil, err
		}
		defer release()
		loaded, err = loadBrokerCapabilities(b.dbPath, arguments.CapabilityID)
		if err != nil {
			return nil, err
		}
		capability = loaded[arguments.CapabilityID]
		executor := b.SSH
		executor.Recorder = Recorder{Store: brokerAuditStore{dbPath: b.dbPath}}
		return executor.Execute(callCtx, capability, identity.Profile, identity.ProjectRoot, arguments.Request)
	default:
		return nil, ErrDenied{Reason: fmt.Sprintf("unknown capability tool %q", call.Name)}
	}
}

func loadBrokerCapabilities(dbPath string, ids ...uint64) (map[uint64]model.Capability, error) {
	s, err := store.Open(dbPath)
	if err != nil {
		return nil, err
	}
	loaded := make(map[uint64]model.Capability, len(ids))
	for _, id := range ids {
		capability, getErr := s.GetCapability(id)
		if getErr != nil {
			return nil, errors.Join(getErr, s.Close())
		}
		loaded[id] = capability
	}
	if err := s.Close(); err != nil {
		return nil, err
	}
	return loaded, nil
}

type brokerAuditStore struct{ dbPath string }

func (s brokerAuditStore) AddCapabilityAuditBounded(
	audit model.CapabilityAudit,
	keepForCapability, keepGlobal int,
) (model.CapabilityAudit, error) {
	database, err := store.Open(s.dbPath)
	if err != nil {
		return model.CapabilityAudit{}, err
	}
	created, addErr := database.AddCapabilityAuditBounded(audit, keepForCapability, keepGlobal)
	return created, errors.Join(addErr, database.Close())
}

func (b *Broker) authenticate(token string) (SessionIdentity, context.Context, error) {
	if token == "" {
		return SessionIdentity{}, nil, ErrDenied{Reason: "capability session token is required"}
	}
	hash := sha256.Sum256([]byte(token))
	b.mu.Lock()
	defer b.mu.Unlock()
	grant := b.sessions[hash]
	if grant == nil || grant.identity.SessionID == "" || grant.ctx.Err() != nil {
		return SessionIdentity{}, nil, ErrDenied{Reason: "capability session token is invalid or stale"}
	}
	return grant.identity, grant.ctx, nil
}

func (b *Broker) track(capabilityID uint64, maximum int, cancel context.CancelFunc) (func(), error) {
	if maximum < 1 {
		maximum = defaultConcurrent
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.active[capabilityID] == nil {
		b.active[capabilityID] = make(map[uint64]context.CancelFunc)
	}
	if len(b.active[capabilityID]) >= maximum {
		return nil, ErrDenied{Reason: "capability concurrency limit reached"}
	}
	b.nextActive++
	operationID := b.nextActive
	b.active[capabilityID][operationID] = cancel
	return func() {
		b.mu.Lock()
		delete(b.active[capabilityID], operationID)
		if len(b.active[capabilityID]) == 0 {
			delete(b.active, capabilityID)
		}
		b.mu.Unlock()
	}, nil
}

// Shutdown invalidates every session and cancels every in-flight operation.
func (b *Broker) Shutdown(context.Context) error {
	b.cancel()
	b.mu.Lock()
	for _, grant := range b.sessions {
		grant.cancel()
	}
	clear(b.sessions)
	for _, operations := range b.active {
		for _, cancel := range operations {
			cancel()
		}
	}
	clear(b.active)
	b.mu.Unlock()
	return nil
}

func decodeToolArguments(raw json.RawMessage, target any) error {
	if len(raw) == 0 {
		return errors.New("tool arguments are required")
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(target); err != nil {
		return fmt.Errorf("invalid tool arguments: %w", err)
	}
	var trailing any
	if err := decoder.Decode(&trailing); !errors.Is(err, io.EOF) {
		return errors.New("tool arguments contain trailing data")
	}
	return nil
}
