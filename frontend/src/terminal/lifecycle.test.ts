import { describe, expect, it, vi } from "vitest";

import {
  closeIntentNeedsConfirmation,
  closeIntentConfirmed,
  PendingSessionCloseCoordinator,
  PendingSessionCloseError,
  PaneLifecycleCoordinator,
  runDescriptorCloseIntent,
  settlePendingCreatedSession,
} from "./lifecycle";
import { PaneRuntimeRegistry } from "./runtime";

interface TestSession {
  sessionId: string;
}

interface TestResources {
  name: string;
}

function fixture() {
  const registry = new PaneRuntimeRegistry<TestSession, TestResources>();
  const closeSession = vi.fn(async () => {});
  const disposeResources = vi.fn();
  const deleteEarlyExit = vi.fn();
  const lifecycle = new PaneLifecycleCoordinator(registry, {
    closeSession,
    disposeResources,
    deleteEarlyExit,
  });
  return { registry, lifecycle, closeSession, disposeResources, deleteEarlyExit };
}

describe("PaneLifecycleCoordinator", () => {
  it("switches A/B without closing or replacing either resource", () => {
    const { registry, closeSession, disposeResources } = fixture();
    const first = registry.ensure("pane-a");
    const second = registry.ensure("pane-b");
    first.resources = { name: "first" };
    second.resources = { name: "second" };
    const firstResources = first.resources;
    const secondResources = second.resources;

    let activePaneId = first.paneId;
    activePaneId = second.paneId;
    activePaneId = first.paneId;
    expect(activePaneId).toBe("pane-a");
    expect(first.resources).toBe(firstResources);
    expect(second.resources).toBe(secondResources);
    expect(closeSession).not.toHaveBeenCalled();
    expect(disposeResources).not.toHaveBeenCalled();
  });

  it("memoizes double close to one backend call and one disposal", async () => {
    const { registry, lifecycle, closeSession, disposeResources, deleteEarlyExit } = fixture();
    const runtime = registry.ensure("pane-a");
    runtime.state = "running";
    runtime.session = { sessionId: "session-a" };
    runtime.resources = { name: "renderer-a" };

    const first = lifecycle.close(runtime.paneId);
    const second = lifecycle.close(runtime.paneId);
    expect(second).toBe(first);
    await Promise.all([first, second]);
    expect(closeSession).toHaveBeenCalledOnce();
    expect(closeSession).toHaveBeenCalledWith("session-a", false);
    expect(disposeResources).toHaveBeenCalledOnce();
    expect(deleteEarlyExit).toHaveBeenCalledWith("session-a");
    expect(runtime).toMatchObject({ state: "closed", session: null, resources: null });
    expect(runtime.activity).toMatchObject({ signal: "none", unread: false });
  });

  it("force-closes the exact current session once", async () => {
    const { registry, lifecycle, closeSession, disposeResources } = fixture();
    const runtime = registry.ensure("pane-a");
    runtime.state = "running";
    runtime.session = { sessionId: "session-force" };
    runtime.resources = { name: "renderer-force" };

    const first = lifecycle.close(runtime.paneId, true);
    const duplicate = lifecycle.close(runtime.paneId, true);
    expect(duplicate).toBe(first);
    await Promise.all([first, duplicate]);

    expect(closeSession).toHaveBeenCalledOnce();
    expect(closeSession).toHaveBeenCalledWith("session-force", true);
    expect(disposeResources).toHaveBeenCalledOnce();
    expect(runtime).toMatchObject({ state: "closed", session: null, resources: null });
  });

  it("does not repeat a successful backend close when local disposal is retried", async () => {
    const registry = new PaneRuntimeRegistry<TestSession, TestResources>();
    const closeSession = vi.fn(async () => {});
    let disposeAttempts = 0;
    const lifecycle = new PaneLifecycleCoordinator(registry, {
      closeSession,
      disposeResources: () => {
        disposeAttempts += 1;
        if (disposeAttempts === 1) throw new Error("dispose failed");
      },
      deleteEarlyExit: vi.fn(),
    });
    const runtime = registry.ensure("pane-a");
    runtime.state = "running";
    runtime.session = { sessionId: "session-a" };
    runtime.resources = { name: "renderer-a" };
    const oldTicket = registry.capture(runtime.paneId)!;

    await expect(lifecycle.close(runtime.paneId)).rejects.toThrow("dispose failed");
    await expect(lifecycle.close(runtime.paneId)).resolves.toBeUndefined();
    expect(closeSession).toHaveBeenCalledOnce();
    expect(disposeAttempts).toBe(2);
    expect(registry.accepts(oldTicket)).toBe(false);
    expect(runtime).toMatchObject({ state: "closed", session: null, resources: null });
  });

  it("rolls back close invalidation after backend failure so existing events resume", async () => {
    const registry = new PaneRuntimeRegistry<TestSession, TestResources>();
    let attempts = 0;
    const closeSession = vi.fn(async () => {
      attempts += 1;
      if (attempts === 1) throw new Error("backend close failed");
    });
    const lifecycle = new PaneLifecycleCoordinator(registry, {
      closeSession,
      disposeResources: vi.fn(),
      deleteEarlyExit: vi.fn(),
    });
    const runtime = registry.ensure("pane-a");
    runtime.state = "running";
    runtime.session = { sessionId: "session-a" };
    runtime.resources = { name: "renderer-a" };
    const oldTicket = registry.capture(runtime.paneId)!;

    await expect(lifecycle.close(runtime.paneId)).rejects.toThrow("backend close failed");
    expect(registry.accepts(oldTicket)).toBe(true);
    expect(runtime).toMatchObject({
      state: "running",
      session: { sessionId: "session-a" },
      resources: { name: "renderer-a" },
      closing: false,
      busy: false,
    });
    await expect(lifecycle.close(runtime.paneId)).resolves.toBeUndefined();
    expect(closeSession).toHaveBeenCalledTimes(2);
  });

  it("never rolls back over a newer pane epoch", async () => {
    const registry = new PaneRuntimeRegistry<TestSession, TestResources>();
    let rejectClose!: (error: Error) => void;
    const closeSession = vi.fn(() => new Promise<void>((_resolve, reject) => {
      rejectClose = reject;
    }));
    const lifecycle = new PaneLifecycleCoordinator(registry, {
      closeSession,
      disposeResources: vi.fn(),
      deleteEarlyExit: vi.fn(),
    });
    const runtime = registry.ensure("pane-a");
    runtime.state = "running";
    runtime.session = { sessionId: "session-a" };
    const oldTicket = registry.capture(runtime.paneId)!;
    const closing = lifecycle.close(runtime.paneId);
    await Promise.resolve();
    const newerTicket = registry.begin(runtime.paneId);
    rejectClose(new Error("backend close failed"));
    await expect(closing).rejects.toThrow("backend close failed");
    expect(registry.accepts(newerTicket)).toBe(true);
    expect(registry.accepts(oldTicket)).toBe(false);
  });

  it("invalidates an opening pane so a late create is force-closed once", async () => {
    const { registry, lifecycle, closeSession, disposeResources } = fixture();
    const runtime = registry.ensure("pane-a");
    runtime.state = "opening";
    runtime.resources = { name: "opening-renderer" };
    const resources = runtime.resources;
    const createTicket = registry.begin(runtime.paneId);

    await lifecycle.close(runtime.paneId);
    expect(registry.accepts(createTicket)).toBe(false);
    const lateSession = { sessionId: "late-session" };
    await settlePendingCreatedSession({
      sessionId: lateSession.sessionId,
      resources,
      accepts: () => registry.accepts(createTicket),
      resourcesDisposed: () => disposeResources.mock.calls.some(([item]) => item === resources),
      forceClose: (sessionId) => closeSession(sessionId, true),
      disposeResources,
    });
    expect(closeSession).toHaveBeenCalledOnce();
    expect(closeSession).toHaveBeenCalledWith("late-session", true);
    expect(disposeResources).toHaveBeenCalledOnce();
    expect(runtime).toMatchObject({ state: "closed", session: null, resources: null });
  });

  it("accepts a current create without closing or disposing it", async () => {
    const closeSession = vi.fn(async () => {});
    const disposeResources = vi.fn();
    const resources = { name: "renderer" };
    await expect(settlePendingCreatedSession({
      sessionId: "session-a",
      resources,
      accepts: () => true,
      resourcesDisposed: () => false,
      forceClose: closeSession,
      disposeResources,
    })).resolves.toBe(true);
    expect(closeSession).not.toHaveBeenCalled();
    expect(disposeResources).not.toHaveBeenCalled();
  });

  it("surfaces stale create force-close failure without discarding authority", async () => {
    const forceClose = vi.fn(async () => { throw new Error("already gone"); });
    const disposeResources = vi.fn(() => { throw new Error("renderer gone"); });
    await expect(settlePendingCreatedSession({
      sessionId: "late-session",
      resources: { name: "renderer" },
      accepts: () => false,
      resourcesDisposed: () => false,
      forceClose,
      disposeResources,
    })).rejects.toThrow("already gone");
    expect(forceClose).toHaveBeenCalledOnce();
    expect(disposeResources).not.toHaveBeenCalled();
  });

  it("retries a stale session exactly, retains permanent failures, and never duplicates success", async () => {
    let attempts = 0;
    const forceClose = vi.fn(async () => {
      attempts += 1;
      if (attempts === 1) throw new Error("transient");
    });
    const disposed = new Set<TestResources>();
    const disposeResources = vi.fn((resources: TestResources) => disposed.add(resources));
    const coordinator = new PendingSessionCloseCoordinator({
      forceClose,
      resourcesDisposed: (resources) => disposed.has(resources),
      disposeResources,
      maximumAttempts: 2,
    });
    const resources = { name: "late-renderer" };
    await expect(coordinator.settle({
      sessionId: "late-session",
      resources,
      accepts: () => false,
    })).resolves.toBe(false);
    expect(forceClose).toHaveBeenCalledTimes(2);
    expect(disposeResources).toHaveBeenCalledOnce();
    await expect(coordinator.settle({
      sessionId: "late-session",
      resources,
      accepts: () => false,
    })).resolves.toBe(false);
    expect(forceClose).toHaveBeenCalledTimes(2);
    expect(coordinator.pendingCount).toBe(0);

    const permanentClose = vi.fn(async () => { throw new Error("permanent"); });
    const permanentResources = { name: "orphan-renderer" };
    const permanent = new PendingSessionCloseCoordinator({
      forceClose: permanentClose,
      resourcesDisposed: (item) => disposed.has(item),
      disposeResources,
      maximumAttempts: 2,
    });
    await expect(permanent.settle({
      sessionId: "orphan-session",
      resources: permanentResources,
      accepts: () => false,
    })).rejects.toBeInstanceOf(PendingSessionCloseError);
    expect(permanentClose).toHaveBeenCalledTimes(2);
    expect(permanent.pendingCount).toBe(1);
    permanent.releaseToProjectShutdown();
    expect(permanent.pendingCount).toBe(0);
    expect(permanentClose).toHaveBeenCalledTimes(2);
  });

  it("bounds successful stale-session deduplication across restarts", async () => {
    const coordinator = new PendingSessionCloseCoordinator<TestResources>({
      forceClose: async () => {},
      resourcesDisposed: () => false,
      disposeResources: () => {},
      maximumRememberedCloses: 3,
    });
    for (let index = 0; index < 10; index += 1) {
      await coordinator.settle({
        sessionId: `session-${index}`,
        resources: { name: `renderer-${index}` },
        accepts: () => false,
      });
    }
    expect(coordinator.rememberedCloseCount).toBe(3);
  });

  it("finishes closed when an exit arrives during close", async () => {
    const registry = new PaneRuntimeRegistry<TestSession, TestResources>();
    let finishClose!: () => void;
    const closeSession = vi.fn(() => new Promise<void>((resolve) => { finishClose = resolve; }));
    const lifecycle = new PaneLifecycleCoordinator(registry, {
      closeSession,
      disposeResources: vi.fn(),
      deleteEarlyExit: vi.fn(),
    });
    const runtime = registry.ensure("pane-a");
    runtime.state = "running";
    runtime.session = { sessionId: "session-a" };
    const closing = lifecycle.close(runtime.paneId);
    runtime.state = "exited";
    await Promise.resolve();
    finishClose();
    await closing;
    expect(runtime.state).toBe("closed");
    expect(runtime.session).toBeNull();
  });

  it("releases local resources repeatedly without a backend call", () => {
    const { registry, lifecycle, closeSession, disposeResources } = fixture();
    const runtime = registry.ensure("pane-a");
    runtime.session = { sessionId: "session-a" };
    runtime.resources = { name: "renderer-a" };
    lifecycle.releaseLocal(runtime.paneId);
    lifecycle.releaseLocal(runtime.paneId);
    expect(closeSession).not.toHaveBeenCalled();
    expect(disposeResources).toHaveBeenCalledOnce();
  });

  it("releases a multi-pane project locally exactly once with zero session RPCs", () => {
    const { registry, lifecycle, closeSession, disposeResources } = fixture();
    for (const paneId of ["a", "b", "c"]) {
      const runtime = registry.ensure(paneId);
      runtime.state = "running";
      runtime.session = { sessionId: `session-${paneId}` };
      runtime.resources = { name: `renderer-${paneId}` };
    }
    lifecycle.releaseManyLocal(["a", "b", "a", "c"]);
    lifecycle.releaseManyLocal(["a", "b", "c"]);
    expect(closeSession).not.toHaveBeenCalled();
    expect(disposeResources).toHaveBeenCalledTimes(3);
    expect(registry.values().every((runtime) =>
      runtime.state === "closed" && runtime.session === null && runtime.resources === null
    )).toBe(true);
  });

  it("invalidates pending creates on project disposal and force-closes late sessions once", async () => {
    const { registry, lifecycle, closeSession, disposeResources } = fixture();
    const pending = ["a", "b"].map((paneId) => {
      const runtime = registry.ensure(paneId);
      runtime.state = "opening";
      runtime.resources = { name: `renderer-${paneId}` };
      return { runtime, resources: runtime.resources, ticket: registry.begin(paneId) };
    });
    lifecycle.releaseManyLocal(pending.map(({ runtime }) => runtime.paneId));
    expect(closeSession).not.toHaveBeenCalled();
    await Promise.all(pending.map(({ runtime, resources, ticket }) =>
      settlePendingCreatedSession({
        sessionId: `late-${runtime.paneId}`,
        resources,
        accepts: () => registry.accepts(ticket),
        resourcesDisposed: () => disposeResources.mock.calls.some(([item]) => item === resources),
        forceClose: (sessionId) => closeSession(sessionId, true),
        disposeResources,
      })
    ));
    expect(closeSession.mock.calls).toEqual([
      ["late-a", true],
      ["late-b", true],
    ]);
    expect(disposeResources).toHaveBeenCalledTimes(2);
    expect(pending.every(({ runtime }) =>
      runtime.state === "closed" && runtime.session === null && runtime.resources === null
    )).toBe(true);
  });
});

describe("descriptor close intent", () => {
  it("prompts for live authority but not authoritative exits or sessionless failures", async () => {
    const session = { sessionId: "session" };
    expect(closeIntentNeedsConfirmation([
      { state: "failed", session },
    ])).toBe(true);
    expect(closeIntentNeedsConfirmation([
      { state: "exited", session },
      { state: "exited", session: null },
      { state: "failed", session: null },
    ])).toBe(false);
    expect(closeIntentNeedsConfirmation([
      { state: "opening", session: null },
      { state: "running", session: null },
    ])).toBe(true);

    const confirm = vi.fn(async () => false);
    await expect(closeIntentConfirmed([{ state: "failed", session }], confirm))
      .resolves.toBe(false);
    expect(confirm).toHaveBeenCalledOnce();
    confirm.mockClear();
    await expect(closeIntentConfirmed([{ state: "exited", session }], confirm))
      .resolves.toBe(true);
    expect(confirm).not.toHaveBeenCalled();
  });

  it("cancels without runtime or structural changes", async () => {
    const { registry, lifecycle, closeSession } = fixture();
    const runtime = registry.ensure("pane-a");
    runtime.state = "running";
    runtime.session = { sessionId: "session-a" };
    const commit = vi.fn();
    const result = await runDescriptorCloseIntent({
      paneIds: [runtime.paneId],
      registry,
      lifecycle,
      confirm: async () => false,
      commit,
    });
    expect(result).toBe("cancelled");
    expect(runtime).toMatchObject({ state: "running", session: { sessionId: "session-a" } });
    expect(closeSession).not.toHaveBeenCalled();
    expect(commit).not.toHaveBeenCalled();
  });

  it("cancels a failed runtime that still retains a live session", async () => {
    const { registry, lifecycle, closeSession } = fixture();
    const runtime = registry.ensure("pane-failed");
    runtime.state = "failed";
    runtime.session = { sessionId: "session-failed" };
    const confirm = vi.fn(async () => false);
    const commit = vi.fn();
    await expect(runDescriptorCloseIntent({
      paneIds: [runtime.paneId],
      registry,
      lifecycle,
      confirm,
      commit,
    })).resolves.toBe("cancelled");
    expect(confirm).toHaveBeenCalledOnce();
    expect(closeSession).not.toHaveBeenCalled();
    expect(commit).not.toHaveBeenCalled();
  });

  it("confirms once and closes every descendant before commit", async () => {
    const { registry, lifecycle, closeSession } = fixture();
    for (const paneId of ["pane-a", "pane-b"]) {
      const runtime = registry.ensure(paneId);
      runtime.state = "running";
      runtime.session = { sessionId: `session-${paneId}` };
    }
    const confirm = vi.fn(async () => true);
    const commit = vi.fn();
    await runDescriptorCloseIntent({
      paneIds: ["pane-a", "pane-b"],
      registry,
      lifecycle,
      confirm,
      commit,
    });
    expect(confirm).toHaveBeenCalledOnce();
    expect(closeSession).toHaveBeenCalledTimes(2);
    expect(commit).toHaveBeenCalledOnce();
  });

  it("keeps structure on partial failure and retries only the failed pane", async () => {
    const registry = new PaneRuntimeRegistry<TestSession, TestResources>();
    const attempts = new Map<string, number>();
    const closeSession = vi.fn(async (sessionId: string) => {
      const count = (attempts.get(sessionId) ?? 0) + 1;
      attempts.set(sessionId, count);
      if (sessionId === "session-b" && count === 1) throw new Error("close failed");
    });
    const lifecycle = new PaneLifecycleCoordinator(registry, {
      closeSession,
      disposeResources: vi.fn(),
      deleteEarlyExit: vi.fn(),
    });
    for (const paneId of ["a", "b"]) {
      const runtime = registry.ensure(paneId);
      runtime.state = paneId === "a" ? "exited" : "failed";
      runtime.session = { sessionId: `session-${paneId}` };
    }
    const commit = vi.fn();
    const options = {
      paneIds: ["a", "b"],
      registry,
      lifecycle,
      confirm: async () => true,
      commit,
    };
    await expect(runDescriptorCloseIntent(options)).rejects.toThrow("could not be closed");
    expect(commit).not.toHaveBeenCalled();
    await expect(runDescriptorCloseIntent(options)).resolves.toBe("closed");
    expect(attempts.get("session-a")).toBe(1);
    expect(attempts.get("session-b")).toBe(2);
    expect(commit).toHaveBeenCalledOnce();
  });
});
