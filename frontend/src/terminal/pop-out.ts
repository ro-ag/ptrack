import type { PaneRuntimeState } from "./runtime";

/// Terminal windows load the same document with this fragment, so the mode
/// marker is the whole difference between the two windows at load time.
const terminalWindowFragment = "#terminal-window=";
const terminalWindowLabelPattern = /^terminal-\d+$/;

/** The window label a terminal window was opened with, or null for the main window. */
export function terminalWindowLabel(hash: string): string | null {
  if (!hash.startsWith(terminalWindowFragment)) return null;
  const label = hash.slice(terminalWindowFragment.length);
  return terminalWindowLabelPattern.test(label) ? label : null;
}

/**
 * Where the pop-out control is shown (§7). One session per terminal window,
 * moved whole: a split tab has no single pane to move, so the control is
 * absent there rather than present and broken.
 */
export function terminalPopOutControl(input: {
  paneCount: number;
  state: PaneRuntimeState;
  hasSession: boolean;
  busy: boolean;
  closing: boolean;
}): { present: boolean; disabled: boolean } {
  return {
    present: input.paneCount === 1 && input.state === "running" && input.hasSession,
    disabled: input.busy || input.closing,
  };
}

export interface TerminalPopOutSteps {
  /** Release the renderer lease and tear the renderer down. The PTY keeps running. */
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
 * The move (§4), in order: release, then open the window. A failure at either
 * step re-claims the session, because a failed pop-out must never leave a
 * session with no owner.
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
 * Backoff for re-claiming a stream the renderer lost without being asked to.
 * Four attempts inside 17.5s, comfortably inside the 30s re-claim grace window
 * after which the session is genuinely gone: retrying past it would only spin.
 */
export const streamReclaimDelays: readonly number[] = [500, 2000, 5000, 10_000];

/** Delay before re-claim attempt `attempt`, or null once the pane must give up. */
export function streamReclaimDelay(attempt: number): number | null {
  return streamReclaimDelays[attempt] ?? null;
}

/**
 * Whether a stream that just ended should be claimed back. A terminal the user
 * closed, a pane being torn down, and a shell that exited are all deliberate
 * endings: only a live pane that still owns its session reconnects.
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
}

export interface StreamReclaimSteps {
  /** Whether re-claiming is still worth it: the pane is live and still owns the session. */
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
 * Claim a lost stream back, bounded. Shared by the dock and the terminal
 * window: a renderer that lost its socket for any reason other than a
 * deliberate ending gets the session back, or says plainly that it could not.
 */
export async function reclaimStream(
  steps: StreamReclaimSteps,
  firstAttempt = 0,
): Promise<"attached" | "abandoned" | "exhausted"> {
  // The budget carries across attaches and is reset only by a stream that
  // actually opened, so a socket that dies the moment it connects cannot spin.
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
 * Said whenever the replay ring wrapped past what a renderer asked for. It is
 * a statement of fact, not a failure: the shell is fine, the scrollback simply
 * predates the move.
 */
export const terminalGapNotice =
  "Earlier output was not carried over. The shell kept running; only " +
  "scrollback older than the replay buffer was dropped.";

/** Shown in the pane a terminal left behind, so the empty pane is explained. */
export const poppedOutPaneNotice =
  "This terminal is running in its own window. Close that window to bring it back here.";

/**
 * Whether any of these panes is holding a popped-out terminal's place.
 * `holders` are the panes sessions were popped out of.
 */
export function panesHoldPoppedOutTerminal(
  paneIds: readonly string[],
  holders: Iterable<string>,
): boolean {
  const held = new Set(holders);
  return paneIds.some((paneId) => held.has(paneId));
}

/**
 * Said when a close would remove a pane holding a popped-out terminal. Such a
 * pane has no session of its own, so no close path ever asks about it — and
 * with the pane gone the window's pop-in has nowhere to hand the session back
 * to and closes the shell instead. Refusing is the only answer that keeps a
 * running shell from dying without a confirmation; the window is where that
 * terminal is closed.
 */
export const poppedOutCloseRefusedNotice =
  "Close the terminal's own window before closing this pane.";
