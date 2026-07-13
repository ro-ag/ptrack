package cli

import (
	"strings"
	"testing"
)

func TestMilestoneCommands(t *testing.T) {
	seedProject(t) // inits project in a temp cwd
	out := mustRun(t, "milestone", "add", "v1.0", "--due", "2026-12-01")
	if !strings.Contains(out, "milestone #1") {
		t.Fatalf("add output: %s", out)
	}
	mustRun(t, "plan", "add", "storage", "--milestone", "1")
	show := mustRun(t, "milestone", "show", "1")
	if !strings.Contains(show, "storage") || !strings.Contains(show, "2026-12-01") {
		t.Errorf("milestone show missing plan or due:\n%s", show)
	}
	list := mustRun(t, "milestone", "list")
	if !strings.Contains(list, "v1.0") {
		t.Errorf("milestone list:\n%s", list)
	}
	if _, err := runCmd(t, "milestone", "done", "1"); err != nil {
		t.Errorf("milestone done: %v", err)
	}
}

func TestIssueCommands(t *testing.T) {
	seedProject(t)
	out := mustRun(t, "issue", "add", "crash on start", "--severity", "high")
	if !strings.Contains(out, "issue #1") || !strings.Contains(out, "high") {
		t.Fatalf("issue add: %s", out)
	}
	mustRun(t, "issue", "add", "typo in docs", "--severity", "low")
	openList := mustRun(t, "issue", "list", "--status", "open")
	if !strings.Contains(openList, "crash on start") {
		t.Errorf("open issues:\n%s", openList)
	}
	mustRun(t, "issue", "close", "1")
	closed := mustRun(t, "issue", "list", "--status", "closed")
	if !strings.Contains(closed, "crash on start") {
		t.Errorf("closed issues:\n%s", closed)
	}
	// invalid severity rejected
	if _, err := runCmd(t, "issue", "add", "x", "--severity", "bogus"); err == nil {
		t.Error("expected invalid severity error")
	}
}

func TestContextShowsOpenIssues(t *testing.T) {
	seedProject(t)
	mustRun(t, "issue", "add", "leak in handler", "--severity", "critical")
	ctx := mustRun(t, "context")
	if !strings.Contains(ctx, "Open issues") || !strings.Contains(ctx, "leak in handler") {
		t.Errorf("context missing open issues:\n%s", ctx)
	}
	if !strings.Contains(ctx, "issues (1 open)") {
		t.Errorf("inventory missing issue counts:\n%s", ctx)
	}
}
