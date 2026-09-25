import { afterEach, describe, expect, it } from "vitest";

import { board, bootApp, holdTimers, plan, snapshot, task } from "../test-support/app-harness";
import { fire } from "../test-support/fake-dom";

const detail = (generation, taskId) => ({
  generation,
  task: task(taskId, "todo", { title: "Write docs" }),
  notes: [],
  commits: [],
  issues: [],
});

function bootBoard(fields = {}, responses = {}) {
  const shown = board({
    plans: [plan(1, { isActive: true, title: "Launch", tasksTotal: 2, tasksDone: 1 })],
    tasks: [task(7, "todo", { title: "Write docs" }), task(8, "done")],
    ...fields,
  });
  return bootApp({
    responses: {
      GetWorkspaceSnapshot: (generation) => snapshot(shown, generation),
      GetTaskDetailV2: detail,
      ...responses,
    },
    open: {},
  });
}

function card(harness, id) {
  return harness.$(`.card[data-task-id="${id}"]`);
}

function zone(harness, id) {
  return harness.$(`.card[data-task-id="${id}"] .card-drag-zone`);
}

describe("board cards", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("opens the task drawer from the card, by click or by Enter, over the visible board", async () => {
    harness = await bootBoard();
    const timers = holdTimers(harness);
    await harness.click(zone(harness, 7));
    // The click waits out a possible double-click before it opens anything.
    expect(harness.backend.names()).not.toContain("GetTaskDetailV2");
    timers.find(({ delay }) => delay === 240).callback();
    await harness.settle();
    expect(harness.backend.callsTo("GetTaskDetailV2")).toEqual([[3, 7]]);
    expect(harness.$("#task-drawer").hidden).toBe(false);
    expect(harness.$("#board").hidden).toBe(false);
    await harness.key(harness.document.body, "Escape");
    expect(harness.$("#task-drawer").hidden).toBe(true);
    await harness.key(zone(harness, 7), "Enter");
    expect(harness.backend.callsTo("GetTaskDetailV2")).toHaveLength(2);
    expect(harness.$("#task-drawer").hidden).toBe(false);
  });

  it("renames on a double-click instead of opening the drawer", async () => {
    harness = await bootBoard();
    const timers = holdTimers(harness);
    await harness.click(zone(harness, 7));
    fire(zone(harness, 7), "dblclick");
    await harness.settle();
    for (const timer of timers) timer.callback();
    await harness.settle();
    expect(harness.backend.names()).not.toContain("GetTaskDetailV2");
    expect(harness.$("#task-drawer").hidden).toBe(true);
  });

  it("offers moves from the card menu, never a select on the card", async () => {
    harness = await bootBoard();
    expect(harness.$$(".card select")).toHaveLength(0);
    fire(card(harness, 7), "contextmenu", { clientX: 10, clientY: 10 });
    await harness.settle();
    const labels = harness.$$(".context-menu button").map((button) => button.textContent);
    expect(labels).toEqual(expect.arrayContaining([
      "Open details", "Move to Doing", "Move to Blocked", "Move to Done", "Add note",
    ]));
    expect(labels).not.toContain("Move to Todo");
  });
});

describe("adding a task", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("refuses a second submit while the first is in flight, then clears the title", async () => {
    let finish;
    harness = await bootBoard({}, {
      AddTaskV2: () => new Promise((resolve) => { finish = resolve; }),
    });
    const button = harness.$("#add-form button");
    await harness.type("#task-title", "Ship it");
    await harness.submit("#add-form");
    await harness.submit("#add-form");
    expect(harness.backend.callsTo("AddTaskV2")).toEqual([[3, 1, "Ship it"]]);
    expect(harness.$("#task-title").readOnly).toBe(true);
    expect(button.disabled).toBe(true);
    finish({ generation: 3 });
    await harness.settle();
    expect(harness.$("#task-title").readOnly).toBe(false);
    expect(button.disabled).toBe(false);
    expect(harness.$("#task-title").value).toBe("");
  });

  it("keeps a title edited while the add was in flight", async () => {
    let finish;
    harness = await bootBoard({}, {
      AddTaskV2: () => new Promise((resolve) => { finish = resolve; }),
    });
    await harness.type("#task-title", "Ship it");
    await harness.submit("#add-form");
    harness.$("#task-title").value = "Ship it twice";
    finish({ generation: 3 });
    await harness.settle();
    expect(harness.$("#task-title").value).toBe("Ship it twice");
  });
});

describe("plan closeout prompt", () => {
  let harness;
  afterEach(() => harness?.dom.restore());
  const finished = {
    plans: [plan(1, { isActive: true, title: "Launch", tasksTotal: 2, tasksDone: 2 })],
    tasks: [task(7, "done"), task(8, "done")],
  };

  it("asks with the dialog, focusing Not now rather than the submit button", async () => {
    harness = await bootBoard(finished);
    expect(harness.$("#plan-dialog").hidden).toBe(false);
    expect(harness.$("#plan-dialog-heading").textContent).toBe("Mark “Launch” done?");
    expect(harness.$("#plan-dialog-cancel").textContent).toBe("Not now");
    expect(harness.document.activeElement.id).toBe("plan-dialog-cancel");
  });

  it("shows a neutral reminder instead when the person is typing", async () => {
    harness = await bootBoard();
    harness.$("#task-title").focus();
    harness.backend.responses.GetWorkspaceSnapshot = (generation) => snapshot(board(finished), generation);
    await harness.emit("workspace:data-changed");
    expect(harness.$("#plan-dialog").hidden).toBe(true);
    expect(harness.document.activeElement.id).toBe("task-title");
    const banner = harness.$("#notice-stack .plan-closeout-banner");
    expect(banner.getAttribute("role")).toBe("status");
    const review = banner.querySelector("button");
    expect(review.textContent).toBe("Review plan closeout");
    await harness.click(review);
    expect(harness.$("#plan-dialog").hidden).toBe(false);
    expect(harness.$("#notice-stack .plan-closeout-banner")).toBeNull();
  });

  it("holds the dialog open and busy while the plan is being closed", async () => {
    let finish;
    harness = await bootBoard(finished, {
      CompletePlanV1: () => new Promise((resolve) => { finish = resolve; }),
    });
    await harness.submit("#plan-dialog-form");
    expect(harness.backend.callsTo("CompletePlanV1")).toEqual([[3, 1]]);
    expect(harness.$("#plan-dialog-cancel").disabled).toBe(true);
    expect(harness.$("#plan-dialog-form").getAttribute("aria-busy")).toBe("true");
    await harness.key(harness.document.body, "Escape");
    await harness.click("#plan-dialog-cancel");
    expect(harness.$("#plan-dialog").hidden).toBe(false);
    finish({ generation: 3, checkpoint: { markdown: "# Checkpoint", openPlans: [] } });
    await harness.settle();
    expect(harness.$("#plan-dialog-cancel").disabled).toBe(false);
  });
});

describe("plan dialog and workspace changes", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("closes a busy plan dialog when its workspace goes away", async () => {
    harness = await bootBoard({
      plans: [plan(1, { isActive: true, title: "Launch", tasksTotal: 2, tasksDone: 2 })],
      tasks: [task(7, "done"), task(8, "done")],
    }, {
      CompletePlanV1: () => new Promise(() => {}),
    });
    await harness.submit("#plan-dialog-form");
    expect(harness.$("#plan-dialog-cancel").disabled).toBe(true);
    harness.app.workspaceController.publish({ status: "welcome", generation: 4 });
    harness.app.shell.renderWorkspaceState({ status: "welcome", generation: 4, version: "1.2.3" }, false);
    await harness.settle();
    expect(harness.$("#plan-dialog").hidden).toBe(true);
    expect(harness.$("#plan-dialog-cancel").disabled).toBe(false);
  });
});

describe("native close refusal", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("says why the window stayed open as a warning notice", async () => {
    harness = await bootApp();
    await harness.emit("app:close-refused", "a save is still running");
    expect(harness.toast()).toBe("p-track could not close yet: a save is still running");
    expect(harness.$("#toast").dataset.tone).toBe("warning");
  });

  it("reports a failed shell-command install from the native menu", async () => {
    harness = await bootApp({ responses: { InstallShellCommand: new Error("permission denied") } });
    await harness.emit("workspace:install-shell-command-requested");
    expect(harness.backend.callsTo("InstallShellCommand")).toEqual([[]]);
    expect(harness.toast()).toBe("Could not install the shell command: permission denied");
  });
});
