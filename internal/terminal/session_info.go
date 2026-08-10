package terminal

import (
	"sort"
	"time"

	"github.com/ro-ag/ptrack/internal/association"
)

const maxSessionSnapshot = 64
const maxRuntimeSessionCandidates = 1_024

type SessionInfo struct {
	ID             string                     `json:"id"`
	ProfileID      string                     `json:"profileId"`
	ProfileKind    ProfileKind                `json:"profileKind"`
	Provider       string                     `json:"provider,omitempty"`
	PID            int                        `json:"pid"`
	CWD            string                     `json:"cwd"`
	State          SessionState               `json:"state"`
	StartedAt      time.Time                  `json:"startedAt"`
	LastActivityAt time.Time                  `json:"lastActivityAt"`
	Association    *association.AssociationV1 `json:"association,omitempty"`
}

// AssociationChange is a prepared compare-and-swap mutation. Preparing
// validates the expected revision and target without changing live metadata;
// commit applies it only if the exact previous value is still current.
type AssociationChange struct {
	SessionID string
	Previous  *association.AssociationV1
	Next      association.AssociationV1
}

func (s *Session) Info() SessionInfo {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.infoLocked()
}

func (s *Session) infoLocked() SessionInfo {
	info := SessionInfo{
		ID:             s.id,
		ProfileID:      s.profile,
		ProfileKind:    s.profileKind,
		Provider:       s.provider,
		PID:            s.pid,
		CWD:            s.cwd,
		State:          s.state,
		StartedAt:      s.startedAt,
		LastActivityAt: s.lastActivityAt,
	}
	if s.association != nil {
		associationCopy := *s.association
		info.Association = &associationCopy
	}
	return info
}

func (m *Manager) SessionSnapshot(limit int) []SessionInfo {
	snapshot, _ := m.SessionSnapshotBounded(limit)
	return snapshot
}

// SessionSnapshotBounded returns a deterministic bounded session view and
// the total number of live/closing sessions before truncation.
func (m *Manager) SessionSnapshotBounded(limit int) ([]SessionInfo, int) {
	return m.sessionSnapshotBounded(limit, maxSessionSnapshot)
}

// RuntimeSessionSnapshotBounded permits a larger, still hard-bounded
// candidate set so per-task aggregation happens before the public row cap.
func (m *Manager) RuntimeSessionSnapshotBounded(limit int) ([]SessionInfo, int) {
	return m.sessionSnapshotBounded(limit, maxRuntimeSessionCandidates)
}

func (m *Manager) sessionSnapshotBounded(limit, maximum int) ([]SessionInfo, int) {
	if limit <= 0 || limit > maximum {
		limit = maximum
	}
	m.mu.Lock()
	sessions := make([]*Session, 0, len(m.sessions)+len(m.closing))
	seen := make(map[*Session]struct{}, len(m.sessions)+len(m.closing))
	for _, session := range m.sessions {
		sessions = append(sessions, session)
		seen[session] = struct{}{}
	}
	for _, session := range m.closing {
		if _, exists := seen[session]; !exists {
			sessions = append(sessions, session)
		}
	}
	m.mu.Unlock()
	snapshot := make([]SessionInfo, 0, min(limit, len(sessions)))
	for _, session := range sessions {
		snapshot = append(snapshot, session.Info())
	}
	sort.SliceStable(snapshot, func(i, j int) bool {
		if snapshot[i].StartedAt.Equal(snapshot[j].StartedAt) {
			return snapshot[i].ID < snapshot[j].ID
		}
		return snapshot[i].StartedAt.After(snapshot[j].StartedAt)
	})
	total := len(snapshot)
	if len(snapshot) > limit {
		snapshot = snapshot[:limit]
	}
	return snapshot, total
}
