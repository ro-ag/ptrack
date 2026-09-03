import { describe, expect, it, vi } from "vitest";

import { terminalPanePresentationPolicy } from "./split-view";
import {
  type ApplicationOverlay,
  type ApplicationOverlayChange,
  ApplicationOverlayCoordinator,
  applicationOverlayKeyboardPolicy,
} from "./application-overlay";

function fakeDock() {
  let workspaceViewVisible = true;
  let paneVisible = false;
  const focus = vi.fn();
  const notifications: boolean[] = [];
  const focusRequests: boolean[] = [];
  return {
    notifications,
    focus,
    focusRequests,
    setWorkspaceViewVisible(visible: boolean) {
      workspaceViewVisible = visible;
    },
    setApplicationOverlayOpen(open: boolean, focusTerminal: false) {
      notifications.push(open);
      focusRequests.push(focusTerminal);
      paneVisible = terminalPanePresentationPolicy({
        workspaceViewVisible,
        applicationOverlayOpen: open,
        terminalHidden: false,
        documentVisible: true,
        activeTab: true,
        selected: true,
        hasResources: true,
        hostVisible: !open,
        bodyVisible: true,
        dockVisible: true,
      }).paneVisible;
    },
    paneVisible() {
      return paneVisible;
    },
  };
}

function fakeAttributeTarget(
  initial = {
    inert: false,
    ariaHidden: null as string | null,
    layer: null as string | null,
  },
) {
  const attributes = new Map<string, string>();
  if (initial.ariaHidden !== null) attributes.set("aria-hidden", initial.ariaHidden);
  if (initial.layer !== null) {
    attributes.set("data-application-overlay-layer", initial.layer);
  }
  return {
    inert: initial.inert,
    getAttribute(name: string) {
      return attributes.get(name) ?? null;
    },
    setAttribute(name: string, value: string) {
      attributes.set(name, value);
    },
    removeAttribute(name: string) {
      attributes.delete(name);
    },
  };
}

function fakeOverlay(
  hidden = true,
  initial?: Parameters<typeof fakeAttributeTarget>[0],
): ApplicationOverlay & { hidden: boolean } {
  return { hidden, ...fakeAttributeTarget(initial) };
}

function change(
  overlay: ApplicationOverlay & { hidden: boolean },
  open: boolean,
): ApplicationOverlayChange {
  overlay.hidden = !open;
  return { overlay, open };
}

describe("application overlay coordinator", () => {
  it.each([
    ["modal", "dialog"],
    ["memory-modal", "memory"],
    ["settings-modal", "settings"],
    ["updates-modal", "updates"],
    ["task-drawer", "drawer"],
    ["issue-drawer", "issue-drawer"],
    ["agent-launch-modal", "agent-launch"],
    ["terminal-association-modal", "terminal-association"],
    ["terminal-writeback-modal", "terminal-writeback"],
    ["task-transition-modal", "task-transition"],
    ["workspace-confirm-modal", "workspace-confirm"],
    ["palette", "palette"],
  ])("maps Escape for application overlay %s", (overlayID, escapeAction) => {
    expect(applicationOverlayKeyboardPolicy(overlayID, false)).toEqual({
      trapTab: true,
      escapeAction,
    });
  });

  it("leaves terminal overlay keyboard handling to the terminal module", () => {
    expect(applicationOverlayKeyboardPolicy("terminal-paste-modal", true)).toEqual({
      trapTab: false,
      escapeAction: null,
    });
    expect(applicationOverlayKeyboardPolicy("terminal-termination-modal", true))
      .toEqual({ trapTab: false, escapeAction: null });
    expect(applicationOverlayKeyboardPolicy("terminal-context-menu", true)).toEqual({
      trapTab: false,
      escapeAction: null,
    });
    expect(applicationOverlayKeyboardPolicy("unknown-modal", false)).toEqual({
      trapTab: true,
      escapeAction: null,
    });
  });

  it("stacks a dialog opened from the drawer above an inert hidden underlay", () => {
    const drawer = fakeOverlay();
    const dialog = fakeOverlay();
    const background = fakeAttributeTarget();
    const coordinator = new ApplicationOverlayCoordinator(
      () => [dialog, drawer],
      background,
    );

    coordinator.reconcile([change(drawer, true)]);
    expect(coordinator.activeOverlay).toBe(drawer);
    expect(drawer.getAttribute("data-application-overlay-layer")).toBe("active");

    coordinator.reconcile([change(dialog, true)]);
    expect(coordinator.activeOverlay).toBe(dialog);
    expect(dialog.inert).toBe(false);
    expect(dialog.getAttribute("aria-hidden")).toBeNull();
    expect(dialog.getAttribute("data-application-overlay-layer")).toBe("active");
    expect(drawer.inert).toBe(true);
    expect(drawer.getAttribute("aria-hidden")).toBe("true");
    expect(drawer.getAttribute("data-application-overlay-layer")).toBe("underlay");
    expect(background.inert).toBe(true);
    expect(background.getAttribute("aria-hidden")).toBe("true");
  });

  it("reactivates the underlying drawer and restores the app only after final close", () => {
    const drawer = fakeOverlay();
    const dialog = fakeOverlay();
    const background = fakeAttributeTarget();
    const coordinator = new ApplicationOverlayCoordinator(
      () => [dialog, drawer],
      background,
    );
    coordinator.reconcile([change(drawer, true), change(dialog, true)]);

    coordinator.reconcile([change(dialog, false)]);
    expect(coordinator.activeOverlay).toBe(drawer);
    expect(drawer.inert).toBe(false);
    expect(drawer.getAttribute("aria-hidden")).toBeNull();
    expect(drawer.getAttribute("data-application-overlay-layer")).toBe("active");
    expect(background.inert).toBe(true);

    coordinator.reconcile([change(drawer, false)]);
    expect(coordinator.activeOverlay).toBeNull();
    expect(background.inert).toBe(false);
    expect(background.getAttribute("aria-hidden")).toBeNull();
  });

  it("makes a later terminal overlay active without duplicate dock notifications", () => {
    const drawer = fakeOverlay();
    const terminalOverlay = fakeOverlay();
    const dock = fakeDock();
    const coordinator = new ApplicationOverlayCoordinator(
      () => [drawer, terminalOverlay],
    );
    coordinator.setDock(dock);

    coordinator.reconcile([change(drawer, true)]);
    coordinator.reconcile();
    coordinator.reconcile([change(terminalOverlay, true)]);
    coordinator.reconcile();
    expect(coordinator.activeOverlay).toBe(terminalOverlay);
    expect(drawer.getAttribute("data-application-overlay-layer")).toBe("underlay");
    expect(terminalOverlay.getAttribute("data-application-overlay-layer")).toBe("active");
    expect(dock.notifications).toEqual([false, true]);

    coordinator.reconcile([change(terminalOverlay, false)]);
    expect(coordinator.activeOverlay).toBe(drawer);
    expect(dock.notifications).toEqual([false, true]);
    coordinator.reconcile([change(drawer, false)]);
    expect(dock.notifications).toEqual([false, true, false]);
    expect(dock.focusRequests).toEqual([false, false, false]);
    expect(dock.focus).not.toHaveBeenCalled();
  });

  it("ignores duplicate opening records without reordering the active layer", () => {
    const drawer = fakeOverlay(false);
    const dialog = fakeOverlay(false);
    const coordinator = new ApplicationOverlayCoordinator(() => [drawer, dialog]);
    coordinator.reconcile();
    expect(coordinator.activeOverlay).toBe(dialog);

    coordinator.reconcile([{ overlay: drawer, open: true }]);
    coordinator.reconcile();
    expect(coordinator.activeOverlay).toBe(dialog);
  });

  it("restores prior overlay, background, and late-dock state", () => {
    const overlay = fakeOverlay(true, {
      inert: false,
      ariaHidden: "false",
      layer: "legacy",
    });
    const background = fakeAttributeTarget({
      inert: true,
      ariaHidden: "false",
      layer: null,
    });
    const coordinator = new ApplicationOverlayCoordinator(
      () => [overlay],
      background,
    );
    coordinator.reconcile([change(overlay, true)]);

    const dock = fakeDock();
    coordinator.setDock(dock);
    expect(dock.notifications).toEqual([true]);
    expect(dock.paneVisible()).toBe(false);
    coordinator.reconcile([change(overlay, false)]);

    expect(overlay.inert).toBe(false);
    expect(overlay.getAttribute("aria-hidden")).toBe("false");
    expect(overlay.getAttribute("data-application-overlay-layer")).toBe("legacy");
    expect(background.inert).toBe(true);
    expect(background.getAttribute("aria-hidden")).toBe("false");
    expect(dock.notifications).toEqual([true, false]);
    expect(dock.focusRequests).toEqual([false, false]);
    expect(dock.focus).not.toHaveBeenCalled();
  });
});
