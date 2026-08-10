import { describe, expect, it, vi } from "vitest";

import { createWorkspace, type IdFactory, type WorkspaceIdKind } from "./model";
import { createCryptoIdFactory, WorkspaceTabController } from "./tab-controller";

function sequentialIds(): IdFactory {
  let sequence = 0;
  return { next: (kind: WorkspaceIdKind) => `${kind}-${++sequence}` };
}

describe("WorkspaceTabController", () => {
  it("owns state and notifies subscribers only when dispatch changes identity", () => {
    const controller = new WorkspaceTabController(sequentialIds());
    const listener = vi.fn();
    controller.subscribe(listener);
    const initial = controller.workspace;

    expect(controller.dispatch({ type: "select-tab", tabId: initial.activeTabId })).toBeNull();
    expect(listener).not.toHaveBeenCalled();

    const next = controller.dispatch({ type: "create-tab", title: "Second" });
    expect(next).toBe(controller.workspace);
    expect(next?.tabs).toHaveLength(2);
    expect(listener).toHaveBeenCalledOnce();
    expect(listener).toHaveBeenCalledWith(next, initial);
  });

  it("supports idempotent unsubscribe and dispose", () => {
    const controller = new WorkspaceTabController(sequentialIds());
    const first = vi.fn();
    const second = vi.fn();
    const unsubscribe = controller.subscribe(first);
    controller.subscribe(second);

    unsubscribe();
    unsubscribe();
    controller.dispatch({ type: "create-tab" });
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledOnce();

    const beforeDispose = controller.workspace;
    controller.dispose();
    controller.dispose();
    expect(controller.dispatch({ type: "create-tab" })).toBeNull();
    expect(controller.workspace).toBe(beforeDispose);
    expect(second).toHaveBeenCalledOnce();
    expect(controller.subscribe(first)).toEqual(expect.any(Function));
  });

  it("prefixes production ids by kind while sourcing entropy from crypto", () => {
    const randomUUID = vi
      .fn<() => `${string}-${string}-${string}-${string}-${string}`>()
      .mockReturnValueOnce("00000000-0000-4000-8000-000000000001")
      .mockReturnValueOnce("00000000-0000-4000-8000-000000000002");
    const ids = createCryptoIdFactory({ randomUUID });

    expect(ids.next("tab")).toBe("tab-00000000-0000-4000-8000-000000000001");
    expect(ids.next("pane")).toBe("pane-00000000-0000-4000-8000-000000000002");
    expect(randomUUID).toHaveBeenCalledTimes(2);
  });

  it("supports intent defaults and deny-before-reduce guards", () => {
    const allowAction = vi.fn(() => true);
    const controller = new WorkspaceTabController(sequentialIds(), undefined, {
      interceptAction(action) {
        return action.type === "create-tab"
          ? { ...action, profileId: "shell", cwd: "" }
          : action;
      },
      allowAction,
    });
    const created = controller.dispatch({ type: "create-tab" });
    expect(created?.tabs[1].root).toMatchObject({ profileId: "shell", cwd: "" });
    expect(allowAction).toHaveBeenCalledOnce();

    const denied = new WorkspaceTabController(sequentialIds(), undefined, {
      allowAction: () => false,
    });
    expect(denied.canDispatch({ type: "create-tab" })).toBe(false);
    expect(denied.dispatch({ type: "create-tab" })).toBeNull();
    expect(denied.workspace.tabs).toHaveLength(1);
  });

  it("atomically replaces valid state and rejects invalid replacements", () => {
    const controller = new WorkspaceTabController(sequentialIds());
    const previous = controller.workspace;
    const replacement = createWorkspace(sequentialIds(), {
      profileId: "shell",
      cwd: "/repo",
    });
    const listener = vi.fn();
    controller.subscribe(listener);
    expect(controller.replace(replacement)).toBe(replacement);
    expect(listener).toHaveBeenCalledWith(replacement, previous);
    expect(controller.replace(replacement)).toBeNull();
    expect(controller.replace({ ...replacement, activeTabId: "missing" })).toBeNull();
    expect(listener).toHaveBeenCalledOnce();
  });
});
