package gui

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"strings"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/gitinfo"
	"github.com/ro-ag/ptrack/internal/launchcontext"
	"github.com/ro-ag/ptrack/internal/store"
	"github.com/ro-ag/ptrack/internal/terminal"
)

const (
	LinkedLaunchContextEnvironment = "PTRACK_LAUNCH_CONTEXT_V1"
	AgentEventEndpointEnvironment  = "PTRACK_AGENT_EVENT_ENDPOINT_V1"
	AgentEventTokenEnvironment     = "PTRACK_AGENT_EVENT_TOKEN_V1"
)

type linkedAgentEventResources interface {
	AgentEventEndpoint() string
	IssueLaunchedEventToken() (string, error)
	BindLaunchedEventToken(token, runID string) error
	RevokeLaunchedEventToken(token string)
}

// LaunchLinkedAgentV2 launches one discovered agent profile with a bounded
// host-built context and atomically publishes the linked terminal/run pair.
// The frontend supplies only a profile ID, CWD, dimensions, and authority-free
// pointer; executable, arguments, context, and authority remain host-owned.
func (a *App) LaunchLinkedAgentV2(
	generation uint64,
	profileID string,
	cwd string,
	rows int,
	columns int,
	pointer association.PointerV1,
) (TerminalSessionV2, error) {
	workspace, manager, release, err := a.beginTerminalOperation(generation, true)
	if err != nil {
		return TerminalSessionV2{}, err
	}
	defer release()
	if manager == nil {
		return TerminalSessionV2{}, errors.New("terminal manager is unavailable")
	}

	profile, err := installedAgentProfile(manager, profileID)
	if err != nil {
		return TerminalSessionV2{}, err
	}
	canonicalCWD, err := a.resolveLinkedLaunchCWD(
		workspace.Context(), workspace.root, cwd,
	)
	if err != nil {
		return TerminalSessionV2{}, err
	}
	environmentManager, ok := manager.(terminalEnvironmentManager)
	if !ok {
		return TerminalSessionV2{}, errors.New("terminal manager cannot inject linked launch context")
	}
	associationManager, ok := manager.(terminalAssociationManager)
	if !ok {
		return TerminalSessionV2{}, errors.New("terminal association manager is unavailable")
	}
	registry := workspace.agentRegistry()
	if registry == nil {
		return TerminalSessionV2{}, errors.New("AgentRun registry is unavailable")
	}

	s, err := store.Open(workspace.dbPath)
	if err != nil {
		return TerminalSessionV2{}, err
	}
	defer s.Close()
	host, err := workspaceAssociationHost(workspace, s)
	if err != nil {
		return TerminalSessionV2{}, err
	}
	launchContext, err := launchcontext.Build(s, host, pointer)
	if err != nil {
		return TerminalSessionV2{}, err
	}

	environment := map[string]string{
		LinkedLaunchContextEnvironment: launchContext.Text,
	}
	eventResources, _ := workspace.agents.(linkedAgentEventResources)
	eventToken := ""
	if eventResources != nil && eventResources.AgentEventEndpoint() != "" {
		eventToken, err = eventResources.IssueLaunchedEventToken()
		if err != nil {
			return TerminalSessionV2{}, err
		}
		environment[AgentEventEndpointEnvironment] = eventResources.AgentEventEndpoint()
		environment[AgentEventTokenEnvironment] = eventToken
	}
	broker := workspace.capabilityBroker()
	capabilityToken := ""
	if broker != nil {
		capabilityToken, err = broker.IssueSessionToken(profile.ID)
		if err != nil {
			if eventToken != "" {
				eventResources.RevokeLaunchedEventToken(eventToken)
			}
			return TerminalSessionV2{}, err
		}
		environment["PTRACK_CAPABILITY_TOKEN"] = capabilityToken
		environment["PTRACK_CAPABILITY_PROJECT"] = workspace.root
		environment["PTRACK_CAPABILITY_GENERATION"] = fmt.Sprint(workspace.Generation())
		environment["PTRACK_CAPABILITY_PROFILE"] = profile.ID
	}

	session, err := environmentManager.CreateWithEnv(
		profile.ID,
		canonicalCWD,
		rows,
		columns,
		environment,
	)
	if err != nil {
		if eventToken != "" {
			eventResources.RevokeLaunchedEventToken(eventToken)
		}
		if capabilityToken != "" {
			broker.RevokeToken(capabilityToken)
		}
		return TerminalSessionV2{}, err
	}
	capabilityBound := false
	cleanup := func(cause error) error {
		if eventToken != "" {
			eventResources.RevokeLaunchedEventToken(eventToken)
		}
		if capabilityToken != "" {
			if capabilityBound {
				broker.RevokeSession(session.SessionID)
			} else {
				broker.RevokeToken(capabilityToken)
			}
		}
		closeErr := manager.Close(session.SessionID, true)
		if errors.Is(closeErr, terminal.ErrSessionNotFound) {
			closeErr = nil
		}
		return errors.Join(cause, closeErr)
	}

	if err := validateLinkedLaunchSession(session, profile, canonicalCWD); err != nil {
		return TerminalSessionV2{}, cleanup(err)
	}
	var terminalAssociation association.AssociationV1
	err = func() error {
		workspace.associationMu.Lock()
		defer workspace.associationMu.Unlock()
		var bindErr error
		terminalAssociation, bindErr = associationManager.Associate(
			session.SessionID,
			host,
			pointer,
		)
		if bindErr != nil {
			return bindErr
		}
		run, registerErr := registry.RegisterLinkedLaunched(agentrun.Registration{
			Profile:    session.ProfileID,
			Provider:   session.Provider,
			PID:        session.PID,
			TerminalID: session.SessionID,
			CWD:        session.CWD,
		}, host, pointer)
		if registerErr != nil {
			return registerErr
		}
		if run.Association == nil ||
			run.Association.Generation != terminalAssociation.Generation ||
			run.Association.Revision != terminalAssociation.Revision ||
			run.Association.Target != terminalAssociation.Target {
			// The production registry makes this impossible by binding before it
			// publishes the run. Fail closed if an alternate registry violates that
			// contract and remove the just-created record before process teardown.
			registry.RollbackLinkedLaunched(run.ID, session.SessionID)
			return errors.New("linked terminal and AgentRun associations differ")
		}
		if eventToken != "" {
			if bindErr := eventResources.BindLaunchedEventToken(eventToken, run.ID); bindErr != nil {
				registry.RollbackLinkedLaunched(run.ID, session.SessionID)
				return bindErr
			}
		}
		if capabilityToken != "" {
			if bindErr := broker.BindSession(capabilityToken, session.SessionID); bindErr != nil {
				registry.RollbackLinkedLaunched(run.ID, session.SessionID)
				return bindErr
			}
			capabilityBound = true
		}
		return nil
	}()
	if err != nil {
		return TerminalSessionV2{}, cleanup(err)
	}

	result := TerminalSessionV2{
		Generation:          workspace.Generation(),
		SessionID:           session.SessionID,
		ProfileID:           session.ProfileID,
		CWD:                 session.CWD,
		State:               session.State,
		StreamURL:           session.StreamURL,
		AssociationRevision: terminalAssociation.Revision,
		LinkedLaunch:        true,
	}
	workspace.recordTerminal(TerminalSession{
		SessionID: session.SessionID,
		ProfileID: session.ProfileID,
		CWD:       session.CWD,
		State:     session.State,
		StreamURL: session.StreamURL,
	})
	a.publishWorkspaceTerminalStatus(workspace, TerminalStatus{
		Generation: workspace.Generation(),
		SessionID:  session.SessionID,
		State:      session.State,
	})
	if session.exitResults != nil {
		a.monitorTerminalExit(workspace, session.SessionID, session.exitResults)
	}
	a.monitorTerminalAttachmentLease(workspace, manager, session)
	a.publishWorkspaceRuntimeChanged(workspace)
	return result, nil
}

// RollbackLinkedAgentLaunchV2 removes a linked run when the frontend cannot
// commit its tab after a successful backend launch. Capability revocation
// always precedes the forced process close.
func (a *App) RollbackLinkedAgentLaunchV2(
	generation uint64,
	sessionID string,
) error {
	workspace, manager, release, err := a.beginTerminalOperation(generation, false)
	if err != nil {
		return err
	}
	defer release()
	if manager == nil {
		return errors.New("terminal manager is unavailable")
	}
	registry := workspace.agentRegistry()
	if registry == nil {
		return errors.New("AgentRun registry is unavailable")
	}
	workspace.associationMu.Lock()
	// The linked run is the host-owned proof that this opaque session identity
	// came from LaunchLinkedAgentV2. An arbitrary or ordinary terminal cannot
	// be force-closed through this rollback endpoint.
	if !registry.HasLinkedTerminal(sessionID) {
		workspace.associationMu.Unlock()
		return errors.New("linked agent launch is unavailable")
	}
	if broker := workspace.capabilityBroker(); broker != nil {
		broker.RevokeSession(sessionID)
	}
	registry.RevokeLaunchedEventTokenForTerminal(sessionID)
	closeErr := manager.Close(sessionID, true)
	if errors.Is(closeErr, terminal.ErrSessionNotFound) {
		closeErr = nil
	}
	if closeErr != nil {
		workspace.associationMu.Unlock()
		return closeErr
	}
	registry.RollbackLinkedTerminal(sessionID)
	workspace.associationMu.Unlock()
	workspace.removeTerminal(sessionID)
	a.publishWorkspaceTerminalStatus(workspace, TerminalStatus{
		Generation: workspace.Generation(),
		SessionID:  sessionID,
		State:      terminal.SessionClosed,
	})
	a.publishWorkspaceRuntimeChanged(workspace)
	return nil
}

func installedAgentProfile(manager terminalManager, profileID string) (terminal.Profile, error) {
	if profileID == "" || profileID != strings.TrimSpace(profileID) {
		return terminal.Profile{}, errors.New("an installed agent profile is required")
	}
	profiles, err := manager.Profiles()
	if err != nil {
		return terminal.Profile{}, fmt.Errorf("discover installed agent profiles: %w", err)
	}
	for _, profile := range profiles {
		if profile.ID != profileID {
			continue
		}
		if profile.Kind != terminal.ProfileAgent {
			return terminal.Profile{}, fmt.Errorf("terminal profile %q is not an agent", profileID)
		}
		return profile, nil
	}
	return terminal.Profile{}, fmt.Errorf("installed agent profile %q is unavailable", profileID)
}

func (a *App) resolveLinkedLaunchCWD(
	ctx context.Context,
	projectRoot string,
	requested string,
) (string, error) {
	if len(requested) > 4096 {
		return "", errors.New("linked launch working directory is too long")
	}
	resolved, err := terminal.ResolveCWD(projectRoot, requested)
	if err != nil {
		return "", err
	}
	canonical, err := filepath.EvalSymlinks(resolved)
	if err != nil {
		return "", fmt.Errorf("canonicalize linked launch working directory: %w", err)
	}
	relative, err := filepath.Rel(projectRoot, canonical)
	if err == nil && relative != ".." &&
		!strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
		return filepath.Clean(canonical), nil
	}
	inspector := a.gitWorktrees
	if inspector == nil {
		inspector = gitinfo.Service{}
	}
	identity, inspectErr := inspector.InspectWorktree(ctx, projectRoot, canonical)
	if inspectErr != nil || !pathInside(identity.Root, canonical) {
		return "", errors.New("linked launch working directory is outside the current project or its existing worktrees")
	}
	return filepath.Clean(canonical), nil
}

func validateLinkedLaunchSession(
	session managedTerminalSession,
	profile terminal.Profile,
	cwd string,
) error {
	if session.SessionID == "" || session.ProfileID != profile.ID ||
		session.ProfileKind != terminal.ProfileAgent || session.Provider != profile.Provider ||
		session.PID <= 0 {
		return errors.New("launched terminal identity does not match the selected agent profile")
	}
	canonicalSessionCWD, err := filepath.EvalSymlinks(session.CWD)
	if err != nil || filepath.Clean(canonicalSessionCWD) != cwd {
		return errors.New("launched terminal working directory does not match validated CWD")
	}
	return nil
}
