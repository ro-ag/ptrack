import { terminalWindowLabel } from "../terminal/pop-out";
import { applicationOverlayKeyboardPolicy } from "./application-overlay";
import { commandShortcut, focusCycleIndex, shortcutIntent } from "./presentation";
import type { AppContext } from "./app-context";
import { element } from "./dom";

const TERMINAL_SURFACES = "#terminal-dock, [data-terminal-overlay]";

const FOCUSABLE_IN_OVERLAY = [
  'button:not([disabled]):not([tabindex="-1"])',
  'input:not([disabled]):not([hidden])',
  'textarea:not([disabled]):not([hidden])',
  'select:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(", ");

function shortcutInput(event: KeyboardEvent) {
  return {
    key: event.key,
    composing: event.isComposing,
    meta: event.metaKey,
    ctrl: event.ctrlKey,
    alt: event.altKey,
    shift: event.shiftKey,
    repeat: event.repeat,
    prevented: event.defaultPrevented,
  };
}

/** Whether a key event happened in the terminal dock or one of its overlays. */
export function terminalHasFocus(active: Element | null, path: readonly EventTarget[]): boolean {
  return (active instanceof Element && Boolean(active.closest(TERMINAL_SURFACES))) ||
    path.some(
      (node) =>
        node instanceof Element &&
        (node.matches(TERMINAL_SURFACES) || Boolean(node.closest(TERMINAL_SURFACES))),
    );
}

export function createShortcuts(ctx: AppContext) {
  const { nativeEventDisposers, workspaceController } = ctx;
  const elements = {
    palette: element("#palette", HTMLDivElement),
    settingsModal: element("#settings-modal", HTMLDivElement),
    taskTitle: element("#task-title", HTMLInputElement),
    terminalPanelToggle: element("#terminal-panel-toggle", HTMLButtonElement),
  };

  function boardShortcutIsBlocked(event: KeyboardEvent): boolean {
    if (
      event.isComposing ||
      workspaceController.state.status !== "open" ||
      ctx.state.firstRunState.phase !== "idle" ||
      ctx.state.firstPlanState.phase !== "idle"
    ) return true;
    const active = document.activeElement;
    const interactive =
      active instanceof HTMLElement &&
      (["INPUT", "SELECT", "TEXTAREA", "BUTTON"].includes(active.tagName) ||
        active.isContentEditable);
    const path = typeof event.composedPath === "function" ? event.composedPath() : [];
    return interactive || terminalHasFocus(active, path) || ctx.snapshot.snapshotDialogIsOpen();
  }

  function trapModalFocus(event: KeyboardEvent): void {
    if (event.key !== "Tab") return;
    const modal = ctx.snapshot.applicationOverlayCoordinator.activeOverlay;
    if (!(modal instanceof HTMLElement)) return;
    const policy = applicationOverlayKeyboardPolicy(
      modal.id,
      modal.hasAttribute("data-terminal-overlay"),
    );
    if (!policy.trapTab) return;
    const focusable = Array.from(
      modal.querySelectorAll<HTMLElement>(FOCUSABLE_IN_OVERLAY),
    ).filter((item) => !item.hidden && !item.closest("[hidden], [inert]"));
    if (focusable.length === 0) return;
    const first = focusable[0];
    const active = document.activeElement;
    const current = active instanceof HTMLElement ? focusable.indexOf(active) : -1;
    const next = focusCycleIndex(focusable.length, current, event.shiftKey);
    if (next < 0) return;
    event.preventDefault();
    (focusable[next] || first).focus();
  }

  function closeActiveApplicationOverlay(event: KeyboardEvent): boolean {
    if (event.key !== "Escape" || event.defaultPrevented) return false;
    const modal = ctx.snapshot.applicationOverlayCoordinator.activeOverlay;
    if (!(modal instanceof HTMLElement)) return false;
    const { escapeAction } = applicationOverlayKeyboardPolicy(
      modal.id,
      modal.hasAttribute("data-terminal-overlay"),
    );
    if (!escapeAction) return false;
    event.preventDefault();
    event.stopImmediatePropagation();
    if (escapeAction === "dialog") ctx.planDialogs.closeDialog();
    else if (escapeAction === "memory") ctx.overview.closeMemoryHistory();
    else if (escapeAction === "settings") ctx.settings.closeSettings();
    else if (escapeAction === "updates") ctx.updates.closeAboutUpdates();
    else if (escapeAction === "drawer") ctx.drawer.closeTaskDetail();
    else if (escapeAction === "issue-drawer") ctx.issues.closeIssueDetail();
    else if (escapeAction === "agent-launch") ctx.agentLaunch.closeAgentLaunchPicker();
    else if (escapeAction === "terminal-association") {
      ctx.association.closeTerminalAssociationEditor();
    } else if (escapeAction === "terminal-writeback") ctx.writeback.closeTerminalWriteback();
    else if (escapeAction === "task-transition") ctx.snapshot.closeTaskTransition();
    else if (escapeAction === "workspace-confirm") {
      ctx.recent.finishWorkspaceConfirmation(false);
    } else if (escapeAction === "palette") ctx.palette.closePalette();
    return true;
  }

  function handleKeydown(event: KeyboardEvent): void {
    trapModalFocus(event);
    if (event.key === "Escape") {
      if (event.defaultPrevented || closeActiveApplicationOverlay(event)) return;
    }
    // A terminal window has no palette, Settings, board, or snapshot: the
    // backend refuses those commands there, so its shortcuts stay with the
    // terminal instead.
    if (terminalWindowLabel(window.location.hash)) return;
    if (runCommandShortcut(event)) return;
    runBoardShortcut(event);
  }

  // ⌘/Ctrl chords. Returns true when the chord was one of the global ones
  // that ends the event's handling.
  function runCommandShortcut(event: KeyboardEvent): boolean {
    const command = commandShortcut(shortcutInput(event));
    if (command === "terminal") {
      // ⌘J toggles the terminal panel from anywhere in an open project,
      // including from inside the terminal, like the View menu item.
      event.preventDefault();
      const toggle = elements.terminalPanelToggle;
      if (workspaceController.state.status === "open" && !toggle.disabled && !toggle.hidden) {
        toggle.click();
      }
      return true;
    }
    if (command === "palette") {
      // ⌘K works globally, even while typing in an input.
      event.preventDefault();
      if (elements.palette.hidden) ctx.palette.openPalette();
      else ctx.palette.closePalette();
      return true;
    }
    if (command === "settings") {
      // ⌘, opens the application dialog, including with no project open.
      event.preventDefault();
      if (elements.settingsModal.hidden) ctx.settings.openSettings(document.activeElement);
      else ctx.settings.closeSettings();
      return true;
    }
    if (command && !boardShortcutIsBlocked(event)) {
      event.preventDefault();
      if (command === "board") ctx.shell.setView("board", true);
      if (command === "overview") ctx.shell.setView("overview", true);
      if (command === "issues") ctx.shell.setView("issues", true);
      if (command === "addTask") {
        ctx.shell.setView("board");
        elements.taskTitle.focus();
      }
    }
    return false;
  }

  // Bare-key board shortcuts, which never fire while typing or in a dialog.
  function runBoardShortcut(event: KeyboardEvent): void {
    const shortcut = shortcutIntent(shortcutInput(event));
    if (shortcut === "refresh" && !boardShortcutIsBlocked(event)) {
      event.preventDefault();
      void ctx.snapshot.loadSnapshot();
    }
    if (shortcut === "addTask" && !boardShortcutIsBlocked(event)) {
      event.preventDefault();
      elements.taskTitle.focus();
    }
  }

  function bind(): void {
    document.addEventListener("keydown", handleKeydown);
    window.addEventListener("beforeunload", () => {
      ctx.layout.disposeLayout();
      ctx.snapshot.refreshLoop.dispose();
      ctx.snapshot.runtimeRefreshes.cancel();
      ctx.terminalBackend.disposeTerminalDock();
      nativeEventDisposers.splice(0).forEach((dispose) => dispose());
    });
  }

  return {
    bind,
  };
}

export type Shortcuts = ReturnType<typeof createShortcuts>;
