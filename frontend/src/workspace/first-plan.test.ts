import { describe, expect, it } from "vitest";

import {
  firstPlanExitFocusTarget,
  firstPlanFocusTarget,
  initialFirstPlanState,
  parseCreateFirstPlanResult,
  parseFirstTaskResult,
  parseFirstTaskStartResult,
  reduceFirstPlan,
  validateOnboardingTitle,
} from "./first-plan";

describe("post-project first plan onboarding", () => {
  it("uses a guaranteed-visible board heading when Skip keeps the sidebar hidden", () => {
    expect(firstPlanExitFocusTarget(0, true)).toBe("plan-title");
    expect(firstPlanExitFocusTarget(0, false)).toBe("project-name");
    expect(firstPlanExitFocusTarget(11, true)).toBe("plan-title");
  });

  it("validates trimmed plan and task titles by UTF-8 byte length", () => {
    expect(validateOnboardingTitle(" Plan one ", "plan")).toMatchObject({
      value: "Plan one",
      error: "",
    });
    expect(validateOnboardingTitle("", "task").error).toBe("Enter a task title.");
    expect(validateOnboardingTitle("a".repeat(240), "plan").error).toBe("");
    expect(validateOnboardingTitle("a".repeat(241), "plan").error).toContain("240");
    expect(validateOnboardingTitle("é".repeat(120), "task").byteLength).toBe(240);
    expect(validateOnboardingTitle("é".repeat(121), "task").error).toContain("240");
  });

  it("preserves the active plan and task input across recoverable failures", () => {
    const plan = reduceFirstPlan(initialFirstPlanState, {
      type: "begin",
      generation: 7,
    });
    const creatingPlan = reduceFirstPlan(plan, {
      type: "createPlan",
      title: "First plan",
    });
    const task = reduceFirstPlan(creatingPlan, {
      type: "planCreated",
      planId: 11,
      title: "First plan",
    });
    const creatingTask = reduceFirstPlan(task, {
      type: "createTask",
      title: "First task",
      startNow: true,
    });
    const failed = reduceFirstPlan(creatingTask, {
      type: "taskFailed",
      message: "try again",
    });

    expect(failed).toMatchObject({
      phase: "task-create-failed",
      generation: 7,
      planId: 11,
      activePlanTitle: "First plan",
      taskTitle: "First task",
      startNow: true,
      message: "try again",
    });
    expect(firstPlanFocusTarget(failed)).toBe("onboarding-task-title");
  });

  it("keeps a created task in Todo when starting fails and supports retry", () => {
    const creating = {
      ...initialFirstPlanState,
      phase: "creating-task" as const,
      generation: 7,
      planId: 11,
      activePlanTitle: "First plan",
      taskTitle: "First task",
      startNow: true,
    };
    const starting = reduceFirstPlan(creating, {
      type: "taskCreated",
      taskId: 21,
      updatedAt: "2026-08-14T12:00:00Z",
      status: "todo",
    });
    const failed = reduceFirstPlan(starting, {
      type: "taskStartFailed",
      message: "resource busy",
    });

    expect(failed).toMatchObject({
      phase: "task-start-failed",
      taskId: 21,
      taskUpdatedAt: "2026-08-14T12:00:00Z",
      message: "resource busy",
    });
    expect(firstPlanFocusTarget(failed)).toBe("onboarding-retry-start");
    expect(reduceFirstPlan(failed, { type: "retryStart" }).phase).toBe("starting-task");
  });

  it("treats a natural first-task replay already in Doing as complete", () => {
    const creating = {
      ...initialFirstPlanState,
      phase: "creating-task" as const,
      generation: 7,
      planId: 11,
      taskTitle: "First task",
      startNow: true,
    };
    expect(reduceFirstPlan(creating, {
      type: "taskCreated",
      taskId: 21,
      updatedAt: "2026-08-14T12:01:00Z",
      status: "doing",
    }).phase).toBe("complete");
  });

  it("pins the first-plan response to the live workspace generation", () => {
    const result = parseCreateFirstPlanResult({
      plan: {
        id: 11,
        title: "First plan",
        status: "active",
        createdAt: "2026-08-14T12:00:00Z",
        updatedAt: "2026-08-14T12:00:00Z",
      },
      state: { status: "open", generation: 7 },
    }, 7, "First plan");
    expect(result.plan.id).toBe(11);
    expect(() => parseCreateFirstPlanResult({
      ...result,
      state: { status: "open", generation: 8 },
    }, 7, "First plan")).toThrow("stale workspace generation");
    expect(() => parseCreateFirstPlanResult(result, 0, "First plan"))
      .toThrow("open workspace generation");
  });

  it("accepts idempotent Todo/Doing first-task receipts and exact Doing starts", () => {
    for (const status of ["todo", "doing"] as const) {
      expect(parseFirstTaskResult({
        task: {
          id: 21,
          planId: 11,
          title: "First task",
          status,
          createdAt: "2026-08-14T12:00:00Z",
          updatedAt: "2026-08-14T12:01:00Z",
        },
        state: { status: "open", generation: 7 },
      }, 7, 11, "First task").task.status).toBe(status);
    }
    expect(parseFirstTaskStartResult({
      task: {
        id: 21,
        planId: 11,
        title: "First task",
        status: "doing",
        createdAt: "2026-08-14T12:00:00Z",
        updatedAt: "2026-08-14T12:02:00Z",
      },
      state: { status: "open", generation: 7 },
    }, 7, 11, 21, "First task").task.status).toBe("doing");
  });
});
