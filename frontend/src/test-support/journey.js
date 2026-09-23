// Scripted first-run journeys: the replies a new folder gets from the picker
// through its first task, and helpers that read the setup screens back.
import { board, snapshot } from "./app-harness";

export const operationId = "o".repeat(43);
export const canonicalRoot = "/projects/alpha";
export const createdAt = "2026-08-14T12:00:00Z";
export const goal = "Make first launch trustworthy";
export const openState = (generation) => ({
  status: "open",
  generation,
  version: "1.2.3",
  project: { root: canonicalRoot, name: "alpha" },
});
export const complete = {
  operationId,
  canonicalRoot,
  outcome: "complete",
  checkpoint: "desktop-bound",
  errorKind: "",
};

/** Whether a person can see `node`: neither it nor an ancestor is hidden. */
export function shown(node) {
  for (let current = node; current && current.tagName; current = current.parentNode) {
    if (current.hidden) return false;
  }
  return Boolean(node);
}

export function journeyResponses(overrides = {}) {
  return {
    PickProjectDirectory: canonicalRoot,
    ValidateProjectTargetV1: { kind: "new", canonicalRoot, operationId },
    InitializeProjectV1: { initialization: complete, state: openState(7) },
    GetInitializationStatusV1: complete,
    GetWorkspaceSnapshot: (generation) => snapshot(board({ planId: 0, plans: [] }), generation),
    CreateFirstPlanV1: {
      plan: { id: 11, title: "Launch plan", status: "active", createdAt, updatedAt: createdAt },
      state: { status: "open", generation: 7 },
    },
    CreateFirstTaskV1: {
      task: { id: 21, planId: 11, title: "Ship the first slice", status: "todo", createdAt, updatedAt: createdAt },
      state: { status: "open", generation: 7 },
    },
    StartFirstTaskV1: {
      task: {
        id: 21,
        planId: 11,
        title: "Ship the first slice",
        status: "doing",
        createdAt,
        updatedAt: "2026-08-14T12:01:00Z",
      },
      state: { status: "open", generation: 7 },
    },
    ...overrides,
  };
}

/** The setup and onboarding sections a person can see right now. */
export function visibleSteps(harness) {
  return [...harness.$$("[id]")]
    .filter(shown)
    .map((node) => node.id)
    .filter((id) => /^(setup|onboarding)-.*(actions|form|review|guide|panel)$/.test(id));
}

/** The journey calls, without the reads every window boot makes. */
export function journeyCalls(harness) {
  const journey = new Set([
    "PickProjectDirectory",
    "ValidateProjectTargetV1",
    "InitializeProjectV1",
    "GetInitializationStatusV1",
    "OpenProject",
    "CreateFirstPlanV1",
    "CreateFirstTaskV1",
    "StartFirstTaskV1",
  ]);
  return harness.backend.calls.filter(([method]) => journey.has(method));
}

export async function reachReview(harness) {
  await harness.click("#state-initialize-project-button");
  await harness.type("#setup-goal", goal);
  await harness.submit("#setup-goal-form");
  await harness.click("#setup-guide-skip");
}

export const request = { operationId, root: canonicalRoot, goal, guideChoice: "skip", guidePreviewToken: "" };
