// The window's frame around a project: the landing it opens on, the views and
// the terminal dock beside them, and how a close and a refresh stay fenced to
// the workspace that asked for them.
import { afterEach, describe, expect, it } from "vitest";

import { bootApp } from "../test-support/app-harness";
import { fire } from "../test-support/fake-dom";
import { shown } from "../test-support/journey";
import { desktopBackend } from "./app-context";

describe("window frame", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("opens Projects on its search field, with no project controls", async () => {
    harness = await bootApp();
    expect(harness.$("#workspace-state-heading").textContent).toBe("Projects");
    expect(harness.document.activeElement.id).toBe("recent-project-search");
    expect(harness.$("#board-panel-toggle").hidden).toBe(true);
    expect(harness.$("#terminal-panel-toggle").hidden).toBe(true);
  });

  it("keeps the terminal dock beside every view of an open project", async () => {
    harness = await bootApp({ open: {} });
    expect(harness.$("#terminal-panel-toggle").hidden).toBe(false);
    for (const view of ["#nav-overview", "#nav-issues", "#nav-board"]) {
      await harness.click(view);
      expect(shown(harness.$("#terminal-dock")), view).toBe(true);
    }
  });

  it("opens the terminal guide from the dock's help button", async () => {
    harness = await bootApp({ open: {} });
    await harness.click("#terminal-help");
    expect(harness.backend.callsTo("OpenHelpDestination")).toEqual([["terminals"]]);
  });

  it("re-reads the snapshot when the window is shown again, not while hidden", async () => {
    harness = await bootApp({ open: {} });
    const reads = () => harness.backend.callsTo("GetWorkspaceSnapshot").length;
    const before = reads();
    harness.document.hidden = true;
    fire(harness.document, "visibilitychange");
    await harness.settle();
    expect(reads()).toBe(before);
    harness.document.hidden = false;
    fire(harness.document, "visibilitychange");
    await harness.settle();
    expect(reads()).toBe(before + 1);
  });

  it("confirms a close only for the workspace it closed", async () => {
    harness = await bootApp({
      open: {},
      responses: {
        CloseProject: {
          state: { status: "closed", generation: 4 },
          requiresConfirmation: false,
          confirmationToken: "",
          activeResources: { terminals: 0, agentRuns: 0 },
        },
      },
    });
    await harness.click("#close-project-button");
    expect(harness.backend.callsTo("CloseProject")).toEqual([[""]]);
    // A newer workspace publishes before the delayed confirmation runs.
    harness.app.workspaceController.publish({ status: "open", generation: 9 });
    const reads = harness.backend.callsTo("GetWorkspaceState").length;
    await new Promise((resolve) => harness.dom.window.setTimeout(resolve, 400));
    await harness.settle();
    expect(harness.backend.callsTo("GetWorkspaceState")).toHaveLength(reads);
  });

  it("re-reads the closed workspace's state after a close", async () => {
    harness = await bootApp({
      open: {},
      responses: {
        CloseProject: {
          state: { status: "closed", generation: 4 },
          requiresConfirmation: false,
          confirmationToken: "",
          activeResources: { terminals: 0, agentRuns: 0 },
        },
      },
    });
    await harness.click("#close-project-button");
    harness.backend.responses.GetWorkspaceState = { status: "welcome", generation: 4, version: "1.2.3" };
    const reads = harness.backend.callsTo("GetWorkspaceState").length;
    await new Promise((resolve) => harness.dom.window.setTimeout(resolve, 400));
    await harness.settle();
    expect(harness.backend.callsTo("GetWorkspaceState")).toHaveLength(reads + 1);
    expect(harness.$("#workspace-state-heading").textContent).toBe("Projects");
  });
});

describe("desktop backend access", () => {
  it("names the backend in its own words when the bridge is missing", () => {
    const saved = globalThis.window;
    globalThis.window = {};
    try {
      expect(() => desktopBackend()).toThrow("The p-track backend is not ready");
    } finally {
      globalThis.window = saved;
    }
  });
});
