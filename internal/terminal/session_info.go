package terminal

import (
	"sort"
	"time"
)

const maxSessionSnapshot = 64

type SessionInfo struct {
	ID             string       `json:"id"`
	ProfileID      string       `json:"profileId"`
	ProfileKind    ProfileKind  `json:"profileKind"`
	Provider       string       `json:"provider,omitempty"`
	PID            int          `json:"pid"`
	CWD            string       `json:"cwd"`
	State          SessionState `json:"state"`
	StartedAt      time.Time    `json:"startedAt"`
	LastActivityAt time.Time    `json:"lastActivityAt"`
}

func (s *Session) Info() SessionInfo {
	s.mu.Lock()
	defer s.mu.Unlock()
	return SessionInfo{
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
}

func (m *Manager) SessionSnapshot(limit int) []SessionInfo {
	if limit <= 0 || limit > maxSessionSnapshot {
		limit = maxSessionSnapshot
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
		return snapshot[i].StartedAt.After(snapshot[j].StartedAt)
	})
	if len(snapshot) > limit {
		snapshot = snapshot[:limit]
	}
	return snapshot
}
