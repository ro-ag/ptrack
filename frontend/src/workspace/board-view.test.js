import { afterEach, describe, expect, it } from "vitest";

import { board, bootApp, plan, snapshot, task } from "../test-support/app-harness";

const plans = [
  plan(1, { isActive: true, title: "Launch", tasksTotal: 1 }),
  plan(2, { title: "Polish" }),
  plan(3, { title: "Beta", status: "done", tasksTotal: 2, tasksDone: 2 }),
];

// The snapshot follows the plan the window asks for; the project's current
// plan is whichever SetActivePlanV1 last made current.
function project({ setActive = (generation) => ({ generation }) } = {}) {
  let active = 1;
  return {
    responses: {
      SetActivePlanV1: (generation, planId) => {
        const reply = setActive(generation, planId);
        active = planId;
        return reply;
      },
      GetWorkspaceSnapshot: (generation, planId) => snapshot(
        board({
          planId: planId || active,
          plans: plans.map((entry) => ({ ...entry, isActive: entry.id === active })),
          tasks: [task(10 + (planId || active))],
        }),
        generation,
      ),
    },
    open: { generation: 3 },
  };
}

function boot(options) {
  return bootApp(project(options));
}

function planRow(harness, id) {
  return [...harness.$$("#sidebar-plan-list .sidebar-plan")]
    .find((row) => row.querySelector(".sidebar-plan-title").textContent.startsWith(`#${id} `));
}

describe("choosing a plan in the sidebar", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("makes an open plan the project's current plan, then shows its board", async () => {
    harness = await boot();
    await harness.click(planRow(harness, 2));
    expect(harness.backend.callsTo("SetActivePlanV1")).toEqual([[3, 2]]);
    const names = harness.backend.names();
    expect(names.lastIndexOf("GetWorkspaceSnapshot")).toBeGreaterThan(names.indexOf("SetActivePlanV1"));
    expect(harness.backend.callsTo("GetWorkspaceSnapshot").at(-1)).toEqual([3, 2]);
    expect(harness.$("#plan-title").textContent).toBe("Polish");
    expect(harness.$("#plan-eyebrow").textContent).toBe("Current plan");
    expect(harness.$(".sidebar-current-title").textContent).toBe("#2 Polish");
    expect(harness.toast()).toBe("");
  });

  it("views a done plan without moving the current plan", async () => {
    harness = await boot();
    await harness.click(planRow(harness, 3));
    expect(harness.backend.callsTo("SetActivePlanV1")).toEqual([]);
    expect(harness.backend.callsTo("GetWorkspaceSnapshot").at(-1)).toEqual([3, 3]);
    expect(harness.$("#plan-title").textContent).toBe("Beta");
    expect(harness.$("#plan-eyebrow").textContent).toBe("Viewing #3 (done)");
    // The pinned card keeps naming the plan new work goes to, and the viewed
    // plan stays in the rows, marked as the one on the board.
    expect(harness.$(".sidebar-current-title").textContent).toBe("#1 Launch");
    expect(planRow(harness, 3).getAttribute("aria-current")).toBe("true");
    expect(planRow(harness, 1)).toBeUndefined();
  });

  it("does not ask again for the plan that is already current", async () => {
    harness = await boot();
    await harness.click(".sidebar-current-card");
    expect(harness.backend.callsTo("SetActivePlanV1")).toEqual([]);
    expect(harness.backend.callsTo("GetWorkspaceSnapshot").at(-1)).toEqual([3, 1]);
  });

  it("keeps the board on its plan when the change is refused", async () => {
    const refusal = "plan #2 is claimed by Ada; take it over with 'ptrack plan use 2 --steal'";
    harness = await boot({ setActive: () => { throw new Error(refusal); } });
    const reads = harness.backend.callsTo("GetWorkspaceSnapshot").length;
    await harness.click(planRow(harness, 2));
    expect(harness.toast()).toBe(refusal);
    expect(harness.$("#status").textContent).toBe("Could not make plan #2 the current plan");
    expect(harness.backend.callsTo("GetWorkspaceSnapshot")).toHaveLength(reads);
    expect(harness.$("#plan-title").textContent).toBe("Launch");
    expect(harness.$(".sidebar-current-title").textContent).toBe("#1 Launch");
  });

  it("ignores a reply from a workspace generation that has moved on", async () => {
    harness = await boot({ setActive: () => ({ generation: 4 }) });
    const reads = harness.backend.callsTo("GetWorkspaceSnapshot").length;
    await harness.click(planRow(harness, 2));
    expect(harness.backend.callsTo("GetWorkspaceSnapshot")).toHaveLength(reads);
    expect(harness.$("#plan-title").textContent).toBe("Launch");
  });

  it("drops a task open that waited on a refused plan change", async () => {
    let refuse = true;
    harness = await boot({
      setActive: (generation) => {
        if (refuse) throw new Error("plan #2 is claimed by Ada; take it over with 'ptrack plan use 2 --steal'");
        return { generation };
      },
    });
    harness.backend.responses.GetTaskDetailV2 = (generation, taskId) => ({
      generation, task: task(taskId), notes: [], commits: [], issues: [],
    });
    // What the palette does for a task on another plan.
    harness.app.drawer.requestPendingTaskDetail(12);
    harness.app.board.selectPlan(2);
    await harness.settle();
    refuse = false;
    await harness.click(planRow(harness, 2));
    expect(harness.$("#plan-title").textContent).toBe("Polish");
    expect(harness.backend.names()).not.toContain("GetTaskDetailV2");
    expect(harness.$("#task-drawer").hidden).toBe(true);
  });
});

