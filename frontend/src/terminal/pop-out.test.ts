import { describe, expect, it, vi } from "vitest";

import {
  panesHoldPoppedOutTerminal,
  popOutTerminal,
  poppedOutCloseRefusedNotice,
  reclaimStream,
  streamLossIsRecoverable,
  streamReclaimDelay,
  streamReclaimDelays,
  terminalPopOutControl,
  terminalWindowLabel,
} from "./pop-out";

describe("terminalWindowLabel", () => {
  it("reads a minted label from the fragment and rejects anything else", () => {
    expect(terminalWindowLabel("#terminal-window=terminal-1")).toBe("terminal-1");
    expect(terminalWindowLabel("#terminal-window=terminal-12")).toBe("terminal-12");
    expect(terminalWindowLabel("")).toBeNull();
    expect(terminalWindowLabel("#terminal-window=")).toBeNull();
    expect(terminalWindowLabel("#terminal-window=main")).toBeNull();
    expect(terminalWindowLabel("#terminal-window=terminal-1/../main")).toBeNull();
    expect(terminalWindowLabel("#plan=4")).toBeNull();
  });
});

/// The pane a popped-out terminal left behind has no session, so no close path
/// confirms its removal — and once it is gone the window's pop-in finds nowhere
/// to hand the session back to and closes a running shell instead. Every close
/// that covers a holder pane is refused rather than confirmed: the terminal is
/// closed in its own window.
describe("panesHoldPoppedOutTerminal", () => {
  // Session id → the pane holding its place, as the dock keeps it.
  const poppedOut = new Map([["session-a", "pane-2"]]);

  it("refuses a close covering a pane that holds a popped-out terminal", () => {
    expect(panesHoldPoppedOutTerminal(["pane-2"], poppedOut.values())).toBe(true);
    // A whole tab closing takes the holder with it.
    expect(panesHoldPoppedOutTerminal(["pane-1", "pane-2"], poppedOut.values()))
      .toBe(true);
    // …and so does a workspace reset, which closes every pane there is.
    expect(panesHoldPoppedOutTerminal(["pane-1", "pane-2", "pane-3"], poppedOut.values()))
      .toBe(true);
  });

  it("leaves every other close alone", () => {
    expect(panesHoldPoppedOutTerminal(["pane-1"], poppedOut.values())).toBe(false);
    expect(panesHoldPoppedOutTerminal([], poppedOut.values())).toBe(false);
    expect(panesHoldPoppedOutTerminal(["pane-2"], new Map().values())).toBe(false);
    // The session id is not a pane id: only the holder matches.
    expect(panesHoldPoppedOutTerminal(["session-a"], poppedOut.values())).toBe(false);
  });

  it("says where the terminal can be closed instead", () => {
    expect(poppedOutCloseRefusedNotice).toContain("window");
  });
});

describe("terminalPopOutControl", () => {
  const running = {
    paneCount: 1,
    state: "running" as const,
    hasSession: true,
    busy: false,
    closing: false,
  };

  it("is present only for a single running pane", () => {
    expect(terminalPopOutControl(running).present).toBe(true);
    expect(terminalPopOutControl({ ...running, paneCount: 2 }).present).toBe(false);
    expect(terminalPopOutControl({ ...running, state: "closed" }).present).toBe(false);
    expect(terminalPopOutControl({ ...running, state: "exited" }).present).toBe(false);
    expect(terminalPopOutControl({ ...running, hasSession: false }).present).toBe(false);
  });

  it("stays present but disabled while the pane is busy or closing", () => {
    expect(terminalPopOutControl({ ...running, busy: true }))
      .toEqual({ present: true, disabled: true });
    expect(terminalPopOutControl({ ...running, closing: true }))
      .toEqual({ present: true, disabled: true });
    expect(terminalPopOutControl(running).disabled).toBe(false);
  });
});

describe("streamLossIsRecoverable", () => {
  const live = {
    state: "running" as const,
    closing: false,
    hasSession: true,
    hasRenderer: true,
  };

  it("reconnects a live pane and nothing the user ended deliberately", () => {
    expect(streamLossIsRecoverable(live)).toBe(true);
    expect(streamLossIsRecoverable({ ...live, state: "opening" })).toBe(true);
    // A closed terminal, an exited shell, and a pane being torn down all stay ended.
    expect(streamLossIsRecoverable({ ...live, closing: true })).toBe(false);
    expect(streamLossIsRecoverable({ ...live, state: "exited" })).toBe(false);
    expect(streamLossIsRecoverable({ ...live, state: "closed" })).toBe(false);
    expect(streamLossIsRecoverable({ ...live, hasSession: false })).toBe(false);
    expect(streamLossIsRecoverable({ ...live, hasRenderer: false })).toBe(false);
  });
});

describe("streamReclaimDelays", () => {
  it("spends the whole budget inside the 30s re-claim grace window", () => {
    const total = streamReclaimDelays.reduce((sum, delay) => sum + delay, 0);

    expect(total).toBeLessThan(30_000);
    expect(streamReclaimDelays.length).toBeLessThanOrEqual(5);
    expect(streamReclaimDelay(streamReclaimDelays.length)).toBeNull();
  });
});

describe("reclaimStream", () => {
  function harness(overrides: Partial<Parameters<typeof reclaimStream>[0]> = {}) {
    const waits: number[] = [];
    const claimed: number[] = [];
    const attached: unknown[] = [];
    const steps = {
      recoverable: () => true,
      sequence: () => 4096,
      wait: async (delay: number) => void waits.push(delay),
      claim: async (fromSequence: number) => {
        claimed.push(fromSequence);
        return { url: "ws://127.0.0.1/terminal/s?token=fresh", fromSequence, gap: false };
      },
      attach: (claim: unknown) => void attached.push(claim),
      reclaiming: () => {},
      exhausted: () => {},
      ...overrides,
    };
    return { steps, waits, claimed, attached };
  }

  it("re-attaches from the last rendered sequence after an unintended loss", async () => {
    const { steps, waits, claimed, attached } = harness();

    await expect(reclaimStream(steps)).resolves.toBe("attached");
    expect(waits).toEqual([streamReclaimDelays[0]]);
    expect(claimed).toEqual([4096]);
    expect(attached).toEqual([
      { url: "ws://127.0.0.1/terminal/s?token=fresh", fromSequence: 4096, gap: false },
    ]);
  });

  it("does not reconnect a stream the pane stopped owning", async () => {
    const { steps, waits, claimed } = harness({ recoverable: () => false });

    await expect(reclaimStream(steps)).resolves.toBe("abandoned");
    expect(waits).toEqual([]);
    expect(claimed).toEqual([]);
  });

  it("continues the backoff from an unspent budget instead of restarting it", async () => {
    const exhausted = vi.fn();
    const { steps, waits } = harness({
      claim: async () => {
        throw new Error("terminal replay sequence is unavailable");
      },
      exhausted,
    });

    await expect(reclaimStream(steps, streamReclaimDelays.length - 1))
      .resolves.toBe("exhausted");
    expect(waits).toEqual([streamReclaimDelays.at(-1)]);
    expect(exhausted).toHaveBeenCalledOnce();
  });

  it("gives up after the bounded attempts rather than retrying forever", async () => {
    const exhausted = vi.fn();
    const { steps, waits, claimed, attached } = harness({
      claim: async (fromSequence: number) => {
        claimed.push(fromSequence);
        throw new Error("terminal replay sequence is unavailable");
      },
      exhausted,
    });

    await expect(reclaimStream(steps)).resolves.toBe("exhausted");
    expect(waits).toEqual([...streamReclaimDelays]);
    expect(claimed).toHaveLength(streamReclaimDelays.length);
    expect(attached).toEqual([]);
    expect(exhausted).toHaveBeenCalledOnce();
  });
});

describe("popOutTerminal", () => {
  it("releases before opening the window and reports the minted label", async () => {
    const order: string[] = [];
    const reclaim = vi.fn(async () => {});

    const result = await popOutTerminal({
      release: () => void order.push("release"),
      open: async () => {
        order.push("open");
        return { label: "terminal-1" };
      },
      reclaim,
    });

    expect(order).toEqual(["release", "open"]);
    expect(result).toEqual({ outcome: "popped-out", label: "terminal-1", error: null });
    expect(reclaim).not.toHaveBeenCalled();
  });

  it("re-claims the session when the window cannot be opened", async () => {
    const failure = new Error("window build failed");
    const reclaim = vi.fn(async () => {});

    const result = await popOutTerminal({
      release: () => {},
      open: () => Promise.reject(failure),
      reclaim,
    });

    expect(reclaim).toHaveBeenCalledOnce();
    expect(result).toEqual({ outcome: "kept", label: "", error: failure });
  });

  it("re-claims the session when the release itself fails, without opening a window", async () => {
    const failure = new Error("renderer teardown failed");
    const open = vi.fn(async () => ({ label: "terminal-1" }));
    const reclaim = vi.fn(async () => {});

    const result = await popOutTerminal({
      release: () => {
        throw failure;
      },
      open,
      reclaim,
    });

    expect(open).not.toHaveBeenCalled();
    expect(reclaim).toHaveBeenCalledOnce();
    expect(result).toEqual({ outcome: "kept", label: "", error: failure });
  });

  it("reports an unowned session when the re-claim fails too", async () => {
    const reclaimFailure = new Error("claim refused");

    const result = await popOutTerminal({
      release: () => {},
      open: () => Promise.reject(new Error("window build failed")),
      reclaim: () => Promise.reject(reclaimFailure),
    });

    expect(result).toEqual({ outcome: "unowned", label: "", error: reclaimFailure });
  });
});
