// The first-plan onboarding after a new project commits: it holds the window
// still, says what it is saving, and hands the board back when it finishes.
import { afterEach, describe, expect, it } from "vitest";

import { bootApp } from "../test-support/app-harness";
import { journeyResponses, reachReview, shown } from "../test-support/journey";

function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}

async function reachOnboarding(overrides = {}) {
  const harness = await bootApp({ responses: journeyResponses(overrides) });
  await reachReview(harness);
  await harness.click("#setup-commit");
  expect(shown(harness.$("#onboarding-plan-form"))).toBe(true);
  return harness;
}

describe("first-plan onboarding", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("holds the plan list, sidebar, panels, palette, and About still until it finishes", async () => {
    harness = await reachOnboarding();
    const { $ } = harness;
    expect($("#sidebar-plan-list").inert).toBe(true);
    expect($("#sidebar-toggle").disabled).toBe(true);
    expect($("#sidebar-resize").inert).toBe(true);
    expect($("#plan-add").disabled).toBe(true);
    expect($("#board-panel-toggle").disabled).toBe(true);
    expect($("#terminal-panel-toggle").disabled).toBe(true);
    expect($("#app-version").disabled).toBe(true);
    expect($("#switch-project-button").disabled).toBe(true);
    expect($("#close-project-button").disabled).toBe(true);
    await harness.key(harness.document.body, "k", { metaKey: true });
    expect($("#palette").hidden).toBe(true);

    await harness.click("#onboarding-skip-plan");
    expect($("#sidebar-plan-list").inert).toBe(false);
    expect($("#sidebar-toggle").disabled).toBe(false);
    expect($("#sidebar-resize").inert).toBe(false);
    expect($("#terminal-panel-toggle").disabled).toBe(false);
    expect($("#app-version").disabled).toBe(false);
    await harness.key(harness.document.body, "k", { metaKey: true });
    expect($("#palette").hidden).toBe(false);
  });

  it("says it is saving or reconciling while each record is in flight", async () => {
    const plan = deferred();
    const task = deferred();
    const start = deferred();
    const replies = journeyResponses();
    harness = await reachOnboarding({
      CreateFirstPlanV1: () => plan.promise,
      CreateFirstTaskV1: () => task.promise,
      StartFirstTaskV1: () => start.promise,
    });
    const { $ } = harness;
    await harness.type("#onboarding-plan-title", "Launch plan");
    await harness.submit("#onboarding-plan-form");
    expect($("#onboarding-status").textContent).toBe("Saving or reconciling the first plan…");
    expect($("#onboarding-operation").getAttribute("aria-busy")).toBe("true");
    plan.resolve(replies.CreateFirstPlanV1);
    await harness.settle();
    expect($("#onboarding-operation").hasAttribute("aria-busy")).toBe(false);

    await harness.type("#onboarding-task-title", "Ship the first slice");
    $("#onboarding-start-now").checked = true;
    await harness.submit("#onboarding-task-form");
    expect($("#onboarding-status").textContent).toBe("Saving or reconciling the first task…");
    task.resolve(replies.CreateFirstTaskV1);
    await harness.settle();
    expect($("#onboarding-detail").textContent)
      .toBe("Task #21 is durable. p-track is reconciling the requested start.");
    start.resolve(replies.StartFirstTaskV1);
    await harness.settle();
    expect(shown($("#post-project-onboarding"))).toBe(false);
  });

  it("opens the new plan's board and moves focus to its title", async () => {
    harness = await reachOnboarding();
    await harness.type("#onboarding-plan-title", "Launch plan");
    await harness.submit("#onboarding-plan-form");
    await harness.click("#onboarding-finish-with-plan");
    expect(harness.backend.callsTo("GetWorkspaceSnapshot").at(-1)).toEqual([7, 11]);
    expect(harness.document.activeElement.id).toBe("plan-title");
    expect(shown(harness.$("#board"))).toBe(true);
  });

  it("skipping the plan opens the empty board and focuses the project name", async () => {
    harness = await reachOnboarding();
    await harness.click("#onboarding-skip-plan");
    expect(harness.backend.callsTo("GetWorkspaceSnapshot").at(-1)).toEqual([7, 0]);
    expect(harness.document.activeElement.id).toBe("project-name");
    expect(harness.backend.names()).not.toContain("CreateFirstPlanV1");
  });

  it("ignores plan selection from the sidebar while onboarding runs", async () => {
    harness = await reachOnboarding();
    harness.app.board.selectPlan(3);
    await harness.settle();
    expect(harness.backend.names()).not.toContain("SetActivePlanV1");
    expect(harness.backend.callsTo("GetWorkspaceSnapshot")).toEqual([]);
  });
});
