import { describe, expect, it, vi } from "vitest";

import {
  PopOutExitLedger,
  returnedWindowTabs,
  streamClaimEnded,
  streamCloseDisposition,
  detachedLastTabCloseTitle,
  detachedTabCloseIntent,
  panesHoldPoppedOutTerminal,
  popOutTerminal,
  poppedOutCloseRefusedNotice,
  poppedOutExitNotice,
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
  const running = { state: "running" as const, hasSession: true };
  const tab = { panes: [running, running], busy: false, closing: false };

  it("is present only when every pane of the tab is running with a session", () => {
    expect(terminalPopOutControl(tab).present).toBe(true);
    expect(terminalPopOutControl({ ...tab, panes: [running] }).present).toBe(true);
    expect(terminalPopOutControl({ ...tab, panes: [] }).present).toBe(false);
    expect(
      terminalPopOutControl({
        ...tab,
        panes: [running, { state: "closed" as const, hasSession: true }],
      }).present,
    ).toBe(false);
    expect(
      terminalPopOutControl({
        ...tab,
        panes: [running, { state: "exited" as const, hasSession: true }],
      }).present,
    ).toBe(false);
    expect(
      terminalPopOutControl({
        ...tab,
        panes: [running, { state: "running" as const, hasSession: false }],
      }).present,
    ).toBe(false);
  });

  it("stays present but disabled while the tab is busy or closing", () => {
    expect(terminalPopOutControl({ ...tab, busy: true }))
      .toEqual({ present: true, disabled: true });
    expect(terminalPopOutControl({ ...tab, closing: true }))
      .toEqual({ present: true, disabled: true });
    expect(terminalPopOutControl(tab).disabled).toBe(false);
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

/// Every tab a terminal window holds closes from that window, the original
/// included: a shell the user can see but cannot stop is a trap. Only the last
/// tab keeps the window's own close as its way out, because the window takes
/// no permission to close itself.
describe("detachedTabCloseIntent", () => {
  it("closes the original tab like any other once the window holds more tabs", () => {
    expect(detachedTabCloseIntent({ tabCount: 2, ended: false }))
      .toEqual({ allowed: true, confirm: true });
    expect(detachedTabCloseIntent({ tabCount: 3, ended: false }).allowed).toBe(true);
  });

  it("skips the confirmation for a shell that already ended", () => {
    expect(detachedTabCloseIntent({ tabCount: 2, ended: true }))
      .toEqual({ allowed: true, confirm: false });
  });

  it("keeps the last tab for the window's own close", () => {
    expect(detachedTabCloseIntent({ tabCount: 1, ended: false }))
      .toEqual({ allowed: false, confirm: false });
    expect(detachedTabCloseIntent({ tabCount: 1, ended: true }).allowed).toBe(false);
    expect(detachedTabCloseIntent({ tabCount: 0, ended: false }).allowed).toBe(false);
    expect(detachedLastTabCloseTitle).toContain("Close this window");
    // Closing the window returns every tab, not only the one it opened with.
    expect(detachedLastTabCloseTitle).toContain("its tabs");
  });
});

describe("poppedOutExitNotice", () => {
  it("says the shell ended in its window, with the code or the error", () => {
    expect(poppedOutExitNotice({ exitCode: 0 }))
      .toBe("Process exited with code 0 in its own window");
    expect(poppedOutExitNotice({ exitCode: 1, error: " spawn failed " }))
      .toBe("spawn failed (in its own window)");
    expect(poppedOutExitNotice({ exitCode: 130, error: null }))
      .toBe("Process exited with code 130 in its own window");
  });
});

describe("stream endings", () => {
  it("never re-claims a stream whose output ended or whose session already exited", () => {
    expect(streamCloseDisposition({ outputEnded: true, sessionEnded: false, recoverable: true }))
      .toBe("ended");
    expect(streamCloseDisposition({ outputEnded: false, sessionEnded: true, recoverable: true }))
      .toBe("ended");
    expect(streamCloseDisposition({ outputEnded: false, sessionEnded: false, recoverable: true }))
      .toBe("reclaim");
    expect(streamCloseDisposition({ outputEnded: false, sessionEnded: false, recoverable: false }))
      .toBe("lost");
  });

  it("reads the claimed session state", () => {
    expect(streamClaimEnded({ state: "running" })).toBe(false);
    expect(streamClaimEnded({})).toBe(false);
    for (const state of ["exited", "failed", "closed"]) {
      expect(streamClaimEnded({ state })).toBe(true);
    }
  });

  it("stops the re-claim loop once a claim reports the shell exited", async () => {
    // The loop this pins: replay, normal close, re-claim, replay, forever.
    let claims = 0;
    let ended = false;
    const attach = async () => {
      const outcome = await reclaimStream({
        recoverable: () => !ended,
        sequence: () => 0,
        wait: async () => {},
        claim: async () => {
          claims += 1;
          return { url: "ws://x", fromSequence: 0, gap: false, state: "exited" };
        },
        attach: (claim) => {
          // What each surface does with an ended claim, then the normal close.
          const disposition = streamCloseDisposition({
            outputEnded: true,
            sessionEnded: streamClaimEnded(claim),
            recoverable: true,
          });
          if (disposition === "ended") ended = true;
        },
        reclaiming: () => {},
        exhausted: () => {},
      });
      return outcome;
    };
    expect(await attach()).toBe("attached");
    expect(await attach()).toBe("abandoned");
    expect(claims).toBe(1);
  });
});

describe("PopOutExitLedger", () => {
  it("keeps an exit that lands while a tab is moving, and only then", () => {
    const ledger = new PopOutExitLedger<{ sessionId: string; exitCode: number }>();
    expect(ledger.record({ sessionId: "a", exitCode: 0 })).toBe(false);
    ledger.begin(["a", "b"]);
    expect(ledger.record({ sessionId: "a", exitCode: 3 })).toBe(true);
    expect(ledger.record({ sessionId: "other", exitCode: 1 })).toBe(false);
    const exits = ledger.finish();
    expect([...exits.entries()]).toEqual([["a", { sessionId: "a", exitCode: 3 }]]);
    // The move is over: later exits take the normal route again.
    expect(ledger.record({ sessionId: "b", exitCode: 0 })).toBe(false);
    expect(ledger.finish().size).toBe(0);
  });
});

describe("returnedWindowTabs", () => {
  const pane = (paneId: string, profileId: string, cwd: string) =>
    ({ kind: "terminal", paneId, profileId, cwd });
  const payload = {
    sessions: ["held-1", "held-2", "new-1", "new-2"],
    shape: {
      id: "tab-original",
      windowTabs: [
        {
          id: "tab-original",
          title: "Build",
          root: {
            kind: "split",
            first: pane("p1", "shell", "/repo"),
            second: pane("p2", "shell", "/repo/web"),
          },
        },
        { id: "tab-2", title: "Terminal 2", root: pane("p3", "zsh", "/repo/api") },
        { id: "tab-3", title: "Logs", root: pane("p4", "", "") },
      ],
    },
  };

  it("hands back every session the window opened itself, in pane order", () => {
    const held = new Set(["held-1", "held-2"]);
    expect(returnedWindowTabs(payload, (id) => held.has(id))).toEqual([
      { sessionId: "new-1", title: "Terminal 2", profileId: "zsh", cwd: "/repo/api" },
      { sessionId: "new-2", title: "Logs", profileId: "", cwd: "" },
    ]);
  });

  it("returns nothing when the window only held the original tab", () => {
    expect(returnedWindowTabs(payload, () => true)).toEqual([]);
  });

  it("still returns a session when the shape is missing or malformed", () => {
    expect(returnedWindowTabs({ sessions: ["x"], shape: { windowTabs: "nope" } }, () => false))
      .toEqual([{ sessionId: "x", title: "Terminal", profileId: "", cwd: "" }]);
  });
});
