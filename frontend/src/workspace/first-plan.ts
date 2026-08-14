export type FirstPlanPhase =
  | "idle"
  | "plan"
  | "creating-plan"
  | "plan-failed"
  | "task"
  | "creating-task"
  | "task-create-failed"
  | "starting-task"
  | "task-start-failed"
  | "complete";

export interface FirstPlanState {
  phase: FirstPlanPhase;
  generation: number;
  planTitle: string;
  planError: string;
  planId: number;
  activePlanTitle: string;
  taskTitle: string;
  taskError: string;
  startNow: boolean;
  taskId: number;
  taskUpdatedAt: string;
  message: string;
}

export const initialFirstPlanState: FirstPlanState = Object.freeze({
  phase: "idle",
  generation: 0,
  planTitle: "",
  planError: "",
  planId: 0,
  activePlanTitle: "",
  taskTitle: "",
  taskError: "",
  startNow: false,
  taskId: 0,
  taskUpdatedAt: "",
  message: "",
});

export type FirstPlanEvent =
  | { type: "begin"; generation: number }
  | { type: "planInvalid"; title: string; message: string }
  | { type: "createPlan"; title: string }
  | { type: "planCreated"; planId: number; title: string }
  | { type: "planFailed"; message: string }
  | { type: "taskInvalid"; title: string; startNow: boolean; message: string }
  | { type: "createTask"; title: string; startNow: boolean }
  | {
    type: "taskCreated";
    taskId: number;
    updatedAt: string;
    status: "todo" | "doing";
  }
  | { type: "taskFailed"; message: string }
  | { type: "retryStart" }
  | { type: "taskStartFailed"; message: string }
  | { type: "taskStarted" }
  | { type: "finish" };

export function reduceFirstPlan(
  state: FirstPlanState,
  event: FirstPlanEvent,
): FirstPlanState {
  switch (event.type) {
    case "begin":
      return {
        ...initialFirstPlanState,
        phase: "plan",
        generation: event.generation,
      };
    case "planInvalid":
      return {
        ...state,
        phase: "plan",
        planTitle: event.title,
        planError: event.message,
      };
    case "createPlan":
      return {
        ...state,
        phase: "creating-plan",
        planTitle: event.title,
        planError: "",
        message: "",
      };
    case "planCreated":
      return {
        ...state,
        phase: "task",
        planId: event.planId,
        activePlanTitle: event.title,
        planTitle: event.title,
        message: "",
      };
    case "planFailed":
      return { ...state, phase: "plan-failed", message: event.message };
    case "taskInvalid":
      return {
        ...state,
        phase: "task",
        taskTitle: event.title,
        startNow: event.startNow,
        taskError: event.message,
      };
    case "createTask":
      return {
        ...state,
        phase: "creating-task",
        taskTitle: event.title,
        startNow: event.startNow,
        taskError: "",
        message: "",
      };
    case "taskCreated":
      return {
        ...state,
        phase: state.startNow && event.status === "todo"
          ? "starting-task"
          : "complete",
        taskId: event.taskId,
        taskUpdatedAt: event.updatedAt,
        message: "",
      };
    case "taskFailed":
      return { ...state, phase: "task-create-failed", message: event.message };
    case "retryStart":
      return { ...state, phase: "starting-task", message: "" };
    case "taskStartFailed":
      return { ...state, phase: "task-start-failed", message: event.message };
    case "taskStarted":
      return { ...state, phase: "complete", message: "" };
    case "finish":
      return { ...initialFirstPlanState };
  }
}

export function firstPlanFocusTarget(state: FirstPlanState): string {
  if (state.phase === "plan-failed" || (state.phase === "plan" && state.planError)) {
    return "onboarding-plan-title";
  }
  if (
    state.phase === "task-create-failed" ||
    (state.phase === "task" && state.taskError)
  ) {
    return "onboarding-task-title";
  }
  if (state.phase === "task-start-failed") return "onboarding-retry-start";
  return "onboarding-heading";
}

export function firstPlanExitFocusTarget(
  planId: number,
  sidebarHidden: boolean,
): "plan-title" | "project-name" {
  return planId > 0 || sidebarHidden ? "plan-title" : "project-name";
}

export interface OnboardingTitleValidation {
  value: string;
  byteLength: number;
  error: string;
}

export function validateOnboardingTitle(
  value: unknown,
  kind: "plan" | "task",
): OnboardingTitleValidation {
  const title = typeof value === "string" ? value.trim() : "";
  const byteLength = new TextEncoder().encode(title).byteLength;
  const label = kind === "plan" ? "plan" : "task";
  let error = "";
  if (!title) error = `Enter a ${label} title.`;
  else if (byteLength > 240) {
    error = `Keep the ${label} title to 240 UTF-8 bytes or fewer.`;
  }
  return { value: title, byteLength, error };
}

export interface FirstPlanResult {
  plan: {
    id: number;
    title: string;
    status: "active";
    createdAt: string;
    updatedAt: string;
  };
  state: { status: "open"; generation: number };
}

export function parseCreateFirstPlanResult(
  value: unknown,
  generation: number,
  title: string,
): FirstPlanResult {
  if (!Number.isSafeInteger(generation) || generation <= 0) {
    throw new Error("Plan creation requires an open workspace generation.");
  }
  if (!value || typeof value !== "object") {
    throw new Error("Plan creation returned an invalid result.");
  }
  const result = value as Record<string, unknown>;
  const plan = result.plan as Record<string, unknown> | null;
  const state = result.state as Record<string, unknown> | null;
  if (
    !plan ||
    !Number.isSafeInteger(plan.id) ||
    Number(plan.id) <= 0 ||
    plan.title !== title ||
    plan.status !== "active" ||
    typeof plan.createdAt !== "string" ||
    !plan.createdAt ||
    typeof plan.updatedAt !== "string" ||
    !plan.updatedAt
  ) {
    throw new Error("Plan creation did not return the requested active plan.");
  }
  if (
    !state ||
    state.status !== "open" ||
    state.generation !== generation
  ) {
    throw new Error("Plan creation returned a stale workspace generation.");
  }
  return {
    plan: {
      id: Number(plan.id),
      title,
      status: "active",
      createdAt: plan.createdAt,
      updatedAt: plan.updatedAt,
    },
    state: { status: "open", generation },
  };
}

export function parseFirstTaskResult(
  value: unknown,
  generation: number,
  planId: number,
  title: string,
): {
  task: {
    id: number;
    planId: number;
    title: string;
    status: "todo" | "doing";
    createdAt: string;
    updatedAt: string;
  };
  state: { status: "open"; generation: number };
} {
  if (!Number.isSafeInteger(generation) || generation <= 0) {
    throw new Error("Task creation requires an open workspace generation.");
  }
  if (!value || typeof value !== "object") {
    throw new Error("Task creation returned an invalid result.");
  }
  const result = value as Record<string, unknown>;
  const task = result.task as Record<string, unknown> | null;
  const state = result.state as Record<string, unknown> | null;
  if (
    !task ||
    !Number.isSafeInteger(task.id) ||
    Number(task.id) <= 0 ||
    task.planId !== planId ||
    task.title !== title ||
    !(task.status === "todo" || task.status === "doing") ||
    typeof task.createdAt !== "string" ||
    !task.createdAt ||
    typeof task.updatedAt !== "string" ||
    !task.updatedAt ||
    !state ||
    state.status !== "open" ||
    state.generation !== generation
  ) {
    throw new Error("Task creation returned a stale or unexpected task.");
  }
  return {
    task: {
      id: Number(task.id),
      planId,
      title,
      status: task.status,
      createdAt: task.createdAt,
      updatedAt: task.updatedAt,
    },
    state: { status: "open", generation },
  };
}

export function parseFirstTaskStartResult(
  value: unknown,
  generation: number,
  planId: number,
  taskId: number,
  title: string,
): {
  task: {
    id: number;
    planId: number;
    title: string;
    status: "doing";
    createdAt: string;
    updatedAt: string;
  };
  state: { status: "open"; generation: number };
} {
  if (!Number.isSafeInteger(generation) || generation <= 0) {
    throw new Error("Task start requires an open workspace generation.");
  }
  if (!value || typeof value !== "object") {
    throw new Error("Task start returned an invalid result.");
  }
  const result = value as Record<string, unknown>;
  const task = result.task as Record<string, unknown> | null;
  const state = result.state as Record<string, unknown> | null;
  if (
    !task ||
    task.id !== taskId ||
    task.planId !== planId ||
    task.title !== title ||
    task.status !== "doing" ||
    typeof task.createdAt !== "string" ||
    !task.createdAt ||
    typeof task.updatedAt !== "string" ||
    !task.updatedAt ||
    !state ||
    state.status !== "open" ||
    state.generation !== generation
  ) {
    throw new Error("Task start returned a stale or unexpected task.");
  }
  return {
    task: {
      id: taskId,
      planId,
      title,
      status: "doing",
      createdAt: task.createdAt,
      updatedAt: task.updatedAt,
    },
    state: { status: "open", generation },
  };
}

export interface FirstPlanJourneyApi {
  CreateFirstPlanV1(generation: number, title: string): Promise<unknown>;
  CreateFirstTaskV1(
    generation: number,
    planId: number,
    title: string,
  ): Promise<unknown>;
  StartFirstTaskV1(
    generation: number,
    taskId: number,
    expectedUpdatedAt: string,
  ): Promise<unknown>;
}

export async function createFirstPlan(
  api: Pick<FirstPlanJourneyApi, "CreateFirstPlanV1">,
  generation: number,
  title: string,
): Promise<FirstPlanResult> {
  return parseCreateFirstPlanResult(
    await api.CreateFirstPlanV1(generation, title),
    generation,
    title,
  );
}

export async function createFirstTask(
  api: Pick<FirstPlanJourneyApi, "CreateFirstTaskV1">,
  generation: number,
  planId: number,
  title: string,
): Promise<ReturnType<typeof parseFirstTaskResult>> {
  return parseFirstTaskResult(
    await api.CreateFirstTaskV1(generation, planId, title),
    generation,
    planId,
    title,
  );
}

export async function startFirstTask(
  api: Pick<FirstPlanJourneyApi, "StartFirstTaskV1">,
  generation: number,
  planId: number,
  taskId: number,
  title: string,
  expectedUpdatedAt: string,
): Promise<ReturnType<typeof parseFirstTaskStartResult>> {
  return parseFirstTaskStartResult(
    await api.StartFirstTaskV1(generation, taskId, expectedUpdatedAt),
    generation,
    planId,
    taskId,
    title,
  );
}
