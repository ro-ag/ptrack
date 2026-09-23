// The stored window layout: read before the project paints, applied through
// the real controls, and written back only for what the person changed.
import { afterEach, describe, expect, it } from "vitest";

import { board, bootApp, holdTimers, plan, projectRoot, snapshot } from "../test-support/app-harness";

const plans = [plan(1, { isActive: true }), plan(2, { title: "Later" })];

function layout(fields = {}) {
  return {
    storage: "ok",
    sidebar: { width: 300, hidden: false },
    panels: { boardHidden: false, terminalHidden: false },
    projects: {},
    ...fields,
  };
}

function bootProject(layoutState) {
  return bootApp({
    responses: {
      GetLayoutState: layoutState,
      SetLayoutState: () => ({ storage: "ok" }),
      GetWorkspaceSnapshot: (generation, planId) =>
        snapshot(board({ planId: planId || 1, plans }), generation),
    },
    open: {},
  });
}

/** Runs the held layout debounce, the write it schedules. */
function flushDebounce(timers) {
  for (const timer of timers.splice(0)) if (timer.delay === 250) timer.callback();
}

describe("window layout", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("reopens a project on its stored view and plan, with the stored sidebar", async () => {
    harness = await bootApp({
      responses: {
        GetLayoutState: layout({
          sidebar: { width: 320, hidden: true },
          projects: { [projectRoot]: { view: "overview", planId: 2, foldedLanes: ["done"] } },
        }),
        GetWorkspaceSnapshot: (generation, planId) => snapshot(board({ planId: planId || 1, plans }), generation),
      },
      open: {},
    });
    expect(harness.backend.callsTo("GetWorkspaceSnapshot")[0]).toEqual([3, 2]);
    expect(harness.$("#sidebar").hidden).toBe(true);
    expect(harness.$("#sidebar-toggle").getAttribute("aria-label")).toBe("Show project sidebar");
    expect(harness.$("#overview-page").hidden).toBe(false);
    expect(harness.$("#app").style.getPropertyValue("--sidebar-width")).toBe("320px");
  });

  it("writes a sidebar change once the debounce settles", async () => {
    harness = await bootProject(layout());
    const timers = holdTimers(harness);
    await harness.click("#sidebar-toggle");
    await harness.click("#sidebar-toggle");
    await harness.click("#sidebar-toggle");
    expect(harness.backend.names()).not.toContain("SetLayoutState");
    flushDebounce(timers);
    const writes = harness.backend.callsTo("SetLayoutState");
    expect(writes).toHaveLength(1);
    expect(writes[0][0].sidebar).toEqual({ width: 300, hidden: true });
    expect(Object.keys(writes[0][0].projects)).toEqual([projectRoot]);
  });

  it("records a panel toggle the person clicked", async () => {
    harness = await bootProject(layout());
    const timers = holdTimers(harness);
    await harness.click("#terminal-panel-toggle");
    flushDebounce(timers);
    const [patch] = harness.backend.callsTo("SetLayoutState").at(-1);
    expect(patch.panels).toEqual({
      boardHidden: harness.$("#board-panel-toggle").getAttribute("aria-expanded") === "false",
      terminalHidden: true,
    });
  });

  it("marks each layout toggle by whether its panel shows, and explains a dimmed board toggle", async () => {
    harness = await bootProject(layout());
    const terminal = harness.$("#terminal-panel-toggle");
    const boardToggle = harness.$("#board-panel-toggle");
    expect(harness.$("#sidebar-toggle").getAttribute("aria-expanded")).toBe("true");
    expect(terminal.getAttribute("aria-expanded")).toBe("true");
    expect(terminal.hasAttribute("aria-pressed")).toBe(false);
    await harness.click(terminal);
    expect(terminal.getAttribute("aria-expanded")).toBe("false");
    expect(terminal.getAttribute("aria-label")).toBe("Show terminal panel");
    // No terminal is running in the harness, so the board cannot be hidden.
    expect(boardToggle.disabled).toBe(true);
    expect(boardToggle.title).toBe("Start a terminal to hide the board");
  });

  it("saves a pending change right away when the workspace is about to change", async () => {
    harness = await bootProject(layout());
    holdTimers(harness);
    await harness.click("#sidebar-toggle");
    expect(harness.backend.names()).not.toContain("SetLayoutState");
    harness.app.shell.beginWorkspaceTransition();
    expect(harness.backend.callsTo("SetLayoutState")).toHaveLength(1);
  });

  it("records the plan on the board once the board has loaded", async () => {
    harness = await bootProject(layout());
    const timers = holdTimers(harness);
    harness.app.snapshot.loadSnapshot(2);
    await harness.settle();
    flushDebounce(timers);
    const [patch] = harness.backend.callsTo("SetLayoutState").at(-1);
    expect(patch.projects[projectRoot]).toMatchObject({ planId: 2, view: "board" });
  });
});
