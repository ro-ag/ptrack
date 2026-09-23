import {
  clampSidebarWidth,
  defaultLayoutState,
  defaultSidebarWidth,
  layoutProjectState,
  layoutStatePatch,
  normalizeLayoutState,
  sidebarHiddenStorageKey,
  sidebarMaximumWidth,
  sidebarWidthFromKey,
  sidebarWidthStorageKey,
  storedSidebarWidth,
  type LayoutState,
} from "./layout";
import type { AppContext } from "./app-context";
import { workspaceView } from "./workspace-shell";
import { element } from "./dom";
import { WorkspacePersistenceScheduler } from "./persistence";

function readLayoutPreference(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeLayoutPreference(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // The layout remains usable when WebView storage is unavailable.
  }
}

export function createLayoutController(ctx: AppContext) {
  const { api } = ctx;
  const elements = {
    app: element("#app", HTMLDivElement),
    boardPanelToggle: element("#board-panel-toggle", HTMLButtonElement),
    panelControls: element(".panel-controls", HTMLElement),
    sidebar: element("#sidebar", HTMLElement),
    sidebarResize: element("#sidebar-resize", HTMLDivElement),
    sidebarToggle: element("#sidebar-toggle", HTMLButtonElement),
    terminalPanelToggle: element("#terminal-panel-toggle", HTMLButtonElement),
  };

  let sidebarWidth = defaultSidebarWidth;
  let sidebarHidden = false;
  let sidebarDragCleanup: (() => void) | null = null;
  let layoutState: LayoutState = defaultLayoutState();
  let panelLayoutRestored = false;

  // The stored layout record is the authority. The localStorage keys stay
  // mirrors of it, written so the sidebar does not reflow before GetLayoutState
  // answers — the same reason the theme keeps its pre-paint key.
  const layoutStateScheduler = new WorkspacePersistenceScheduler(
    {
      setTimeout: (callback: () => void, delay: number) => window.setTimeout(callback, delay),
      clearTimeout: (handle: number) => window.clearTimeout(handle),
    },
    () => writeLayoutState(),
  );

  // Projects whose layout changed since the last save. A save carries all of
  // them, so a project switch inside the debounce window cannot drop the
  // previous project's change.
  const dirtyLayoutProjects = new Set<string>();

  function writeLayoutState(): void {
    const roots = new Set(dirtyLayoutProjects);
    const openRoot = ctx.state.workspaceState.project?.root || "";
    if (openRoot) roots.add(openRoot);
    dirtyLayoutProjects.clear();
    try {
      void api()
        .SetLayoutState(layoutStatePatch(layoutState, roots))
        .catch(() => {});
    } catch {
      // Layout persistence is optional; the live layout is unchanged.
    }
  }

  function recordSidebarLayout(): void {
    layoutState.sidebar = { width: sidebarWidth, hidden: sidebarHidden };
    layoutStateScheduler.markDirty();
  }

  // Only what the user did counts, and a click on a toggle is the only signal
  // that means it: the dock also forces the panels open when the last session
  // exits and when it disposes, and that teardown must not erase the record.
  // The listener sits on the group so it runs after the dock's own handler and
  // reads what the dock settled on.
  function recordPanelLayout(event: Event): void {
    if (
      !panelLayoutRestored ||
      !(event.target instanceof Element) ||
      !event.target.closest("#board-panel-toggle, #terminal-panel-toggle")
    ) return;
    const next = {
      boardHidden: elements.boardPanelToggle.getAttribute("aria-expanded") === "false",
      terminalHidden: elements.terminalPanelToggle.getAttribute("aria-expanded") === "false",
    };
    if (
      next.boardHidden === layoutState.panels.boardHidden &&
      next.terminalHidden === layoutState.panels.terminalHidden
    ) return;
    layoutState.panels = next;
    layoutStateScheduler.markDirty();
  }

  // The dock owns the panel toggles, so the stored record is applied through
  // them. The dock refuses a board change while no session is live, and a
  // refusal leaves the record alone rather than overwriting the preference this
  // restore exists to honor.
  function restorePanelLayout(): void {
    panelLayoutRestored = false;
    const toggles: ReadonlyArray<readonly [HTMLButtonElement, boolean]> = [
      [elements.boardPanelToggle, layoutState.panels.boardHidden],
      [elements.terminalPanelToggle, layoutState.panels.terminalHidden],
    ];
    for (const [toggle, hidden] of toggles) {
      if (toggle.disabled) continue;
      if ((toggle.getAttribute("aria-expanded") === "false") !== hidden) toggle.click();
    }
    // The clicks above dispatch synchronously, so the guard has done its whole
    // job by the time the loop ends: every click after this one is the user's,
    // and a dock that refused the restore must not silence them for the rest of
    // its life. The refusal itself is still not adopted — it happened under the
    // guard — so the stored record stands until a real gesture moves it.
    panelLayoutRestored = true;
  }

  function restoreProjectLayout(projectRoot: string): void {
    const stored = layoutProjectState(layoutState, projectRoot);
    ctx.state.view = workspaceView(stored.view);
    ctx.state.expandedLanes.clear();
    ctx.state.foldedLanes.clear();
    for (const lane of stored.foldedLanes) ctx.state.foldedLanes.add(lane);
  }

  // Nothing is recorded before the board loads: the restored plan is still only
  // a hint at that point, and writing a zero over it would lose it.
  function recordProjectLayout(): void {
    const projectRoot = ctx.state.workspaceState.project?.root;
    if (!projectRoot || !ctx.state.board) return;
    const next = {
      view: ctx.state.view,
      planId: Number(ctx.state.board.planId || 0),
      foldedLanes: [...ctx.state.foldedLanes].sort(),
    };
    const current = layoutState.projects[projectRoot];
    if (current && JSON.stringify(current) === JSON.stringify(next)) return;
    layoutState.projects[projectRoot] = next;
    dirtyLayoutProjects.add(projectRoot);
    layoutStateScheduler.markDirty();
  }

  // The stored plan is a hint. The backend still resolves it, and a plan that no
  // longer resolves silently falls back to the active plan.
  function restoredPlanId(projectRoot: string | undefined): number {
    return layoutProjectState(layoutState, projectRoot || "").planId;
  }

  function applyLayoutState(next: LayoutState): void {
    layoutState = next;
    setSidebarWidth(layoutState.sidebar.width, false);
    setSidebarHidden(layoutState.sidebar.hidden, false);
    writeLayoutPreference(sidebarWidthStorageKey, String(sidebarWidth));
    writeLayoutPreference(sidebarHiddenStorageKey, String(sidebarHidden));
    restorePanelLayout();
  }

  async function loadLayoutState(): Promise<void> {
    let stored: LayoutState;
    try {
      stored = normalizeLayoutState(await api().GetLayoutState());
    } catch {
      return;
    }
    if (stored.storage !== "ok") {
      // No readable record yet, so the mirror is what this window is already
      // using; adopting a default width here would move the sidebar for nothing.
      layoutState = { ...stored, sidebar: { width: sidebarWidth, hidden: sidebarHidden } };
      return;
    }
    applyLayoutState(stored);
  }

  function setSidebarWidth(width: number, persist = true): void {
    sidebarWidth = clampSidebarWidth(width, window.innerWidth);
    const maximum = sidebarMaximumWidth(window.innerWidth);
    elements.app.style.setProperty("--sidebar-width", `${sidebarWidth}px`);
    elements.sidebarResize.setAttribute("aria-valuemax", String(maximum));
    elements.sidebarResize.setAttribute("aria-valuenow", String(sidebarWidth));
    if (persist) {
      writeLayoutPreference(sidebarWidthStorageKey, String(sidebarWidth));
      recordSidebarLayout();
    }
  }

  function setSidebarHidden(hidden: boolean, persist = true): void {
    sidebarHidden = Boolean(hidden);
    elements.sidebar.hidden = sidebarHidden;
    elements.sidebarResize.hidden = sidebarHidden;
    elements.app.dataset.sidebarHidden = String(sidebarHidden);
    elements.sidebarToggle.setAttribute("aria-expanded", String(!sidebarHidden));
    const label = sidebarHidden
      ? "Show project sidebar"
      : "Hide project sidebar";
    elements.sidebarToggle.setAttribute("aria-label", label);
    elements.sidebarToggle.title = label;
    if (persist) {
      writeLayoutPreference(sidebarHiddenStorageKey, String(sidebarHidden));
      recordSidebarLayout();
    }
  }

  function beginSidebarResize(event: PointerEvent): void {
    if (ctx.state.firstPlanState.phase !== "idle" || sidebarHidden || event.button !== 0) return;
    event.preventDefault();
    sidebarDragCleanup?.();
    const startX = event.clientX;
    const startWidth = sidebarWidth;
    const pointerID = event.pointerId;
    const move = (moveEvent: PointerEvent) => {
      if (moveEvent.pointerId !== pointerID) return;
      setSidebarWidth(startWidth + moveEvent.clientX - startX, false);
    };
    const cleanup = () => {
      elements.sidebarResize.removeEventListener("pointermove", move);
      elements.sidebarResize.removeEventListener("pointerup", finish);
      elements.sidebarResize.removeEventListener("pointercancel", finish);
      elements.sidebarResize.removeEventListener("lostpointercapture", finish);
      if (elements.sidebarResize.hasPointerCapture(pointerID)) {
        elements.sidebarResize.releasePointerCapture(pointerID);
      }
      if (sidebarDragCleanup === cleanup) sidebarDragCleanup = null;
    };
    const finish = (finishEvent: Event) => {
      const pointerId = "pointerId" in finishEvent ? finishEvent.pointerId : undefined;
      if (
        finishEvent.type !== "lostpointercapture" &&
        pointerId !== pointerID
      ) return;
      cleanup();
      writeLayoutPreference(sidebarWidthStorageKey, String(sidebarWidth));
      recordSidebarLayout();
    };
    sidebarDragCleanup = cleanup;
    elements.sidebarResize.setPointerCapture(pointerID);
    elements.sidebarResize.addEventListener("pointermove", move);
    elements.sidebarResize.addEventListener("pointerup", finish);
    elements.sidebarResize.addEventListener("pointercancel", finish);
    elements.sidebarResize.addEventListener("lostpointercapture", finish);
  }

  function resizeSidebarFromKeyboard(event: KeyboardEvent): void {
    if (ctx.state.firstPlanState.phase !== "idle" || sidebarHidden) return;
    const nextWidth = sidebarWidthFromKey(sidebarWidth, event.key, window.innerWidth);
    if (nextWidth === null) return;
    event.preventDefault();
    setSidebarWidth(nextWidth);
  }

  function flushLayoutState(): void {
    layoutStateScheduler.flush();
  }

  function disposeLayout(): void {
    sidebarDragCleanup?.();
    layoutStateScheduler.flush();
  }

  // The dock that honored the stored panel layout is gone; the next one restores
  // it again before its toggles count as the user's.
  function forgetPanelLayoutRestore(): void {
    panelLayoutRestored = false;
  }

  function initializeSidebarLayout(): void {
    sidebarWidth = storedSidebarWidth(
      readLayoutPreference(sidebarWidthStorageKey),
      window.innerWidth,
    );
    sidebarHidden = readLayoutPreference(sidebarHiddenStorageKey) === "true";
    setSidebarWidth(sidebarWidth, false);
    setSidebarHidden(sidebarHidden, false);
    elements.panelControls.addEventListener("click", recordPanelLayout);
  }

  function sidebarHeadingUnavailableForFocus(): boolean {
    return sidebarHidden || window.matchMedia("(max-width: 600px)").matches;
  }

  function bind(): void {
    initializeSidebarLayout();
    elements.sidebarToggle.addEventListener("click", () => {
      if (ctx.state.firstPlanState.phase !== "idle") return;
      setSidebarHidden(!sidebarHidden);
    });
    elements.sidebarResize.addEventListener("pointerdown", beginSidebarResize);
    elements.sidebarResize.addEventListener("keydown", resizeSidebarFromKeyboard);
    window.addEventListener("resize", () => setSidebarWidth(sidebarWidth, false));
  }

  return {
    bind,
    restorePanelLayout,
    restoreProjectLayout,
    recordProjectLayout,
    restoredPlanId,
    applyLayoutState,
    loadLayoutState,
    flushLayoutState,
    disposeLayout,
    forgetPanelLayoutRestore,
    sidebarHeadingUnavailableForFocus,
  };
}

export type LayoutController = ReturnType<typeof createLayoutController>;
