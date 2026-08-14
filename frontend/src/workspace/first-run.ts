export type FirstRunIntent = "initialize" | "open";

export type FirstRunPhase =
  | "idle"
  | "picking"
  | "validating"
  | "existing"
  | "target-new"
  | "goal"
  | "guide"
  | "guide-previewing"
  | "guide-stale"
  | "review"
  | "committing"
  | "reconciling"
  | "uncertain"
  | "recovery"
  | "failed";

export type FirstRunRecoveryMode =
  | "none"
  | "no-write"
  | "durable"
  | "blocked"
  | "ambiguous";

export interface FirstRunState {
  phase: FirstRunPhase;
  intent: FirstRunIntent;
  canonicalRoot: string;
  operationId: string;
  goal: string;
  goalError: string;
  guideChoice: ProjectGuideChoice;
  guideAvailable: boolean | null;
  guidePreviewToken: string;
  guideFiles: ProjectGuidePreviewFile[];
  guidePostCommit: boolean;
  guideSkipAllowed: boolean;
  guidePartiallyApplied: boolean;
  storageAlreadyCreated: boolean;
  resumedOperation: boolean;
  resumeLocked: boolean;
  resumeNoWrite: boolean;
  message: string;
  checkpoint: string;
  errorKind: string;
  recoveryMode: FirstRunRecoveryMode;
  returnFocusId: string;
}

export const initialFirstRunState: FirstRunState = Object.freeze({
  phase: "idle",
  intent: "initialize",
  canonicalRoot: "",
  operationId: "",
  goal: "",
  goalError: "",
  guideChoice: "skip",
  guideAvailable: null,
  guidePreviewToken: "",
  guideFiles: [],
  guidePostCommit: false,
  guideSkipAllowed: true,
  guidePartiallyApplied: false,
  storageAlreadyCreated: false,
  resumedOperation: false,
  resumeLocked: false,
  resumeNoWrite: false,
  message: "",
  checkpoint: "",
  errorKind: "",
  recoveryMode: "none",
  returnFocusId: "state-initialize-project-button",
});

export type FirstRunEvent =
  | { type: "pick"; intent: FirstRunIntent; returnFocusId: string }
  | { type: "repick"; intent: FirstRunIntent; returnFocusId: string }
  | { type: "pickerCancelled"; restore?: FirstRunState }
  | { type: "validate" }
  | { type: "existing"; canonicalRoot: string }
  | { type: "new"; canonicalRoot: string; operationId: string }
  | { type: "goalDrafted"; goal: string }
  | { type: "continueToGoal" }
  | {
    type: "resume";
    canonicalRoot: string;
    operationId: string;
    goal: string;
    guideChoice: ProjectGuideChoice;
    initialization: InitializationStatus;
  }
  | {
    type: "recovery";
    canonicalRoot?: string;
    operationId?: string;
    message: string;
    checkpoint?: string;
    errorKind?: string;
    durable?: boolean;
    resumable?: boolean;
  }
  | { type: "goalInvalid"; goal: string; message: string }
  | { type: "goalAccepted"; goal: string }
  | { type: "guidePreviewStarted" }
  | { type: "guidePreviewed"; preview: ProjectGuidePreview }
  | { type: "guidePreviewFailed"; message: string }
  | { type: "guideInstalled" }
  | { type: "guideSkipped" }
  | {
    type: "guideStale";
    postCommit: boolean;
    checkpoint?: string;
    skipAllowed?: boolean;
    partiallyApplied?: boolean;
    message?: string;
  }
  | { type: "back" }
  | { type: "commit" }
  | { type: "reconcile" }
  | { type: "uncertain"; message: string; checkpoint?: string }
  | {
    type: "failed";
    canonicalRoot?: string;
    operationId?: string;
    message: string;
    checkpoint?: string;
    errorKind?: string;
  }
  | { type: "reset"; focusId?: string };

export function reduceFirstRun(
  state: FirstRunState,
  event: FirstRunEvent,
): FirstRunState {
  switch (event.type) {
    case "pick":
      return {
        ...initialFirstRunState,
        phase: "picking",
        intent: event.intent,
        returnFocusId: event.returnFocusId,
      };
    case "repick":
      return {
        ...state,
        phase: "picking",
        intent: event.intent,
        goalError: "",
        message: "",
        returnFocusId: event.returnFocusId,
      };
    case "pickerCancelled":
      return event.restore
        ? { ...event.restore }
        : { ...initialFirstRunState, returnFocusId: state.returnFocusId };
    case "validate":
      return { ...state, phase: "validating", message: "", goalError: "" };
    case "existing":
      return {
        ...state,
        phase: "existing",
        canonicalRoot: event.canonicalRoot,
        operationId: "",
        message: "",
      };
    case "new":
      return {
        ...state,
        phase: "goal",
        canonicalRoot: event.canonicalRoot,
        operationId: event.operationId,
        message: "",
      };
    case "goalDrafted":
      return { ...state, goal: event.goal, goalError: "" };
    case "continueToGoal":
      if (state.phase !== "target-new") return state;
      return { ...state, phase: "goal", goalError: "", message: "" };
    case "resume": {
      const checkpoint = event.initialization.checkpoint;
      const guideApplied = event.initialization.checkpoint === "guide-applied";
      const partial = event.initialization.errorKind ===
        PROJECT_GUIDE_PARTIALLY_APPLIED_ERROR;
      const stale = event.initialization.errorKind === PROJECT_GUIDE_STALE_ERROR;
      const interruptedBeforeCommit = event.initialization.errorKind ===
        INTERRUPTED_BEFORE_COMMIT_ERROR && checkpoint === "none" &&
        event.initialization.outcome === "ready";
      const needsGuideReview = event.guideChoice === "install" &&
        !guideApplied && checkpoint !== "desktop-bound";
      const storageAlreadyCreated = [
        "runtime-committed",
        "project-committed",
        "guide-applied",
        "desktop-bound",
      ].includes(checkpoint);
      const resumeLocked = checkpoint !== "none" ||
        event.initialization.outcome === "in-progress" ||
        event.initialization.outcome === "recovery-required";
      const resumeNoWrite = checkpoint === "none" &&
        event.initialization.outcome === "ready";
      return {
        ...initialFirstRunState,
        phase: needsGuideReview ? "guide-stale" : "review",
        intent: state.intent,
        canonicalRoot: event.canonicalRoot,
        operationId: event.operationId,
        goal: event.goal,
        guideChoice: event.guideChoice,
        guidePostCommit: [
          "project-committed",
          "guide-applied",
          "desktop-bound",
        ].includes(checkpoint),
        guideSkipAllowed: event.guideChoice === "skip" ||
          (needsGuideReview && (stale || interruptedBeforeCommit) && !partial),
        guidePartiallyApplied: partial,
        checkpoint: event.initialization.checkpoint,
        errorKind: event.initialization.errorKind,
        recoveryMode: resumeLocked ? "durable" : "no-write",
        storageAlreadyCreated,
        resumedOperation: true,
        resumeLocked,
        resumeNoWrite,
        message: partial
          ? "Project guidance was partially applied before setup stopped."
          : stale
          ? PROJECT_GUIDE_PREVIEW_STALE
          : interruptedBeforeCommit
          ? "The previous attempt stopped before committing project files."
          : needsGuideReview
          ? "Review the current guide files before resuming initialization."
          : guideApplied
          ? "The durable guide step is complete."
          : storageAlreadyCreated
          ? "Private project storage is already durable."
          : "This initialization operation has durable progress and will resume safely.",
        returnFocusId: state.returnFocusId,
      };
    }
    case "goalInvalid":
      return {
        ...state,
        phase: "goal",
        goal: event.goal,
        goalError: event.message,
      };
    case "goalAccepted":
      return {
        ...state,
        phase: "guide",
        goal: event.goal,
        goalError: "",
        guideChoice: "skip",
        guideAvailable: null,
        guidePreviewToken: "",
        guideFiles: [],
        guidePostCommit: false,
        guideSkipAllowed: true,
        message: "",
      };
    case "guidePreviewStarted":
      return { ...state, phase: "guide-previewing", message: "" };
    case "guidePreviewed":
      return {
        ...state,
        phase: "guide",
        guideChoice: state.guideSkipAllowed ? "skip" : state.guideChoice,
        guideAvailable: event.preview.available,
        guidePreviewToken: event.preview.available
          ? event.preview.previewToken
          : "",
        guideFiles: event.preview.available ? event.preview.files : [],
        message: event.preview.message,
      };
    case "guidePreviewFailed":
      return {
        ...state,
        phase: "guide",
        guideChoice: state.guideSkipAllowed ? "skip" : state.guideChoice,
        guideAvailable: null,
        guidePreviewToken: "",
        guideFiles: [],
        message: event.message,
      };
    case "guideInstalled":
      if (
        state.phase !== "guide" ||
        state.guideAvailable !== true ||
        !state.guidePreviewToken ||
        state.guideFiles.length === 0
      ) return state;
      return { ...state, phase: "review", guideChoice: "install", message: "" };
    case "guideSkipped":
      if (
        !(state.phase === "guide" || state.phase === "guide-stale") ||
        !state.guideSkipAllowed
      ) return state;
      return {
        ...state,
        phase: "review",
        guideChoice: "skip",
        guidePreviewToken: "",
        guideFiles: [],
        message: "",
      };
    case "guideStale":
      return {
        ...state,
        phase: "guide-stale",
        guidePreviewToken: "",
        guideFiles: [],
        guidePostCommit: event.postCommit,
        guideSkipAllowed: event.skipAllowed !== false,
        guidePartiallyApplied: event.partiallyApplied === true,
        checkpoint: event.checkpoint || state.checkpoint,
        storageAlreadyCreated: event.postCommit || state.storageAlreadyCreated,
        resumeLocked: event.postCommit || state.resumeLocked,
        message: event.message || PROJECT_GUIDE_PREVIEW_STALE,
      };
    case "back":
      if (state.phase === "committing") return state;
      if (state.resumeLocked) return state;
      if (state.resumedOperation) {
        return { ...initialFirstRunState, returnFocusId: state.returnFocusId };
      }
      if (state.phase === "goal") {
        return { ...state, phase: "target-new", goalError: "", message: "" };
      }
      if (state.phase === "review") return { ...state, phase: "guide" };
      if (state.phase === "guide" || state.phase === "guide-stale") {
        return { ...state, phase: "goal", message: "" };
      }
      return { ...initialFirstRunState, returnFocusId: state.returnFocusId };
    case "commit":
      return {
        ...state,
        phase: "committing",
        message: "",
        errorKind: "",
        recoveryMode: "durable",
        resumeLocked: true,
      };
    case "reconcile":
      return { ...state, phase: "reconciling", message: "" };
    case "uncertain":
      return {
        ...state,
        phase: "uncertain",
        message: event.message,
        checkpoint: event.checkpoint || state.checkpoint,
      };
    case "recovery": {
      const recoveryIsDurable = event.durable === true;
      const recoveryMode = recoveryIsDurable
        ? event.resumable === false ? "blocked" : "durable"
        : "ambiguous";
      return {
        ...state,
        phase: "recovery",
        canonicalRoot: event.canonicalRoot || state.canonicalRoot,
        operationId: event.operationId === undefined
          ? state.operationId
          : event.operationId,
        message: event.message,
        checkpoint: event.checkpoint || "",
        errorKind: event.errorKind || "",
        recoveryMode,
        resumeLocked: recoveryIsDurable,
        resumeNoWrite: false,
      };
    }
    case "failed":
      return {
        ...state,
        phase: "failed",
        canonicalRoot: event.canonicalRoot || state.canonicalRoot,
        operationId: event.operationId === undefined
          ? state.operationId
          : event.operationId,
        message: event.message,
        checkpoint: event.checkpoint || "",
        errorKind: event.errorKind || "",
        recoveryMode: "no-write",
        resumeLocked: false,
        resumeNoWrite: true,
      };
    case "reset":
      return {
        ...initialFirstRunState,
        returnFocusId: event.focusId || initialFirstRunState.returnFocusId,
      };
  }
}

export function firstRunFocusTarget(state: FirstRunState): string {
  if (state.phase === "idle") return state.returnFocusId;
  if (state.phase === "goal" && state.goalError) return "setup-goal";
  return "setup-heading";
}

export function canOpenPreservedFirstRunProject(state: FirstRunState): boolean {
  return state.resumeLocked &&
    (["recovery", "guide", "guide-stale", "review"] as FirstRunPhase[]).includes(
      state.phase,
    ) &&
    ["project-committed", "guide-applied", "desktop-bound"].includes(
      state.checkpoint,
    );
}

export type ProjectGuideChoice = "skip" | "install";
export type ProjectGuideFileAction = "create" | "update" | "no-change";

export interface ProjectGuidePreviewFile {
  path: "AGENTS.md" | "CLAUDE.md";
  action: ProjectGuideFileAction;
  additions: number;
  deletions: number;
  diff: string;
}

export interface ProjectGuidePreview {
  available: boolean;
  message: string;
  previewToken: string;
  files: ProjectGuidePreviewFile[];
}

export const PROJECT_GUIDANCE_UNAVAILABLE =
  "Project guidance is not available on this platform yet";
export const PROJECT_GUIDE_PREVIEW_STALE =
  "The guide file changed since preview.";
export const PROJECT_GUIDE_STALE_ERROR = "project-guide-preview-stale";
export const PROJECT_GUIDE_PARTIALLY_APPLIED_ERROR =
  "project-guide-partially-applied";
export const INTERRUPTED_BEFORE_COMMIT_ERROR = "interrupted-before-commit";

const GUIDE_TARGETS = ["AGENTS.md", "CLAUDE.md"] as const;
const MAX_GUIDE_DIFF_BYTES = 65_536;
const MAX_GUIDE_CHANGED_LINES = 4_096;

export function parseProjectGuidePreview(value: unknown): ProjectGuidePreview {
  if (!value || typeof value !== "object") {
    throw new Error("Guide preview returned an invalid result.");
  }
  const result = value as Record<string, unknown>;
  const available = result.available;
  const message = typeof result.message === "string" ? result.message : "";
  const previewToken = typeof result.previewToken === "string"
    ? result.previewToken
    : "";
  if (typeof available !== "boolean") {
    throw new Error("Guide preview did not report platform availability.");
  }
  if (!available) {
    if (
      message !== PROJECT_GUIDANCE_UNAVAILABLE ||
      previewToken ||
      !Array.isArray(result.files) ||
      result.files.length !== 0
    ) {
      throw new Error("Unavailable guide preview returned an unsafe result.");
    }
    return { available, message, previewToken, files: [] };
  }
  if (!previewToken || new TextEncoder().encode(previewToken).byteLength > 512) {
    throw new Error("Guide preview did not return a bounded preview token.");
  }
  if (!Array.isArray(result.files) || result.files.length !== GUIDE_TARGETS.length) {
    throw new Error("Guide preview did not return every guide target.");
  }
  const seen = new Set<string>();
  const files = result.files.map((entry): ProjectGuidePreviewFile => {
    if (!entry || typeof entry !== "object") {
      throw new Error("Guide preview returned an invalid file change.");
    }
    const file = entry as Record<string, unknown>;
    const path = file.path;
    const action = file.action;
    const additions = file.additions;
    const deletions = file.deletions;
    const diff = file.diff;
    if (!GUIDE_TARGETS.includes(path as typeof GUIDE_TARGETS[number]) || seen.has(String(path))) {
      throw new Error("Guide preview returned an unexpected or duplicate target.");
    }
    if (!(action === "create" || action === "update" || action === "no-change")) {
      throw new Error("Guide preview returned an unknown file action.");
    }
    if (
      !Number.isSafeInteger(additions) ||
      !Number.isSafeInteger(deletions) ||
      Number(additions) < 0 ||
      Number(deletions) < 0 ||
      Number(additions) > MAX_GUIDE_CHANGED_LINES ||
      Number(deletions) > MAX_GUIDE_CHANGED_LINES
    ) {
      throw new Error("Guide preview returned an unbounded line count.");
    }
    if (
      typeof diff !== "string" ||
      new TextEncoder().encode(diff).byteLength > MAX_GUIDE_DIFF_BYTES
    ) {
      throw new Error("Guide preview returned an unbounded diff.");
    }
    if (action === "no-change" && (additions !== 0 || deletions !== 0 || diff)) {
      throw new Error("An unchanged guide target reported file changes.");
    }
    seen.add(String(path));
    return {
      path: path as ProjectGuidePreviewFile["path"],
      action,
      additions: Number(additions),
      deletions: Number(deletions),
      diff,
    };
  });
  if (GUIDE_TARGETS.some((path) => !seen.has(path))) {
    throw new Error("Guide preview did not return every guide target.");
  }
  return { available, message, previewToken, files };
}

export function projectGuideCommitFields(state: FirstRunState): {
  guideChoice: ProjectGuideChoice;
  guidePreviewToken: string;
} {
  if (state.guideChoice === "skip") {
    if (!state.guideSkipAllowed) {
      throw new Error("Partially applied project guidance cannot be skipped.");
    }
    return { guideChoice: "skip", guidePreviewToken: "" };
  }
  if (
    state.guidePostCommit &&
    state.checkpoint === "guide-applied" &&
    !state.guidePreviewToken
  ) {
    return { guideChoice: "install", guidePreviewToken: "" };
  }
  if (!state.guidePreviewToken || state.guideFiles.length !== GUIDE_TARGETS.length) {
    throw new Error("Guide installation requires a current preview.");
  }
  return { guideChoice: "install", guidePreviewToken: state.guidePreviewToken };
}

export function isProjectGuidePreviewStale(value: unknown): boolean {
  if (value instanceof Error) return value.message === PROJECT_GUIDE_STALE_ERROR;
  return value === PROJECT_GUIDE_STALE_ERROR;
}

export function isProjectGuidePartiallyApplied(value: unknown): boolean {
  if (value instanceof Error) {
    return value.message === PROJECT_GUIDE_PARTIALLY_APPLIED_ERROR;
  }
  return value === PROJECT_GUIDE_PARTIALLY_APPLIED_ERROR;
}

export interface GoalValidation {
  value: string;
  byteLength: number;
  error: string;
}

export function validateNorthStarGoal(value: unknown): GoalValidation {
  const goal = typeof value === "string" ? value.trim() : "";
  const byteLength = new TextEncoder().encode(goal).byteLength;
  let error = "";
  if (!goal) error = "Enter a north-star goal for this project.";
  else if (byteLength > 4_096) {
    error = "Keep the north-star goal to 4,096 UTF-8 bytes or fewer.";
  }
  return { value: goal, byteLength, error };
}

export interface ProjectTargetValidation {
  kind: "new" | "existing" | "recovery-required";
  canonicalRoot: string;
  operationId: string;
  reason: string;
  resume: ProjectInitializationResume | null;
}

export interface ProjectInitializationResume {
  initialization: InitializationStatus;
  goal: string;
  guideChoice: ProjectGuideChoice;
}

export interface PendingInitialization {
  pending: boolean;
  initialization: InitializationStatus | null;
  validation: ProjectTargetValidation | null;
}

export function parsePendingInitialization(value: unknown): PendingInitialization {
  if (!value || typeof value !== "object") {
    throw new Error("Pending initialization returned an invalid result.");
  }
  const result = value as Record<string, unknown>;
  if (typeof result.pending !== "boolean") {
    throw new Error("Pending initialization did not report whether setup is pending.");
  }
  const hasValidation = Object.prototype.hasOwnProperty.call(result, "validation");
  const hasInitialization = Object.prototype.hasOwnProperty.call(result, "initialization");
  const expectedKeys = result.pending
    ? ["initialization", "pending", "validation"]
    : ["pending"];
  const actualKeys = Object.keys(result).sort();
  if (
    actualKeys.length !== expectedKeys.length ||
    actualKeys.some((key, index) => key !== expectedKeys[index])
  ) {
    throw new Error("Pending initialization returned unexpected fields.");
  }
  if (!result.pending) {
    if (hasValidation || hasInitialization) {
      throw new Error("Completed initialization returned unexpected recovery metadata.");
    }
    return { pending: false, initialization: null, validation: null };
  }
  if (!hasValidation || !hasInitialization) {
    throw new Error("Pending initialization omitted recovery metadata.");
  }
  const initialization = parseInitializationStatus(result.initialization);
  if (initialization.outcome === "complete") {
    throw new Error("Pending initialization was already complete.");
  }
  const validation = parseProjectTargetValidation(result.validation);
  if (validation.kind === "existing" ||
    (validation.kind === "new" && !validation.resume)) {
    throw new Error("Pending initialization was not resumable or recoverable.");
  }
  if (validation.canonicalRoot !== initialization.canonicalRoot) {
    throw new Error("Pending initialization changed its canonical root.");
  }
  if (validation.resume && (
    validation.operationId !== initialization.operationId ||
    validation.resume.initialization.operationId !== initialization.operationId ||
    validation.resume.initialization.checkpoint !== initialization.checkpoint ||
    validation.resume.initialization.outcome !== initialization.outcome ||
    validation.resume.initialization.errorKind !== initialization.errorKind
  )) {
    throw new Error("Pending initialization changed its durable status.");
  }
  return { pending: true, initialization, validation };
}

export function pendingInitializationEvent(
  pending: PendingInitialization,
): FirstRunEvent | null {
  if (!pending.pending) return null;
  const validation = pending.validation;
  const initialization = pending.initialization;
  if (!validation || !initialization) {
    throw new Error("Pending initialization omitted its authoritative state.");
  }
  if (validation.kind === "recovery-required") {
    return {
      type: "recovery",
      canonicalRoot: validation.canonicalRoot,
      operationId: initialization.operationId,
      message: validation.reason ||
        "This preserved project setup cannot be resumed automatically.",
      checkpoint: initialization.checkpoint,
      errorKind: initialization.errorKind,
      durable: true,
      resumable: false,
    };
  }
  if (!validation.resume) {
    throw new Error("Pending initialization did not include durable setup choices.");
  }
  return {
    type: "resume",
    canonicalRoot: validation.canonicalRoot,
    operationId: validation.operationId,
    goal: validation.resume.goal,
    guideChoice: validation.resume.guideChoice,
    initialization: validation.resume.initialization,
  };
}

export async function resolveFirstRunStartupState(
  getWorkspaceState: () => Promise<unknown>,
  getPendingInitialization: () => Promise<unknown>,
): Promise<{
  state: Record<string, unknown>;
  pending: PendingInitialization | null;
}> {
  const initial = await getWorkspaceState();
  if (!initial || typeof initial !== "object") {
    throw new Error("Desktop startup returned an invalid workspace state.");
  }
  const initialState = initial as Record<string, unknown>;
  if (initialState.status !== "welcome") {
    return { state: initialState, pending: null };
  }
  const pending = parsePendingInitialization(await getPendingInitialization());
  const refreshed = await getWorkspaceState();
  if (!refreshed || typeof refreshed !== "object") {
    throw new Error("Desktop startup refresh returned an invalid workspace state.");
  }
  return { state: refreshed as Record<string, unknown>, pending };
}

export function parseProjectTargetValidation(value: unknown): ProjectTargetValidation {
  if (!value || typeof value !== "object") throw new Error("Project validation returned an invalid result.");
  const result = value as Record<string, unknown>;
  const kind = result.kind;
  const canonicalRoot = typeof result.canonicalRoot === "string"
    ? result.canonicalRoot
    : "";
  const operationId = typeof result.operationId === "string" ? result.operationId : "";
  const reason = typeof result.reason === "string" ? result.reason : "";
  if (!(["new", "existing", "recovery-required"] as unknown[]).includes(kind)) {
    throw new Error("Project validation returned an unknown outcome.");
  }
  if (!canonicalRoot) throw new Error("Project validation did not return a canonical root.");
  if (kind === "new" && !operationId) {
    throw new Error("Project validation did not return an operation ID.");
  }
  const resumeFields = ["initialization", "goal", "guideChoice"]
    .filter((field) => Object.prototype.hasOwnProperty.call(result, field));
  if (resumeFields.length === 0) {
    return { kind, canonicalRoot, operationId, reason, resume: null } as ProjectTargetValidation;
  }
  if (resumeFields.length !== 3 || kind !== "new") {
    throw new Error("Project validation returned incomplete resume metadata.");
  }
  const initialization = parseInitializationStatus(result.initialization);
  const goalValidation = validateNorthStarGoal(result.goal);
  const guideChoice = result.guideChoice;
  if (
    goalValidation.error ||
    goalValidation.value !== result.goal ||
    !(guideChoice === "skip" || guideChoice === "install")
  ) {
    throw new Error("Project validation returned invalid durable setup choices.");
  }
  if (
    initialization.operationId !== operationId ||
    initialization.canonicalRoot !== canonicalRoot
  ) {
    throw new Error("Project validation resume metadata changed operation identity.");
  }
  if (![
    "none",
    "prepared",
    "runtime-committed",
    "project-committed",
    "guide-applied",
    "desktop-bound",
  ].includes(initialization.checkpoint)) {
    throw new Error("Project validation returned a non-resumable checkpoint.");
  }
  if (
    (initialization.outcome === "complete") !==
      (initialization.checkpoint === "desktop-bound")
  ) {
    throw new Error("Project validation returned an inconsistent resume outcome.");
  }
  return {
    kind,
    canonicalRoot,
    operationId,
    reason,
    resume: {
      initialization,
      goal: goalValidation.value,
      guideChoice,
    },
  } as ProjectTargetValidation;
}

export interface InitializationStatus {
  operationId: string;
  canonicalRoot: string;
  outcome: "ready" | "in-progress" | "recovery-required" | "complete";
  checkpoint: string;
  errorKind: string;
}

export function parseInitializationStatus(value: unknown): InitializationStatus {
  if (!value || typeof value !== "object") throw new Error("Initialization returned an invalid status.");
  const result = value as Record<string, unknown>;
  const outcome = result.outcome;
  if (!(["ready", "in-progress", "recovery-required", "complete"] as unknown[]).includes(outcome)) {
    throw new Error("Initialization returned an unknown outcome.");
  }
  const operationId = typeof result.operationId === "string" ? result.operationId : "";
  const canonicalRoot = typeof result.canonicalRoot === "string" ? result.canonicalRoot : "";
  const checkpoint = typeof result.checkpoint === "string" ? result.checkpoint : "";
  const errorKind = typeof result.errorKind === "string" ? result.errorKind : "";
  if (!operationId || !canonicalRoot) {
    throw new Error("Initialization status is missing its operation identity.");
  }
  if (![
    "none",
    "prepared",
    "runtime-committed",
    "project-committed",
    "guide-applied",
    "desktop-bound",
  ].includes(checkpoint)) {
    throw new Error("Initialization status returned an unknown checkpoint.");
  }
  const legalOutcome = checkpoint === "none"
    ? outcome === "ready" || outcome === "in-progress"
    : checkpoint === "desktop-bound"
    ? outcome === "complete"
    : outcome === "in-progress" || outcome === "recovery-required";
  if (!legalOutcome) {
    throw new Error("Initialization status returned an inconsistent checkpoint outcome.");
  }
  return {
    operationId,
    canonicalRoot,
    outcome,
    checkpoint,
    errorKind,
  } as InitializationStatus;
}

export interface InitializeProjectResult {
  status: InitializationStatus;
  state: Record<string, unknown> | null;
}

const initializationFailureMessages: Readonly<Record<string, string>> = Object.freeze({
  "authority-shutdown-failed":
    "p-track could not pause the current desktop runtime. Close active work and try again.",
  "project-not-found": "This folder is no longer available.",
  unsupported: "This p-track build cannot initialize projects from the desktop.",
  "filesystem-error": "p-track could not safely access the selected folder.",
  "recovery-required": "Project state changed during setup and requires recovery.",
  "runtime-busy": "Another p-track process is using project state. Close it and try again.",
  "initialization-failed": "p-track could not initialize this project safely.",
});

export function initializationFailureMessage(errorKind: string): string {
  return initializationFailureMessages[errorKind] ||
    "Initialization stopped before making a durable change.";
}

export function initializationStatusMatchesOperation(
  status: InitializationStatus,
  operationId: string,
  canonicalRoot: string,
): boolean {
  return status.operationId === operationId && status.canonicalRoot === canonicalRoot;
}

export function completedInitializationWorkspaceMatches(
  value: unknown,
  canonicalRoot: string,
): boolean {
  if (!value || typeof value !== "object" || !canonicalRoot) return false;
  const state = value as Record<string, unknown>;
  const project = state.project && typeof state.project === "object"
    ? state.project as Record<string, unknown>
    : null;
  return state.status === "open" &&
    Number.isSafeInteger(state.generation) && Number(state.generation) > 0 &&
    project?.root === canonicalRoot;
}

export function parseInitializeProjectResult(value: unknown): InitializeProjectResult {
  if (!value || typeof value !== "object") throw new Error("Initialization returned an invalid result.");
  const result = value as Record<string, unknown>;
  const status = parseInitializationStatus(result.initialization);
  const state = result.state && typeof result.state === "object"
    ? result.state as Record<string, unknown>
    : null;
  if (status.outcome === "complete" && (!state || state.status !== "open")) {
    throw new Error("Initialization completed without an open workspace.");
  }
  if (status.outcome === "complete" && status.checkpoint !== "desktop-bound") {
    throw new Error("Initialization completed before the desktop workspace was bound.");
  }
  return { status, state };
}
