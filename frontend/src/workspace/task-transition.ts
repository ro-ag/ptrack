export interface TaskTransitionConfirmation {
  token: string;
  expiresAt: string;
  activeTerminals: number;
  activeAgents: number;
}

export interface TaskTransitionResult {
  generation: number;
  taskId: number;
  fromStatus: string;
  toStatus: string;
  applied: boolean;
  requiresConfirmation: boolean;
  confirmation?: TaskTransitionConfirmation;
}

export interface ExpectedTaskTransition {
  generation: number;
  taskId: number;
  fromStatus: string;
  toStatus: string;
}

const validCount = (value: unknown) =>
  Number.isSafeInteger(value) && Number(value) >= 0;

export function taskTransitionCanStart(
  hasActiveRequest: boolean,
  busy: boolean,
): boolean {
  return !hasActiveRequest && !busy;
}

export type TaskTransitionOrigin = "card-select" | "drawer-select" | "drag";
export type TaskTransitionFocusIntent = "card-select" | "drawer-select" | "drag" | "none";

export function taskTransitionFocusIntent(
  origin: TaskTransitionOrigin,
  drawerOpen: boolean,
  drawerMatchesTask: boolean,
): TaskTransitionFocusIntent {
  if (origin !== "drawer-select") return origin;
  if (!drawerOpen) return "card-select";
  return drawerMatchesTask ? "drawer-select" : "none";
}

export function taskTransitionResponseIsCurrent(
  result: TaskTransitionResult | null | undefined,
  expected: ExpectedTaskTransition,
): boolean {
  if (!result ||
    Number(result.generation) !== expected.generation ||
    Number(result.taskId) !== expected.taskId ||
    result.fromStatus !== expected.fromStatus ||
    result.toStatus !== expected.toStatus) {
    return false;
  }
  if (result.applied) {
    return !result.requiresConfirmation && result.confirmation === undefined;
  }
  const challenge = result.confirmation;
  return result.requiresConfirmation === true &&
    typeof challenge?.token === "string" && challenge.token.length > 0 &&
    typeof challenge.expiresAt === "string" && challenge.expiresAt.length > 0 &&
    validCount(challenge.activeTerminals) && validCount(challenge.activeAgents);
}

function resourceCount(count: number, singular: string): string {
  return `${count} active ${singular}${count === 1 ? "" : "s"}`;
}

export function taskTransitionConfirmationCopy(
  taskId: number,
  fromLabel: string,
  toLabel: string,
  confirmation: TaskTransitionConfirmation,
): string {
  const resources = [
    resourceCount(confirmation.activeTerminals, "terminal"),
    resourceCount(confirmation.activeAgents, "agent"),
  ].join(" and ");
  return `Task #${taskId} has ${resources} linked to it. Move from ${fromLabel} to ${toLabel}? ` +
    "This changes only task status; linked sessions, processes, and capabilities stay unchanged.";
}
