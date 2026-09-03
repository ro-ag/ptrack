import {
  maximumTabTitleLength,
  maximumWorkspaceTabs,
  normalizeTabTitle,
  type WorkspaceTab,
} from "./model";
import type { WorkspaceAction } from "./reducer";
import { WorkspaceTabController } from "./tab-controller";
import type { PaneIndicator, PaneIndicatorKind } from "../terminal/activity";
import { terminalControlIcon, type TerminalControlIcon } from "../terminal/control-icon";

export type TabRenameKeyIntent = "begin" | "commit" | "cancel";
export type TabMoveDirection = "left" | "right";

export interface TabControlPolicy {
  moveLeftDisabled: boolean;
  moveRightDisabled: boolean;
  duplicateDisabled: boolean;
  closeDisabled: boolean;
}

export function tabFocusIndex(
  key: string,
  currentIndex: number,
  count: number,
): number | null {
  if (count <= 0) return null;
  if (key === "Home") return 0;
  if (key === "End") return count - 1;
  if (key === "ArrowLeft") return currentIndex <= 0 ? count - 1 : currentIndex - 1;
  if (key === "ArrowRight") {
    return currentIndex < 0 || currentIndex >= count - 1 ? 0 : currentIndex + 1;
  }
  return null;
}

export function tabMoveIndex(
  currentIndex: number,
  direction: TabMoveDirection,
  count: number,
): number | null {
  if (currentIndex < 0 || currentIndex >= count) return null;
  const target = direction === "left" ? currentIndex - 1 : currentIndex + 1;
  return target < 0 || target >= count ? null : target;
}

export function tabRenameKeyIntent(key: string): TabRenameKeyIntent | null {
  if (key === "F2") return "begin";
  if (key === "Enter") return "commit";
  if (key === "Escape") return "cancel";
  return null;
}

export function normalizedTabRename(value: string): string | null {
  return value.trim().length === 0 ? null : normalizeTabTitle(value);
}

export function canCreateWorkspaceTab(
  count: number,
  maximum = maximumWorkspaceTabs,
): boolean {
  return Number.isInteger(count) && count >= 0 && count < maximum;
}

export function tabControlPolicy(
  index: number,
  count: number,
  maximum = maximumWorkspaceTabs,
): TabControlPolicy {
  return {
    moveLeftDisabled: index <= 0,
    moveRightDisabled: index < 0 || index >= count - 1,
    duplicateDisabled: !canCreateWorkspaceTab(count, maximum),
    closeDisabled: count <= 1,
  };
}

export interface WorkspaceTabBarOptions {
  tabList: HTMLElement;
  actionToolbar: HTMLElement;
  newTabButton: HTMLButtonElement;
  controller: WorkspaceTabController;
  closeIntent?: (
    action: Extract<WorkspaceAction, { type: "close-tab" | "close-pane" }>,
  ) => void;
  indicatorForTab?: (tab: WorkspaceTab) => PaneIndicator;
}

export interface ActiveTabActionState {
  tab: WorkspaceTab;
  index: number;
  controls: TabControlPolicy;
}

export function activeTabActionState(
  tabs: readonly WorkspaceTab[],
  activeTabId: string,
  maximum = maximumWorkspaceTabs,
): ActiveTabActionState | null {
  const index = tabs.findIndex((tab) => tab.id === activeTabId);
  const tab = tabs[index];
  return tab
    ? { tab, index, controls: tabControlPolicy(index, tabs.length, maximum) }
    : null;
}

export interface TabIndicatorPresentation {
  glyph: string;
  label: string;
}

export function workspaceTabElementIds(tabId: string): {
  tabButtonId: string;
  panelId: string;
} {
  let encoded = "";
  for (let index = 0; index < tabId.length; index += 1) {
    encoded += tabId.charCodeAt(index).toString(16).padStart(4, "0");
  }
  return {
    tabButtonId: `terminal-tab-${encoded}`,
    panelId: `terminal-tab-panel-${encoded}`,
  };
}

export function structuralCloseFocusTarget(
  action: "close-tab" | "close-pane",
): "active-tab" | "active-pane" {
  return action === "close-tab" ? "active-tab" : "active-pane";
}

export function restoreConnectedFocus(
  target: { readonly isConnected: boolean; focus(): void } | null,
): boolean {
  if (!target?.isConnected) return false;
  target.focus();
  return true;
}

export function tabIndicatorPresentation(
  kind: PaneIndicatorKind,
): TabIndicatorPresentation {
  return {
    failed: { glyph: "!", label: "failed" },
    completed: { glyph: "✓", label: "completed" },
    exited: { glyph: "■", label: "exited" },
    activity: { glyph: "●", label: "new terminal activity" },
    opening: { glyph: "◌", label: "opening" },
    running: { glyph: "▶", label: "running" },
    waiting: { glyph: "○", label: "waiting for activity" },
    closed: { glyph: "—", label: "closed" },
  }[kind];
}

function button(
  icon: TerminalControlIcon,
  label: string,
  onClick: () => void,
  disabled = false,
): HTMLButtonElement {
  const result = document.createElement("button");
  result.type = "button";
  result.className = "terminal-tab-action";
  result.append(terminalControlIcon(icon));
  result.setAttribute("aria-label", label);
  result.title = label;
  result.disabled = disabled;
  result.addEventListener("click", onClick);
  return result;
}

export class WorkspaceTabBar {
  readonly #tabList: HTMLElement;
  readonly #actionToolbar: HTMLElement;
  readonly #newTabButton: HTMLButtonElement;
  readonly #controller: WorkspaceTabController;
  readonly #closeIntent: WorkspaceTabBarOptions["closeIntent"];
  readonly #indicatorForTab: WorkspaceTabBarOptions["indicatorForTab"];
  readonly #tabButtons = new Map<string, HTMLButtonElement>();
  readonly #unsubscribe: () => void;
  readonly #onNewTab = (): void => {
    const next = this.#controller.dispatch({ type: "create-tab" });
    if (next) this.#focusTab(next.activeTabId);
  };
  #renamingTabId: string | null = null;
  #renameInput: HTMLInputElement | null = null;
  #disposed = false;

  constructor(options: WorkspaceTabBarOptions) {
    this.#tabList = options.tabList;
    this.#actionToolbar = options.actionToolbar;
    this.#newTabButton = options.newTabButton;
    this.#controller = options.controller;
    this.#closeIntent = options.closeIntent;
    this.#indicatorForTab = options.indicatorForTab;
    this.#newTabButton.addEventListener("click", this.#onNewTab);
    this.#unsubscribe = this.#controller.subscribe(() => this.#render());
    this.#render();
  }

  #dispatch(action: WorkspaceAction, focusTabId?: string): void {
    const next = this.#controller.dispatch(action);
    if (focusTabId) this.#focusTab(focusTabId);
    else if (next) this.#focusTab(next.activeTabId);
  }

  #focusTab(tabId: string): void {
    this.#tabButtons.get(tabId)?.focus();
  }

  focusActiveTab(): void {
    if (this.#disposed) return;
    this.#focusTab(this.#controller.workspace.activeTabId);
  }

  #beginRename(tabId: string): void {
    if (this.#disposed) return;
    this.#renamingTabId = tabId;
    this.#render();
    this.#renameInput?.focus();
    this.#renameInput?.select();
  }

  #finishRename(tab: WorkspaceTab, value: string, commit: boolean): void {
    if (this.#renamingTabId !== tab.id) return;
    const title = commit ? normalizedTabRename(value) : null;
    this.#renamingTabId = null;
    if (title) {
      const next = this.#controller.dispatch({
        type: "rename-tab",
        tabId: tab.id,
        title,
      });
      if (!next) this.#render();
    } else {
      this.#render();
    }
    this.#focusTab(tab.id);
  }

  #refreshIndicator(tab: WorkspaceTab, tabButton: HTMLButtonElement): void {
    const indicator = this.#indicatorForTab?.(tab) ?? {
      kind: "closed",
      unread: false,
    };
    const presentation = tabIndicatorPresentation(indicator.kind);
    tabButton.setAttribute(
      "aria-label",
      `${tab.title}: ${presentation.label}${indicator.unread ? ", unread" : ""}`,
    );
    tabButton.dataset.indicator = indicator.kind;
    tabButton.dataset.unread = String(indicator.unread);
    tabButton.title = tabButton.getAttribute("aria-label") ?? tab.title;
    if (tabButton.firstElementChild) {
      tabButton.firstElementChild.textContent = presentation.glyph;
    }
  }

  #renderTab(tab: WorkspaceTab, index: number, count: number): HTMLElement {
    const item = document.createElement("div");
    item.className = "terminal-tab-item";
    item.setAttribute("role", "presentation");

    const tabButton = document.createElement("button");
    tabButton.type = "button";
    tabButton.className = "terminal-tab";
    const elementIds = workspaceTabElementIds(tab.id);
    tabButton.id = elementIds.tabButtonId;
    tabButton.setAttribute("role", "tab");
    tabButton.setAttribute("aria-controls", elementIds.panelId);
    const selected = tab.id === this.#controller.workspace.activeTabId;
    tabButton.setAttribute("aria-selected", String(selected));
    tabButton.tabIndex = selected ? 0 : -1;
    const indicatorGlyph = document.createElement("span");
    indicatorGlyph.className = "terminal-tab-indicator";
    indicatorGlyph.setAttribute("aria-hidden", "true");
    const title = document.createElement("span");
    title.className = "terminal-tab-label";
    title.textContent = tab.title;
    tabButton.append(indicatorGlyph, title);
    this.#refreshIndicator(tab, tabButton);
    tabButton.addEventListener("click", () => {
      this.#dispatch({ type: "select-tab", tabId: tab.id }, tab.id);
    });
    tabButton.addEventListener("keydown", (event) => {
      const intent = tabRenameKeyIntent(event.key);
      if (intent === "begin") {
        event.preventDefault();
        this.#beginRename(tab.id);
        return;
      }
      const targetIndex = tabFocusIndex(event.key, index, count);
      if (targetIndex === null) return;
      event.preventDefault();
      const target = this.#controller.workspace.tabs[targetIndex];
      if (target) this.#dispatch({ type: "select-tab", tabId: target.id }, target.id);
    });
    this.#tabButtons.set(tab.id, tabButton);
    item.append(tabButton);
    const closeAction: WorkspaceAction = { type: "close-tab", tabId: tab.id };
    const close = button("close", `Close ${tab.title} tab`, () => {
      if (this.#closeIntent) this.#closeIntent(closeAction);
      else this.#dispatch(closeAction);
    }, tabControlPolicy(index, count).closeDisabled || !this.#controller.canDispatch(closeAction));
    close.classList.add("terminal-tab-close");
    if (count === 1) close.title = "Keep one tab open. Use Stop session to stop its process.";
    item.append(close);

    return item;
  }

  #renderActions(): void {
    const workspace = this.#controller.workspace;
    const active = activeTabActionState(workspace.tabs, workspace.activeTabId);
    if (!active) {
      this.#renamingTabId = null;
      this.#actionToolbar.replaceChildren();
      return;
    }
    const { tab, index, controls } = active;
    if (this.#renamingTabId && this.#renamingTabId !== tab.id) {
      this.#renamingTabId = null;
    }
    if (this.#renamingTabId === tab.id) {
      const input = document.createElement("input");
      input.className = "terminal-tab-rename";
      input.type = "text";
      input.value = tab.title;
      input.maxLength = maximumTabTitleLength;
      input.setAttribute("aria-label", `Rename ${tab.title}`);
      input.addEventListener("keydown", (event) => {
        const intent = tabRenameKeyIntent(event.key);
        if (intent === "commit") {
          event.preventDefault();
          if (normalizedTabRename(input.value)) {
            this.#finishRename(tab, input.value, true);
          } else {
            input.setAttribute("aria-invalid", "true");
          }
        } else if (intent === "cancel") {
          event.preventDefault();
          this.#finishRename(tab, input.value, false);
        }
      });
      input.addEventListener("input", () => input.removeAttribute("aria-invalid"));
      input.addEventListener("blur", () => {
        if (this.#renamingTabId === tab.id) {
          this.#finishRename(tab, input.value, normalizedTabRename(input.value) !== null);
        }
      });
      this.#renameInput = input;
      this.#actionToolbar.replaceChildren(input);
      return;
    }

    const duplicateAction: WorkspaceAction = { type: "duplicate-tab", tabId: tab.id };
    this.#actionToolbar.replaceChildren(
      button("left", `Move ${tab.title} left`, () => {
        const target = tabMoveIndex(index, "left", workspace.tabs.length);
        if (target !== null) {
          this.#dispatch({ type: "reorder-tab", tabId: tab.id, toIndex: target }, tab.id);
        }
      }, controls.moveLeftDisabled),
      button("right", `Move ${tab.title} right`, () => {
        const target = tabMoveIndex(index, "right", workspace.tabs.length);
        if (target !== null) {
          this.#dispatch({ type: "reorder-tab", tabId: tab.id, toIndex: target }, tab.id);
        }
      }, controls.moveRightDisabled),
      button("rename", `Rename ${tab.title}`, () => this.#beginRename(tab.id)),
      button("duplicate", `Duplicate ${tab.title}`, () => {
        this.#dispatch(duplicateAction);
      }, controls.duplicateDisabled || !this.#controller.canDispatch(duplicateAction)),
    );
  }

  #render(): void {
    if (this.#disposed) return;
    const workspace = this.#controller.workspace;
    this.#tabButtons.clear();
    this.#renameInput = null;
    const fragment = document.createDocumentFragment();
    workspace.tabs.forEach((tab, index) => {
      fragment.append(this.#renderTab(tab, index, workspace.tabs.length));
    });
    this.#tabList.replaceChildren(fragment);
    this.#renderActions();
    this.#newTabButton.disabled =
      !canCreateWorkspaceTab(workspace.tabs.length) ||
      !this.#controller.canDispatch({ type: "create-tab" });
  }

  refresh(): void {
    if (this.#disposed) return;
    for (const tab of this.#controller.workspace.tabs) {
      const tabButton = this.#tabButtons.get(tab.id);
      if (tabButton) this.#refreshIndicator(tab, tabButton);
    }
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#unsubscribe();
    this.#newTabButton.removeEventListener("click", this.#onNewTab);
    this.#newTabButton.disabled = true;
    this.#tabButtons.clear();
    this.#renameInput = null;
    this.#tabList.replaceChildren();
    this.#actionToolbar.replaceChildren();
  }
}
