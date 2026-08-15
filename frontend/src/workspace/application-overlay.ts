export interface ApplicationOverlay {
  readonly hidden: boolean;
  inert: boolean;
  getAttribute(name: "aria-hidden" | "data-application-overlay-layer"): string | null;
  setAttribute(
    name: "aria-hidden" | "data-application-overlay-layer",
    value: string,
  ): void;
  removeAttribute(name: "aria-hidden" | "data-application-overlay-layer"): void;
}

export interface ApplicationOverlayChange {
  readonly overlay: ApplicationOverlay;
  readonly open: boolean;
}

export interface ApplicationOverlayDock {
  setApplicationOverlayOpen(open: boolean, focusTerminal: false): void;
}

export interface ApplicationOverlayBackground {
  inert: boolean;
  getAttribute(name: "aria-hidden"): string | null;
  setAttribute(name: "aria-hidden", value: string): void;
  removeAttribute(name: "aria-hidden"): void;
}

interface AccessibilityState {
  readonly inert: boolean;
  readonly ariaHidden: string | null;
}

interface OverlayState extends AccessibilityState {
  readonly layer: string | null;
}

export type ApplicationOverlayEscapeAction =
  | "dialog"
  | "memory"
  | "settings"
  | "updates"
  | "drawer"
  | "agent-launch"
  | "terminal-association"
  | "terminal-writeback"
  | "task-transition"
  | "workspace-confirm"
  | "palette";

const escapeActionByOverlayID: Readonly<
  Record<string, ApplicationOverlayEscapeAction>
> = {
  modal: "dialog",
  "memory-modal": "memory",
  "settings-modal": "settings",
  "updates-modal": "updates",
  "task-drawer": "drawer",
  "agent-launch-modal": "agent-launch",
  "terminal-association-modal": "terminal-association",
  "terminal-writeback-modal": "terminal-writeback",
  "task-transition-modal": "task-transition",
  "workspace-confirm-modal": "workspace-confirm",
  palette: "palette",
};

export function applicationOverlayKeyboardPolicy(
  overlayID: string,
  terminalOverlay: boolean,
): {
  readonly trapTab: boolean;
  readonly escapeAction: ApplicationOverlayEscapeAction | null;
} {
  if (terminalOverlay) return { trapTab: false, escapeAction: null };
  return {
    trapTab: true,
    escapeAction: escapeActionByOverlayID[overlayID] || null,
  };
}

export function applicationOverlayIsOpen(
  overlays: Iterable<ApplicationOverlay>,
): boolean {
  return [...overlays].some((overlay) => !overlay.hidden);
}

export class ApplicationOverlayCoordinator {
  readonly #overlays: () => Iterable<ApplicationOverlay>;
  readonly #background: ApplicationOverlayBackground | null;
  readonly #openingOrder: ApplicationOverlay[] = [];
  readonly #knownVisibility = new Map<ApplicationOverlay, boolean>();
  readonly #overlayStates = new Map<ApplicationOverlay, OverlayState>();
  #dock: ApplicationOverlayDock | null = null;
  #lastOpen: boolean | null = null;
  #lastDockOpen: boolean | null = null;
  #backgroundState: AccessibilityState | null = null;

  constructor(
    overlays: () => Iterable<ApplicationOverlay>,
    background: ApplicationOverlayBackground | null = null,
  ) {
    this.#overlays = overlays;
    this.#background = background;
  }

  get activeOverlay(): ApplicationOverlay | null {
    for (let index = this.#openingOrder.length - 1; index >= 0; index -= 1) {
      const overlay = this.#openingOrder[index];
      if (!overlay.hidden) return overlay;
    }
    return null;
  }

  isOpen(): boolean {
    return this.activeOverlay !== null || applicationOverlayIsOpen(this.#overlays());
  }

  setDock(dock: ApplicationOverlayDock | null): void {
    if (this.#dock === dock) {
      this.reconcile();
      return;
    }
    this.#dock = dock;
    this.#lastDockOpen = null;
    this.reconcile();
  }

  reconcile(changes: Iterable<ApplicationOverlayChange> = []): boolean {
    const overlays = [...this.#overlays()];
    const present = new Set(overlays);
    for (const { overlay, open } of changes) {
      if (!present.has(overlay) || this.#knownVisibility.get(overlay) === open) {
        continue;
      }
      this.#knownVisibility.set(overlay, open);
      this.#removeFromOpeningOrder(overlay);
      if (open) this.#openingOrder.push(overlay);
    }
    for (const overlay of overlays) {
      const open = !overlay.hidden;
      if (this.#knownVisibility.get(overlay) !== open) {
        this.#knownVisibility.set(overlay, open);
        this.#removeFromOpeningOrder(overlay);
        if (open) this.#openingOrder.push(overlay);
      } else if (open && !this.#openingOrder.includes(overlay)) {
        this.#openingOrder.push(overlay);
      }
    }
    for (const overlay of [...this.#knownVisibility.keys()]) {
      if (present.has(overlay)) continue;
      this.#knownVisibility.delete(overlay);
      this.#removeFromOpeningOrder(overlay);
    }
    for (let index = this.#openingOrder.length - 1; index >= 0; index -= 1) {
      const overlay = this.#openingOrder[index];
      if (!present.has(overlay) || overlay.hidden) this.#openingOrder.splice(index, 1);
    }

    const active = this.activeOverlay;
    this.#syncOverlayLayers(overlays, active);
    const open = active !== null;
    if (this.#lastOpen !== open) this.#syncBackground(open);
    if (this.#dock && this.#lastDockOpen !== open) {
      this.#dock.setApplicationOverlayOpen(open, false);
      this.#lastDockOpen = open;
    }
    this.#lastOpen = open;
    return open;
  }

  #removeFromOpeningOrder(overlay: ApplicationOverlay): void {
    const index = this.#openingOrder.indexOf(overlay);
    if (index >= 0) this.#openingOrder.splice(index, 1);
  }

  #captureOverlayState(overlay: ApplicationOverlay): OverlayState {
    let state = this.#overlayStates.get(overlay);
    if (!state) {
      state = {
        inert: overlay.inert,
        ariaHidden: overlay.getAttribute("aria-hidden"),
        layer: overlay.getAttribute("data-application-overlay-layer"),
      };
      this.#overlayStates.set(overlay, state);
    }
    return state;
  }

  #syncOverlayLayers(
    overlays: readonly ApplicationOverlay[],
    active: ApplicationOverlay | null,
  ): void {
    const visible = new Set(overlays.filter((overlay) => !overlay.hidden));
    for (const overlay of visible) {
      const state = this.#captureOverlayState(overlay);
      if (overlay === active) {
        this.#restoreAccessibilityState(overlay, state);
        overlay.setAttribute("data-application-overlay-layer", "active");
      } else {
        overlay.inert = true;
        overlay.setAttribute("aria-hidden", "true");
        overlay.setAttribute("data-application-overlay-layer", "underlay");
      }
    }
    for (const [overlay, state] of [...this.#overlayStates.entries()]) {
      if (visible.has(overlay)) continue;
      this.#restoreAccessibilityState(overlay, state);
      if (state.layer === null) {
        overlay.removeAttribute("data-application-overlay-layer");
      } else {
        overlay.setAttribute("data-application-overlay-layer", state.layer);
      }
      this.#overlayStates.delete(overlay);
    }
  }

  #restoreAccessibilityState(
    target: ApplicationOverlay | ApplicationOverlayBackground,
    state: AccessibilityState,
  ): void {
    target.inert = state.inert;
    if (state.ariaHidden === null) target.removeAttribute("aria-hidden");
    else target.setAttribute("aria-hidden", state.ariaHidden);
  }

  #syncBackground(open: boolean): void {
    if (!this.#background) return;
    if (open) {
      if (!this.#backgroundState) {
        this.#backgroundState = {
          inert: this.#background.inert,
          ariaHidden: this.#background.getAttribute("aria-hidden"),
        };
      }
      this.#background.inert = true;
      this.#background.setAttribute("aria-hidden", "true");
      return;
    }
    if (!this.#backgroundState) return;
    this.#restoreAccessibilityState(this.#background, this.#backgroundState);
    this.#backgroundState = null;
  }
}
