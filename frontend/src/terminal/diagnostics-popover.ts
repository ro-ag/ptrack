import {
  terminalDiagnosticsTop,
  type TerminalDiagnosticStream,
  type TerminalDiagnosticView,
} from "./diagnostics";
import type { StreamState } from "./client";

// Shared diagnostics popover DOM for dock and detached windows.

export interface TerminalDiagnosticsElements {
  toggle: HTMLButtonElement;
  popover: HTMLElement;
  close: HTMLButtonElement;
  process: HTMLElement;
  stream: HTMLElement;
  renderer: HTMLElement;
  layout: HTMLElement;
  updated: HTMLElement;
  /** The header rows the popover hangs just below. */
  header: HTMLElement;
  /** The positioned surface the popover is placed inside. */
  surface: HTMLElement;
}

/** How a pane's stream reads in the diagnostics: idle without a session. */
export function terminalDiagnosticStream(
  hasSession: boolean,
  state: StreamState | undefined,
): TerminalDiagnosticStream {
  if (!hasSession || !state) return "idle";
  return ({
    closed: "disconnected",
    connecting: "connecting",
    open: "connected",
    error: "failed",
  } satisfies Record<StreamState, TerminalDiagnosticStream>)[state];
}

export class TerminalDiagnosticsPopover {
  readonly #elements: TerminalDiagnosticsElements;
  readonly #view: () => TerminalDiagnosticView;
  readonly #disposers: Array<() => void> = [];
  #open = false;
  // Keep the popover below a wrapping header.
  #observer: ResizeObserver | null = null;

  constructor(elements: TerminalDiagnosticsElements, view: () => TerminalDiagnosticView) {
    this.#elements = elements;
    this.#view = view;
  }

  get open(): boolean {
    return this.#open;
  }

  mount(): void {
    const { toggle, popover, close } = this.#elements;
    this.#listen(toggle, "click", () => this.setOpen(!this.#open));
    this.#listen(popover, "keydown", (event) => {
      const keyEvent = event as KeyboardEvent;
      if (keyEvent.key !== "Escape") return;
      keyEvent.preventDefault();
      this.setOpen(false, true);
    });
    this.#listen(close, "click", () => this.setOpen(false, true));
    // Leave toggle clicks to their own handler.
    this.#listen(document, "pointerdown", (event) => {
      if (!this.#open) return;
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (popover.contains(target) || toggle.contains(target)) return;
      this.setOpen(false);
    }, true);
  }

  setOpen(open: boolean, restoreFocus = false): void {
    const { toggle, popover, header, surface } = this.#elements;
    this.#open = open;
    popover.hidden = !open;
    toggle.setAttribute("aria-expanded", String(open));
    const label = open ? "Hide terminal diagnostics" : "Show terminal diagnostics";
    toggle.setAttribute("aria-label", label);
    toggle.title = label;
    this.#observer?.disconnect();
    this.#observer = null;
    if (open) {
      this.render(this.#view());
      this.#place();
      if (typeof ResizeObserver === "function") {
        this.#observer = new ResizeObserver(() => this.#place());
        this.#observer.observe(header);
        this.#observer.observe(surface);
      }
      popover.focus();
    } else if (restoreFocus) {
      toggle.focus();
    }
  }

  render(view: TerminalDiagnosticView): void {
    const values = new Map(view.rows.map((row) => [row.key, row.value]));
    const { process, stream, renderer, layout, updated } = this.#elements;
    process.textContent = values.get("process") ?? "Stopped";
    stream.textContent = values.get("stream") ?? "Idle";
    renderer.textContent = values.get("renderer") ?? "Not created";
    layout.textContent = values.get("layout") ?? "Default";
    updated.textContent = values.get("updated") ?? "Not recorded";
  }

  dispose(): void {
    this.#observer?.disconnect();
    this.#observer = null;
    for (const dispose of this.#disposers.splice(0)) dispose();
  }

  #listen(
    target: EventTarget,
    type: string,
    listener: (event: Event) => void,
    capture = false,
  ): void {
    target.addEventListener(type, listener, capture);
    this.#disposers.push(() => target.removeEventListener(type, listener, capture));
  }

  /**
   * Places the popover below the measured header.
   */
  #place(): void {
    const { popover, header, surface } = this.#elements;
    const surfaceTop = surface.getBoundingClientRect().top;
    const headerBottom = header.getBoundingClientRect().bottom - surfaceTop;
    const top = terminalDiagnosticsTop({
      headerBottom,
      dockHeight: surface.clientHeight,
    });
    popover.style.setProperty("--terminal-diagnostics-top", `${top}px`);
  }
}
