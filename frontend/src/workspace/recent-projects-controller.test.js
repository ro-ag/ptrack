// The Projects landing's recent-project actions, driven through the window:
// what each one asks the backend, what it refuses to replay, and what the
// landing says while it works.
import { afterEach, describe, expect, it } from "vitest";

import { board, bootApp, snapshot } from "../test-support/app-harness";
import { relativeTime } from "./format";
import { recentOpenFailureMessage } from "./recent-projects-controller";

const entry = (id, availability = "available") => ({
  entryId: `entry-${id}`,
  base: `base-${id}`,
  name: `Project ${id}`,
  canonicalPath: `/projects/p${id}`,
  lastOpenedAt: "2026-09-20T10:00:00Z",
  availability,
});

const opened = (id, fields = {}) => ({
  entryId: `entry-${id}`,
  registryBase: `base-${id}`,
  registryStatus: "unchanged",
  open: {
    state: { status: "open", generation: 5, version: "1.2.3", project: { root: `/projects/p${id}`, name: `Project ${id}` } },
    requiresConfirmation: false,
    confirmationToken: "",
  },
  ...fields,
});

function actionButton(harness, entryId, action) {
  return harness.$$("button").find((button) => button.dataset.recentFocusKey === `${entryId}:${action}`);
}

function landing(projects, responses = {}) {
  return bootApp({
    responses: {
      GetRecentProjectsV1: { projects },
      GetWorkspaceSnapshot: (generation) => snapshot(board(), generation),
      ...responses,
    },
  });
}

describe("recent projects on the landing", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("re-reads the list, then opens the refreshed entry", async () => {
    harness = await landing([entry(1)], { OpenRecentProjectV1: opened(1) });
    const button = actionButton(harness, "entry-1", "open");
    expect(button.textContent).toBe("Open");
    expect(button.getAttribute("aria-label")).toBe("Open Project 1");
    expect(button.getAttribute("aria-describedby")).toBeTruthy();
    const reads = harness.backend.callsTo("GetRecentProjectsV1").length;
    await harness.click(button);
    expect(harness.backend.callsTo("GetRecentProjectsV1")).toHaveLength(reads + 1);
    expect(harness.backend.callsTo("OpenRecentProjectV1"))
      .toEqual([["entry-1", "base-1", "/projects/p1", "", ""]]);
    expect(harness.$("#workspace-state-screen").hidden).toBe(true);
    expect(harness.toast()).toBe("");
  });

  it("never replays Open when the refreshed list no longer carries the entry", async () => {
    let reads = 0;
    harness = await landing([], {
      GetRecentProjectsV1: () => ({ projects: (reads += 1) === 1 ? [entry(1)] : [entry(1, "missing")] }),
    });
    await harness.click(actionButton(harness, "entry-1", "open"));
    expect(harness.backend.names()).not.toContain("OpenRecentProjectV1");
    expect(harness.$("#recent-project-error").textContent).toBe(
      "p-track could not confirm the recent-project action: The recent project changed while p-track refreshed it. Review the updated list and choose again.",
    );
  });

  it("holds the other project journeys while an open is in flight", async () => {
    let finish;
    harness = await landing([entry(1)], {
      OpenRecentProjectV1: () => new Promise((resolve) => { finish = resolve; }),
    });
    await harness.click(actionButton(harness, "entry-1", "open"));
    expect(harness.$("#state-initialize-project-button").disabled).toBe(true);
    expect(harness.$("#state-open-project-button").disabled).toBe(true);
    expect(harness.$("#app-version").disabled).toBe(true);
    expect(harness.$("#recent-project-list").getAttribute("aria-busy")).toBe("true");
    harness.app.lifecycle.requestInitializeProject();
    harness.app.lifecycle.requestOpenProject();
    await harness.settle();
    expect(harness.backend.names()).not.toContain("PickProjectDirectory");
    finish(opened(1));
    await harness.settle();
    expect(harness.$("#workspace-state-screen").hidden).toBe(true);
  });

  it("warns when the project opened but its recent entry went stale", async () => {
    harness = await landing([entry(1)], {
      OpenRecentProjectV1: opened(1, {
        registryStatus: "stale",
        open: { ...opened(1).open, warning: "Terminals were not restored." },
      }),
    });
    const reads = harness.backend.callsTo("GetRecentProjectsV1").length;
    await harness.click(actionButton(harness, "entry-1", "open"));
    expect(harness.toast()).toBe(
      "Terminals were not restored. Project opened, but its recent entry changed elsewhere and was not updated.",
    );
    expect(harness.backend.callsTo("GetRecentProjectsV1")).toHaveLength(reads + 2);
  });

  it("forgets an unavailable entry only after confirming, leaving project files alone", async () => {
    let listed = [entry(1, "missing")];
    harness = await landing([], {
      GetRecentProjectsV1: () => ({ projects: listed }),
      ForgetRecentProjectV1: (entryId) => {
        listed = [];
        return { entryId, registryBase: "base-1", forgotten: true };
      },
    });
    expect(actionButton(harness, "entry-1", "locate").textContent).toBe("Locate…");
    const forget = actionButton(harness, "entry-1", "forget");
    expect(forget.getAttribute("aria-label")).toBe("Forget Project 1 from Recent projects only");
    await harness.click(forget);
    expect(harness.$("#workspace-confirm-modal").hidden).toBe(false);
    expect(harness.$("#workspace-confirm-detail").textContent).toBe(
      "Remove “Project 1” at /projects/p1 from Recent projects only. Project files will not be changed.",
    );
    expect(harness.backend.names()).not.toContain("ForgetRecentProjectV1");
    await harness.click("#workspace-confirm-submit");
    expect(harness.backend.callsTo("ForgetRecentProjectV1")).toEqual([["entry-1", "base-1"]]);
    expect(harness.$("#recent-project-status").textContent)
      .toBe("Removed “Project 1” from Recent projects. Project files were not changed.");
  });

  it("points at the last project it did not reopen without taking focus", async () => {
    harness = await landing([entry(1), entry(2)], {
      GetPreferences: {
        storage: "ok",
        preferences: { startup: { restoreLastProject: true, lastProjectRoot: "/projects/p2" } },
      },
    });
    expect(harness.$("#recent-project-status").textContent)
      .toBe("“Project 2” is preselected as the last project p-track recorded. Confirm it to continue.");
    expect(actionButton(harness, "entry-2", "open")).toBeDefined();
    expect(harness.document.activeElement.id).toBe("recent-project-search");
  });

  it("retries a permission-blocked entry by resolving it before opening", async () => {
    let listed = [entry(1, "permission-required")];
    harness = await landing([], {
      GetRecentProjectsV1: () => ({ projects: listed }),
      ResolveRecentProjectV1: (entryId, base, path) => {
        listed = [entry(1)];
        return { entryId, base, canonicalRoot: path, name: "Project 1", resolution: "ready", confirmationToken: "" };
      },
      OpenRecentProjectV1: opened(1),
    });
    const retry = actionButton(harness, "entry-1", "retry");
    expect(retry.textContent).toBe("Try Again");
    await harness.click(retry);
    expect(harness.backend.callsTo("ResolveRecentProjectV1")).toEqual([["entry-1", "base-1", "/projects/p1"]]);
    expect(harness.backend.callsTo("OpenRecentProjectV1")).toHaveLength(1);
    expect(harness.$("#workspace-state-screen").hidden).toBe(true);
  });

  it("ignores the native Open menu while a recent-project open is in flight", async () => {
    let finish;
    harness = await landing([entry(1)], {
      OpenRecentProjectV1: () => new Promise((resolve) => { finish = resolve; }),
    });
    await harness.click(actionButton(harness, "entry-1", "open"));
    await harness.emit("workspace:open-requested");
    expect(harness.backend.names()).not.toContain("PickProjectDirectory");
    finish(opened(1));
    await harness.settle();
  });

  it("explains an expired list without replaying Open", () => {
    expect(recentOpenFailureMessage(entry(1), "recent-project-entry-stale")).toBe(
      "The Recent projects list for “Project 1” changed or expired. p-track refreshed it without replaying Open. Review the row and choose again.",
    );
    expect(recentOpenFailureMessage(entry(1), "disk offline")).toBe(
      "p-track could not confirm that “Project 1” opened. The recent entry was not replayed: disk offline",
    );
  });

  it("dates each project in words, with the exact time in its markup", async () => {
    harness = await landing([entry(1)]);
    const times = harness.$$("#welcome-panel time").filter((node) => node.dateTime === "2026-09-20T10:00:00.000Z");
    expect(times.length).toBeGreaterThan(0);
    expect(times[0].textContent).toContain(relativeTime(Date.parse("2026-09-20T10:00:00Z"), "long"));
  });

  it("asks before leaving live terminals, and cancels the switch when declined", async () => {
    harness = await landing([entry(1)], {
      OpenRecentProjectV1: opened(1, {
        open: {
          ...opened(1).open,
          requiresConfirmation: true,
          confirmationToken: "switch-1",
          activeResources: { terminals: 2, agentRuns: 0 },
        },
      }),
      CancelWorkspaceChange: { cancelled: true },
    });
    await harness.click(actionButton(harness, "entry-1", "open"));
    expect(harness.$("#workspace-confirm-modal").hidden).toBe(false);
    await harness.click("#workspace-confirm-cancel");
    expect(harness.backend.callsTo("CancelWorkspaceChange")).toEqual([["switch-1"]]);
    expect(harness.backend.callsTo("OpenRecentProjectV1")).toHaveLength(1);
    expect(harness.$("#recent-project-status").textContent).toBe("Project unchanged.");
  });
});

