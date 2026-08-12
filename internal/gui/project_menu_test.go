package gui

import (
	"context"
	"net/url"
	"reflect"
	"testing"
	"time"

	"github.com/wailsapp/wails/v2/pkg/menu"
	"github.com/wailsapp/wails/v2/pkg/menu/keys"
)

func TestProjectWorkspaceMenuPlatformLayouts(t *testing.T) {
	tests := []struct {
		goos string
		want []string
	}{
		{
			goos: "darwin",
			want: []string{"<app-role>", "File", "Project", "<edit-role>", "View", "<window-role>", "Help"},
		},
		{goos: "windows", want: []string{"File", "Project", "View", "Help"}},
		{goos: "linux", want: []string{"File", "Project", "View", "Help"}},
	}
	for _, test := range tests {
		t.Run(test.goos, func(t *testing.T) {
			applicationMenu := newProjectWorkspaceMenuForGOOS(
				newMenuTestApp(nil),
				test.goos,
				func(context.Context, string) {},
			)
			if got := topLevelMenuShape(applicationMenu); !reflect.DeepEqual(got, test.want) {
				t.Fatalf("top-level menus = %#v, want %#v", got, test.want)
			}
		})
	}
}

func TestDarwinMenuPreservesNativeRolesAndConventionalItems(t *testing.T) {
	applicationMenu := newProjectWorkspaceMenuForGOOS(
		newMenuTestApp(nil),
		"darwin",
		func(context.Context, string) {},
	)

	assertRole(t, applicationMenu.Items[0], menu.AppMenuRole)
	assertRole(t, applicationMenu.Items[3], menu.EditMenuRole)
	assertRole(t, applicationMenu.Items[5], menu.WindowMenuRole)

	file := submenuByLabel(t, applicationMenu, "File")
	assertMenuLabels(t, file, "Open Project…", "Switch Project…", "<separator>", "Close Project")
	assertAccelerator(t, itemByLabel(t, file, "Open Project…"), keys.CmdOrCtrl("o"))
	if item := itemByLabel(t, file, "Close Project"); item.Accelerator != nil {
		t.Fatalf("Close Project accelerator = %#v, want nil", item.Accelerator)
	}

	project := submenuByLabel(t, applicationMenu, "Project")
	assertMenuLabels(
		t,
		project,
		"Settings…",
		"Network Capabilities…",
		"<separator>",
		"Install 'ptrack' Shell Command…",
	)
	assertAccelerator(t, itemByLabel(t, project, "Settings…"), keys.CmdOrCtrl(","))

	view := submenuByLabel(t, applicationMenu, "View")
	assertMenuLabels(
		t,
		view,
		"Board",
		"Intelligence",
		"Capabilities",
		"<separator>",
		"Toggle Terminal Panel",
		"Command Palette…",
	)
	assertAccelerator(t, itemByLabel(t, view, "Board"), keys.CmdOrCtrl("1"))
	assertAccelerator(t, itemByLabel(t, view, "Intelligence"), keys.CmdOrCtrl("2"))
	assertAccelerator(t, itemByLabel(t, view, "Capabilities"), keys.CmdOrCtrl("3"))
	if item := itemByLabel(t, view, "Toggle Terminal Panel"); item.Accelerator != nil {
		t.Fatalf("Toggle Terminal Panel accelerator = %#v, want nil", item.Accelerator)
	}
	if item := itemByLabel(t, view, "Command Palette…"); item.Accelerator != nil {
		t.Fatalf("Command Palette accelerator = %#v, want nil", item.Accelerator)
	}

	help := submenuByLabel(t, applicationMenu, "Help")
	assertMenuLabels(
		t,
		help,
		"Help Center",
		"Keyboard Shortcuts",
		"<separator>",
		"Check for Updates…",
		"Report Issue",
	)
}

func TestNonDarwinMenuShapeOmitsDarwinOnlyItems(t *testing.T) {
	for _, goos := range []string{"windows", "linux"} {
		t.Run(goos, func(t *testing.T) {
			applicationMenu := newProjectWorkspaceMenuForGOOS(
				newMenuTestApp(nil),
				goos,
				func(context.Context, string) {},
			)
			for _, item := range applicationMenu.Items {
				if item.Role != 0 {
					t.Fatalf("non-Darwin top-level item has native Darwin role %d", item.Role)
				}
			}
			project := submenuByLabel(t, applicationMenu, "Project")
			assertMenuLabels(t, project, "Settings…", "Network Capabilities…")
			if item := itemByLabel(t, project, "Settings…"); item.Accelerator != nil {
				t.Fatalf("non-Darwin Settings accelerator = %#v, want nil", item.Accelerator)
			}
			for _, item := range applicationMenu.Items {
				if item.Label == "Edit" {
					t.Fatal("non-Darwin menu exposes an unsafe synthetic Edit bridge")
				}
			}
		})
	}
}

func TestProjectWorkspaceMenuCallbacksEmitExactEvents(t *testing.T) {
	var events []string
	app := newMenuTestApp(func(_ context.Context, name string, _ any) {
		events = append(events, name)
	})
	applicationMenu := newProjectWorkspaceMenuForGOOS(
		app,
		"darwin",
		func(context.Context, string) {},
	)

	tests := []struct {
		menu  string
		label string
		want  string
	}{
		{menu: "File", label: "Open Project…", want: projectMenuOpenEvent},
		{menu: "File", label: "Switch Project…", want: projectMenuSwitchEvent},
		{menu: "File", label: "Close Project", want: projectMenuCloseEvent},
		{menu: "Project", label: "Settings…", want: settingsMenuEvent},
		{menu: "Project", label: "Network Capabilities…", want: capabilitiesMenuEvent},
		{menu: "Project", label: "Install 'ptrack' Shell Command…", want: installShellMenuEvent},
		{menu: "View", label: "Board", want: boardMenuEvent},
		{menu: "View", label: "Intelligence", want: intelligenceMenuEvent},
		{menu: "View", label: "Capabilities", want: capabilitiesMenuEvent},
		{menu: "View", label: "Toggle Terminal Panel", want: terminalPanelMenuEvent},
		{menu: "View", label: "Command Palette…", want: commandPaletteMenuEvent},
		{menu: "Help", label: "Check for Updates…", want: updatesMenuEvent},
	}
	for _, test := range tests {
		item := itemByLabel(t, submenuByLabel(t, applicationMenu, test.menu), test.label)
		if item.Click == nil {
			t.Fatalf("%s > %s has no callback", test.menu, test.label)
		}
		item.Click(&menu.CallbackData{MenuItem: item})
		if got := events[len(events)-1]; got != test.want {
			t.Fatalf("%s > %s emitted %q, want %q", test.menu, test.label, got, test.want)
		}
	}
}

func TestHelpDestinationAllowlistAndCallbacks(t *testing.T) {
	if helpCenterURL != "https://ro-ag.github.io/ptrack/help/" {
		t.Fatalf("Help Center URL = %q", helpCenterURL)
	}
	if helpKeyboardShortcutsURL != "https://ro-ag.github.io/ptrack/help/reference/shortcuts/" {
		t.Fatalf("keyboard shortcuts URL = %q", helpKeyboardShortcutsURL)
	}
	tests := []struct {
		destination helpDestination
		want        string
	}{
		{destination: helpCenterDestination, want: helpCenterURL},
		{destination: helpKeyboardShortcutsDestination, want: helpKeyboardShortcutsURL},
		{destination: helpTerminalsDestination, want: helpTerminalsURL},
		{destination: helpCapabilitiesDestination, want: helpCapabilitiesURL},
		{destination: helpReportIssueDestination, want: helpReportIssueURL},
	}
	for _, test := range tests {
		got, err := helpDestinationURL(test.destination)
		if err != nil {
			t.Fatalf("helpDestinationURL(%q): %v", test.destination, err)
		}
		if got != test.want {
			t.Fatalf("helpDestinationURL(%q) = %q, want %q", test.destination, got, test.want)
		}
		parsed, err := url.Parse(got)
		if err != nil || parsed.Scheme != "https" ||
			(parsed.Host != "ro-ag.github.io" && parsed.Host != "github.com") {
			t.Fatalf("allowed help URL %q is not an exact project HTTPS URL", got)
		}
	}

	var opened []string
	app := newMenuTestApp(nil)
	applicationMenu := newProjectWorkspaceMenuForGOOS(
		app,
		"darwin",
		func(_ context.Context, target string) {
			opened = append(opened, target)
		},
	)
	help := submenuByLabel(t, applicationMenu, "Help")
	for _, label := range []string{"Help Center", "Keyboard Shortcuts", "Report Issue"} {
		item := itemByLabel(t, help, label)
		item.Click(&menu.CallbackData{MenuItem: item})
	}
	if want := []string{helpCenterURL, helpKeyboardShortcutsURL, helpReportIssueURL}; !reflect.DeepEqual(opened, want) {
		t.Fatalf("opened URLs = %#v, want %#v", opened, want)
	}
}

func TestFrontendHelpDestinationsUseAllowlistAndLifecycleBoundary(t *testing.T) {
	var opened []string
	app := newMenuTestApp(nil)
	opener := func(_ context.Context, target string) {
		app.lifecycleMu.Lock()
		app.lifecycleMu.Unlock()
		opened = append(opened, target)
	}

	for _, destination := range []helpDestination{
		helpTerminalsDestination,
		helpCapabilitiesDestination,
	} {
		if err := app.openFrontendHelpDestination(destination, opener); err != nil {
			t.Fatalf("openFrontendHelpDestination(%q): %v", destination, err)
		}
	}
	if want := []string{helpTerminalsURL, helpCapabilitiesURL}; !reflect.DeepEqual(opened, want) {
		t.Fatalf("opened URLs = %#v, want %#v", opened, want)
	}

	if err := app.openFrontendHelpDestination(helpDestination("https://attacker.invalid"), opener); err == nil {
		t.Fatal("frontend URL-shaped destination was accepted")
	}
	if len(opened) != 2 {
		t.Fatalf("unknown destination opened browser: %#v", opened)
	}

	app.onShutdown(context.Background())
	if err := app.openFrontendHelpDestination(helpTerminalsDestination, opener); err == nil {
		t.Fatal("frontend Help destination was accepted after shutdown")
	}
	if len(opened) != 2 {
		t.Fatalf("post-shutdown Help destination opened browser: %#v", opened)
	}
}

func TestUnknownHelpDestinationIsRejectedWithoutOpeningBrowser(t *testing.T) {
	called := false
	err := openHelpDestination(
		context.Background(),
		helpDestination("https://attacker.invalid"),
		func(context.Context, string) { called = true },
	)
	if err == nil {
		t.Fatal("unknown help destination was accepted")
	}
	if called {
		t.Fatal("browser opener was called for an unknown destination")
	}
}

func TestMenuAcceleratorsDoNotStealWindowOrTerminalCommands(t *testing.T) {
	tests := []struct {
		goos  string
		menu  string
		label string
		want  *keys.Accelerator
	}{
		{goos: "darwin", menu: "File", label: "Open Project…", want: keys.CmdOrCtrl("o")},
		{goos: "darwin", menu: "File", label: "Close Project"},
		{goos: "darwin", menu: "Project", label: "Settings…", want: keys.CmdOrCtrl(",")},
		{goos: "darwin", menu: "View", label: "Board", want: keys.CmdOrCtrl("1")},
		{goos: "darwin", menu: "View", label: "Intelligence", want: keys.CmdOrCtrl("2")},
		{goos: "darwin", menu: "View", label: "Capabilities", want: keys.CmdOrCtrl("3")},
		{goos: "darwin", menu: "View", label: "Toggle Terminal Panel"},
		{goos: "darwin", menu: "View", label: "Command Palette…"},
		{goos: "windows", menu: "File", label: "Close Project"},
		{goos: "windows", menu: "File", label: "Open Project…"},
		{goos: "windows", menu: "Project", label: "Settings…"},
		{goos: "windows", menu: "View", label: "Board"},
		{goos: "windows", menu: "View", label: "Intelligence"},
		{goos: "windows", menu: "View", label: "Capabilities"},
		{goos: "windows", menu: "View", label: "Toggle Terminal Panel"},
		{goos: "windows", menu: "View", label: "Command Palette…"},
		{goos: "linux", menu: "File", label: "Close Project"},
		{goos: "linux", menu: "File", label: "Open Project…"},
		{goos: "linux", menu: "Project", label: "Settings…"},
		{goos: "linux", menu: "View", label: "Board"},
		{goos: "linux", menu: "View", label: "Intelligence"},
		{goos: "linux", menu: "View", label: "Capabilities"},
		{goos: "linux", menu: "View", label: "Toggle Terminal Panel"},
		{goos: "linux", menu: "View", label: "Command Palette…"},
	}
	menus := make(map[string]*menu.Menu)
	for _, test := range tests {
		applicationMenu := menus[test.goos]
		if applicationMenu == nil {
			applicationMenu = newProjectWorkspaceMenuForGOOS(
				newMenuTestApp(nil),
				test.goos,
				func(context.Context, string) {},
			)
			menus[test.goos] = applicationMenu
		}
		item := itemByLabel(t, submenuByLabel(t, applicationMenu, test.menu), test.label)
		if !reflect.DeepEqual(item.Accelerator, test.want) {
			t.Errorf("%s %s > %s accelerator = %#v, want %#v", test.goos, test.menu, test.label, item.Accelerator, test.want)
		}
	}
}

func TestMenuCallbacksStopAtShutdownBoundary(t *testing.T) {
	var events int
	var opens int
	app := newMenuTestApp(func(context.Context, string, any) { events++ })
	applicationMenu := newProjectWorkspaceMenuForGOOS(
		app,
		"darwin",
		func(context.Context, string) { opens++ },
	)
	itemByLabel(t, submenuByLabel(t, applicationMenu, "File"), "Open Project…").Click(nil)
	itemByLabel(t, submenuByLabel(t, applicationMenu, "Help"), "Help Center").Click(nil)
	if events != 1 || opens != 1 {
		t.Fatalf("pre-shutdown callbacks = events %d opens %d, want 1 each", events, opens)
	}

	app.onShutdown(context.Background())
	app.lifecycleMu.Lock()
	ctx := app.wailsContext
	app.lifecycleMu.Unlock()
	if ctx != nil {
		t.Fatal("Wails context was retained after shutdown began")
	}
	itemByLabel(t, submenuByLabel(t, applicationMenu, "File"), "Open Project…").Click(nil)
	itemByLabel(t, submenuByLabel(t, applicationMenu, "Help"), "Help Center").Click(nil)
	if events != 1 || opens != 1 {
		t.Fatalf("post-shutdown callbacks = events %d opens %d, want unchanged", events, opens)
	}
}

func TestMenuCallbacksInvokeExternalFunctionsWithoutLifecycleLock(t *testing.T) {
	var app *App
	emitterCalled := false
	openerCalled := false
	app = newMenuTestApp(func(context.Context, string, any) {
		app.lifecycleMu.Lock()
		app.lifecycleMu.Unlock()
		emitterCalled = true
	})
	applicationMenu := newProjectWorkspaceMenuForGOOS(
		app,
		"darwin",
		func(context.Context, string) {
			app.lifecycleMu.Lock()
			app.lifecycleMu.Unlock()
			openerCalled = true
		},
	)
	emitItem := itemByLabel(t, submenuByLabel(t, applicationMenu, "File"), "Open Project…")
	openItem := itemByLabel(t, submenuByLabel(t, applicationMenu, "Help"), "Help Center")
	done := make(chan struct{})
	go func() {
		emitItem.Click(nil)
		openItem.Click(nil)
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("menu callback invoked an external function while holding lifecycleMu")
	}
	if !emitterCalled || !openerCalled {
		t.Fatalf("callbacks = emitter %t opener %t, want both true", emitterCalled, openerCalled)
	}
}

func newMenuTestApp(emitter terminalEventEmitter) *App {
	app := newWorkspaceCoordinator(nil, emitter)
	app.wailsContext = context.Background()
	return app
}

func topLevelMenuShape(applicationMenu *menu.Menu) []string {
	result := make([]string, 0, len(applicationMenu.Items))
	for _, item := range applicationMenu.Items {
		switch item.Role {
		case menu.AppMenuRole:
			result = append(result, "<app-role>")
		case menu.EditMenuRole:
			result = append(result, "<edit-role>")
		case menu.WindowMenuRole:
			result = append(result, "<window-role>")
		default:
			result = append(result, item.Label)
		}
	}
	return result
}

func submenuByLabel(t *testing.T, applicationMenu *menu.Menu, label string) *menu.Menu {
	t.Helper()
	item := itemByLabel(t, applicationMenu, label)
	if item.SubMenu == nil {
		t.Fatalf("menu item %q is not a submenu", label)
	}
	return item.SubMenu
}

func itemByLabel(t *testing.T, target *menu.Menu, label string) *menu.MenuItem {
	t.Helper()
	for _, item := range target.Items {
		if item.Label == label {
			return item
		}
	}
	t.Fatalf("menu item %q is missing", label)
	return nil
}

func assertMenuLabels(t *testing.T, target *menu.Menu, want ...string) {
	t.Helper()
	got := make([]string, 0, len(target.Items))
	for _, item := range target.Items {
		if item.IsSeparator() {
			got = append(got, "<separator>")
		} else {
			got = append(got, item.Label)
		}
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("menu labels = %#v, want %#v", got, want)
	}
}

func assertRole(t *testing.T, item *menu.MenuItem, want menu.Role) {
	t.Helper()
	if item.Role != want {
		t.Fatalf("menu role = %d, want %d", item.Role, want)
	}
	if item.SubMenu != nil {
		t.Fatalf("native role %d unexpectedly has a custom submenu", want)
	}
}

func assertAccelerator(t *testing.T, item *menu.MenuItem, want *keys.Accelerator) {
	t.Helper()
	if !reflect.DeepEqual(item.Accelerator, want) {
		t.Fatalf("%q accelerator = %#v, want %#v", item.Label, item.Accelerator, want)
	}
}
