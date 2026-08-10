// Package association defines the authority-free pointer persisted by a
// terminal tab and the host-owned, generation-scoped association attached to
// a live terminal session or AgentRun.
package association

import (
	"errors"
	"fmt"
	"path/filepath"
	"strings"
)

const VersionV1 uint8 = 1

var (
	ErrUnsupportedVersion = errors.New("unsupported association version")
	ErrInvalidTarget      = errors.New("invalid association target")
	ErrStaleAssociation   = errors.New("stale association")
)

// PointerV1 is safe to persist with a terminal tab. It intentionally contains
// no project generation, live identity, token, environment, runtime context,
// or output. Zero plan and task IDs mean project-only context.
type PointerV1 struct {
	Version uint8  `json:"version"`
	PlanID  uint64 `json:"planId,omitempty"`
	TaskID  uint64 `json:"taskId,omitempty"`
}

// TargetV1 is the validated plan/task target copied into a live association.
type TargetV1 struct {
	PlanID uint64 `json:"planId,omitempty"`
	TaskID uint64 `json:"taskId,omitempty"`
}

// AssociationV1 is minted by Host only after validating a persisted pointer
// against the current project store. LiveID is the opaque terminal session or
// AgentRun identity; Revision increases on every accepted reassociation.
// Association metadata is descriptive context and grants no capabilities.
type AssociationV1 struct {
	Version     uint8    `json:"version"`
	ProjectRoot string   `json:"projectRoot"`
	Generation  uint64   `json:"generation"`
	LiveID      string   `json:"liveId"`
	Target      TargetV1 `json:"target"`
	Revision    uint64   `json:"revision"`
}

// Catalog exposes only the existence and ownership checks needed to validate
// a plan/task target in the current project.
type Catalog interface {
	ValidatePlan(planID uint64) error
	TaskPlan(taskID uint64) (uint64, error)
}

// Host owns association validation for one canonical project generation.
type Host struct {
	projectRoot string
	generation  uint64
	catalog     Catalog
}

// ProjectRoot identifies the canonical project this host validates. It is
// exposed so server-side consumers can fence authoritative stores to the same
// project before reading project-only pointers that need no catalog lookup.
func (h *Host) ProjectRoot() string {
	if h == nil {
		return ""
	}
	return h.projectRoot
}

// Generation identifies the live workspace generation owned by this host.
func (h *Host) Generation() uint64 {
	if h == nil {
		return 0
	}
	return h.generation
}

// NewHost creates the association authority for the current workspace.
func NewHost(projectRoot string, generation uint64, catalog Catalog) (*Host, error) {
	if generation == 0 {
		return nil, fmt.Errorf("%w: workspace generation must be nonzero", ErrStaleAssociation)
	}
	absolute, err := filepath.Abs(projectRoot)
	if err != nil {
		return nil, fmt.Errorf("resolve association project root: %w", err)
	}
	canonical, err := filepath.EvalSymlinks(absolute)
	if err != nil {
		return nil, fmt.Errorf("canonicalize association project root: %w", err)
	}
	return &Host{
		projectRoot: filepath.Clean(canonical),
		generation:  generation,
		catalog:     catalog,
	}, nil
}

// Bind validates pointer and returns the next association for liveID. Previous
// may be nil for a first binding. A previous association from another live
// identity, project, generation, or version is rejected instead of reused.
func (h *Host) Bind(
	liveID string,
	pointer PointerV1,
	previous *AssociationV1,
) (AssociationV1, error) {
	if h == nil {
		return AssociationV1{}, errors.New("association host is required")
	}
	liveID = strings.TrimSpace(liveID)
	if liveID == "" {
		return AssociationV1{}, fmt.Errorf("%w: live identity is required", ErrInvalidTarget)
	}
	target, err := h.Validate(pointer)
	if err != nil {
		return AssociationV1{}, err
	}
	revision := uint64(1)
	if previous != nil {
		if previous.Version != VersionV1 ||
			previous.ProjectRoot != h.projectRoot ||
			previous.Generation != h.generation ||
			previous.LiveID != liveID ||
			previous.Revision == 0 || previous.Revision == ^uint64(0) {
			return AssociationV1{}, ErrStaleAssociation
		}
		revision = previous.Revision + 1
	}
	return AssociationV1{
		Version:     VersionV1,
		ProjectRoot: h.projectRoot,
		Generation:  h.generation,
		LiveID:      liveID,
		Target:      target,
		Revision:    revision,
	}, nil
}

// Validate resolves an authority-free pointer against the current catalog.
func (h *Host) Validate(pointer PointerV1) (TargetV1, error) {
	if h == nil {
		return TargetV1{}, errors.New("association host is required")
	}
	if pointer.Version != VersionV1 {
		return TargetV1{}, fmt.Errorf("%w: %d", ErrUnsupportedVersion, pointer.Version)
	}
	if pointer.TaskID != 0 && pointer.PlanID == 0 {
		return TargetV1{}, fmt.Errorf("%w: task requires a plan", ErrInvalidTarget)
	}
	if pointer.PlanID == 0 {
		return TargetV1{}, nil
	}
	if h.catalog == nil {
		return TargetV1{}, fmt.Errorf("%w: project catalog is unavailable", ErrInvalidTarget)
	}
	if err := h.catalog.ValidatePlan(pointer.PlanID); err != nil {
		return TargetV1{}, fmt.Errorf("%w: plan #%d: %v", ErrInvalidTarget, pointer.PlanID, err)
	}
	if pointer.TaskID != 0 {
		planID, err := h.catalog.TaskPlan(pointer.TaskID)
		if err != nil {
			return TargetV1{}, fmt.Errorf("%w: task #%d: %v", ErrInvalidTarget, pointer.TaskID, err)
		}
		if planID != pointer.PlanID {
			return TargetV1{}, fmt.Errorf(
				"%w: task #%d belongs to plan #%d, not plan #%d",
				ErrInvalidTarget,
				pointer.TaskID,
				planID,
				pointer.PlanID,
			)
		}
	}
	return TargetV1{PlanID: pointer.PlanID, TaskID: pointer.TaskID}, nil
}
