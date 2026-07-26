package gui

import (
	"context"
	"io/fs"
	"os"
	"path/filepath"
	"time"

	"github.com/ro-ag/ptrack/internal/store"
	"github.com/ro-ag/ptrack/internal/terminal"
	"github.com/wailsapp/wails/v2"
	"github.com/wailsapp/wails/v2/pkg/options"
	"github.com/wailsapp/wails/v2/pkg/options/assetserver"
)

// Run opens the Wails kanban board for the project containing the current
// directory. initialPlan is zero to use the active plan.
func Run(initialPlan uint64, assets fs.FS) error {
	cwd, err := os.Getwd()
	if err != nil {
		return err
	}
	dbPath, err := store.FindProjectDB(cwd)
	if err != nil {
		return err
	}
	projectRoot := filepath.Dir(filepath.Dir(dbPath))
	projectRoot, err = filepath.Abs(projectRoot)
	if err != nil {
		return err
	}
	projectRoot, err = filepath.EvalSymlinks(projectRoot)
	if err != nil {
		return err
	}
	profiles, err := terminal.DiscoverProfiles()
	if err != nil {
		return err
	}
	manager, err := terminal.NewManager(projectRoot, profiles, terminal.GoPTYFactory{})
	if err != nil {
		return err
	}
	app, err := newAppWithTerminal(
		dbPath,
		initialPlan,
		productionTerminalManager{manager: manager},
		nil,
	)
	if err != nil {
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		_ = manager.Shutdown(shutdownCtx)
		return err
	}
	defer app.onShutdown(context.Background())
	board, err := app.GetBoard(0)
	if err != nil {
		return err
	}

	return wails.Run(&options.App{
		Title:     "P-TRACK — " + board.ProjectName,
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
		Bind:        []interface{}{app},
		OnStartup:   app.onStartup,
		OnShutdown:  app.onShutdown,
	})
}
