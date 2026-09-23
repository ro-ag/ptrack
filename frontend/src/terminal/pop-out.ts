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
 * Where the pop-out control is shown (step 3 §1). The unit of movement is the
 * tab: the control is present when every pane of the tab is running with a
 * session, because a tab moves whole or not at all.
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
  /**
   * The session state when the ticket was minted. An exited session still
   * replays what it kept, but its stream then ends for good.
   */
  state?: string;
}

/** Whether a claimed session already ended: its stream closing is final. */
export function streamClaimEnded(claim: Pick<StreamClaim, "state">): boolean {
  return claim.state === "exited" || claim.state === "failed" || claim.state === "closed";
}

/**
 * What a renderer does once its stream closed. A normal closure is the
 * server saying the output ended — the shell exited — so re-claiming would
 * only replay the same scrollback forever; any other loss of a live pane is
 * worth claiming back.
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
 * Exits that arrive while a tab is between its pane and its window. Tearing
 * the pane down clears its session, and the window has not subscribed yet,
 * so an exit in that gap reached nobody and the held pane kept promising a
 * terminal that was already gone. The dock records it here instead and
 * applies it once the move settled, whichever way it went.
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
 * Tabs a terminal window created itself have no pane in the main window
 * holding their place. Pop-in hands each of them back as a new tab instead of
 * closing a running shell the user never asked to stop. Sessions are listed
 * in the window's pane order, which is how they pair with the returned tabs.
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

/**
 * Closing a tab inside a terminal window. The original tab — the one the
 * window was opened for — closes like any other once the window holds more
 * than one tab; refusing it left a shell the user could not stop from the
 * window that shows it. Alone, it keeps the window's own close as its way out
 * (the window has no permission to close itself). A shell that has already
 * ended needs no confirmation.
 */
export function detachedTabCloseIntent(input: {
  tabCount: number;
  ended: boolean;
}): { allowed: boolean; confirm: boolean } {
  const allowed = input.tabCount > 1;
  return { allowed, confirm: allowed && !input.ended };
}

/**
 * Title of the close control on the only tab left in a terminal window.
 * Closing the window returns every tab it holds, including ones opened there.
 */
export const detachedLastTabCloseTitle =
  "Close this window to return its tabs to p-track.";

/**
 * Said in the pane a popped-out terminal left behind once its shell ended in
 * the window — closed there, or exited on its own. The pane stops holding
 * the terminal's place: nothing is coming back to it.
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
