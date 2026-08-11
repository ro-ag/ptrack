package gui

import (
	"context"
	"fmt"
	"runtime"

	"github.com/wailsapp/wails/v2/pkg/menu"
	"github.com/wailsapp/wails/v2/pkg/menu/keys"
	wailsruntime "github.com/wailsapp/wails/v2/pkg/runtime"
)

const (
	projectMenuOpenEvent     = "workspace:open-requested"
	projectMenuSwitchEvent   = "workspace:switch-requested"
	projectMenuCloseEvent    = "workspace:close-requested"
	settingsMenuEvent        = "workspace:settings-requested"
	capabilitiesMenuEvent    = "workspace:capabilities-requested"
	boardMenuEvent           = "workspace:board-requested"
	intelligenceMenuEvent    = "workspace:intelligence-requested"
	terminalPanelMenuEvent   = "workspace:terminal-panel-toggle-requested"
	commandPaletteMenuEvent  = "workspace:command-palette-requested"
	installShellMenuEvent    = "workspace:install-shell-command-requested"
	updatesMenuEvent         = "update:open-requested"
	helpCenterURL            = "https://github.com/ro-ag/ptrack#readme"
	helpKeyboardShortcutsURL = "https://github.com/ro-ag/ptrack#desktop-keyboard-shortcuts"
	helpReportIssueURL       = "https://github.com/ro-ag/ptrack/issues/new"
)

type helpDestination string

const (
	helpCenterDestination            helpDestination = "help-center"
	helpKeyboardShortcutsDestination helpDestination = "keyboard-shortcuts"
	helpReportIssueDestination       helpDestination = "report-issue"
)

type helpURLOpener func(context.Context, string)

func helpDestinationURL(destination helpDestination) (string, error) {
	switch destination {
	case helpCenterDestination:
		return helpCenterURL, nil
	case helpKeyboardShortcutsDestination:
		return helpKeyboardShortcutsURL, nil
	case helpReportIssueDestination:
		return helpReportIssueURL, nil
	default:
		return "", fmt.Errorf("unknown help destination %q", destination)
	}
}

// openHelpDestination resolves a symbolic destination through the fixed
// allowlist above. Callers cannot pass URLs from the frontend, project data,
// or terminal input into the browser opener.
func openHelpDestination(
	ctx context.Context,
	destination helpDestination,
	opener helpURLOpener,
) error {
	url, err := helpDestinationURL(destination)
	if err != nil {
		return err
	}
	opener(ctx, url)
	return nil
}

func newProjectWorkspaceMenu(app *App) *menu.Menu {
	return newProjectWorkspaceMenuForGOOS(
		app,
		runtime.GOOS,
		func(ctx context.Context, url string) {
			wailsruntime.BrowserOpenURL(ctx, url)
		},
	)
}

func newProjectWorkspaceMenuForGOOS(
	app *App,
	goos string,
	helpOpener helpURLOpener,
) *menu.Menu {
	result := menu.NewMenu()
	if goos == "darwin" {
		// Keep Wails' native App role intact. In Wails v2 it has no SubMenu,
		// so replacing it to append custom items would lose Services/Hide/Quit.
		result.Append(menu.AppMenu())
	}
	addFileMenu(result, app, goos)
	addProjectMenu(result, app, goos)
	if goos == "darwin" {
		result.Append(menu.EditMenu())
	}
	// Wails v2 exposes a reliable native Edit role only on Darwin. A
	// synthetic non-Darwin bridge cannot safely preserve terminal selection,
	// native clipboard, and multiline-paste review, so those platforms omit
	// Edit instead of shipping commands that bypass terminal safety.
	addViewMenu(result, app, goos)
	if goos == "darwin" {
		result.Append(menu.WindowMenu())
	}
	addHelpMenu(result, app, helpOpener)
	return result
}

func addFileMenu(result *menu.Menu, app *App, goos string) {
	file := result.AddSubmenu("File")
	var openAccelerator *keys.Accelerator
	if goos == "darwin" {
		openAccelerator = keys.CmdOrCtrl("o")
	}
	file.AddText("Open Project…", openAccelerator, emitMenuEvent(app, projectMenuOpenEvent))
	file.AddText("Switch Project…", nil, emitMenuEvent(app, projectMenuSwitchEvent))
	file.AddSeparator()
	// Closing a project is not the same as closing the native window, so it
	// deliberately does not claim the platform-standard Cmd/Ctrl+W shortcut.
	file.AddText("Close Project", nil, emitMenuEvent(app, projectMenuCloseEvent))
}

func addProjectMenu(result *menu.Menu, app *App, goos string) {
	project := result.AddSubmenu("Project")
	var settingsAccelerator *keys.Accelerator
	if goos == "darwin" {
		settingsAccelerator = keys.CmdOrCtrl(",")
	}
	project.AddText("Settings…", settingsAccelerator, emitMenuEvent(app, settingsMenuEvent))
	project.AddText(
		"Network Capabilities…",
		nil,
		emitMenuEvent(app, capabilitiesMenuEvent),
	)
	if goos == "darwin" {
		project.AddSeparator()
		project.AddText(
			"Install 'ptrack' Shell Command…",
			nil,
			emitMenuEvent(app, installShellMenuEvent),
		)
	}
}

func addViewMenu(result *menu.Menu, app *App, goos string) {
	view := result.AddSubmenu("View")
	var boardAccelerator *keys.Accelerator
	var intelligenceAccelerator *keys.Accelerator
	var capabilitiesAccelerator *keys.Accelerator
	if goos == "darwin" {
		boardAccelerator = keys.CmdOrCtrl("1")
		intelligenceAccelerator = keys.CmdOrCtrl("2")
		capabilitiesAccelerator = keys.CmdOrCtrl("3")
	}
	view.AddText("Board", boardAccelerator, emitMenuEvent(app, boardMenuEvent))
	view.AddText(
		"Intelligence",
		intelligenceAccelerator,
		emitMenuEvent(app, intelligenceMenuEvent),
	)
	view.AddText(
		"Capabilities",
		capabilitiesAccelerator,
		emitMenuEvent(app, capabilitiesMenuEvent),
	)
	view.AddSeparator()
	view.AddText(
		"Toggle Terminal Panel",
		nil,
		emitMenuEvent(app, terminalPanelMenuEvent),
	)
	view.AddText(
		"Command Palette…",
		nil,
		emitMenuEvent(app, commandPaletteMenuEvent),
	)
}

func addHelpMenu(result *menu.Menu, app *App, opener helpURLOpener) {
	help := result.AddSubmenu("Help")
	addHelpDestination(help, app, opener, "Help Center", helpCenterDestination)
	addHelpDestination(
		help,
		app,
		opener,
		"Keyboard Shortcuts",
		helpKeyboardShortcutsDestination,
	)
	help.AddSeparator()
	help.AddText("Check for Updates…", nil, emitMenuEvent(app, updatesMenuEvent))
	addHelpDestination(help, app, opener, "Report Issue", helpReportIssueDestination)
}

func addHelpDestination(
	help *menu.Menu,
	app *App,
	opener helpURLOpener,
	label string,
	destination helpDestination,
) {
	help.AddText(label, nil, func(*menu.CallbackData) {
		ctx, release, ok := app.acquireRuntimeCall()
		if !ok {
			return
		}
		defer release()
		_ = openHelpDestination(ctx, destination, opener)
	})
}

func emitMenuEvent(app *App, name string) menu.Callback {
	return func(*menu.CallbackData) {
		app.emitTerminalEvent(name, nil)
	}
}
