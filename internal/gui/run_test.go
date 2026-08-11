package gui

import (
	"strings"
	"testing"
	"testing/fstest"
)

func TestWailsAppOptionsConfigureNativeMenuAndAboutMetadata(t *testing.T) {
	app := newWorkspaceCoordinator(nil, nil)
	assets := fstest.MapFS{
		"index.html": {Data: []byte("<!doctype html>")},
	}
	configured := newWailsAppOptions(app, assets, "darwin", "v1.2.3")

	if configured.Title != "p-track Project Workspace" {
		t.Fatalf("title = %q", configured.Title)
	}
	if configured.Menu == nil {
		t.Fatal("native application menu is not configured")
	}
	if got := topLevelMenuShape(configured.Menu); len(got) == 0 || got[0] != "<app-role>" {
		t.Fatalf("Darwin menu shape = %#v, want native App role first", got)
	}
	if configured.Mac == nil || configured.Mac.About == nil {
		t.Fatal("macOS About metadata is not configured")
	}
	if configured.Mac.About.Title != "p-track" {
		t.Fatalf("About title = %q, want p-track", configured.Mac.About.Title)
	}
	for _, want := range []string{
		"Version v1.2.3",
		"Persistent project memory for humans and AI agents.",
		"© 2026 ro-ag",
		"Apache License 2.0",
	} {
		if !strings.Contains(configured.Mac.About.Message, want) {
			t.Fatalf("About message %q does not contain %q", configured.Mac.About.Message, want)
		}
	}
	if configured.AssetServer == nil || configured.AssetServer.Assets == nil {
		t.Fatal("asset server is not configured")
	}
	if len(configured.Bind) != 1 || configured.Bind[0] != app {
		t.Fatalf("bindings = %#v, want the workspace app", configured.Bind)
	}
	if configured.OnBeforeClose == nil {
		t.Fatal("runtime-call close fence is not configured")
	}
}
