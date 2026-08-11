import {
  findTerminalPane,
  maximumPaneDepth,
  maximumPanesPerTab,
  maximumSplitRatio,
  minimumSplitRatio,
  normalizeSplitRatio,
  paneCount,
  paneIds,
  type PaneNode,
  type SplitDirection,
  type TerminalPane,
  type Workspace,
} from "./model";
import type { WorkspaceAction } from "./reducer";
import type { WorkspaceTabController } from "./tab-controller";
import { workspaceTabElementIds } from "./tab-bar";

export type PaneDirection = "left" | "right" | "up" | "down";

export interface PaneFocusShortcutEvent {
  type: string;
  key: string;
  altKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  repeat: boolean;
  isComposing: boolean;
}

export interface PaneFocusShortcutIntent {
  direction: PaneDirection;
  focus: boolean;
}

export interface PaneRect {
  paneId: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface SplitRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface SplitControlPolicy {
  canSplitRight: boolean;
  canSplitDown: boolean;
  canClose: boolean;
}

export function splitControlsRestricted(
  tabAssociationPresent: boolean,
  paneLinkedOrigin: boolean,
): boolean {
  return tabAssociationPresent || paneLinkedOrigin;
}

export interface TerminalPanePresentationPolicy {
  paneVisible: boolean;
  foreground: boolean;
  webglAllowed: boolean;
}

export type SplitDragEnd = "pointerup" | "lostpointercapture" | "cancel" | "escape";

function leafDepth(node: PaneNode, paneId: string, depth = 1): number | null {
  if (node.kind === "terminal") return node.paneId === paneId ? depth : null;
  return leafDepth(node.first, paneId, depth + 1) ??
    leafDepth(node.second, paneId, depth + 1);
}

export function leafRects(root: PaneNode, bounds: SplitRect): PaneRect[] {
  const result: PaneRect[] = [];
  const visit = (node: PaneNode, rect: SplitRect): void => {
    if (node.kind === "terminal") {
      result.push({ paneId: node.paneId, ...rect });
      return;
    }
    if (node.direction === "horizontal") {
      const firstWidth = rect.width * node.ratio;
      visit(node.first, { ...rect, width: firstWidth });
      visit(node.second, {
        x: rect.x + firstWidth,
        y: rect.y,
        width: rect.width - firstWidth,
        height: rect.height,
      });
      return;
    }
    const firstHeight = rect.height * node.ratio;
    visit(node.first, { ...rect, height: firstHeight });
    visit(node.second, {
      x: rect.x,
      y: rect.y + firstHeight,
      width: rect.width,
      height: rect.height - firstHeight,
    });
  };
  visit(root, bounds);
  return result;
}

function overlap(startA: number, lengthA: number, startB: number, lengthB: number): number {
  return Math.max(0, Math.min(startA + lengthA, startB + lengthB) - Math.max(startA, startB));
}

export function paneInDirection(
  rects: readonly PaneRect[],
  fromPaneId: string,
  direction: PaneDirection,
): string | null {
  const source = rects.find((rect) => rect.paneId === fromPaneId);
  if (!source) return null;
  const horizontal = direction === "left" || direction === "right";
  const sign = direction === "left" || direction === "up" ? -1 : 1;
  const sourcePrimary = horizontal
    ? source.x + source.width / 2
    : source.y + source.height / 2;
  const sourceOrthogonal = horizontal
    ? source.y + source.height / 2
    : source.x + source.width / 2;
  const candidates = rects.flatMap((rect, index) => {
    if (rect === source || rect.paneId === source.paneId) return [];
    const primary = horizontal
      ? rect.x + rect.width / 2
      : rect.y + rect.height / 2;
    if ((primary - sourcePrimary) * sign <= 0) return [];
    const primaryGap = horizontal
      ? sign > 0
        ? Math.max(0, rect.x - (source.x + source.width))
        : Math.max(0, source.x - (rect.x + rect.width))
      : sign > 0
        ? Math.max(0, rect.y - (source.y + source.height))
        : Math.max(0, source.y - (rect.y + rect.height));
    const orthogonalOverlap = horizontal
      ? overlap(source.y, source.height, rect.y, rect.height)
      : overlap(source.x, source.width, rect.x, rect.width);
    const orthogonal = horizontal
      ? rect.y + rect.height / 2
      : rect.x + rect.width / 2;
    return [{
      rect,
      index,
      overlaps: orthogonalOverlap > 0 ? 0 : 1,
      primaryGap,
      orthogonalGap: Math.abs(orthogonal - sourceOrthogonal),
    }];
  });
  candidates.sort((a, b) =>
    a.overlaps - b.overlaps ||
    a.primaryGap - b.primaryGap ||
    a.orthogonalGap - b.orthogonalGap ||
    a.index - b.index
  );
  return candidates[0]?.rect.paneId ?? null;
}

export function paneFocusShortcutIntent(
  event: PaneFocusShortcutEvent,
  mac: boolean,
): PaneFocusShortcutIntent | null {
  if (
    (event.type !== "keydown" && event.type !== "keyup") ||
    event.isComposing ||
    !event.altKey ||
    (mac ? !event.metaKey || event.ctrlKey : !event.ctrlKey || event.metaKey)
  ) return null;
  const direction = ({
    ArrowLeft: "left",
    ArrowRight: "right",
    ArrowUp: "up",
    ArrowDown: "down",
  }[event.key] as PaneDirection | undefined);
  return direction
    ? { direction, focus: event.type === "keydown" && !event.repeat }
    : null;
}

export function activeTabDockInteractionEligible(input: {
  paneCount: number;
  hasResources: boolean;
  hasLiveRuntime: boolean;
}): boolean {
  return input.paneCount > 1 || input.hasResources || input.hasLiveRuntime;
}

export function terminalPanePresentationPolicy(input: {
  workspaceViewVisible: boolean;
  applicationOverlayOpen: boolean;
  terminalHidden: boolean;
  documentVisible: boolean;
  activeTab: boolean;
  selected: boolean;
  hasResources: boolean;
  hostVisible: boolean;
  bodyVisible: boolean;
  dockVisible: boolean;
}): TerminalPanePresentationPolicy {
  const paneVisible = input.workspaceViewVisible && !input.applicationOverlayOpen &&
    !input.terminalHidden && input.activeTab;
  const webglAllowed = paneVisible && input.documentVisible;
  return {
    paneVisible,
    webglAllowed,
    foreground: webglAllowed && input.selected && input.hasResources &&
      input.hostVisible && input.bodyVisible && input.dockVisible,
  };
}

export function splitControlPolicy(root: PaneNode, paneId: string): SplitControlPolicy {
  const depth = leafDepth(root, paneId);
  const canSplit = depth !== null &&
    depth < maximumPaneDepth &&
    paneCount(root) < maximumPanesPerTab;
  return {
    canSplitRight: canSplit,
    canSplitDown: canSplit,
    canClose: depth !== null && paneCount(root) > 1,
  };
}

export function pointerSplitRatio(
  direction: SplitDirection,
  bounds: SplitRect,
  clientX: number,
  clientY: number,
): number {
  const extent = direction === "horizontal" ? bounds.width : bounds.height;
  if (!Number.isFinite(extent) || extent <= 0) return 0.5;
  const offset = direction === "horizontal"
    ? clientX - bounds.x
    : clientY - bounds.y;
  return normalizeSplitRatio(offset / extent);
}

export function keyboardSplitRatio(
  direction: SplitDirection,
  current: number,
  key: string,
): number | null {
  if (key === "Home") return minimumSplitRatio;
  if (key === "End") return maximumSplitRatio;
  if (key === "PageUp") return normalizeSplitRatio(current - 0.1);
  if (key === "PageDown") return normalizeSplitRatio(current + 0.1);
  const delta = direction === "horizontal"
    ? key === "ArrowLeft"
      ? -0.02
      : key === "ArrowRight"
        ? 0.02
        : null
    : key === "ArrowUp"
      ? -0.02
      : key === "ArrowDown"
        ? 0.02
        : null;
  return delta === null ? null : normalizeSplitRatio(current + delta);
}

export function splitDragOutcome(
  originalRatio: number,
  previewRatio: number,
  end: SplitDragEnd,
): { ratio: number; commit: boolean } {
  const commit = end === "pointerup" || end === "lostpointercapture";
  return {
    ratio: commit ? normalizeSplitRatio(previewRatio) : normalizeSplitRatio(originalRatio),
    commit,
  };
}

export function separatorAria(direction: SplitDirection, ratio: number): {
  orientation: "horizontal" | "vertical";
  valueMin: number;
  valueMax: number;
  valueNow: number;
} {
  return {
    orientation: direction === "horizontal" ? "vertical" : "horizontal",
    valueMin: minimumSplitRatio * 100,
    valueMax: maximumSplitRatio * 100,
    valueNow: Math.round(normalizeSplitRatio(ratio) * 100),
  };
}

export function preferredWebglPaneIds(
  root: PaneNode,
  selectedPaneId: string,
  visiblePaneIds: ReadonlySet<string> = new Set(paneIds(root)),
  budget = 4,
): string[] {
  const boundedBudget = Math.max(0, Math.trunc(budget));
  if (boundedBudget === 0) return [];
  const visible = paneIds(root).filter((paneId) => visiblePaneIds.has(paneId));
  if (!visible.includes(selectedPaneId)) return visible.slice(0, boundedBudget);
  return [selectedPaneId, ...visible.filter((paneId) => paneId !== selectedPaneId)]
    .slice(0, boundedBudget);
}

export function appendPreservingIdentity<T>(
  mount: { append(node: T): void },
  host: T,
): T {
  mount.append(host);
  return host;
}

export interface WorkspaceSplitViewOptions {
  container: HTMLElement;
  controller: WorkspaceTabController;
  hostForPane(paneId: string): HTMLElement | null;
  closePane(action: Extract<WorkspaceAction, { type: "close-pane" }>): void;
  linkedOriginForPane?(paneId: string): boolean;
  fitPanes?(paneIds: readonly string[]): void;
  visibilityChanged?(visiblePaneIds: readonly string[]): void;
}

interface PointerDrag {
  pointerId: number;
  tabId: string;
  splitId: string;
  direction: SplitDirection;
  originalRatio: number;
  previewRatio: number;
  node: HTMLElement;
  separator: HTMLElement;
  paneIds: string[];
  frame: number | null;
  cancelled: boolean;
}

export class WorkspaceSplitView {
  readonly #container: HTMLElement;
  readonly #controller: WorkspaceTabController;
  readonly #hostForPane: (paneId: string) => HTMLElement | null;
  readonly #closePane: WorkspaceSplitViewOptions["closePane"];
  readonly #linkedOriginForPane?: WorkspaceSplitViewOptions["linkedOriginForPane"];
  readonly #fitPanes?: WorkspaceSplitViewOptions["fitPanes"];
  readonly #visibilityChanged?: WorkspaceSplitViewOptions["visibilityChanged"];
  readonly #panels = new Map<string, HTMLElement>();
  readonly #panelRoots = new Map<string, PaneNode>();
  readonly #mounts = new Map<string, HTMLElement>();
  #workspace: Workspace;
  #drag: PointerDrag | null = null;
  #focusSplitId: string | null = null;
  #disposed = false;

  constructor(options: WorkspaceSplitViewOptions) {
    this.#container = options.container;
    this.#controller = options.controller;
    this.#hostForPane = options.hostForPane;
    this.#closePane = options.closePane;
    this.#linkedOriginForPane = options.linkedOriginForPane;
    this.#fitPanes = options.fitPanes;
    this.#visibilityChanged = options.visibilityChanged;
    this.#workspace = options.controller.workspace;
    this.refresh(this.#workspace);
  }

  mountForPane(paneId: string): HTMLElement | null {
    return this.#mounts.get(paneId) ?? null;
  }

  focusPaneSelector(paneId: string): boolean {
    const panel = [...this.#panels.values()].find((candidate) => !candidate.hidden);
    const leaf = panel
      ? [...panel.querySelectorAll<HTMLElement>(".terminal-split-leaf")]
        .find((candidate) => candidate.dataset.paneId === paneId)
      : null;
    const selector = leaf?.querySelector<HTMLButtonElement>(".terminal-split-leaf-select");
    selector?.focus();
    return Boolean(selector);
  }

  refresh(workspace = this.#controller.workspace): void {
    if (this.#disposed) return;
    this.#workspace = workspace;
    this.#mounts.clear();
    const liveTabs = new Set(workspace.tabs.map((tab) => tab.id));
    for (const [tabId, panel] of this.#panels) {
      if (!liveTabs.has(tabId)) {
        panel.remove();
        this.#panels.delete(tabId);
        this.#panelRoots.delete(tabId);
      }
    }
    for (const tab of workspace.tabs) {
      let panel = this.#panels.get(tab.id);
      if (!panel) {
        panel = document.createElement("div");
        panel.className = "terminal-split-tab-panel";
        panel.setAttribute("role", "tabpanel");
        panel.dataset.tabId = tab.id;
        this.#panels.set(tab.id, panel);
        this.#container.append(panel);
      }
      panel.hidden = tab.id !== workspace.activeTabId;
      panel.setAttribute("aria-label", tab.title);
      const elementIds = workspaceTabElementIds(tab.id);
      panel.id = elementIds.panelId;
      panel.setAttribute("aria-labelledby", elementIds.tabButtonId);
      if (this.#panelRoots.get(tab.id) !== tab.root) {
        const rendered = this.#renderNode(tab.id, tab.root, tab.activePaneId);
        panel.append(rendered);
        for (const child of [...panel.children]) {
          if (child !== rendered) child.remove();
        }
        this.#panelRoots.set(tab.id, tab.root);
      } else {
        for (const leaf of panel.querySelectorAll<HTMLElement>(".terminal-split-leaf")) {
          const selected = leaf.dataset.paneId === tab.activePaneId;
          leaf.dataset.selected = String(selected);
          leaf.querySelector<HTMLElement>(".terminal-split-leaf-select")
            ?.setAttribute("aria-pressed", String(selected));
        }
        for (const paneId of paneIds(tab.root)) {
          const mount = panel.querySelectorAll<HTMLElement>(".terminal-split-leaf-mount")
            .item(paneIds(tab.root).indexOf(paneId));
          if (mount) this.#mounts.set(paneId, mount);
        }
      }
    }
    const activeTab = workspace.tabs.find((tab) => tab.id === workspace.activeTabId);
    const visible = activeTab ? paneIds(activeTab.root) : [];
    this.#visibilityChanged?.(visible);
    if (this.#focusSplitId) {
      const target = [...this.#container.querySelectorAll<HTMLElement>("[data-split-id]")]
        .find((element) => element.dataset.splitId === this.#focusSplitId);
      this.#focusSplitId = null;
      target?.focus();
    }
  }

  #renderNode(tabId: string, node: PaneNode, activePaneId: string): HTMLElement {
    if (node.kind === "terminal") return this.#renderLeaf(tabId, node, activePaneId);
    const wrapper = document.createElement("div");
    wrapper.className = `terminal-split-node terminal-split-${node.direction}`;
    wrapper.style.setProperty("--split-ratio", `${node.ratio * 100}%`);
    const first = this.#renderNode(tabId, node.first, activePaneId);
    const second = this.#renderNode(tabId, node.second, activePaneId);
    const separator = document.createElement("div");
    separator.className = "terminal-split-separator";
    separator.dataset.splitId = node.splitId;
    separator.tabIndex = 0;
    separator.setAttribute("role", "separator");
    separator.setAttribute("aria-label", "Resize terminal panes");
    const aria = separatorAria(node.direction, node.ratio);
    separator.setAttribute("aria-orientation", aria.orientation);
    separator.setAttribute("aria-valuemin", String(aria.valueMin));
    separator.setAttribute("aria-valuemax", String(aria.valueMax));
    separator.setAttribute("aria-valuenow", String(aria.valueNow));
    const children = paneIds(node);
    separator.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && this.#drag?.separator === separator) {
        event.preventDefault();
        this.#finishDrag("escape");
        return;
      }
      const ratio = keyboardSplitRatio(node.direction, node.ratio, event.key);
      if (ratio === null) return;
      event.preventDefault();
      this.#focusSplitId = node.splitId;
      this.#controller.dispatch({
        type: "resize-split",
        tabId,
        splitId: node.splitId,
        ratio,
      });
    });
    separator.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      event.preventDefault();
      separator.setPointerCapture(event.pointerId);
      this.#drag = {
        pointerId: event.pointerId,
        tabId,
        splitId: node.splitId,
        direction: node.direction,
        originalRatio: node.ratio,
        previewRatio: node.ratio,
        node: wrapper,
        separator,
        paneIds: children,
        frame: null,
        cancelled: false,
      };
    });
    separator.addEventListener("pointermove", (event) => this.#previewDrag(event));
    separator.addEventListener("pointerup", (event) => {
      if (this.#drag?.pointerId !== event.pointerId) return;
      this.#finishDrag("pointerup");
    });
    separator.addEventListener("pointercancel", (event) => {
      if (this.#drag?.pointerId !== event.pointerId) return;
      this.#finishDrag("cancel");
    });
    separator.addEventListener("lostpointercapture", (event) => {
      if (this.#drag?.pointerId !== event.pointerId || this.#drag.cancelled) return;
      this.#finishDrag("lostpointercapture");
    });
    wrapper.append(first, separator, second);
    return wrapper;
  }

  #renderLeaf(tabId: string, pane: TerminalPane, activePaneId: string): HTMLElement {
    const frame = document.createElement("section");
    frame.className = "terminal-split-leaf";
    frame.dataset.paneId = pane.paneId;
    frame.dataset.selected = String(pane.paneId === activePaneId);
    const chrome = document.createElement("div");
    chrome.className = "terminal-split-leaf-chrome";
    const select = document.createElement("button");
    select.type = "button";
    select.className = "terminal-split-leaf-select";
    select.textContent = pane.profileId || "Terminal";
    select.setAttribute("aria-label", `Select terminal pane ${select.textContent}`);
    select.setAttribute("aria-pressed", String(pane.paneId === activePaneId));
    select.addEventListener("click", () => {
      this.#controller.dispatch({ type: "focus-pane", tabId, paneId: pane.paneId });
    });
    const workspaceTab = this.#workspace.tabs.find((tab) => tab.id === tabId);
    const policy = splitControlPolicy(workspaceTab?.root ?? pane, pane.paneId);
    const linked = splitControlsRestricted(
      workspaceTab?.association !== undefined,
      this.#linkedOriginForPane?.(pane.paneId) === true,
    );
    const splitRight = this.#button("Split right", "→", linked || !policy.canSplitRight, () => {
      this.#controller.dispatch({
        type: "split-pane",
        tabId,
        paneId: pane.paneId,
        direction: "horizontal",
        profileId: pane.profileId,
        cwd: pane.cwd,
      });
    });
    const splitDown = this.#button("Split down", "↓", linked || !policy.canSplitDown, () => {
      this.#controller.dispatch({
        type: "split-pane",
        tabId,
        paneId: pane.paneId,
        direction: "vertical",
        profileId: pane.profileId,
        cwd: pane.cwd,
      });
    });
    const close = this.#button("Close pane", "×", !policy.canClose, () => {
      this.#closePane({ type: "close-pane", tabId, paneId: pane.paneId });
    });
    chrome.append(select, splitRight, splitDown, close);
    const mount = document.createElement("div");
    mount.className = "terminal-split-leaf-mount";
    this.#mounts.set(pane.paneId, mount);
    const host = this.#hostForPane(pane.paneId);
    if (host) appendPreservingIdentity(mount, host);
    frame.addEventListener("pointerdown", () => {
      if (pane.paneId !== activePaneId) {
        this.#controller.dispatch({ type: "focus-pane", tabId, paneId: pane.paneId });
      }
    });
    frame.append(chrome, mount);
    return frame;
  }

  #button(
    label: string,
    text: string,
    disabled: boolean,
    action: () => void,
  ): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = text;
    button.setAttribute("aria-label", label);
    button.title = label;
    button.disabled = disabled;
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      action();
    });
    return button;
  }

  #previewDrag(event: PointerEvent): void {
    const drag = this.#drag;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const rect = drag.node.getBoundingClientRect();
    drag.previewRatio = pointerSplitRatio(
      drag.direction,
      { x: rect.left, y: rect.top, width: rect.width, height: rect.height },
      event.clientX,
      event.clientY,
    );
    if (drag.frame !== null) return;
    drag.frame = requestAnimationFrame(() => {
      if (this.#drag !== drag) return;
      drag.frame = null;
      drag.node.style.setProperty("--split-ratio", `${drag.previewRatio * 100}%`);
      drag.separator.setAttribute("aria-valuenow", String(Math.round(drag.previewRatio * 100)));
      this.#fitPanes?.(drag.paneIds);
    });
  }

  #finishDrag(end: SplitDragEnd): void {
    const drag = this.#drag;
    if (!drag) return;
    this.#drag = null;
    drag.cancelled = end === "cancel" || end === "escape";
    if (drag.frame !== null) cancelAnimationFrame(drag.frame);
    const outcome = splitDragOutcome(drag.originalRatio, drag.previewRatio, end);
    drag.node.style.setProperty("--split-ratio", `${outcome.ratio * 100}%`);
    drag.separator.setAttribute("aria-valuenow", String(Math.round(outcome.ratio * 100)));
    if (drag.separator.hasPointerCapture(drag.pointerId)) {
      drag.separator.releasePointerCapture(drag.pointerId);
    }
    drag.separator.focus();
    this.#fitPanes?.(drag.paneIds);
    if (!outcome.commit || outcome.ratio === drag.originalRatio) return;
    this.#focusSplitId = drag.splitId;
    this.#controller.dispatch({
      type: "resize-split",
      tabId: drag.tabId,
      splitId: drag.splitId,
      ratio: outcome.ratio,
    });
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    if (this.#drag) this.#finishDrag("cancel");
    for (const panel of this.#panels.values()) panel.remove();
    this.#panels.clear();
    this.#panelRoots.clear();
    this.#mounts.clear();
  }
}
