import { describe, expect, it, vi } from "vitest";

import { RefreshGate, RefreshLoop, WorkspaceController } from "./controller";

describe("WorkspaceController", () => {
  it("rejects responses captured before a project generation changes", () => {
    const controller = new WorkspaceController();
    controller.publish({ status: "open", generation: 1 });
    const oldRefresh = controller.capture();

    const transition = controller.beginTransition();
    expect(controller.publish({ status: "open", generation: 2 }, transition)).toBe(true);

    expect(controller.accepts(oldRefresh, 1)).toBe(false);
    expect(controller.accepts(controller.capture(), 2)).toBe(true);
  });

  it("rejects superseded transitions and invalidates requests on close", () => {
    const controller = new WorkspaceController();
    const first = controller.beginTransition();
    const second = controller.beginTransition();

    expect(controller.publish({ status: "open", generation: 1 }, first)).toBe(false);
    expect(controller.publish({ status: "open", generation: 1 }, second)).toBe(true);
    const refresh = controller.capture();
    controller.publish({ status: "welcome", generation: 1 });
    expect(controller.accepts(refresh, 1)).toBe(false);
  });
});

describe("RefreshLoop", () => {
  it("is idempotently disposable and never invokes work after disposal", () => {
    vi.useFakeTimers();
    const work = vi.fn();
    const loop = new RefreshLoop(work, 15_000);

    loop.start();
    vi.advanceTimersByTime(15_000);
    expect(work).toHaveBeenCalledTimes(1);
    loop.dispose();
    loop.dispose();
    vi.advanceTimersByTime(60_000);
    expect(work).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });
});

describe("RefreshGate", () => {
  it("coalesces overlapping explicit refreshes into one follow-up", () => {
    const gate = new RefreshGate();
    expect(gate.tryBegin()).toBe(true);
    expect(gate.tryBegin(true)).toBe(false);
    expect(gate.tryBegin(true)).toBe(false);
    expect(gate.finish()).toBe(true);
    expect(gate.tryBegin()).toBe(true);
    expect(gate.finish()).toBe(false);
  });
});
