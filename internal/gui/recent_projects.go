package gui

import (
	"os"
	"time"

	"github.com/ro-ag/ptrack/internal/store"
)

const recentProjectLimit = 20

type RecentProject struct {
	Name      string `json:"name"`
	Path      string `json:"path"`
	LastSeen  string `json:"lastSeen"`
	Available bool   `json:"available"`
}

func (a *App) GetRecentProjects() ([]RecentProject, error) {
	global, err := store.OpenGlobal()
	if err != nil {
		return nil, err
	}
	defer global.Close()
	refs, err := global.ListRecentProjects(recentProjectLimit)
	if err != nil {
		return nil, err
	}
	recent := make([]RecentProject, 0, len(refs))
	for _, ref := range refs {
		available := false
		if info, statErr := os.Stat(ref.Path); statErr == nil && info.IsDir() {
			_, findErr := store.FindProjectDB(ref.Path)
			available = findErr == nil
		}
		recent = append(recent, RecentProject{
			Name:      ref.Name,
			Path:      ref.Path,
			LastSeen:  ref.LastSeen.Format(time.RFC3339),
			Available: available,
		})
	}
	return recent, nil
}
