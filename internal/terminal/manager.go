package terminal

import (
	"context"
	"crypto/rand"
	"encoding/base64"
	"errors"
	"fmt"
	"os"
	"sync"
)

var (
	ErrManagerShutdown = errors.New("terminal manager is shut down")
	ErrProfileNotFound = errors.New("terminal profile not found")
	ErrSessionNotFound = errors.New("terminal session not found")
)

type Manager struct {
	projectRoot string
	factory     PTYFactory

	mu           sync.Mutex
	profiles     map[string]Profile
	sessions     map[string]*Session
	closing      map[string]*Session
	shuttingDown bool
	creates      sync.WaitGroup
	streamServer *streamServer

	shutdownOnce sync.Once
	shutdownDone chan struct{}
	shutdownErr  error
}

func NewManager(projectRoot string, profiles []Profile, factory PTYFactory) (*Manager, error) {
	if factory == nil {
		return nil, errors.New("terminal PTY factory is required")
	}
	canonicalRoot, err := resolveCWD(projectRoot, "")
	if err != nil {
		return nil, fmt.Errorf("resolve project root: %w", err)
	}

	profileMap := make(map[string]Profile, len(profiles))
	for _, source := range profiles {
		profile, validateErr := ValidateProfile(source)
		if validateErr != nil {
			return nil, fmt.Errorf("validate terminal profile %q: %w", source.ID, validateErr)
		}
		if _, exists := profileMap[profile.ID]; exists {
			return nil, fmt.Errorf("duplicate terminal profile ID %q", profile.ID)
		}
		profileMap[profile.ID] = profile
	}
	if len(profileMap) == 0 {
		return nil, errors.New("at least one terminal profile is required")
	}

	manager := &Manager{
		projectRoot:  canonicalRoot,
		factory:      factory,
		profiles:     profileMap,
		sessions:     make(map[string]*Session),
		closing:      make(map[string]*Session),
		shutdownDone: make(chan struct{}),
	}
	manager.streamServer, err = newStreamServer(manager)
	if err != nil {
		return nil, err
	}
	return manager, nil
}

func (m *Manager) Profiles() []Profile {
	m.mu.Lock()
	defer m.mu.Unlock()
	profiles := make([]Profile, 0, len(m.profiles))
	for _, profile := range m.profiles {
		profiles = append(profiles, cloneProfile(profile))
	}
	return profiles
}

func (m *Manager) Create(profileID, requestedCWD string, rows, columns int) (*Session, error) {
	return m.CreateWithEnv(profileID, requestedCWD, rows, columns, nil)
}

// CreateWithEnv starts a session with host-minted per-launch environment
// values layered over the immutable profile. It is used for capability tokens
// that must exist before the child process starts.
func (m *Manager) CreateWithEnv(
	profileID, requestedCWD string,
	rows, columns int,
	extraEnvironment map[string]string,
) (*Session, error) {
	m.mu.Lock()
	if m.shuttingDown {
		m.mu.Unlock()
		return nil, ErrManagerShutdown
	}
	profile, exists := m.profiles[profileID]
	if !exists {
		m.mu.Unlock()
		return nil, fmt.Errorf("%w: %s", ErrProfileNotFound, profileID)
	}
	profile = cloneProfile(profile)
	m.creates.Add(1)
	m.mu.Unlock()
	defer m.creates.Done()

	cwd, err := resolveCWD(m.projectRoot, requestedCWD)
	if err != nil {
		return nil, err
	}
	overrides := cloneEnvironment(profile.Env)
	for key, value := range extraEnvironment {
		if !safeEnvironmentEntry(key, value) {
			return nil, fmt.Errorf("unsafe per-launch environment override %q", key)
		}
		overrides[key] = value
	}
	environment, err := buildEnvironment(os.Environ(), overrides)
	if err != nil {
		return nil, err
	}
	id, err := randomOpaqueValue()
	if err != nil {
		return nil, fmt.Errorf("create terminal session ID: %w", err)
	}
	token, err := randomOpaqueValue()
	if err != nil {
		return nil, fmt.Errorf("create terminal stream token: %w", err)
	}

	session := newSession(StartRequest{
		Executable: profile.Executable,
		Args:       append([]string(nil), profile.Args...),
		Env:        environment,
		CWD:        cwd,
		Rows:       rows,
		Columns:    columns,
	}, sessionDependencies{
		factory:            m.factory,
		startupBufferBytes: defaultStartupBufferBytes,
		gracefulTimeout:    defaultGracefulTimeout,
	})
	session.setMetadata(id, token, profile.ID, profile.Kind, profile.Provider, cwd)
	if err := session.start(); err != nil {
		return nil, err
	}

	m.mu.Lock()
	if m.shuttingDown {
		m.mu.Unlock()
		closeErr := session.Close(true)
		return nil, errors.Join(ErrManagerShutdown, closeErr)
	}
	if _, collision := m.sessions[id]; collision {
		m.mu.Unlock()
		closeErr := session.Close(true)
		return nil, errors.Join(errors.New("terminal session ID collision"), closeErr)
	}
	m.sessions[id] = session
	m.mu.Unlock()
	return session, nil
}

func cloneEnvironment(source map[string]string) map[string]string {
	clone := make(map[string]string, len(source))
	for key, value := range source {
		clone[key] = value
	}
	return clone
}

func (m *Manager) Get(sessionID string) (*Session, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	session, exists := m.sessions[sessionID]
	if !exists {
		return nil, fmt.Errorf("%w: %s", ErrSessionNotFound, sessionID)
	}
	return session, nil
}

func (m *Manager) Resize(sessionID string, rows, columns int) error {
	session, err := m.Get(sessionID)
	if err != nil {
		return err
	}
	return session.Resize(rows, columns)
}

func (m *Manager) CloseSession(sessionID string, force bool) error {
	m.mu.Lock()
	session, exists := m.sessions[sessionID]
	if !exists {
		m.mu.Unlock()
		return fmt.Errorf("%w: %s", ErrSessionNotFound, sessionID)
	}
	delete(m.sessions, sessionID)
	m.closing[sessionID] = session
	m.mu.Unlock()
	closeErr := session.Close(force)
	m.mu.Lock()
	if m.closing[sessionID] == session {
		delete(m.closing, sessionID)
	}
	m.mu.Unlock()
	return closeErr
}

func (m *Manager) StreamURL(sessionID string) (string, error) {
	session, err := m.Get(sessionID)
	if err != nil {
		return "", err
	}
	return m.streamServer.sessionURL(session), nil
}

func (m *Manager) Shutdown(ctx context.Context) error {
	m.shutdownOnce.Do(func() {
		m.mu.Lock()
		m.shuttingDown = true
		m.mu.Unlock()
		go m.runShutdown()
	})

	select {
	case <-m.shutdownDone:
		return m.shutdownErr
	case <-ctx.Done():
		return ctx.Err()
	}
}

func (m *Manager) runShutdown() {
	var shutdownErrors []error
	if err := m.streamServer.Shutdown(); err != nil {
		shutdownErrors = append(shutdownErrors, err)
	}
	m.creates.Wait()

	m.mu.Lock()
	sessions := make([]*Session, 0, len(m.sessions))
	for _, session := range m.sessions {
		sessions = append(sessions, session)
	}
	for _, session := range m.closing {
		sessions = append(sessions, session)
	}
	m.mu.Unlock()

	errorsBySession := make(chan error, len(sessions))
	var wait sync.WaitGroup
	for _, session := range sessions {
		wait.Add(1)
		go func() {
			defer wait.Done()
			if err := session.Close(false); err != nil {
				errorsBySession <- err
			}
		}()
	}
	wait.Wait()
	close(errorsBySession)

	for err := range errorsBySession {
		shutdownErrors = append(shutdownErrors, err)
	}
	m.mu.Lock()
	clear(m.sessions)
	clear(m.closing)
	m.shutdownErr = errors.Join(shutdownErrors...)
	m.mu.Unlock()
	close(m.shutdownDone)
}

func randomOpaqueValue() (string, error) {
	value := make([]byte, 32)
	if _, err := rand.Read(value); err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(value), nil
}
