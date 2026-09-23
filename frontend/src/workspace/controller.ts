export type WorkspaceStatus =
  | "welcome"
  | "loading"
  | "open"
  | "error"
  | "closed";

export interface WorkspaceState {
  status: WorkspaceStatus;
  generation: number;
}

export interface WorkspaceTicket {
  readonly epoch: number;
  readonly generation: number;
}

export class WorkspaceController {
  #epoch = 0;
  #state: WorkspaceState = { status: "welcome", generation: 0 };

  get state(): WorkspaceState {
    return this.#state;
  }

  beginTransition(): WorkspaceTicket {
    this.#epoch += 1;
    this.#state = {
      status: "loading",
      generation: this.#state.generation,
    };
    return this.capture();
  }

  publish(state: WorkspaceState, transition?: WorkspaceTicket): boolean {
    if (transition && transition.epoch !== this.#epoch) return false;
    if (!transition) this.#epoch += 1;
    this.#state = state;
    return true;
  }

  capture(): WorkspaceTicket {
    return {
      epoch: this.#epoch,
      generation: this.#state.generation,
    };
  }

  /** True while no transition or publish has happened since `ticket`. */
  isCurrent(ticket: WorkspaceTicket): boolean {
    return ticket.epoch === this.#epoch;
  }

  accepts(ticket: WorkspaceTicket, responseGeneration: number): boolean {
    return (
      this.#state.status === "open" &&
      ticket.epoch === this.#epoch &&
      ticket.generation === this.#state.generation &&
      responseGeneration === this.#state.generation
    );
  }
}

// RefreshLoop runs the background poll. While `paused()` answers true (the
// window is hidden) ticks are skipped, and `resume()` runs one catch-up
// refresh when the window becomes visible again.
export class RefreshLoop {
  readonly #work: () => void;
  readonly #intervalMilliseconds: number;
  readonly #paused: () => boolean;
  #timer: ReturnType<typeof setInterval> | null = null;
  #disposed = false;

  constructor(
    work: () => void,
    intervalMilliseconds: number,
    paused: () => boolean = () => false,
  ) {
    this.#work = work;
    this.#intervalMilliseconds = intervalMilliseconds;
    this.#paused = paused;
  }

  start(): void {
    if (this.#disposed || this.#timer !== null) return;
    this.#timer = setInterval(() => {
      if (!this.#paused()) this.#work();
    }, this.#intervalMilliseconds);
  }

  resume(): void {
    if (this.#disposed || this.#timer === null || this.#paused()) return;
    this.#work();
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    if (this.#timer !== null) {
      clearInterval(this.#timer);
      this.#timer = null;
    }
  }
}

// RuntimeRefreshCoalescer bounds host runtime-event bursts to one trailing
// workspace refresh per interval. The latest generation is retained so a
// project transition can still fence the callback when it runs.
export class RuntimeRefreshCoalescer {
  readonly #work: (generation: number) => void;
  readonly #delayMilliseconds: number;
  #timer: ReturnType<typeof setTimeout> | null = null;
  #generation = 0;

  constructor(work: (generation: number) => void, delayMilliseconds = 150) {
    this.#work = work;
    this.#delayMilliseconds = Math.max(1, Math.trunc(delayMilliseconds));
  }

  request(generation: number): void {
    if (!Number.isFinite(generation) || generation <= 0) return;
    this.#generation = Math.trunc(generation);
    if (this.#timer !== null) return;
    this.#timer = setTimeout(() => {
      this.#timer = null;
      const current = this.#generation;
      this.#generation = 0;
      this.#work(current);
    }, this.#delayMilliseconds);
  }

  cancel(): void {
    if (this.#timer !== null) clearTimeout(this.#timer);
    this.#timer = null;
    this.#generation = 0;
  }
}

export class RefreshGate {
  #running = false;
  #queued = false;
  #handoffPending = false;
  #idleWaiters: Array<() => void> = [];

  tryBegin(queueIfBusy = false): boolean {
    if (this.#running) {
      this.#queued ||= queueIfBusy;
      return false;
    }
    this.#handoffPending = false;
    this.#running = true;
    return true;
  }

  finish(): boolean {
    this.#running = false;
    const queued = this.#queued;
    this.#queued = false;
    this.#handoffPending = queued;
    if (!queued) {
      const waiters = this.#idleWaiters.splice(0);
      for (const resolve of waiters) resolve();
    }
    return queued;
  }

  whenIdle(): Promise<void> {
    if (!this.#running && !this.#handoffPending) return Promise.resolve();
    return new Promise((resolve) => this.#idleWaiters.push(resolve));
  }

  cancelQueued(): void {
    this.#queued = false;
    this.#handoffPending = false;
    if (!this.#running) {
      const waiters = this.#idleWaiters.splice(0);
      for (const resolve of waiters) resolve();
    }
  }

  reset(): void {
    this.#running = false;
    this.#queued = false;
    this.#handoffPending = false;
    const waiters = this.#idleWaiters.splice(0);
    for (const resolve of waiters) resolve();
  }
}

// RequestSequence orders overlapping reads of one resource: only the most
// recently started request may apply its answer.
export class RequestSequence {
  #latest = 0;

  next(): number {
    this.#latest += 1;
    return this.#latest;
  }

  isCurrent(request: number): boolean {
    return request === this.#latest;
  }

  /** Makes every request started so far stale. */
  invalidate(): void {
    this.#latest += 1;
  }
}

// InFlightOperations refuses a second start of the same operation while the
// first is still pending, so a double Enter or double click submits once.
export class InFlightOperations {
  readonly #active = new Set<string>();

  begin(key: string): boolean {
    if (this.#active.has(key)) return false;
    this.#active.add(key);
    return true;
  }

  end(key: string): void {
    this.#active.delete(key);
  }

  has(key: string): boolean {
    return this.#active.has(key);
  }

  clear(): void {
    this.#active.clear();
  }
}

// GenerationSlot holds one deferred request for the workspace generation that
// made it. Taking it always empties the slot, and yields the value only while
// that same generation is still the open one.
export class GenerationSlot<T> {
  #entry: { value: T; generation: number } | null = null;

  set(value: T, generation: number): void {
    this.#entry = { value, generation };
  }

  get pending(): boolean {
    return this.#entry !== null;
  }

  take(state: WorkspaceState): T | null {
    const entry = this.#entry;
    this.#entry = null;
    if (!entry || state.status !== "open" || state.generation !== entry.generation) return null;
    return entry.value;
  }

  clear(): void {
    this.#entry = null;
  }
}
