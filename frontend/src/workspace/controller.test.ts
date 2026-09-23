import { describe, expect, it, vi } from "vitest";

import {
  GenerationSlot,
  InFlightOperations,
  RefreshGate,
  RefreshLoop,
  RequestSequence,
  RuntimeRefreshCoalescer,
  WorkspaceController,
} from "./controller";

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

  it("lets a deferred close follow-up see that a new open started", () => {
    const controller = new WorkspaceController();
    const close = controller.beginTransition();
    expect(controller.publish({ status: "closed", generation: 1 }, close)).toBe(true);
    const followUp = controller.capture();
    expect(controller.isCurrent(followUp)).toBe(true);
    controller.beginTransition();
    expect(controller.isCurrent(followUp)).toBe(false);
    expect(controller.publish({ status: "welcome", generation: 1 }, followUp)).toBe(false);
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

  it("skips ticks while paused and refreshes once on resume", () => {
    vi.useFakeTimers();
    const work = vi.fn();
    let hidden = true;
    const loop = new RefreshLoop(work, 15_000, () => hidden);

    loop.resume();
    expect(work).not.toHaveBeenCalled();
    loop.start();
    vi.advanceTimersByTime(45_000);
    expect(work).not.toHaveBeenCalled();
    loop.resume();
    expect(work).not.toHaveBeenCalled();
    hidden = false;
    loop.resume();
    expect(work).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(15_000);
    expect(work).toHaveBeenCalledTimes(2);
    loop.dispose();
    loop.resume();
    expect(work).toHaveBeenCalledTimes(2);
    vi.useRealTimers();
  });
});

describe("RequestSequence", () => {
  it("lets only the latest request apply and invalidates on demand", () => {
    const sequence = new RequestSequence();
    const first = sequence.next();
    const second = sequence.next();
    expect(sequence.isCurrent(first)).toBe(false);
    expect(sequence.isCurrent(second)).toBe(true);
    sequence.invalidate();
    expect(sequence.isCurrent(second)).toBe(false);
  });
});

describe("InFlightOperations", () => {
  it("refuses a duplicate start until the first finishes", () => {
    const operations = new InFlightOperations();
    expect(operations.begin("add-task")).toBe(true);
    expect(operations.begin("add-task")).toBe(false);
    expect(operations.begin("rename:1")).toBe(true);
    expect(operations.has("add-task")).toBe(true);
    operations.end("add-task");
    expect(operations.has("add-task")).toBe(false);
    expect(operations.begin("add-task")).toBe(true);
    operations.clear();
    expect(operations.has("rename:1")).toBe(false);
  });
});

describe("RuntimeRefreshCoalescer", () => {
  it("coalesces a heartbeat burst and retains only the latest generation", () => {
    vi.useFakeTimers();
    const work = vi.fn();
    const coalescer = new RuntimeRefreshCoalescer(work, 100);

    coalescer.request(4);
    coalescer.request(4);
    coalescer.request(5);
    vi.advanceTimersByTime(99);
    expect(work).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(work).toHaveBeenCalledTimes(1);
    expect(work).toHaveBeenCalledWith(5);

    coalescer.request(5);
    coalescer.cancel();
    vi.advanceTimersByTime(100);
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

  it("lets exact refresh callers await an occupied gate", async () => {
    const gate = new RefreshGate();
    expect(gate.tryBegin()).toBe(true);
    let idle = false;
    const waiting = gate.whenIdle().then(() => { idle = true; });
    await Promise.resolve();
    expect(idle).toBe(false);
    gate.finish();
    await waiting;
    expect(idle).toBe(true);

    expect(gate.tryBegin()).toBe(true);
    const resetWaiting = gate.whenIdle();
    gate.reset();
    await expect(resetWaiting).resolves.toBeUndefined();
  });

  it("keeps exact waiters behind a queued follow-up", async () => {
    const gate = new RefreshGate();
    expect(gate.tryBegin()).toBe(true);
    expect(gate.tryBegin(true)).toBe(false);
    let idle = false;
    const waiting = gate.whenIdle().then(() => { idle = true; });
    expect(gate.finish()).toBe(true);
    await Promise.resolve();
    expect(idle).toBe(false);

    let gapIdle = false;
    const gapWaiting = gate.whenIdle().then(() => { gapIdle = true; });
    await Promise.resolve();
    expect(gapIdle).toBe(false);
    expect(gate.tryBegin()).toBe(true);
    expect(gate.finish()).toBe(false);
    await waiting;
    await gapWaiting;
    expect(idle).toBe(true);
    expect(gapIdle).toBe(true);
  });

  it("cancels a stranded queued handoff when the workspace leaves open", async () => {
    const gate = new RefreshGate();
    expect(gate.tryBegin()).toBe(true);
    expect(gate.tryBegin(true)).toBe(false);
    expect(gate.finish()).toBe(true);
    let idle = false;
    const waiting = gate.whenIdle().then(() => { idle = true; });
    gate.cancelQueued();
    await waiting;
    expect(idle).toBe(true);
  });
});

describe("GenerationSlot", () => {
  it("yields a deferred task only to the generation that asked for it", () => {
    const slot = new GenerationSlot<number>();
    slot.set(7, 3);
    expect(slot.pending).toBe(true);
    expect(slot.take({ status: "open", generation: 4 })).toBeNull();
    expect(slot.pending).toBe(false);

    slot.set(7, 4);
    expect(slot.take({ status: "loading", generation: 4 })).toBeNull();
    slot.set(7, 4);
    expect(slot.take({ status: "open", generation: 4 })).toBe(7);
    expect(slot.take({ status: "open", generation: 4 })).toBeNull();

    slot.set(9, 4);
    slot.clear();
    expect(slot.take({ status: "open", generation: 4 })).toBeNull();
  });
});
