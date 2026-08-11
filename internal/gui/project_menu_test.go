package gui

import (
	"context"
	"testing"

	"github.com/wailsapp/wails/v2/pkg/menu"
)

func TestProjectWorkspaceMenuExposesLifecycleActions(t *testing.T) {
	var events []string
	app := newWorkspaceCoordinator(nil, func(_ context.Context, name string, _ any) {
		events = append(events, name)
	})
	app.onStartup(context.Background())
	applicationMenu := newProjectWorkspaceMenu(app)

	var projectMenu *menu.Menu
	for _, item := range applicationMenu.Items {
		if item.Label == "Project" {
			projectMenu = item.SubMenu
			break
		}
	}
	if projectMenu == nil {
		t.Fatal("Project menu is missing")
	}
	wantLabels := []string{"Open Project…", "Switch Project…", "Close Project"}
	for _, want := range wantLabels {
		found := false
		for _, item := range projectMenu.Items {
			if item.Label == want {
				found = true
				item.Click(&menu.CallbackData{MenuItem: item})
				break
			}
		}
		if !found {
			t.Fatalf("menu item %q is missing", want)
		}
	}
	wantEvents := []string{
		projectMenuOpenEvent,
		projectMenuSwitchEvent,
		projectMenuCloseEvent,
	}
	for index, want := range wantEvents {
		if events[index] != want {
			t.Fatalf("event %d = %q, want %q", index, events[index], want)
		}
	}

	var settingsMenu *menu.Menu
	for _, item := range applicationMenu.Items {
		if item.Label == "Settings" {
			settingsMenu = item.SubMenu
			break
		}
	}
	if settingsMenu == nil {
		t.Fatal("Settings menu is missing")
	}
	foundCapabilities := false
	foundUpdates := false
	for _, item := range settingsMenu.Items {
		if item.Label == "Updates…" {
			foundUpdates = true
			item.Click(&menu.CallbackData{MenuItem: item})
			if got := events[len(events)-1]; got != updatesMenuEvent {
				t.Fatalf("updates event = %q, want %q", got, updatesMenuEvent)
			}
		}
		if item.Label == "Network Capabilities…" {
			foundCapabilities = true
			item.Click(&menu.CallbackData{MenuItem: item})
			break
		}
	}
	if !foundUpdates {
		t.Fatal("Updates menu item is missing")
	}
	if !foundCapabilities {
		t.Fatal("Network Capabilities menu item is missing")
	}
	if got := events[len(events)-1]; got != capabilitiesMenuEvent {
		t.Fatalf("capabilities event = %q, want %q", got, capabilitiesMenuEvent)
	}
}
