// Facts only source can show, checked across every window module rather than
// one file: what the window must never write or call, wherever the code that
// would do it lives. Behavior is covered by the controller tests beside each
// module; these scans guard against regressions no rendered DOM would reveal.
import { describe, expect, it } from "vitest";

import { moduleSource, windowModules, windowSource } from "./test-support/window-sources";

// Code only: a comment may name what the code must not do.
function code(source) {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .split("\n")
    .filter((line) => !line.trim().startsWith("//"))
    .join("\n");
}

function offenders(pattern) {
  return [...windowModules]
    .filter(([, source]) => pattern.test(code(source)))
    .map(([path]) => path);
}

describe("window module hygiene", () => {
  it("scans the modules the page ships", () => {
    for (const path of ["app.js", "main.js", "workspace/board-view.ts", "settings/controller.ts", "terminal/pane.ts"]) {
      expect(windowModules.has(path), path).toBe(true);
    }
    expect([...windowModules.keys()].some((path) => /\.test\.|test-support/.test(path))).toBe(false);
  });

  it("never parses markup from strings", () => {
    // Every rendered value goes through textContent or attributes; a diff,
    // a task title, or an issue body is never HTML.
    expect(offenders(/\.(innerHTML|outerHTML)\b|insertAdjacentHTML|document\.write\(/)).toEqual([]);
  });

  it("sets ARIA booleans explicitly instead of toggling their presence", () => {
    expect(offenders(/toggleAttribute\(\s*["']aria-/)).toEqual([]);
  });

  it("never reaches into project storage or asks for window geometry", () => {
    expect(offenders(/\.ptrack\/ptrack\.redb/)).toEqual([]);
    // The window is Rust-owned; the eviction counter is backend-owned.
    expect(offenders(/WindowState/)).toEqual([]);
    expect(offenders(/usedAt/)).toEqual([]);
  });

  it("calls only the current command versions", () => {
    expect(offenders(/\bMoveTaskV2\b/)).toEqual([]);
    expect(offenders(/Wails/)).toEqual([]);
  });

  it("opens help through the native help destination, never a hard-coded site", () => {
    expect(offenders(/ro-ag\.github\.io\/ptrack\/help/)).toEqual([]);
  });

  it("keeps retired views, labels, and messages out of the window", () => {
    // Capabilities moved to pam; Settings is a dialog, not a view.
    expect(offenders(/setView\("capabilities"/)).toEqual([]);
    expect(offenders(/setView\("settings"/)).toEqual([]);
    for (const retired of [
      "Creating the first task in Todo…",
      "safely stored in Todo while p-track starts it",
      "The first plan was not created",
      "The first task was not created",
      "The task remains in Todo",
      "projects.filter((project) => project.available)",
    ]) {
      expect(windowSource, retired).not.toContain(retired);
    }
    // No string the UI shows says Welcome (identifiers may).
    const welcome = [...windowModules].flatMap(([path, source]) =>
      source.split("\n")
        .filter((line) => !line.trim().startsWith("//") && /(["`])[^"`]*\bWelcome\b[^"`]*\1/.test(line))
        .map((line) => `${path}: ${line.trim()}`));
    expect(welcome).toEqual([]);
  });

  it("uses the shared formatters instead of private copies", () => {
    for (const helper of ["function compactBytes", "formatUpdateBytes", "relativeTimestamp", "timelineCaption"]) {
      expect(windowSource, helper).not.toContain(helper);
    }
  });

  it("never focuses a preselected project or an Escape target behind the overlay policy", () => {
    // Preselection points at the last project; it never takes focus.
    expect(offenders(/preselect[^\n]*\.focus\(\)/)).toEqual([]);
    // Escape goes through the overlay coordinator, not per-dialog checks.
    expect(offenders(/event\.key === "Escape" && !elements\.[A-Za-z]+\.hidden/)).toEqual([]);
  });

  it("records layout from the person's clicks, not from observed attributes", () => {
    expect(offenders(/MutationObserver\(recordPanelLayout\)/)).toEqual([]);
    expect(offenders(/panelLayoutRestored = Boolean\(/)).toEqual([]);
  });

  it("keeps the Settings live region in the document and its copy control an icon", () => {
    expect(offenders(/settingsSaveStatus\.remove\(\)/)).toEqual([]);
    expect(offenders(/copy\.textContent = "Copy"/)).toEqual([]);
  });

  it("has one writer for each shared record", () => {
    // Updates stay a single source of truth on the existing command.
    expect(windowSource.match(/api\(\)\.SetAutomaticUpdateChecks\(/g)).toHaveLength(1);
    // Settings owns the Unicode mode; the dock only follows it.
    expect(offenders(/saveUnicodeMode|writeModernUnicodeSetting/)).toEqual([]);
  });
});

// Wiring the harness has no fixture for yet (live agent runs, a stale guide
// preview, a workspace switch mid-read), pinned in the module that owns it.
describe("window wiring without a harness fixture", () => {
  it("guards each agent action against a second submit while it runs", () => {
    const agents = moduleSource("workspace/agent-activity-view.ts");
    expect(agents).toContain('"handoff-send"');
    expect(agents).toContain('"workflow-prepare"');
    expect(agents.split("`worktree:${item.runId}`").length - 1).toBe(2);
  });

  it("drops deferred work and agent forms when the workspace changes", () => {
    const shell = moduleSource("workspace/workspace-shell.ts");
    expect(shell).toContain("ctx.recent.cancelLandingReadsForEpoch(workspaceController.capture().epoch)");
    expect(shell).toContain("ctx.drawer.clearPendingTaskDetail()");
    expect(shell).toContain("ctx.agentActivity.hideAgentActionForms()");
    expect(shell).toContain("ctx.planDialogs.abandonPlanDialog()");
    expect(moduleSource("workspace/task-drawer.ts")).toContain("new GenerationSlot<number>()");
  });

  it("offers Skip Guide on a stale preview only when skipping is still allowed", () => {
    expect(moduleSource("workspace/first-run-view.ts"))
      .toContain("elements.setupGuideStaleSkip.hidden = !run.guideSkipAllowed");
  });

  it("says a relocated recent project opened without confirming the move", () => {
    expect(moduleSource("workspace/recent-projects-controller.ts"))
      .toContain("showError(new Error(RECENT_RELOCATION_UNCONFIRMED))");
  });
});

