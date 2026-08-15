export type RecentProjectAvailability =
  | "available"
  | "missing"
  | "permission-required"
  | "changed";

export interface RecentProjectEntry {
  entryId: string;
  base: string;
  name: string;
  canonicalPath: string;
  lastOpenedAt: string;
  availability: RecentProjectAvailability;
}

export interface RecentProjectResolution {
  entryId: string;
  base: string;
  canonicalRoot: string;
  name: string;
  resolution: "ready" | "confirmation-required";
  confirmationToken: string;
}

export interface RecentProjectOpenResult {
  entryId: string;
  registryBase: string;
  registryStatus: "unchanged" | "relocated" | "stale";
  open: Record<string, unknown> & {
    state: Record<string, unknown>;
    requiresConfirmation: boolean;
  };
}

export type RecentProjectIntent = "open" | "locate" | "retry" | "forget";
export type RecentProjectsPhase =
  | "idle"
  | "loading"
  | "picking"
  | "resolving"
  | "confirming-relocation"
  | "opening"
  | "confirming-forget"
  | "forgetting"
  | "error";

export interface RecentProjectsState {
  projects: RecentProjectEntry[];
  phase: RecentProjectsPhase;
  listLoading: boolean;
  listError: string;
  operationId: number;
  activeEntryId: string;
  activeBase: string;
  intent: RecentProjectIntent | "";
  message: string;
  announcement: string;
}

export const initialRecentProjectsState: RecentProjectsState = Object.freeze({
  projects: [],
  phase: "idle",
  listLoading: false,
  listError: "",
  operationId: 0,
  activeEntryId: "",
  activeBase: "",
  intent: "",
  message: "",
  announcement: "",
});

export const RECENT_RELOCATION_UNCONFIRMED =
  "Project opened, but p-track could not confirm the recent-entry update. The bounded registry list was reloaded without replaying the open.";

export type RecentProjectsEvent =
  | { type: "loadStarted" }
  | { type: "loaded"; projects: RecentProjectEntry[]; announcement?: string }
  | { type: "loadFailed"; message: string }
  | { type: "alert"; message: string }
  | {
    type: "begin";
    operationId: number;
    entry: RecentProjectEntry;
    intent: RecentProjectIntent;
  }
  | { type: "resolving" }
  | { type: "confirmRelocation" }
  | { type: "opening" }
  | { type: "confirmForget" }
  | { type: "forgetting" }
  | { type: "failed"; message: string }
  | { type: "settled"; announcement?: string };

export function reduceRecentProjects(
  state: RecentProjectsState,
  event: RecentProjectsEvent,
): RecentProjectsState {
  switch (event.type) {
    case "loadStarted":
      if (state.phase !== "idle" || state.listLoading) return state;
      return {
        ...state,
        phase: state.projects.length === 0
          ? "loading"
          : state.phase,
        listLoading: true,
        listError: "",
      };
    case "loaded":
      if (!state.listLoading) return state;
      return {
        ...state,
        projects: event.projects,
        phase: state.phase === "loading" ? "idle" : state.phase,
        listLoading: false,
        listError: "",
        announcement: event.announcement ?? state.announcement,
      };
    case "loadFailed":
      if (!state.listLoading) return state;
      return {
        ...state,
        phase: state.phase === "loading" ? "idle" : state.phase,
        listLoading: false,
        listError: event.message,
      };
    case "alert":
      return { ...state, phase: "idle", message: event.message };
    case "begin":
      return {
        ...state,
        phase: event.intent === "locate"
          ? "picking"
          : event.intent === "forget"
            ? "confirming-forget"
            : event.intent === "open"
              ? "opening"
              : "resolving",
        operationId: event.operationId,
        activeEntryId: event.entry.entryId,
        activeBase: event.entry.base,
        intent: event.intent,
        message: "",
        announcement: "",
      };
    case "resolving":
      return { ...state, phase: "resolving", message: "" };
    case "confirmRelocation":
      return { ...state, phase: "confirming-relocation", message: "" };
    case "opening":
      return { ...state, phase: "opening", message: "" };
    case "confirmForget":
      return { ...state, phase: "confirming-forget", message: "" };
    case "forgetting":
      return { ...state, phase: "forgetting", message: "" };
    case "failed":
      return { ...state, phase: "error", message: event.message };
    case "settled":
      return {
        ...state,
        phase: "idle",
        operationId: 0,
        activeEntryId: "",
        activeBase: "",
        intent: "",
        message: "",
        announcement: event.announcement ?? "",
      };
  }
}

function requiredString(value: unknown, label: string, maximum = 32_768): string {
  if (typeof value !== "string" || !value || value.length > maximum) {
    throw new Error(`Recent project ${label} is invalid.`);
  }
  return value;
}

export function parseRecentProjects(value: unknown): RecentProjectEntry[] {
  if (!value || typeof value !== "object") {
    throw new Error("Recent projects returned an invalid result.");
  }
  const projects = (value as Record<string, unknown>).projects;
  if (!Array.isArray(projects)) {
    throw new Error("Recent projects returned an invalid list.");
  }
  if (projects.length > 20) {
    throw new Error("Recent projects exceeded the 20-entry limit.");
  }
  const seen = new Set<string>();
  const parsed = projects.map((candidate) => {
    if (!candidate || typeof candidate !== "object") {
      throw new Error("Recent projects returned an invalid entry.");
    }
    const project = candidate as Record<string, unknown>;
    const entryId = requiredString(project.entryId, "entry ID", 1024);
    const base = requiredString(project.base, "base", 4096);
    const name = requiredString(project.name, "name", 4096);
    const canonicalPath = requiredString(project.canonicalPath, "path");
    const lastOpenedAt = requiredString(project.lastOpenedAt, "last-opened time", 128);
    const availability = project.availability;
    if (
      availability !== "available" &&
      availability !== "missing" &&
      availability !== "permission-required" &&
      availability !== "changed"
    ) {
      throw new Error("Recent project availability is invalid.");
    }
    if (!Number.isFinite(Date.parse(lastOpenedAt))) {
      throw new Error("Recent project last-opened time is invalid.");
    }
    if (seen.has(entryId)) throw new Error("Recent project entry IDs must be unique.");
    seen.add(entryId);
    return {
      entryId,
      base,
      name,
      canonicalPath,
      lastOpenedAt,
      availability,
    };
  });
  for (let index = 1; index < parsed.length; index += 1) {
    if (
      Date.parse(parsed[index - 1].lastOpenedAt) <
        Date.parse(parsed[index].lastOpenedAt)
    ) {
      throw new Error("Recent projects were not newest first.");
    }
  }
  return parsed;
}

export function parseRecentProjectResolution(
  value: unknown,
  expected: Pick<RecentProjectEntry, "entryId" | "base">,
): RecentProjectResolution {
  if (!value || typeof value !== "object") {
    throw new Error("Recent project resolution returned an invalid result.");
  }
  const result = value as Record<string, unknown>;
  const resolution = result.resolution;
  const confirmationToken = typeof result.confirmationToken === "string"
    ? result.confirmationToken
    : "";
  if (
    result.entryId !== expected.entryId ||
    result.base !== expected.base ||
    !(resolution === "ready" || resolution === "confirmation-required") ||
    confirmationToken.length > 4096 ||
    (resolution === "confirmation-required" && !confirmationToken) ||
    (resolution === "ready" && confirmationToken !== "")
  ) {
    throw new Error("Recent project resolution no longer matches this entry.");
  }
  return {
    entryId: expected.entryId,
    base: expected.base,
    canonicalRoot: requiredString(result.canonicalRoot, "resolved path"),
    name: requiredString(result.name, "resolved name", 4096),
    resolution,
    confirmationToken,
  };
}

export function parseRecentProjectOpenResult(
  value: unknown,
  expected: Pick<RecentProjectEntry, "entryId">,
): RecentProjectOpenResult {
  if (!value || typeof value !== "object") {
    throw new Error("Recent project open returned an invalid result.");
  }
  const result = value as Record<string, unknown>;
  const open = result.open as Record<string, unknown> | null;
  const state = open?.state as Record<string, unknown> | null;
  const registryStatus = result.registryStatus;
  if (
    result.entryId !== expected.entryId ||
    !open ||
    !state ||
    state.status !== "open" ||
    !Number.isSafeInteger(state.generation) ||
    Number(state.generation) <= 0 ||
    typeof open.requiresConfirmation !== "boolean" ||
    !(registryStatus === "unchanged" || registryStatus === "relocated" ||
      registryStatus === "stale")
  ) {
    throw new Error("Recent project open returned a stale or invalid result.");
  }
  if (
    open.requiresConfirmation &&
    (typeof open.confirmationToken !== "string" || !open.confirmationToken ||
      open.confirmationToken.length > 4096 || registryStatus !== "unchanged")
  ) {
    throw new Error("Recent project open omitted its workspace confirmation token.");
  }
  if (open.requiresConfirmation) {
    const resources = open.activeResources as Record<string, unknown> | null;
    if (
      !resources ||
      !Number.isSafeInteger(resources.terminals) || Number(resources.terminals) < 0 ||
      !Number.isSafeInteger(resources.agentRuns) || Number(resources.agentRuns) < 0 ||
      (resources.pendingAdmissions !== undefined &&
        (!Number.isSafeInteger(resources.pendingAdmissions) ||
          Number(resources.pendingAdmissions) < 0))
    ) {
      throw new Error("Recent project open omitted its active-resource summary.");
    }
  }
  return {
    entryId: expected.entryId,
    registryBase: requiredString(result.registryBase, "registry base", 4096),
    registryStatus,
    open: open as RecentProjectOpenResult["open"],
  };
}

export function parseForgetRecentProjectResult(
  value: unknown,
  expected: Pick<RecentProjectEntry, "entryId">,
): { entryId: string; registryBase: string; forgotten: true } {
  if (!value || typeof value !== "object") {
    throw new Error("Forget recent project returned an invalid result.");
  }
  const result = value as Record<string, unknown>;
  if (result.entryId !== expected.entryId || result.forgotten !== true) {
    throw new Error("Forget recent project returned a stale or invalid result.");
  }
  return {
    entryId: expected.entryId,
    registryBase: requiredString(result.registryBase, "registry base", 4096),
    forgotten: true,
  };
}

export function recentProjectPrimaryAction(
  availability: RecentProjectAvailability,
): "open" | "locate" | "retry" {
  if (availability === "available") return "open";
  if (availability === "permission-required") return "retry";
  return "locate";
}

// A last project that did not open on its own — it moved, its folder is gone,
// or its permission lapsed — lands on Welcome with its own row preselected, so
// the user confirms which entry it was instead of guessing. Preselecting is
// all this does: the recorded root is matched, never opened.
export function preselectedRecentProject(
  projects: readonly RecentProjectEntry[],
  startup: { restoreLastProject: boolean; lastProjectRoot: string | null },
): string {
  if (!startup.restoreLastProject || !startup.lastProjectRoot) return "";
  const recorded = startup.lastProjectRoot;
  return projects.find((project) => project.canonicalPath === recorded)?.entryId ?? "";
}

export function recentProjectFocusKey(entryId: string, action: string): string {
  return `${entryId}:${action}`;
}

export function focusAfterForgottenProject(
  projects: RecentProjectEntry[],
  forgottenEntryId: string,
): string {
  const index = projects.findIndex((project) => project.entryId === forgottenEntryId);
  const next = index >= 0
    ? projects[index + 1] ?? projects[index - 1]
    : undefined;
  return next
    ? recentProjectFocusKey(next.entryId, recentProjectPrimaryAction(next.availability))
    : "recent-project-heading";
}
