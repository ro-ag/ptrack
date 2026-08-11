export interface TerminalSize {
  rows: number;
  columns: number;
}

export interface TerminalResizeDispatcherOptions {
  now(): number;
  setTimer(callback: () => void, delay: number): number;
  clearTimer(timer: number): void;
  accepted(): boolean;
  dispatch(size: TerminalSize): void;
  intervalMilliseconds?: number;
}

function sameSize(left: TerminalSize | null, right: TerminalSize): boolean {
  return left?.rows === right.rows && left.columns === right.columns;
}

export class TerminalResizeDispatcher {
  readonly #options: TerminalResizeDispatcherOptions;
  readonly #interval: number;
  #pending: TerminalSize | null = null;
  #lastDispatched: TerminalSize | null = null;
  #lastDispatchAt = Number.NEGATIVE_INFINITY;
  #timer: number | null = null;
  #disposed = false;

  constructor(options: TerminalResizeDispatcherOptions) {
    this.#options = options;
    this.#interval = Math.max(0, options.intervalMilliseconds ?? 100);
  }

  queue(size: TerminalSize): void {
    if (this.#disposed || !Number.isInteger(size.rows) || size.rows <= 0 ||
      !Number.isInteger(size.columns) || size.columns <= 0) return;
    const next = { rows: size.rows, columns: size.columns };
    if (sameSize(this.#pending, next) ||
      (this.#timer === null && sameSize(this.#lastDispatched, next))) return;
    this.#pending = next;
    this.#schedule();
  }

  invalidate(size: TerminalSize): void {
    if (sameSize(this.#lastDispatched, size)) this.#lastDispatched = null;
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#pending = null;
    if (this.#timer !== null) this.#options.clearTimer(this.#timer);
    this.#timer = null;
  }

  #schedule(): void {
    if (this.#disposed || this.#timer !== null || this.#pending === null) return;
    const elapsed = this.#options.now() - this.#lastDispatchAt;
    const delay = Math.max(0, this.#interval - elapsed);
    this.#timer = this.#options.setTimer(() => this.#flush(), delay);
  }

  #flush(): void {
    this.#timer = null;
    const size = this.#pending;
    this.#pending = null;
    if (this.#disposed || !size || !this.#options.accepted()) return;
    if (!sameSize(this.#lastDispatched, size)) {
      this.#lastDispatched = size;
      this.#lastDispatchAt = this.#options.now();
      this.#options.dispatch(size);
    }
    this.#schedule();
  }
}
