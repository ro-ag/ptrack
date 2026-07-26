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

  accepts(ticket: WorkspaceTicket, responseGeneration: number): boolean {
    return (
      this.#state.status === "open" &&
      ticket.epoch === this.#epoch &&
      ticket.generation === this.#state.generation &&
      responseGeneration === this.#state.generation
    );
  }
}

export class RefreshLoop {
  readonly #work: () => void;
  readonly #intervalMilliseconds: number;
  #timer: ReturnType<typeof setInterval> | null = null;
  #disposed = false;

  constructor(work: () => void, intervalMilliseconds: number) {
    this.#work = work;
    this.#intervalMilliseconds = intervalMilliseconds;
  }

  start(): void {
    if (this.#disposed || this.#timer !== null) return;
    this.#timer = setInterval(this.#work, this.#intervalMilliseconds);
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

export class RefreshGate {
  #running = false;
  #queued = false;

  tryBegin(queueIfBusy = false): boolean {
    if (this.#running) {
      this.#queued ||= queueIfBusy;
      return false;
    }
    this.#running = true;
    return true;
  }

  finish(): boolean {
    this.#running = false;
    const queued = this.#queued;
    this.#queued = false;
    return queued;
  }

  reset(): void {
    this.#running = false;
    this.#queued = false;
  }
}
