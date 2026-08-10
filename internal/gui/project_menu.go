package gui

import (
	"runtime"

	"github.com/wailsapp/wails/v2/pkg/menu"
	"github.com/wailsapp/wails/v2/pkg/menu/keys"
)

const (
	projectMenuOpenEvent   = "workspace:open-requested"
	projectMenuSwitchEvent = "workspace:switch-requested"
	projectMenuCloseEvent  = "workspace:close-requested"
	capabilitiesMenuEvent  = "workspace:capabilities-requested"
)

func newProjectWorkspaceMenu(app *App) *menu.Menu {
	result := menu.NewMenu()
	if runtime.GOOS == "darwin" {
		result.Append(menu.AppMenu())
	}
	project := result.AddSubmenu("Project")
	project.AddText("Open Project…", keys.CmdOrCtrl("o"), func(*menu.CallbackData) {
		app.emitTerminalEvent(projectMenuOpenEvent, nil)
	})
	project.AddText("Switch Project…", nil, func(*menu.CallbackData) {
		app.emitTerminalEvent(projectMenuSwitchEvent, nil)
	})
	project.AddSeparator()
	project.AddText("Close Project", keys.CmdOrCtrl("w"), func(*menu.CallbackData) {
		app.emitTerminalEvent(projectMenuCloseEvent, nil)
	})
	settings := result.AddSubmenu("Settings")
	settings.AddText("Network Capabilities…", nil, func(*menu.CallbackData) {
		app.emitTerminalEvent(capabilitiesMenuEvent, nil)
	})
	if runtime.GOOS == "darwin" {
		settings.AddText("Install 'ptrack' Shell Command…", nil, func(*menu.CallbackData) {
			app.InstallShellCommand()
		})
	}
	if runtime.GOOS == "darwin" {
		result.Append(menu.EditMenu())
		result.Append(menu.WindowMenu())
	}
	return result
}
