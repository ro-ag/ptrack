import type { PaneRuntimeState } from "./runtime";

/// Fragment that identifies a detached terminal window.
const terminalWindowFragment = "#terminal-window=";
const terminalWindowLabelPattern = /^terminal-\d+$/;

/** The window label a terminal window was opened with, or null for the main window. */
export function terminalWindowLabel(hash: string): string | null {
  if (!hash.startsWith(terminalWindowFragment)) return null;
  const label = hash.slice(terminalWindowFragment.length);
  return terminalWindowLabelPattern.test(label) ? label : null;
}

/**
 * Whether a whole tab can move to a detached window.
 */
export function terminalPopOutControl(input: {
  panes: ReadonlyArray<{ state: PaneRuntimeState; hasSession: boolean }>;
  busy: boolean;
  closing: boolean;
}): { present: boolean; disabled: boolean } {
  return {
    present: input.panes.length > 0 &&
      input.panes.every((pane) => pane.state === "running" && pane.hasSession),
    disabled: input.busy || input.closing,
  };
}

export interface TerminalPopOutSteps {
  /** Release the renderer lease; the PTY keeps running. */
  release(): void | Promise<void>;
  open(): Promise<{ label: string }>;
  /** Re-claim the session into the pane it was about to leave. */
  reclaim(): Promise<void>;
}

export interface TerminalPopOutResult {
  /**
   * `popped-out` — the window owns the session.
   * `kept` — the move failed and the main window re-claimed it.
   * `unowned` — the move failed and so did the re-claim: nobody renders it.
   */
  outcome: "popped-out" | "kept" | "unowned";
  label: string;
  error: unknown;
}

/**
 * Releases sessions before opening the window and reclaims them on failure.
 */
export async function popOutTerminal(
  steps: TerminalPopOutSteps,
): Promise<TerminalPopOutResult> {
  try {
    await steps.release();
    const opened = await steps.open();
    return { outcome: "popped-out", label: opened.label, error: null };
  } catch (error) {
    try {
      await steps.reclaim();
      return { outcome: "kept", label: "", error };
    } catch (reclaimError) {
      return { outcome: "unowned", label: "", error: reclaimError };
    }
  }
}

/**
 * Reclaim backoff stays within the server's 30-second grace window.
 */
export const streamReclaimDelays: readonly number[] = [500, 2000, 5000, 10_000];

/** Delay before re-claim attempt `attempt`, or null once the pane must give up. */
export function streamReclaimDelay(attempt: number): number | null {
  return streamReclaimDelays[attempt] ?? null;
}

/**
 * Only live panes that still own a session reclaim lost streams.
 */
export function streamLossIsRecoverable(input: {
  state: PaneRuntimeState;
  closing: boolean;
  hasSession: boolean;
  hasRenderer: boolean;
}): boolean {
  return input.hasSession &&
    input.hasRenderer &&
    !input.closing &&
    (input.state === "running" || input.state === "opening");
}

export interface StreamClaim {
  url: string;
  fromSequence: number;
  gap: boolean;
  /**
   * State at ticket minting; ended sessions replay only.
   */
  state?: string;
}

/** Whether a claimed session already ended: its stream closing is final. */
export function streamClaimEnded(claim: Pick<StreamClaim, "state">): boolean {
  return claim.state === "exited" || claim.state === "failed" || claim.state === "closed";
}

/**
 * Normal closure marks ended output; other live stream losses are reclaimed.
 */
export function streamCloseDisposition(input: {
  outputEnded: boolean;
  sessionEnded: boolean;
  recoverable: boolean;
}): "ended" | "reclaim" | "lost" {
  if (input.outputEnded || input.sessionEnded) return "ended";
  return input.recoverable ? "reclaim" : "lost";
}

/** Said while a stream ended but the exit itself has not been reported yet. */
export const streamOutputEndedNotice = "Output ended";

/**
 * Records exits during a pop-out before either surface owns the session.
 */
export class PopOutExitLedger<Exit extends { sessionId: string }> {
  #moving: Map<string, Exit | null> | null = null;

  begin(sessionIds: Iterable<string>): void {
    this.#moving = new Map([...sessionIds].map((sessionId) => [sessionId, null]));
  }

  /** Keeps an exit for a session that is moving. Returns whether it was kept. */
  record(exit: Exit): boolean {
    if (!this.#moving?.has(exit.sessionId)) return false;
    this.#moving.set(exit.sessionId, exit);
    return true;
  }

  /** Ends the move and hands back every exit that arrived during it. */
  finish(): Map<string, Exit> {
    const exits = new Map<string, Exit>();
    for (const [sessionId, exit] of this.#moving ?? []) {
      if (exit) exits.set(sessionId, exit);
    }
    this.#moving = null;
    return exits;
  }
}

/** A tab the terminal window created that comes back as a new dock tab. */
export interface ReturnedWindowTab {
  sessionId: string;
  title: string;
  profileId: string;
  cwd: string;
}

/**
 * Tabs created in a detached window return as new dock tabs in pane order.
 */
export function returnedWindowTabs(
  payload: { sessions?: readonly string[]; shape?: unknown },
  held: (sessionId: string) => boolean,
): ReturnedWindowTab[] {
  const shape = payload.shape as { windowTabs?: unknown } | undefined;
  const tabs = Array.isArray(shape?.windowTabs) ? shape.windowTabs : [];
  const panes: { title: string; profileId: string; cwd: string }[] = [];
  const visit = (node: unknown, title: string): void => {
    const value = node as {
      kind?: string;
      profileId?: unknown;
      cwd?: unknown;
      first?: unknown;
      second?: unknown;
    } | null;
    if (!value || typeof value !== "object") return;
    if (value.kind === "terminal") {
      panes.push({
        title,
        profileId: typeof value.profileId === "string" ? value.profileId : "",
        cwd: typeof value.cwd === "string" ? value.cwd : "",
      });
      return;
    }
    if (value.kind === "split") {
      visit(value.first, title);
      visit(value.second, title);
    }
  };
  for (const tab of tabs) {
    const value = tab as { title?: unknown; root?: unknown } | null;
    visit(value?.root, typeof value?.title === "string" ? value.title : "");
  }
  const returned: ReturnedWindowTab[] = [];
  (payload.sessions ?? []).forEach((sessionId, index) => {
    if (!sessionId || held(sessionId)) return;
    const pane = panes[index];
    returned.push({
      sessionId,
      title: pane?.title || "Terminal",
      profileId: pane?.profileId ?? "",
      cwd: pane?.cwd ?? "",
    });
  });
  return returned;
}

export interface StreamReclaimSteps {
  /** Whether the live pane still owns the session. */
  recoverable(): boolean;
  /** The last sequence the renderer drew, so nothing is replayed twice. */
  sequence(): number;
  wait(delay: number): Promise<void>;
  claim(fromSequence: number): Promise<StreamClaim>;
  attach(claim: StreamClaim): void;
  reclaiming(): void;
  exhausted(): void;
}

/**
 * Reclaims a lost stream with bounded retries for both terminal surfaces.
 */
export async function reclaimStream(
  steps: StreamReclaimSteps,
  firstAttempt = 0,
): Promise<"attached" | "abandoned" | "exhausted"> {
  // Reset only after a stream opens to prevent reconnect loops.
  for (let attempt = firstAttempt; ; attempt += 1) {
    if (!steps.recoverable()) return "abandoned";
    const delay = streamReclaimDelay(attempt);
    if (delay === null) {
      steps.exhausted();
      return "exhausted";
    }
    steps.reclaiming();
    await steps.wait(delay);
    if (!steps.recoverable()) return "abandoned";
    try {
      const claim = await steps.claim(steps.sequence());
      if (!steps.recoverable()) return "abandoned";
      steps.attach(claim);
      return "attached";
    } catch {
      // The session may still be inside its grace window: try again.
    }
  }
}

export const reclaimingStreamNotice = "Reconnecting…";

/** The terminal window's own stream status, in words rather than a colour. */
export function terminalWindowStatusLabel(
  state: "closed" | "connecting" | "open" | "error",
): string {
  return {
    closed: "Disconnected",
    connecting: "Connecting…",
    open: "Connected",
    error: "Stream failed",
  }[state];
}

export const streamReclaimFailedNotice =
  "Terminal stream disconnected and could not be re-claimed";

/**
 * Reports scrollback omitted after the replay buffer wrapped.
 */
export const terminalGapNotice =
  "Earlier output was not carried over. The shell kept running; only " +
  "scrollback older than the replay buffer was dropped.";

/** Shown in the pane a terminal left behind, so the empty pane is explained. */
export const poppedOutPaneNotice =
  "This terminal is running in its own window. Close that window to bring it back here.";

/**
 * Whether a holder pane appears in the closing set.
 */
export function panesHoldPoppedOutTerminal(
  paneIds: readonly string[],
  holders: Iterable<string>,
): boolean {
  const held = new Set(holders);
  return paneIds.some((paneId) => held.has(paneId));
}

/**
 * Refuses closes that would strand a detached terminal's return target.
 */
export const poppedOutCloseRefusedNotice =
  "Close the terminal's own window before closing this pane.";

/**
 * Closing policy for tabs in a detached terminal window.
 */
export function detachedTabCloseIntent(input: {
  tabCount: number;
  ended: boolean;
}): { allowed: boolean; confirm: boolean } {
  const allowed = input.tabCount > 1;
  return { allowed, confirm: allowed && !input.ended };
}

/**
 * Title for closing the final tab via the window close control.
 */
export const detachedLastTabCloseTitle =
  "Close this window to return its tabs to p-track.";

/**
 * Notice shown when a detached shell ends and its holder is released.
 */
export function poppedOutExitNotice(exit: {
  exitCode: number;
  error?: string | null;
}): string {
  const error = exit.error?.trim();
  return error
    ? `${error} (in its own window)`
    : `Process exited with code ${exit.exitCode} in its own window`;
}
