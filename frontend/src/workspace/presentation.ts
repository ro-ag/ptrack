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
      eyebrow: "p-track projects",
      heading: "Choose a project",
      detail: "Open a directory containing a p-track project to begin.",
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

// commandShortcut routes primary-modifier (⌘/Ctrl) chords. "palette" is
// global; the caller decides whether the view shortcuts are blocked by an
// input, a modal, or the terminal.
export function commandShortcut(
  input: ShortcutInput,
): "palette" | "board" | "overview" | "addTask" | null {
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
