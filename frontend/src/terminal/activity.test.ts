import { describe, expect, it } from "vitest";

import {
  acknowledgePaneActivity,
  aggregateTabIndicator,
  paneIndicator,
  paneIndicatorChanged,
  recordExit,
  recordOutput,
  resetPaneActivity,
  type PaneIndicator,
} from "./activity";

describe("pane activity", () => {
  it("records only background output as unread O(1) metadata", () => {
    const initial = resetPaneActivity("shell");
    expect(recordOutput(initial, true, 10)).toBe(initial);
    const activity = recordOutput(initial, false, 12);
    expect(activity).toEqual({
      profileKind: "shell",
      signal: "activity",
      unread: true,
      lastSignalAt: 12,
      exitCode: null,
    });
    expect(Object.keys(activity).sort()).toEqual([
      "exitCode", "lastSignalAt", "profileKind", "signal", "unread",
    ]);
  });

  it("presents one unread pulse for repeated hidden output", () => {
    let activity = resetPaneActivity("shell");
    let presentationChanges = 0;
    for (const now of [10, 11, 12]) {
      const before = paneIndicator(activity, "running", false);
      const next = recordOutput(activity, false, now);
      const after = paneIndicator(next, "running", false);
      if (paneIndicatorChanged(before, after)) presentationChanges += 1;
      activity = next;
    }
    expect(presentationChanges).toBe(1);
    expect(activity).toMatchObject({
      signal: "activity", unread: true, lastSignalAt: 12,
    });
    expect(JSON.stringify(activity)).not.toContain("output");
  });

  it("clears hidden unread state when visible without erasing outcomes", () => {
    const hidden = recordOutput(resetPaneActivity("agent"), false, 10);
    const visible = acknowledgePaneActivity(hidden);
    expect(paneIndicator(hidden, "running", false)).toEqual({
      kind: "activity", unread: true,
    });
    expect(paneIndicator(visible, "running", true)).toEqual({
      kind: "running", unread: false,
    });

    const completed = recordExit(visible, "agent", "exited", 0, null, 11);
    const acknowledged = acknowledgePaneActivity(completed);
    expect(paneIndicator(acknowledged, "exited", true)).toEqual({
      kind: "completed", unread: false,
    });
  });

  it("classifies authoritative agent outcomes without storing errors", () => {
    const initial = resetPaneActivity("agent");
    expect(recordExit(initial, "agent", "exited", 0, "", 20)).toMatchObject({
      signal: "completed", unread: true, exitCode: 0,
    });
    expect(recordExit(initial, "agent", "exited", 7, null, 21)).toMatchObject({
      signal: "failed", exitCode: 7,
    });
    const failed = recordExit(initial, "agent", "exited", 0, "secret error", 22);
    expect(failed.signal).toBe("failed");
    expect(JSON.stringify(failed)).not.toContain("secret error");
  });

  it("treats clean shell and unknown exits as exited and failed state as failed", () => {
    expect(recordExit(resetPaneActivity("shell"), "shell", "exited", 9, null, 1).signal)
      .toBe("exited");
    expect(recordExit(resetPaneActivity(), null, "exited", 0, null, 2).signal)
      .toBe("exited");
    expect(recordExit(resetPaneActivity("shell"), "shell", "failed", 0, null, 3).signal)
      .toBe("failed");
  });

  it("acknowledges unread state without erasing a terminal outcome", () => {
    const completed = recordExit(
      resetPaneActivity("agent"), "agent", "exited", 0, null, 10,
    );
    const acknowledged = acknowledgePaneActivity(completed);
    expect(acknowledged).toMatchObject({ signal: "completed", unread: false });
    expect(paneIndicator(acknowledged, "exited", true)).toEqual({
      kind: "completed", unread: false,
    });
  });

  it("distinguishes foreground running from background waiting without inferring input", () => {
    const activity = acknowledgePaneActivity(
      recordOutput(resetPaneActivity("agent"), false, 4),
    );
    expect(paneIndicator(activity, "running", true).kind).toBe("running");
    expect(paneIndicator(activity, "running", false).kind).toBe("waiting");
    expect(paneIndicator(resetPaneActivity("agent"), "exited", false).kind)
      .toBe("exited");
  });

  it("aggregates by stable priority while preserving any unread signal", () => {
    const indicators = new Map<string, PaneIndicator>([
      ["waiting", { kind: "waiting", unread: false }],
      ["done", { kind: "completed", unread: false }],
      ["output", { kind: "activity", unread: true }],
      ["failed", { kind: "failed", unread: false }],
    ]);
    expect(aggregateTabIndicator(
      ["waiting", "done", "output"],
      (paneId) => indicators.get(paneId)!,
    )).toEqual({ kind: "completed", unread: true });
    expect(aggregateTabIndicator(
      ["waiting", "failed", "output"],
      (paneId) => indicators.get(paneId)!,
    )).toEqual({ kind: "failed", unread: true });
    expect(aggregateTabIndicator([], () => ({ kind: "running", unread: true })))
      .toEqual({ kind: "closed", unread: false });
  });
});
