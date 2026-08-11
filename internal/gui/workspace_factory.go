package gui

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/ro-ag/ptrack/internal/store"
	"github.com/ro-ag/ptrack/internal/terminal"
	wailsruntime "github.com/wailsapp/wails/v2/pkg/runtime"
)

func buildProductionWorkspace(path string, initialPlan uint64) (*WorkspaceContext, error) {
	if path == "" {
		var err error
		path, err = os.Getwd()
		if err != nil {
			return nil, fmt.Errorf("get current directory: %w", err)
		}
	}
	dbPath, err := store.FindProjectDB(path)
	if err != nil {
		return nil, err
	}
	root := filepath.Dir(filepath.Dir(dbPath))
	root, err = filepath.Abs(root)
	if err != nil {
		return nil, fmt.Errorf("resolve GUI project root: %w", err)
	}
	root, err = filepath.EvalSymlinks(root)
	if err != nil {
		return nil, fmt.Errorf("canonicalize GUI project root: %w", err)
	}
	dbPath = filepath.Join(root, ".ptrack", "ptrack.db")
	globalHome, err := store.GlobalHome()
	if err != nil {
		return nil, err
	}
	discoveredProfiles, err := terminal.DiscoverProfiles()
	if err != nil {
		return nil, err
	}
	configuredProfiles := []terminal.Profile(nil)
	profileConfigPath := filepath.Join(globalHome, "terminal-profiles.json")
	profileConfig, configErr := terminal.LoadProfileConfig(profileConfigPath)
	if configErr == nil {
		configuredProfiles = profileConfig.Profiles
	} else if !errors.Is(configErr, os.ErrNotExist) {
		return nil, configErr
	}
	profiles, err := terminal.MergeProfiles(discoveredProfiles, configuredProfiles)
	if err != nil {
		return nil, err
	}
	manager, err := terminal.NewManager(root, profiles, terminal.GoPTYFactory{})
	if err != nil {
		return nil, err
	}
	workspace := newWorkspaceContext(workspaceContextConfig{
		root:         root,
		dbPath:       dbPath,
		name:         filepath.Base(root),
		initialPlan:  initialPlan,
		terminals:    productionTerminalManager{manager: manager},
		agents:       newWorkspaceAgentResources(root, globalHome),
		capabilities: newWorkspaceCapabilityResources(globalHome, root, dbPath),
	})
	registerGUIProjectBestEffort(root)
	return workspace, nil
}

func registerGUIProjectBestEffort(root string) {
	global, err := store.OpenGlobal()
	if err != nil {
		return
	}
	defer global.Close()
	_ = global.RegisterProject(filepath.Base(root), root)
}

// PickProjectDirectory opens the platform-native directory browser. An empty
// result means the user cancelled.
func (a *App) PickProjectDirectory() (string, error) {
	ctx, release, ok := a.acquireRuntimeCall()
	if !ok {
		return "", errors.New("application window is unavailable")
	}
	defer release()
	defaultDirectory := ""
	if state := a.GetWorkspaceState(); state.Project != nil {
		defaultDirectory = state.Project.Root
	}
	return wailsruntime.OpenDirectoryDialog(ctx, wailsruntime.OpenDialogOptions{
		DefaultDirectory:     defaultDirectory,
		Title:                "Open p-track Project",
		CanCreateDirectories: false,
		ResolvesAliases:      true,
	})
}

func closeWorkspaceWithTimeout(workspace *WorkspaceContext) error {
	if workspace == nil {
		return nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	return workspace.Close(ctx)
}
