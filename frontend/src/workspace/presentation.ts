import type { WorkspaceStatus } from "./controller";

interface WorkspaceCopy {
  eyebrow: string;
  heading: string;
  detail: string;
}

export function workspaceStateCopy(
  status: WorkspaceStatus,
  error = "",
): WorkspaceCopy {
  const copy: Record<WorkspaceStatus, WorkspaceCopy> = {
    welcome: {
      eyebrow: "P-TRACK projects",
      heading: "Choose a project",
      detail: "Open a directory containing a P-TRACK project to begin.",
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
