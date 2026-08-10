package gui

import (
	"errors"
	"fmt"
	"time"

	"github.com/ro-ag/ptrack/internal/capability"
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

// CapabilityView is the normalized Settings representation.
type CapabilityView struct {
	Capability     model.Capability `json:"capability"`
	EffectiveScope string           `json:"effective_scope"`
	State          string           `json:"state"`
	Error          string           `json:"error,omitempty"`
}

// CapabilitySettingsV2 is generation-scoped so project-switch responses can
// never populate the next project's Settings view.
type CapabilitySettingsV2 struct {
	Generation   uint64           `json:"generation"`
	Capabilities []CapabilityView `json:"capabilities"`
}

// CapabilityViewV2 wraps one generation-scoped mutation or preview result.
type CapabilityViewV2 struct {
	Generation uint64         `json:"generation"`
	View       CapabilityView `json:"view"`
}

// CapabilityAuditsV2 returns bounded newest-first audit metadata.
type CapabilityAuditsV2 struct {
	Generation uint64                  `json:"generation"`
	Audits     []model.CapabilityAudit `json:"audits"`
}

// CapabilityDiagnosticV2 returns one generation-scoped connection test.
type CapabilityDiagnosticV2 struct {
	Generation uint64                          `json:"generation"`
	Diagnostic capability.ConnectionDiagnostic `json:"diagnostic"`
}

// GetCapabilitiesV2 lists every project-local capability.
func (a *App) GetCapabilitiesV2(generation uint64) (CapabilitySettingsV2, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return CapabilitySettingsV2{}, err
	}
	defer release()
	defer s.Close()
	capabilities, err := s.ListCapabilities()
	if err != nil {
		return CapabilitySettingsV2{}, err
	}
	views := make([]CapabilityView, 0, len(capabilities))
	for _, stored := range capabilities {
		views = append(views, capabilityView(stored, time.Now()))
	}
	return CapabilitySettingsV2{Generation: workspace.Generation(), Capabilities: views}, nil
}

// PreviewCapabilityV2 normalizes a draft and returns the exact approval scope.
func (a *App) PreviewCapabilityV2(generation uint64, draft model.Capability) (CapabilityViewV2, error) {
	workspace, err := a.currentWorkspace(generation)
	if err != nil {
		return CapabilityViewV2{}, err
	}
	release, err := workspace.beginOperation(generation, false)
	if err != nil {
		return CapabilityViewV2{}, err
	}
	defer release()
	preview, err := capability.Normalize(draft)
	if err != nil {
		return CapabilityViewV2{}, err
	}
	return CapabilityViewV2{
		Generation: workspace.Generation(),
		View:       CapabilityView{Capability: preview.Capability, EffectiveScope: preview.EffectiveScope, State: "draft"},
	}, nil
}

// SaveCapabilityV2 creates or edits a disabled draft. Existing approvals are
// preserved only for non-material edits; store policy revokes material edits.
func (a *App) SaveCapabilityV2(generation uint64, draft model.Capability) (CapabilityViewV2, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return CapabilityViewV2{}, err
	}
	defer release()
	defer s.Close()
	preview, err := capability.Normalize(draft)
	if err != nil {
		return CapabilityViewV2{}, err
	}
	normalized := preview.Capability
	if draft.ID == 0 {
		normalized = capability.Disable(normalized)
		normalized, err = s.AddCapability(normalized)
	} else {
		existing, getErr := s.GetCapability(draft.ID)
		if getErr != nil {
			return CapabilityViewV2{}, getErr
		}
		normalized.ID = existing.ID
		normalized.Revision = existing.Revision
		normalized.Enabled = existing.Enabled
		normalized.ApprovedAt = existing.ApprovedAt
		normalized.ExpiresAt = existing.ExpiresAt
		if err = s.UpdateCapability(normalized); err == nil {
			normalized, err = s.GetCapability(existing.ID)
		}
	}
	if err != nil {
		return CapabilityViewV2{}, err
	}
	if broker := workspace.capabilityBroker(); broker != nil {
		broker.RevokeCapability(normalized.ID)
	}
	return CapabilityViewV2{Generation: workspace.Generation(), View: capabilityView(normalized, time.Now())}, nil
}

// EnableCapabilityV2 binds approval to the exact digest shown in preview.
func (a *App) EnableCapabilityV2(generation, capabilityID uint64, expectedDigest string) (CapabilityViewV2, error) {
	return a.mutateCapability(generation, capabilityID, func(stored model.Capability) (model.Capability, error) {
		return capability.Approve(stored, expectedDigest, time.Now())
	})
}

// DisableCapabilityV2 revokes approval immediately.
func (a *App) DisableCapabilityV2(generation, capabilityID uint64) (CapabilityViewV2, error) {
	return a.mutateCapability(generation, capabilityID, func(stored model.Capability) (model.Capability, error) {
		return capability.Disable(stored), nil
	})
}

// ExpireCapabilityV2 expires an enabled grant without erasing its approval
// history, so Settings can distinguish expired from merely disabled.
func (a *App) ExpireCapabilityV2(generation, capabilityID uint64) (CapabilityViewV2, error) {
	return a.mutateCapability(generation, capabilityID, func(stored model.Capability) (model.Capability, error) {
		if !stored.Enabled || stored.ApprovedAt.IsZero() {
			return model.Capability{}, errors.New("only an enabled capability can be expired")
		}
		stored.ExpiresAt = time.Now()
		return stored, nil
	})
}

func (a *App) mutateCapability(
	generation, capabilityID uint64,
	mutate func(model.Capability) (model.Capability, error),
) (CapabilityViewV2, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return CapabilityViewV2{}, err
	}
	defer release()
	defer s.Close()
	stored, err := s.GetCapability(capabilityID)
	if err != nil {
		return CapabilityViewV2{}, err
	}
	updated, err := mutate(stored)
	if err != nil {
		return CapabilityViewV2{}, err
	}
	if err := s.UpdateCapability(updated); err != nil {
		return CapabilityViewV2{}, err
	}
	updated, err = s.GetCapability(capabilityID)
	if err != nil {
		return CapabilityViewV2{}, err
	}
	if broker := workspace.capabilityBroker(); broker != nil {
		broker.RevokeCapability(capabilityID)
	}
	return CapabilityViewV2{Generation: workspace.Generation(), View: capabilityView(updated, time.Now())}, nil
}

// RemoveCapabilityV2 deletes a grant while retaining its bounded audit trail.
func (a *App) RemoveCapabilityV2(generation, capabilityID uint64) (WorkspaceMutationResult, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return WorkspaceMutationResult{}, err
	}
	defer release()
	defer s.Close()
	if err := s.DeleteCapability(capabilityID); err != nil {
		return WorkspaceMutationResult{}, err
	}
	if broker := workspace.capabilityBroker(); broker != nil {
		broker.RevokeCapability(capabilityID)
	}
	return WorkspaceMutationResult{Generation: workspace.Generation()}, nil
}

// GetCapabilityAuditsV2 returns bounded metadata only.
func (a *App) GetCapabilityAuditsV2(generation, capabilityID uint64, limit int) (CapabilityAuditsV2, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return CapabilityAuditsV2{}, err
	}
	defer release()
	defer s.Close()
	if limit <= 0 || limit > 100 {
		limit = 25
	}
	audits, err := s.ListCapabilityAudits(capabilityID, limit)
	if err != nil {
		return CapabilityAuditsV2{}, err
	}
	return CapabilityAuditsV2{Generation: workspace.Generation(), Audits: audits}, nil
}

// TestCapabilityV2 runs an explicit non-mutating connection test against a
// draft or stored capability. sshCapabilityID supplies the separate SSH grant
// needed to test a Git-over-SSH remote.
func (a *App) TestCapabilityV2(
	generation uint64,
	draft model.Capability,
	sshCapabilityID uint64,
) (CapabilityDiagnosticV2, error) {
	workspace, err := a.currentWorkspace(generation)
	if err != nil {
		return CapabilityDiagnosticV2{}, err
	}
	release, err := workspace.beginOperation(generation, false)
	if err != nil {
		return CapabilityDiagnosticV2{}, err
	}
	defer release()
	tester := capability.ConnectionTester{}
	var diagnostic capability.ConnectionDiagnostic
	switch draft.Kind {
	case model.CapabilityHTTP:
		diagnostic = tester.TestHTTP(workspace.Context(), draft)
	case model.CapabilityGit:
		var sshDraft *model.Capability
		if sshCapabilityID != 0 {
			s, openErr := store.Open(workspace.dbPath)
			if openErr != nil {
				return CapabilityDiagnosticV2{}, openErr
			}
			ssh, getErr := s.GetCapability(sshCapabilityID)
			closeErr := s.Close()
			if err := errors.Join(getErr, closeErr); err != nil {
				return CapabilityDiagnosticV2{}, err
			}
			sshDraft = &ssh
		}
		diagnostic = tester.TestGit(workspace.Context(), draft, sshDraft, workspace.root)
	case model.CapabilitySSH:
		diagnostic = tester.TestSSH(workspace.Context(), draft)
	default:
		return CapabilityDiagnosticV2{}, fmt.Errorf("unsupported capability kind %q", draft.Kind)
	}
	return CapabilityDiagnosticV2{Generation: workspace.Generation(), Diagnostic: diagnostic}, nil
}

func capabilityView(stored model.Capability, now time.Time) CapabilityView {
	preview, err := capability.Normalize(stored)
	if err != nil {
		return CapabilityView{Capability: stored, State: "invalid", Error: err.Error()}
	}
	state := "disabled"
	if preview.Capability.Enabled && !preview.Capability.ExpiresAt.After(now) {
		state = "expired"
	} else if preview.Capability.Enabled {
		state = "enabled"
	}
	return CapabilityView{Capability: preview.Capability, EffectiveScope: preview.EffectiveScope, State: state}
}
