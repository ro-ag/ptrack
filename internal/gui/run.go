package gui

import (
	"context"
	"errors"
	"io/fs"

	"github.com/ro-ag/ptrack/internal/store"
	"github.com/wailsapp/wails/v2"
	"github.com/wailsapp/wails/v2/pkg/options"
	"github.com/wailsapp/wails/v2/pkg/options/assetserver"
	wailsruntime "github.com/wailsapp/wails/v2/pkg/runtime"
)

// Run opens the Wails project workspace. An empty startPath resolves from the
// current directory. initialPlan is zero to use the active plan.
func Run(startPath string, initialPlan uint64, assets fs.FS) error {
	emitter := terminalEventEmitter(func(ctx context.Context, name string, payload any) {
		wailsruntime.EventsEmit(ctx, name, payload)
	})
	app := newWorkspaceCoordinator(buildProductionWorkspace, emitter)
	candidate, err := buildProductionWorkspace(startPath, initialPlan)
	switch {
	case err == nil:
		candidate.setGeneration(1)
		if activateErr := candidate.activate(); activateErr != nil {
			app.workspaceStatus = WorkspaceError
			app.workspaceError = activateErr.Error()
			_ = closeWorkspaceWithTimeout(candidate)
		} else {
			app.lastGeneration = 1
			app.workspace = candidate
			app.workspaceStatus = WorkspaceOpen
			app.syncLegacyWorkspaceFieldsLocked(candidate)
		}
	case errors.Is(err, store.ErrNoProject):
		// Welcome is a valid startup state.
	default:
		app.workspaceStatus = WorkspaceError
		app.workspaceError = err.Error()
	}
	defer app.onShutdown(context.Background())

	return wails.Run(&options.App{
		Title:     "P-TRACK Project Workspace",
		Width:     1440,
		Height:    900,
		MinWidth:  880,
		MinHeight: 560,
		BackgroundColour: &options.RGBA{
			R: 8,
			G: 13,
			B: 18,
			A: 255,
		},
		AssetServer: &assetserver.Options{Assets: assets},
		Menu:        newProjectWorkspaceMenu(app),
		Bind:        []interface{}{app},
		OnStartup:   app.onStartup,
		OnShutdown:  app.onShutdown,
	})
}
