export type TerminalProfileKind = "shell" | "agent";
export type PaneActivitySignal =
  | "none"
  | "activity"
  | "completed"
  | "exited"
  | "failed";
export type PaneIndicatorKind =
  | "failed"
  | "completed"
  | "exited"
  | "activity"
  | "opening"
  | "running"
  | "waiting"
  | "closed";
export type IndicatorRuntimeState =
  | "closed"
  | "opening"
  | "running"
  | "exited"
  | "failed";

export interface PaneActivity {
  profileKind: TerminalProfileKind | null;
  signal: PaneActivitySignal;
  unread: boolean;
  lastSignalAt: number | null;
  exitCode: number | null;
}

export interface PaneIndicator {
  kind: PaneIndicatorKind;
  unread: boolean;
}

export function paneIndicatorChanged(
  previous: PaneIndicator,
  next: PaneIndicator,
): boolean {
  return previous.kind !== next.kind || previous.unread !== next.unread;
}

const indicatorPriority: Record<PaneIndicatorKind, number> = {
  closed: 0,
  waiting: 1,
  running: 2,
  opening: 3,
  activity: 4,
  exited: 5,
  completed: 6,
  failed: 7,
};

function timestamp(now: number, fallback: number | null): number | null {
  return Number.isFinite(now) ? Math.max(0, now) : fallback;
}

function normalizedExitCode(exitCode: number | null): number | null {
  return typeof exitCode === "number" && Number.isFinite(exitCode)
    ? Math.trunc(exitCode)
    : null;
}

export function resetPaneActivity(
  profileKind: TerminalProfileKind | null = null,
): PaneActivity {
  return {
    profileKind,
    signal: "none",
    unread: false,
    lastSignalAt: null,
    exitCode: null,
  };
}

export function recordOutput(
  activity: PaneActivity,
  foreground: boolean,
  now: number,
): PaneActivity {
  if (foreground || activity.signal === "completed" ||
    activity.signal === "exited" || activity.signal === "failed") {
    return activity;
  }
  return {
    ...activity,
    signal: "activity",
    unread: true,
    lastSignalAt: timestamp(now, activity.lastSignalAt),
  };
}

export function recordExit(
  activity: PaneActivity,
  profileKind: TerminalProfileKind | null,
  state: "exited" | "failed",
  exitCode: number | null,
  error: string | null | undefined,
  now: number,
): PaneActivity {
  const code = normalizedExitCode(exitCode);
  const failed = state === "failed" || Boolean(error?.trim()) ||
    (profileKind === "agent" && code !== 0);
  const signal: PaneActivitySignal = failed
    ? "failed"
    : profileKind === "agent" && code === 0
      ? "completed"
      : "exited";
  return {
    profileKind,
    signal,
    unread: true,
    lastSignalAt: timestamp(now, activity.lastSignalAt),
    exitCode: code,
  };
}

export function acknowledgePaneActivity(activity: PaneActivity): PaneActivity {
  return activity.unread ? { ...activity, unread: false } : activity;
}

export function paneIndicator(
  activity: PaneActivity,
  state: IndicatorRuntimeState,
  foreground: boolean,
): PaneIndicator {
  let kind: PaneIndicatorKind;
  if (activity.signal === "failed" || state === "failed") kind = "failed";
  else if (activity.signal === "completed") kind = "completed";
  else if (activity.signal === "exited" || state === "exited") kind = "exited";
  else if (activity.signal === "activity" && activity.unread) kind = "activity";
  else if (state === "opening") kind = "opening";
  else if (state === "running") kind = foreground ? "running" : "waiting";
  else kind = "closed";
  return { kind, unread: activity.unread };
}

export function aggregateTabIndicator(
  paneIds: readonly string[],
  indicatorForPane: (paneId: string) => PaneIndicator,
): PaneIndicator {
  let aggregate: PaneIndicator = { kind: "closed", unread: false };
  for (const paneId of paneIds) {
    const indicator = indicatorForPane(paneId);
    if (indicatorPriority[indicator.kind] > indicatorPriority[aggregate.kind]) {
      aggregate = { kind: indicator.kind, unread: aggregate.unread || indicator.unread };
    } else if (indicator.unread && !aggregate.unread) {
      aggregate = { ...aggregate, unread: true };
    }
  }
  return aggregate;
}
