package gui

import (
	"context"

	"github.com/ro-ag/ptrack/internal/capability"
)

type workspaceCapabilityBroker interface {
	workspaceShutdowner
	IssueSessionToken(profile string) (string, error)
	BindSession(token, sessionID string) error
	RevokeToken(token string)
	RevokeSession(sessionID string)
	RevokeCapability(capabilityID uint64)
}

type workspaceCapabilityResources struct {
	globalHome string
	root       string
	dbPath     string
	server     *capability.BrokerServer
}

func newWorkspaceCapabilityResources(globalHome, root, dbPath string) *workspaceCapabilityResources {
	return &workspaceCapabilityResources{globalHome: globalHome, root: root, dbPath: dbPath}
}

func (r *workspaceCapabilityResources) Activate(generation uint64) error {
	if r.server != nil {
		return nil
	}
	server, err := capability.StartBrokerServer(capability.BrokerServerConfig{
		GlobalHome: r.globalHome, ProjectRoot: r.root, DBPath: r.dbPath, Generation: generation,
	})
	if err != nil {
		return err
	}
	r.server = server
	return nil
}

func (r *workspaceCapabilityResources) IssueSessionToken(profile string) (string, error) {
	return r.server.Broker.IssueSessionToken(profile)
}

func (r *workspaceCapabilityResources) BindSession(token, sessionID string) error {
	return r.server.Broker.BindSession(token, sessionID)
}

func (r *workspaceCapabilityResources) RevokeToken(token string) {
	r.server.Broker.RevokeToken(token)
}

func (r *workspaceCapabilityResources) RevokeSession(sessionID string) {
	r.server.Broker.RevokeSession(sessionID)
}

func (r *workspaceCapabilityResources) RevokeCapability(capabilityID uint64) {
	r.server.Broker.RevokeCapability(capabilityID)
}

func (r *workspaceCapabilityResources) Shutdown(ctx context.Context) error {
	if r.server == nil {
		return nil
	}
	return r.server.Shutdown(ctx)
}
