import {
  PaneRuntimeRegistry,
  type PaneRuntime,
  type PaneSession,
} from "./runtime";
import { resetPaneActivity } from "./activity";

export interface PaneLifecycleOperations<Resources> {
  closeSession(sessionId: string, force: boolean): Promise<void>;
  disposeResources(resources: Resources): void;
  deleteEarlyExit(sessionId: string): void;
}

export interface PendingCreatedSessionOptions<Resources> {
  sessionId: string;
  resources: Resources;
  accepts(): boolean;
  resourcesDisposed(resources: Resources): boolean;
  forceClose(sessionId: string): Promise<void>;
  disposeResources(resources: Resources): void;
}

export async function settlePendingCreatedSession<Resources>(
  options: PendingCreatedSessionOptions<Resources>,
): Promise<boolean> {
  if (options.accepts() && !options.resourcesDisposed(options.resources)) return true;
  await options.forceClose(options.sessionId);
  if (!options.resourcesDisposed(options.resources)) {
    options.disposeResources(options.resources);
  }
  return false;
}

export class PendingSessionCloseError extends Error {
  readonly sessionId: string;
  readonly cause: unknown;

  constructor(sessionId: string, cause: unknown) {
    super(`Late terminal session ${sessionId} could not be closed`);
    this.name = "PendingSessionCloseError";
    this.sessionId = sessionId;
    this.cause = cause;
  }
}

interface PendingSessionClose<Resources> {
  sessionId: string;
  resources: Resources;
  closePromise: Promise<boolean> | null;
}

export class PendingSessionCloseCoordinator<Resources> {
  readonly #forceClose: (sessionId: string) => Promise<void>;
  readonly #resourcesDisposed: (resources: Resources) => boolean;
  readonly #disposeResources: (resources: Resources) => void;
  readonly #maximumAttempts: number;
  readonly #maximumRememberedCloses: number;
  readonly #pending = new Map<string, PendingSessionClose<Resources>>();
  readonly #closed = new Map<string, true>();

  constructor(options: {
    forceClose(sessionId: string): Promise<void>;
    resourcesDisposed(resources: Resources): boolean;
    disposeResources(resources: Resources): void;
    maximumAttempts?: number;
    maximumRememberedCloses?: number;
  }) {
    this.#forceClose = options.forceClose;
    this.#resourcesDisposed = options.resourcesDisposed;
    this.#disposeResources = options.disposeResources;
    this.#maximumAttempts = Math.max(1, Math.trunc(options.maximumAttempts ?? 2));
    this.#maximumRememberedCloses = Math.max(
      1,
      Math.trunc(options.maximumRememberedCloses ?? 128),
    );
  }

  get pendingCount(): number {
    return this.#pending.size;
  }

  get rememberedCloseCount(): number {
    return this.#closed.size;
  }

  settle(options: {
    sessionId: string;
    resources: Resources;
    accepts(): boolean;
  }): Promise<boolean> {
    if (options.accepts() && !this.#resourcesDisposed(options.resources)) {
      return Promise.resolve(true);
    }
    if (this.#closed.has(options.sessionId)) {
      if (!this.#resourcesDisposed(options.resources)) {
        this.#disposeResources(options.resources);
      }
      return Promise.resolve(false);
    }
    let pending = this.#pending.get(options.sessionId);
    if (!pending) {
      pending = {
        sessionId: options.sessionId,
        resources: options.resources,
        closePromise: null,
      };
      this.#pending.set(options.sessionId, pending);
    }
    if (pending.closePromise) return pending.closePromise;
    const closePromise = this.#close(pending);
    pending.closePromise = closePromise;
    void closePromise.catch(() => {}).finally(() => {
      if (pending?.closePromise === closePromise) pending.closePromise = null;
    });
    return closePromise;
  }

  async #close(pending: PendingSessionClose<Resources>): Promise<boolean> {
    let lastError: unknown;
    for (let attempt = 0; attempt < this.#maximumAttempts; attempt += 1) {
      try {
        await settlePendingCreatedSession({
          sessionId: pending.sessionId,
          resources: pending.resources,
          accepts: () => false,
          resourcesDisposed: this.#resourcesDisposed,
          forceClose: this.#forceClose,
          disposeResources: this.#disposeResources,
        });
        this.#pending.delete(pending.sessionId);
        this.#rememberClosed(pending.sessionId);
        return false;
      } catch (error) {
        lastError = error;
      }
    }
    if (!this.#resourcesDisposed(pending.resources)) {
      this.#disposeResources(pending.resources);
    }
    throw new PendingSessionCloseError(pending.sessionId, lastError);
  }

  #rememberClosed(sessionId: string): void {
    this.#closed.delete(sessionId);
    this.#closed.set(sessionId, true);
    while (this.#closed.size > this.#maximumRememberedCloses) {
      const oldest = this.#closed.keys().next().value;
      if (oldest === undefined) break;
      this.#closed.delete(oldest);
    }
  }

  async retryPending(): Promise<void> {
    const results = await Promise.allSettled(
      [...this.#pending.values()].map((pending) =>
        this.settle({
          sessionId: pending.sessionId,
          resources: pending.resources,
          accepts: () => false,
        })
      ),
    );
    const errors = results
      .filter((result): result is PromiseRejectedResult => result.status === "rejected")
      .map((result) => result.reason);
    if (errors.length > 0) {
      throw new AggregateError(errors, "Late terminal sessions remain pending cleanup");
    }
  }

  releaseToProjectShutdown(): void {
    for (const pending of this.#pending.values()) {
      if (!this.#resourcesDisposed(pending.resources)) {
        this.#disposeResources(pending.resources);
      }
    }
    this.#pending.clear();
    this.#closed.clear();
  }
}

export function closeIntentNeedsConfirmation(
  runtimes: ReadonlyArray<Pick<PaneRuntime, "state" | "session">>,
): boolean {
  return runtimes.some(
    (runtime) => runtime.state === "opening" || runtime.state === "running" ||
      (runtime.state === "failed" && runtime.session !== null),
  );
}

export async function closeIntentConfirmed(
  runtimes: ReadonlyArray<Pick<PaneRuntime, "state" | "session">>,
  confirm: () => Promise<boolean>,
): Promise<boolean> {
  return !closeIntentNeedsConfirmation(runtimes) || await confirm();
}

export class PaneLifecycleCoordinator<
  Session extends PaneSession,
  Resources,
> {
  readonly #registry: PaneRuntimeRegistry<Session, Resources>;
  readonly #operations: PaneLifecycleOperations<Resources>;

  constructor(
    registry: PaneRuntimeRegistry<Session, Resources>,
    operations: PaneLifecycleOperations<Resources>,
  ) {
    this.#registry = registry;
    this.#operations = operations;
  }

  close(paneId: string, force = false): Promise<void> {
    const runtime = this.#registry.ensure(paneId);
    if (runtime.closePromise) return runtime.closePromise;
    const previousTicket = this.#registry.capture(paneId);
    const sessionId = runtime.session?.sessionId ?? null;
    const resources = runtime.resources;
    let backendClosed = sessionId === null || runtime.closingSessionId === sessionId;
    const closeTicket = this.#registry.invalidate(paneId);
    runtime.closing = true;
    runtime.busy = true;
    runtime.closingSessionId = sessionId;

    const closePromise = Promise.resolve().then(async () => {
      try {
        if (sessionId && !backendClosed) {
          await this.#operations.closeSession(sessionId, force);
          backendClosed = true;
        }
        if (sessionId) this.#operations.deleteEarlyExit(sessionId);
        if (resources && runtime.resources === resources) {
          this.#operations.disposeResources(resources);
          runtime.resources = null;
        }
        if (sessionId === null || runtime.session?.sessionId === sessionId) {
          runtime.session = null;
        }
        runtime.state = "closed";
        runtime.detail = "";
        runtime.title = "";
        runtime.activity = resetPaneActivity();
        runtime.closing = false;
        runtime.busy = false;
        runtime.closingSessionId = null;
      } catch (error) {
        if (!backendClosed && previousTicket && closeTicket) {
          this.#registry.rollbackInvalidation(runtime, closeTicket, previousTicket);
        }
        if (runtime.closingSessionId === sessionId) {
          runtime.closing = false;
          runtime.busy = false;
          if (!backendClosed) runtime.closingSessionId = null;
        }
        throw error;
      }
    });
    runtime.closePromise = closePromise;
    void closePromise.catch(() => {
      if (runtime.closePromise === closePromise) runtime.closePromise = null;
    });
    return closePromise;
  }

  async closeMany(paneIds: readonly string[], force = false): Promise<void> {
    const uniquePaneIds = [...new Set(paneIds)];
    const results = await Promise.allSettled(
      uniquePaneIds.map((paneId) => this.close(paneId, force)),
    );
    const errors = results
      .filter((result): result is PromiseRejectedResult => result.status === "rejected")
      .map((result) => result.reason);
    if (errors.length > 0) {
      throw new AggregateError(errors, "One or more terminal panes could not be closed");
    }
  }

  prepareOpen(paneId: string): void {
    const runtime = this.#registry.ensure(paneId);
    runtime.closePromise = null;
    runtime.closingSessionId = null;
    runtime.closing = false;
  }

  releaseLocal(paneId: string): void {
    const runtime = this.#registry.get(paneId);
    if (!runtime) return;
    this.#registry.invalidate(paneId);
    const resources = runtime.resources;
    runtime.resources = null;
    if (resources) this.#operations.disposeResources(resources);
    runtime.session = null;
    runtime.state = "closed";
    runtime.detail = "";
    runtime.title = "";
    runtime.activity = resetPaneActivity();
    runtime.closing = false;
    runtime.busy = false;
    runtime.closingSessionId = null;
  }

  releaseManyLocal(paneIds: readonly string[]): void {
    for (const paneId of new Set(paneIds)) this.releaseLocal(paneId);
  }
}

export interface DescriptorCloseIntentOptions<
  Session extends PaneSession,
  Resources,
> {
  paneIds: readonly string[];
  registry: PaneRuntimeRegistry<Session, Resources>;
  lifecycle: PaneLifecycleCoordinator<Session, Resources>;
  confirm(): Promise<boolean>;
  commit(): void;
}

export async function runDescriptorCloseIntent<
  Session extends PaneSession,
  Resources,
>(
  options: DescriptorCloseIntentOptions<Session, Resources>,
): Promise<"cancelled" | "closed"> {
  const runtimes = options.paneIds.map((paneId) => options.registry.ensure(paneId));
  if (!(await closeIntentConfirmed(runtimes, options.confirm))) {
    return "cancelled";
  }
  await options.lifecycle.closeMany(options.paneIds);
  options.commit();
  return "closed";
}
