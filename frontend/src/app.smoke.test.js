import { afterEach, describe, expect, it } from "vitest";

import { board, bootApp, plan, snapshot, task } from "./test-support/app-harness";

describe("window composition", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("boots the Projects landing from the backend's welcome state", async () => {
    harness = await bootApp();
    const { $, backend } = harness;
    expect(backend.names()).toContain("GetWorkspaceState");
    expect(backend.names()).toContain("GetRecentProjectsV1");
    expect($("#workspace-state-heading").textContent).toBe("Projects");
    expect($("#app-version").textContent).toBe("v1.2.3");
    expect(harness.toast()).toBe("");
  });

  it("boots straight into an open project and renders its board", async () => {
    harness = await bootApp({
      open: {
        snapshot: snapshot(board({
          plans: [plan(1, { isActive: true, title: "Launch", tasksTotal: 2, tasksDone: 1 })],
          tasks: [task(7, "todo", { title: "Write docs" }), task(8, "done")],
        })),
      },
    });
    const { $, $$ } = harness;
    expect(harness.toast()).toBe("");
    expect($("#plan-title").textContent).toBe("Launch");
    expect($("#workspace-state-screen").hidden).toBe(true);
    expect([...$$(".card-title")].map((node) => node.textContent)).toEqual(["Write docs", "Task 8"]);
    expect($("#status").textContent).toMatch(/^Snapshot synced/);
  });
});
