import type { WorkspaceStatus } from "./controller";

interface WorkspaceCopy {
  eyebrow: string;
  heading: string;
  detail: string;
}

interface ProjectGuideReviewFile {
  path: string;
  action: "create" | "update" | "no-change";
  additions: number;
  deletions: number;
}

export interface ProjectGuideReviewCopy {
  label: string;
  detail: string;
  changes: string[];
}

export function firstRunRecoveryActions(
  mode: "durable" | "blocked" | "ambiguous" | "no-write" | "none",
  checkpoint: string,
): {
  resume: boolean;
  open: boolean;
  help: boolean;
  chooseAnother: boolean;
  returnToWelcome: boolean;
} {
  const resumable = mode === "durable";
  return {
    resume: resumable,
    open: resumable && [
      "project-committed",
      "guide-applied",
      "desktop-bound",
    ].includes(checkpoint),
    help: mode === "durable" || mode === "blocked" || mode === "ambiguous",
    chooseAnother: mode === "blocked" || mode === "ambiguous",
    returnToWelcome: mode === "blocked" || mode === "ambiguous",
  };
}

export function projectGuideRecoveryCopy(
  kind: "stale" | "partially-applied",
): { heading: string; detail: string; error: string } {
  if (kind === "partially-applied") {
    return {
      heading: "Review the applied guide changes",
      detail:
        "At least one guide file is already durable. Review the exact current files to finish the same initialization operation.",
      error: "Project guidance was partially applied before setup stopped.",
    };
  }
  return {
    heading: "Review the guide again",
    detail:
      "Private project storage is already durable. Review the current guide files or explicitly skip them to finish initialization.",
    error: "The guide file changed since preview.",
  };
}

export function projectGuideReviewCopy(
  choice: "skip" | "install",
  files: ProjectGuideReviewFile[] = [],
): ProjectGuideReviewCopy {
  if (choice === "skip") {
    return {
      label: "Skip Guide",
      detail: "No guide files will change.",
      changes: [],
    };
  }
  const changes = files.map((file) => {
    if (file.action === "no-change") return `${file.path} · no change`;
    const action = file.action === "create" ? "create" : "update";
    return `${file.path} · ${action} · +${file.additions} −${file.deletions}`;
  });
  return {
    label: "Install Guide",
    detail: changes.some((change) => !change.endsWith("no change"))
      ? "Only the previewed guide changes will be applied."
      : "The guide files already match the preview.",
    changes,
  };
}

export function durableProjectGuideReviewCopy(
  choice: "skip" | "install",
): ProjectGuideReviewCopy {
  if (choice === "skip") {
    return {
      label: "Skip Guide",
      detail: "Skip Guide is already durable for this initialization operation.",
      changes: [],
    };
  }
  return {
    label: "Install Guide",
    detail: "The durable guide step is complete and will not be replayed.",
    changes: ["AGENTS.md and CLAUDE.md · guide step already applied"],
  };
}

export function postProjectOnboardingActions(
  phase: "plan" | "plan-failed" | "task" | "task-create-failed" | "task-start-failed",
): { primary: string; secondary: string } {
  if (phase === "plan" || phase === "plan-failed") {
    return {
      primary: phase === "plan-failed" ? "Try Again" : "Create Plan",
      secondary: "Skip for Now",
    };
  }
  if (phase === "task-start-failed") {
    return { primary: "Try Starting Again", secondary: "Finish Setup" };
  }
  return {
    primary: phase === "task-create-failed" ? "Try Again" : "Create Task",
    secondary: "Finish with Plan",
  };
}

export function appVersionLabel(value: unknown): string {
  if (typeof value !== "string") return "dev";
  const version = value.trim();
  if (!version || version.toLowerCase() === "dev") return "dev";
  return version.toLowerCase().startsWith("v") ? version : `v${version}`;
}

export function workspaceStateCopy(
  status: WorkspaceStatus,
  error = "",
): WorkspaceCopy {
  const copy: Record<WorkspaceStatus, WorkspaceCopy> = {
    welcome: {
      eyebrow: "p-track projects",
      heading: "Start with a project",
      detail: "Initialize p-track in a folder, or open a project you already use.",
    },
    loading: {
      eyebrow: "Project workspace",
      heading: "Opening project…",
      detail:
        "Preparing project storage, Git intelligence, terminals, and agent-run tracking.",
    },
    open: {
      eyebrow: "Project workspace",
      heading: "Project open",
      detail: "The current project workspace is ready.",
    },
    error: {
      eyebrow: "Project workspace error",
      heading: "This project could not be opened",
      detail: error || "Choose another project and try again.",
    },
    closed: {
      eyebrow: "Project workspace",
      heading: "Project closed",
      detail:
        "Project resources were cleaned up. Choose another project when you are ready.",
    },
  };
  return copy[status];
}

export function confirmationCopy(
  action: "close" | "switch",
  terminals: number,
  agentRuns: number,
  pendingAdmissions = 0,
): { heading: string; submit: string; detail: string } {
  const terminalText = `${terminals} active terminal${terminals === 1 ? "" : "s"}`;
  const agentText = `${agentRuns} registered agent run${agentRuns === 1 ? "" : "s"}`;
  const pendingText = pendingAdmissions
    ? ` ${pendingAdmissions} resource operation${pendingAdmissions === 1 ? "" : "s"} still finishing.`
    : "";
  return {
    heading: action === "close" ? "Close this project?" : "Switch projects?",
    submit: action === "close" ? "Close project" : "Switch project",
    detail:
      `${terminalText} and ${agentText} will be stopped.${pendingText} ` +
      "Their project resources will be cleaned up before the transition completes.",
  };
}

export function focusCycleIndex(
  count: number,
  current: number,
  backwards: boolean,
): number {
  if (count <= 0) return -1;
  if (backwards) return current <= 0 ? count - 1 : current - 1;
  return current < 0 || current >= count - 1 ? 0 : current + 1;
}

interface ShortcutInput {
  key: string;
  composing?: boolean;
  meta?: boolean;
  ctrl?: boolean;
  alt?: boolean;
  shift?: boolean;
  repeat?: boolean;
  prevented?: boolean;
}

interface SnapshotSection {
  state: string;
  snapshot?: unknown;
  error?: string;
}

export function preserveSectionOnError<T extends SnapshotSection>(
  previous: T | null | undefined,
  next: T,
): T {
  if (
    next.state === "error" &&
    previous?.snapshot !== undefined
  ) {
    return {
      ...previous,
      state: "stale",
      error: next.error,
    };
  }
  return next;
}

export function shortcutIntent(
  input: ShortcutInput,
): "refresh" | "addTask" | null {
  if (
    input.composing ||
    input.meta ||
    input.ctrl ||
    input.alt ||
    input.shift ||
    input.repeat ||
    input.prevented
  ) return null;
  if (input.key.toLowerCase() === "r") return "refresh";
  if (input.key === "/") return "addTask";
  return null;
}

export interface LinkedTaskRuntimeSummary {
  terminals?: number;
  liveTerminals?: number;
  agents?: number;
  liveAgents?: number;
  terminalBackedRuns?: number;
  externalRuns?: number;
  truncated?: boolean;
}

export interface LinkedRuntimePresentation {
  compact: string;
  detail: string;
  state: "live" | "historical";
}

export function linkedTaskRuntimePresentation(
  summary: LinkedTaskRuntimeSummary | null | undefined,
): LinkedRuntimePresentation | null {
  const terminals = Math.max(0, Number(summary?.terminals) || 0);
  const agents = Math.max(0, Number(summary?.agents) || 0);
  const truncated = summary?.truncated === true;
  if (terminals === 0 && agents === 0 && !truncated) return null;
  const liveTerminals = Math.min(
    terminals,
    Math.max(0, Number(summary?.liveTerminals) || 0),
  );
  const liveAgents = Math.min(
    agents,
    Math.max(0, Number(summary?.liveAgents) || 0),
  );
  const live = liveTerminals + liveAgents;
  const historical = terminals + agents - live;
  const resources = [
    terminals ? `${terminals} terminal${terminals === 1 ? "" : "s"}` : "",
    agents ? `${agents} agent${agents === 1 ? "" : "s"}` : "",
  ].filter(Boolean).join(" · ");
  if (terminals === 0 && agents === 0) {
    return {
      compact: "Runtime capped",
      detail: "Linked runtime may be omitted because the project candidate bound was reached",
      state: "historical",
    };
  }
  return {
    compact: `${live > 0 ? "Live" : "History"} · ${terminals}T ${agents}A`,
    detail:
      `${truncated ? "At least " : ""}${resources} · ${live} live` +
      `${historical ? ` · ${historical} historical` : ""}` +
      `${truncated ? " · additional entries may be omitted" : ""}`,
    state: live > 0 ? "live" : "historical",
  };
}

export interface RuntimeAssociationSummary {
  planId?: number;
  taskId?: number;
  revision?: number;
}

export interface AgentIntelligencePresentation {
  state?: unknown;
  confidence?: unknown;
  eventCount?: unknown;
}

export function agentIntelligenceLabel(
  intelligence: AgentIntelligencePresentation | null | undefined,
): string {
  const states = [
    "unknown",
    "working",
    "waiting",
    "blocked",
    "completed",
    "failed",
    "potentiallyDrifting",
  ];
  if (typeof intelligence?.state !== "string" ||
    !states.includes(intelligence.state)) return "";
  const confidence = ["low", "medium", "high"].includes(
    String(intelligence.confidence),
  )
    ? ` · ${String(intelligence.confidence)} confidence`
    : "";
  const rawCount = Number(intelligence.eventCount);
  const eventCount = Number.isFinite(rawCount)
    ? Math.max(0, Math.trunc(rawCount))
    : 0;
  return `intelligence ${intelligence.state}${confidence} · ${eventCount} structured event${eventCount === 1 ? "" : "s"}`;
}

export interface IntelligenceAssociation {
  planId?: number;
  taskId?: number;
  revision?: number;
}

// A handoff preview belongs to the exact task association shown when its
// request started. Workspace generation alone cannot fence a same-generation
// detach or relink.
export function handoffPreviewResponseIsCurrent(
  requestedTaskId: number,
  expected: IntelligenceAssociation | null | undefined,
  received: IntelligenceAssociation | null | undefined,
  currentTaskId: number,
): boolean {
  if (!expected || !received || requestedTaskId <= 0 || currentTaskId !== requestedTaskId) {
    return false;
  }
  return Number(expected.planId || 0) === Number(received.planId || 0) &&
    Number(expected.taskId || 0) === requestedTaskId &&
    Number(received.taskId || 0) === requestedTaskId &&
    Number(expected.revision || 0) === Number(received.revision || 0);
}

export function runtimeAssociationLabel(
  association: RuntimeAssociationSummary | null | undefined,
): string {
  if (!association) return "unlinked";
  if (association.taskId) {
    return `plan #${association.planId} · task #${association.taskId}`;
  }
  if (association.planId) return `plan #${association.planId}`;
  return "project";
}

export function runtimeCountLabel(
  terminals: ReadonlyArray<{ live?: boolean }> = [],
  agents: ReadonlyArray<{ live?: boolean }> = [],
): { compact: string; detail: string } {
  const liveTerminals = terminals.filter((item) => item.live).length;
  const liveAgents = agents.filter((item) => item.live).length;
  return {
    compact: `${liveTerminals}T · ${liveAgents}A`,
    detail:
      `${liveTerminals}/${terminals.length} live terminals · ` +
      `${liveAgents}/${agents.length} live agents`,
  };
}

export const agentActivityStates = [
  "running",
  "waiting",
  "blocked",
  "completed",
  "failed",
  "stale",
  "unknown",
] as const;

export type AgentActivityState = typeof agentActivityStates[number];

export interface AgentActivityItemPresentation {
  state?: unknown;
  ownership?: unknown;
  [key: string]: unknown;
}

export interface AgentActivitySectionPresentation {
  items?: unknown;
  bounds?: {
    shown?: unknown;
    total?: unknown;
    more?: unknown;
  };
  conflicts?: unknown;
  conflictBounds?: {
    shown?: unknown;
    total?: unknown;
    more?: unknown;
  };
  analysisIncomplete?: unknown;
  notifications?: unknown;
  notificationBounds?: {
    shown?: unknown;
    total?: unknown;
    more?: unknown;
  };
  notificationsIncomplete?: unknown;
  handoffs?: unknown;
  worktrees?: unknown;
  worktreeBounds?: { more?: unknown };
	worktreesIncomplete?: unknown;
	workflows?: unknown;
	workflowTargets?: unknown;
	workflowTargetsIncomplete?: unknown;
}

// Keep the browser-side view bounded and explicit about omitted rows even if
// it receives a partial or malformed snapshot during an app upgrade.
export function agentActivityPresentation(
  section: AgentActivitySectionPresentation | null | undefined,
): {
  items: Array<AgentActivityItemPresentation & { state: AgentActivityState }>;
  counts: Array<{ state: AgentActivityState; count: number }>;
  conflicts: Array<{
    planId: number;
    taskId: number;
    agentCount: number;
    ownerCount: number;
    runIds: string[];
  }>;
  analysisIncomplete: boolean;
  notifications: Array<{
    id: string;
    runId: string;
    kind: "approvalRequested" | "question" | "failure" | "completion";
    observedAt: string;
    terminalBacked: boolean;
    association?: unknown;
  }>;
  notificationsIncomplete: boolean;
  handoffs: {
    items: Array<{
      id: string;
      sourceRunId: string;
      targetRunId: string;
      createdAt: string;
      expiresAt: string;
      preview: { text: string; includedEventIds: string[]; truncated: boolean };
    }>;
    incomplete: boolean;
  };
  worktrees: Array<{ root: string; branch: string; head: string }>;
	worktreesIncomplete: boolean;
	workflows: {
		items: Array<{
			id: string;
			runId: string;
			kind: "validation" | "commit" | "pullRequest" | "merge";
			state: "proposed" | "approved";
			branch: string;
			head: string;
			targetBranch: string;
			targetHead: string;
			status: { staged: number; unstaged: number; untracked: number; conflicted: number; ahead: number; behind: number };
		}>;
		incomplete: boolean;
	};
	workflowTargets: string[];
	workflowTargetsIncomplete: boolean;
  compact: string;
  detail: string;
} {
  const source = Array.isArray(section?.items) ? section.items.slice(0, 64) : [];
  const items = source.flatMap((candidate) => {
    const item = candidate && typeof candidate === "object"
      ? candidate as AgentActivityItemPresentation
      : {};
    const runId = boundedPresentationID(item.runId);
    if (!runId) return [];
    const state = agentActivityStates.includes(item.state as AgentActivityState)
      ? item.state as AgentActivityState
      : "unknown";
    const association = sanitizeRuntimeAssociation(item.association);
    const ownership = sanitizeAgentOwnership(item.ownership);
    const worktree = sanitizeAgentWorktree(item.worktree);
    const registrationKind = ["launched", "external"].includes(String(item.registrationKind))
      ? item.registrationKind as "launched" | "external"
      : "";
    const confidence = ["low", "medium", "high"].includes(String(item.confidence))
      ? item.confidence as "low" | "medium" | "high"
      : "";
    const safeItem: AgentActivityItemPresentation & { state: AgentActivityState } = {
      runId,
      state,
      ...(registrationKind ? { registrationKind } : {}),
      ...(typeof item.terminalBacked === "boolean" ? { terminalBacked: item.terminalBacked } : {}),
      ...(typeof item.terminalPresent === "boolean" ? { terminalPresent: item.terminalPresent } : {}),
      ...(typeof item.correspondingTerminal === "boolean" ? { correspondingTerminal: item.correspondingTerminal } : {}),
      ...(typeof item.live === "boolean" ? { live: item.live } : {}),
      ...(association ? { association } : {}),
      ...(confidence ? { confidence } : {}),
      ...(Number.isFinite(Number(item.evidenceCount))
        ? { evidenceCount: nonnegativeInteger(item.evidenceCount) }
        : {}),
      ...(Number.isFinite(Number(item.eventCount))
        ? { eventCount: nonnegativeInteger(item.eventCount) }
        : {}),
      ...(boundedPresentationTimestamp(item.lastEventAt)
        ? { lastEventAt: boundedPresentationTimestamp(item.lastEventAt) }
        : {}),
      ...(ownership ? { ownership } : {}),
      ...(worktree ? { worktree } : {}),
    };
    return [safeItem];
  });
  const counts = agentActivityStates
    .map((state) => ({
      state,
      count: items.filter((item) => item.state === state).length,
    }))
    .filter(({ count }) => count > 0);
  const rawTotal = Number(section?.bounds?.total);
  const total = Number.isFinite(rawTotal)
    ? Math.max(items.length, Math.trunc(rawTotal))
    : items.length;
  const omitted = Math.max(0, total - items.length);
  const conflictSource = Array.isArray(section?.conflicts)
    ? section.conflicts.slice(0, 64)
    : [];
  const conflicts = conflictSource.flatMap((candidate) => {
    if (!candidate || typeof candidate !== "object") return [];
    const value = candidate as Record<string, unknown>;
    const planId = Math.trunc(Number(value.planId));
    const taskId = Math.trunc(Number(value.taskId));
    const agentCount = Math.max(0, Math.trunc(Number(value.agentCount)) || 0);
    const ownerCount = Math.min(
      agentCount,
      Math.max(0, Math.trunc(Number(value.ownerCount)) || 0),
    );
    if (planId <= 0 || taskId <= 0 || agentCount < 2) return [];
    const runIds = Array.isArray(value.runIds)
      ? value.runIds.filter((runId): runId is string => typeof runId === "string").slice(0, 16)
      : [];
    return [{ planId, taskId, agentCount, ownerCount, runIds }];
  });
  const notificationKinds = [
    "approvalRequested", "question", "failure", "completion",
  ] as const;
  const notificationSource = Array.isArray(section?.notifications)
    ? section.notifications.slice(0, 64)
    : [];
  const notifications = notificationSource.flatMap((candidate) => {
    if (!candidate || typeof candidate !== "object") return [];
    const value = candidate as Record<string, unknown>;
    const id = boundedPresentationID(value.id);
    const runId = boundedPresentationID(value.runId);
    const observedAt = boundedPresentationTimestamp(value.observedAt);
    const association = sanitizeRuntimeAssociation(value.association);
    if (!id || !runId || !observedAt ||
      !notificationKinds.includes(value.kind as typeof notificationKinds[number])) return [];
    return [{
      id,
      runId,
      kind: value.kind as typeof notificationKinds[number],
      observedAt,
      terminalBacked: value.terminalBacked === true,
      ...(association ? { association } : {}),
    }];
  });
  const handoffSection = section?.handoffs && typeof section.handoffs === "object"
    ? section.handoffs as Record<string, unknown>
    : {};
  const handoffSource = Array.isArray(handoffSection.items)
    ? handoffSection.items.slice(0, 64)
    : [];
  const handoffItems = handoffSource.flatMap((candidate) => {
    if (!candidate || typeof candidate !== "object") return [];
    const value = candidate as Record<string, unknown>;
    const preview = value.preview && typeof value.preview === "object"
      ? value.preview as Record<string, unknown>
      : {};
    const text = typeof preview.text === "string" ? preview.text : "";
    if (typeof value.id !== "string" || typeof value.sourceRunId !== "string" ||
      typeof value.targetRunId !== "string" || value.sourceRunId === value.targetRunId ||
      typeof value.createdAt !== "string" || typeof value.expiresAt !== "string" ||
      text.length === 0 || new TextEncoder().encode(text).length > 2048) return [];
    const includedEventIds = Array.isArray(preview.includedEventIds)
      ? preview.includedEventIds.filter((id): id is string => typeof id === "string").slice(0, 8)
      : [];
    return [{
      id: value.id,
      sourceRunId: value.sourceRunId,
      targetRunId: value.targetRunId,
      createdAt: value.createdAt,
      expiresAt: value.expiresAt,
      preview: { text, includedEventIds, truncated: preview.truncated === true },
    }];
  });
  const worktreeSource = Array.isArray(section?.worktrees)
    ? section.worktrees.slice(0, 64)
    : [];
	const worktrees = worktreeSource.flatMap((candidate) => {
    if (!candidate || typeof candidate !== "object") return [];
    const value = candidate as Record<string, unknown>;
    const root = boundedWorktreeRoot(value.root);
    const branch = typeof value.branch === "string" && value.branch.length <= 512
      ? value.branch
      : "";
    const head = normalizedWorktreeHead(value.head);
    return root && head ? [{ root, branch, head }] : [];
	});
	const workflowSection = section?.workflows && typeof section.workflows === "object"
		? section.workflows as Record<string, unknown>
		: {};
	const workflowKinds = ["validation", "commit", "pullRequest", "merge"] as const;
	const workflowStates = ["proposed", "approved"] as const;
	const workflowItems = (Array.isArray(workflowSection.items) ? workflowSection.items : [])
		.slice(0, 64)
		.flatMap((candidate) => {
			if (!candidate || typeof candidate !== "object") return [];
			const value = candidate as Record<string, unknown>;
			const kind = value.kind as typeof workflowKinds[number];
			const state = value.state as typeof workflowStates[number];
			const head = typeof value.head === "string" && /^[0-9a-f]{40}([0-9a-f]{24})?$/i.test(value.head)
				? value.head.toLowerCase()
				: "";
			const branch = typeof value.branch === "string" && value.branch.length > 0 && value.branch.length <= 512
				? value.branch
				: "";
			const targetBranch = typeof value.targetBranch === "string" && value.targetBranch.length <= 512
				? value.targetBranch
				: "";
			const targetHead = typeof value.targetHead === "string" && /^[0-9a-f]{40}([0-9a-f]{24})?$/i.test(value.targetHead)
				? value.targetHead.toLowerCase()
				: "";
			if (typeof value.id !== "string" || typeof value.runId !== "string" ||
				!workflowKinds.includes(kind) || !workflowStates.includes(state) || !head || !branch ||
				(["pullRequest", "merge"].includes(kind) && (!targetBranch || !targetHead)) ||
				(["validation", "commit"].includes(kind) && (targetBranch || targetHead))) return [];
			const rawStatus = value.status && typeof value.status === "object"
				? value.status as Record<string, unknown>
				: {};
			const count = (name: string) => Math.max(0, Math.trunc(Number(rawStatus[name])) || 0);
			return [{
				id: value.id, runId: value.runId, kind, state, branch, head, targetBranch, targetHead,
				status: {
					staged: count("staged"), unstaged: count("unstaged"),
					untracked: count("untracked"), conflicted: count("conflicted"),
					ahead: count("ahead"), behind: count("behind"),
				},
			}];
		});
	const workflowTargets = Array.isArray(section?.workflowTargets)
		? section.workflowTargets.filter((target): target is string =>
			typeof target === "string" && target.length > 0 && target.length <= 512 && !/[\r\n\0]/.test(target)
		).slice(0, 100)
		: [];
  return {
    items,
    counts,
    conflicts,
    analysisIncomplete: section?.analysisIncomplete === true ||
      Number(section?.conflictBounds?.more || 0) > 0,
    notifications,
    notificationsIncomplete: section?.notificationsIncomplete === true ||
      Number(section?.notificationBounds?.more || 0) > 0,
    handoffs: {
      items: handoffItems,
      incomplete: handoffSection.incomplete === true ||
        Number((handoffSection.bounds as Record<string, unknown> | undefined)?.more || 0) > 0,
    },
    worktrees,
		worktreesIncomplete: section?.worktreesIncomplete === true ||
			Number(section?.worktreeBounds?.more || 0) > 0,
		workflows: {
			items: workflowItems,
			incomplete: workflowSection.incomplete === true ||
				Number((workflowSection.bounds as Record<string, unknown> | undefined)?.more || 0) > 0,
		},
		workflowTargets,
		workflowTargetsIncomplete: section?.workflowTargetsIncomplete === true,
    compact: omitted ? `${items.length}/${total}` : String(total),
    detail: `${total} registered agent${total === 1 ? "" : "s"}` +
      (omitted ? ` · ${omitted} older entr${omitted === 1 ? "y" : "ies"} omitted` : ""),
  };
}

export function agentActivityAnnouncement(
  activity: {
    items: ReadonlyArray<{ runId?: unknown; state?: unknown; [key: string]: unknown }>;
    notifications: ReadonlyArray<{ id?: unknown }>;
  },
  previousKey = "",
): { key: string; text: string } | null {
  const states = activity.items.flatMap((item) => {
    const runId = boundedPresentationID(item.runId);
    const state = agentActivityStates.includes(item.state as AgentActivityState)
      ? item.state as AgentActivityState
      : "unknown";
    return runId ? [`${runId}:${state}`] : [];
  }).sort();
  const notificationIDs = activity.notifications
    .flatMap((item) => {
      const id = boundedPresentationID(item.id);
      return id ? [id] : [];
    })
    .sort();
  const key = JSON.stringify([states, notificationIDs]);
  if (key === previousKey) return null;

  const counts = agentActivityStates.flatMap((state) => {
    const count = states.filter((entry) => entry.endsWith(`:${state}`)).length;
    return count ? [`${count} ${state}`] : [];
  });
  const stateText = counts.length ? counts.join(", ") : "no registered agents";
  const notificationText = notificationIDs.length
    ? ` ${notificationIDs.length} structured notification${notificationIDs.length === 1 ? "" : "s"}.`
    : " No structured notifications.";
  return { key, text: `Agent activity updated: ${stateText}.${notificationText}` };
}

export function workflowMutationFocusKey(
  action: "approve" | "dismiss",
  proposalID: unknown,
): string {
  const id = boundedPresentationID(proposalID);
  return id ? `workflow:${action}:${id}` : "";
}

export function mutationFocusFallback(
  focusKey: string,
): "workflowPrepare" | "handoffSend" | "" {
  if (focusKey.startsWith("workflow:approve:") ||
    focusKey.startsWith("workflow:dismiss:")) return "workflowPrepare";
  if (focusKey.startsWith("handoff:")) return "handoffSend";
  return "";
}

export function worktreeSelectionForRerender(
  options: ReadonlyArray<string>,
  confirmed: unknown,
  focusedSelection?: { runId?: unknown; value?: unknown } | null,
  runID?: unknown,
): string {
  const available = options.filter((option) => typeof option === "string" && option.length > 0);
  const confirmedValue = typeof confirmed === "string" && available.includes(confirmed)
    ? confirmed
    : available[0] || "";
  const currentRunID = boundedPresentationID(runID);
  const focusedRunID = boundedPresentationID(focusedSelection?.runId);
  const focusedValue = typeof focusedSelection?.value === "string"
    ? focusedSelection.value
    : "";
  return currentRunID && focusedRunID === currentRunID && available.includes(focusedValue)
    ? focusedValue
    : confirmedValue;
}

function boundedWorktreeRoot(value: unknown): string {
  return typeof value === "string" && value.length > 0 && value.length <= 4096
    ? value
    : "";
}

function boundedPresentationID(value: unknown): string {
  return typeof value === "string" && value.length > 0 && value.length <= 512 &&
    !/[\0\r\n]/.test(value)
    ? value
    : "";
}

function boundedPresentationTimestamp(value: unknown): string {
  return typeof value === "string" && value.length > 0 && value.length <= 64 &&
    !/[\0\r\n]/.test(value)
    ? value
    : "";
}

function nonnegativeInteger(value: unknown): number {
  return Math.max(0, Math.trunc(Number(value)) || 0);
}

function sanitizeRuntimeAssociation(value: unknown): unknown | null {
  if (!value || typeof value !== "object") return null;
  const association = value as Record<string, unknown>;
  const planId = nonnegativeInteger(association.planId);
  const taskId = nonnegativeInteger(association.taskId);
  const revision = nonnegativeInteger(association.revision);
  if (revision <= 0 || (planId <= 0 && taskId <= 0)) return null;
  return { planId, taskId, revision };
}

function sanitizeAgentOwnership(value: unknown): unknown | null {
  if (!value || typeof value !== "object") return null;
  const ownership = value as Record<string, unknown>;
  const planId = nonnegativeInteger(ownership.planId);
  const taskId = nonnegativeInteger(ownership.taskId);
  const associationRevision = nonnegativeInteger(ownership.associationRevision);
  if (planId <= 0 || taskId <= 0 || associationRevision <= 0) return null;
  return { planId, taskId, associationRevision };
}

function normalizedWorktreeHead(value: unknown): string {
  return typeof value === "string" && /^[0-9a-f]{40}([0-9a-f]{24})?$/i.test(value)
    ? value.toLowerCase()
    : "";
}

function sanitizeAgentWorktree(value: unknown): unknown | null {
  if (!value || typeof value !== "object") return null;
  const worktree = value as Record<string, unknown>;
  const identity = worktree.identity && typeof worktree.identity === "object"
    ? worktree.identity as Record<string, unknown>
    : {};
  const root = boundedWorktreeRoot(identity.root);
  const head = normalizedWorktreeHead(identity.head);
  if (worktree.verified !== true || !root || !head) return null;
  return {
    identity: {
      root,
      branch: typeof identity.branch === "string" && identity.branch.length <= 512
        ? identity.branch
        : "",
      head,
      linked: identity.linked === true,
    },
    verified: true,
    isolated: worktree.isolated === true,
    cwdMatches: worktree.cwdMatches === true,
  };
}

export function runtimeEventIsCurrent(
  eventGeneration: unknown,
  currentGeneration: number,
  workspaceOpen: boolean,
): boolean {
  return workspaceOpen && Number.isSafeInteger(Number(eventGeneration)) &&
    Number(eventGeneration) > 0 &&
    Number(eventGeneration) === currentGeneration;
}

const driftKinds = [
  "checkoutChangedPath",
  "untrackedFile",
  "unlinkedCommit",
  "crossTaskPathOverlap",
  "taskDriftSignal",
] as const;

export function driftPresentation(section: unknown): {
  findings: Array<{
    kind: typeof driftKinds[number];
    severity: "info" | "warning";
    scope: "projectUnattributed" | "agent" | "taskComparison";
    path: string;
    sha: string;
    runIds: string[];
    evidenceCount: number;
  }>;
  incomplete: boolean;
} {
  const value = section && typeof section === "object"
    ? section as Record<string, unknown>
    : {};
  const source = Array.isArray(value.findings) ? value.findings.slice(0, 64) : [];
  const findings = source.flatMap((candidate) => {
    if (!candidate || typeof candidate !== "object") return [];
    const finding = candidate as Record<string, unknown>;
    const kind = finding.kind as typeof driftKinds[number];
    const severity = finding.severity;
    const scope = finding.scope;
    if (!driftKinds.includes(kind) || !["info", "warning"].includes(String(severity)) ||
      !["projectUnattributed", "agent", "taskComparison"].includes(String(scope))) return [];
    const path = typeof finding.path === "string" && !finding.path.startsWith("/") &&
      finding.path !== ".." && !finding.path.startsWith("../") &&
      !finding.path.includes("/../") && finding.path.length <= 512
      ? finding.path
      : "";
    const sha = typeof finding.sha === "string" && /^[0-9a-f]{7,64}$/i.test(finding.sha)
      ? finding.sha.toLowerCase()
      : "";
    if (["checkoutChangedPath", "untrackedFile", "crossTaskPathOverlap"].includes(kind) && !path) return [];
    if (kind === "unlinkedCommit" && !sha) return [];
    const runIds = Array.isArray(finding.runIds)
      ? finding.runIds.filter((id): id is string => typeof id === "string").slice(0, 16)
      : [];
    return [{
      kind,
      severity: severity as "info" | "warning",
      scope: scope as "projectUnattributed" | "agent" | "taskComparison",
      path,
      sha,
      runIds,
      evidenceCount: Math.max(0, Math.trunc(Number(finding.evidenceCount)) || 0),
    }];
  });
  const bounds = value.bounds && typeof value.bounds === "object"
    ? value.bounds as Record<string, unknown>
    : {};
  return {
    findings,
    incomplete: value.incomplete === true || Number(bounds.more || 0) > 0,
  };
}

// commandShortcut routes primary-modifier (⌘/Ctrl) chords. "palette" is
// global; the caller decides whether the view shortcuts are blocked by an
// input, a modal, or the terminal.
export function commandShortcut(
  input: ShortcutInput,
): "palette" | "board" | "overview" | "settings" | "addTask" | null {
  if (
    input.composing ||
    input.repeat ||
    input.prevented ||
    input.alt ||
    (!input.meta && !input.ctrl)
  ) return null;
  const key = input.key.toLowerCase();
  if (key === "k") return "palette";
  if (input.shift) return null;
  if (key === "1") return "board";
  if (key === "2") return "overview";
  if (key === "3") return "settings";
  if (key === "n") return "addTask";
  return null;
}

export interface PaletteResult {
  kind: "plan" | "task" | "note";
  id: number;
  planId: number;
  title: string;
  snippet: string;
}

export interface PaletteGroup {
  kind: PaletteResult["kind"];
  label: string;
  items: PaletteResult[];
}

// groupSearchResults buckets flat SearchV2 hits into display groups,
// always in Plans → Tasks → Notes order, skipping empty groups.
export function groupSearchResults(results: PaletteResult[]): PaletteGroup[] {
  const labels: Record<PaletteResult["kind"], string> = {
    plan: "Plans",
    task: "Tasks",
    note: "Notes",
  };
  const groups: PaletteGroup[] = [];
  for (const kind of ["plan", "task", "note"] as const) {
    const items = results.filter((result) => result.kind === kind);
    if (items.length > 0) groups.push({ kind, label: labels[kind], items });
  }
  return groups;
}

export interface PaletteTarget {
  view: "board" | "overview";
  planId: number;
  taskId: number;
}

// paletteTarget maps a result to its activation: plans and tasks land on
// the board (tasks also open their detail drawer), notes land on the
// overview's Recent memory.
export function paletteTarget(result: PaletteResult): PaletteTarget {
  if (result.kind === "note") return { view: "overview", planId: 0, taskId: 0 };
  return {
    view: "board",
    planId: result.planId,
    taskId: result.kind === "task" ? result.id : 0,
  };
}

export interface LaneInfo {
  status: string;
  taskCount: number;
}

// collapsedLaneStatuses picks the lanes that render as slim rails. Empty
// lanes collapse by default (unless re-expanded this session); populated
// lanes collapse only when the user folded them manually. An all-empty
// board stays expanded — an all-rails board would be useless.
export function collapsedLaneStatuses(
  lanes: LaneInfo[],
  expanded: ReadonlySet<string>,
  folded: ReadonlySet<string> = new Set(),
): string[] {
  const empty = lanes.filter((lane) => lane.taskCount === 0);
  if (empty.length === lanes.length) return [];
  return lanes
    .filter((lane) =>
      lane.taskCount === 0 ? !expanded.has(lane.status) : folded.has(lane.status),
    )
    .map((lane) => lane.status);
}

export interface HeatmapDay {
  date: string; // YYYY-MM-DD
  count: number;
}

export interface HeatmapCell {
  date: string; // "" for leading padding cells
  count: number;
  level: number; // 0..4
}

export function heatLevel(count: number, max: number): number {
  if (count <= 0 || max <= 0) return 0;
  return Math.max(1, Math.min(4, Math.ceil((count / max) * 4)));
}

// heatmapWeeks buckets a dense, oldest-first daily series into
// GitHub-style week columns of 7 rows (Sunday on top), padding the first
// week so dates land on their weekday row.
export function heatmapWeeks(days: HeatmapDay[]): HeatmapCell[][] {
  const max = days.reduce((top, day) => Math.max(top, day.count), 0);
  const columns: HeatmapCell[][] = [];
  let column: HeatmapCell[] = [];
  days.forEach((day, index) => {
    if (index === 0) {
      const offset = new Date(`${day.date}T00:00:00`).getDay();
      for (let pad = 0; pad < offset; pad += 1) {
        column.push({ date: "", count: 0, level: 0 });
      }
    }
    column.push({ date: day.date, count: day.count, level: heatLevel(day.count, max) });
    if (column.length === 7) {
      columns.push(column);
      column = [];
    }
  });
  if (column.length > 0) columns.push(column);
  return columns;
}
