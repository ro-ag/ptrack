import { describe, expect, it, vi } from "vitest";

import {
  completeLinkedLaunchTransaction,
  installedAgentProfiles,
  LinkedLaunchPersistenceStage,
  linkedAssociationPointer,
  persistUnlessLinkedLaunchStaged,
  selectedInstalledAgentProfile,
} from "./linked-launch";
import {
  createWorkspace,
  type IdFactory,
  type WorkspaceIdKind,
} from "../workspace/model";
import {
  loadTerminalWorkspace,
  saveTerminalWorkspace,
  terminalWorkspaceStorageKey,
  type StorageLike,
} from "../workspace/persistence";
import { reduceWorkspace } from "../workspace/reducer";
import { WorkspaceController } from "../workspace/controller";

function workspaceIDs(): IdFactory {
  let next = 0;
  return { next: (kind: WorkspaceIdKind) => `${kind}-${++next}` };
}

class LinkedLaunchMemoryStorage implements StorageLike {
  readonly values = new Map<string, string>();
  readonly writes: string[] = [];
  getItem(key: string) { return this.values.get(key) ?? null; }
  setItem(key: string, value: string) {
    this.values.set(key, value);
    this.writes.push(value);
  }
  removeItem(key: string) { this.values.delete(key); }
}

describe("linked agent launch selection", () => {
  it("exposes only discovered agent profiles and selects the exact ID", () => {
    const profiles = installedAgentProfiles([
      { id: "shell-default", name: "Shell", kind: "shell" },
      { id: "agent-alpha", name: "Alpha", kind: "agent" },
      { id: "agent-beta", name: "Beta", kind: "agent" },
    ]);
    expect(profiles.map((profile) => profile.id)).toEqual([
      "agent-alpha",
      "agent-beta",
    ]);
    expect(selectedInstalledAgentProfile(profiles, "agent-beta").name).toBe("Beta");
    expect(() => selectedInstalledAgentProfile(profiles, "agent")).toThrow(
      "Select an installed agent profile",
    );
    expect(installedAgentProfiles([
      { id: "shell-default", name: "Shell", kind: "shell" },
    ])).toEqual([]);
  });

  it("builds only versioned plan and task pointers", () => {
    expect(linkedAssociationPointer(2)).toEqual({ version: 1, planId: 2 });
    expect(linkedAssociationPointer(2, 9)).toEqual({
      version: 1,
      planId: 2,
      taskId: 9,
    });
    expect(() => linkedAssociationPointer(0)).toThrow();
    expect(() => linkedAssociationPointer(2, Number.NaN)).toThrow();
  });
});

describe("completeLinkedLaunchTransaction", () => {
  it("uses the exact launched session and keeps a successful linked tab", async () => {
    const session = { sessionId: "session-beta", profileId: "agent-beta" };
    const tab = { tabId: "tab-linked" };
    const attach = vi.fn(async () => {});
    const closeSession = vi.fn(async () => {});
    const rollbackTab = vi.fn();
    await expect(completeLinkedLaunchTransaction({
      launch: async () => session,
      createTab: (created) => created === session ? tab : null,
      attach,
      closeSession,
      rollbackTab,
    })).resolves.toEqual({ session, tab });
    expect(attach).toHaveBeenCalledWith(tab, session);
    expect(closeSession).not.toHaveBeenCalled();
    expect(rollbackTab).not.toHaveBeenCalled();
  });

  it("force-closes a backend session when tab creation fails", async () => {
    const session = { sessionId: "session-without-tab" };
    const closeSession = vi.fn(async () => {});
    const rollbackTab = vi.fn();
    await expect(completeLinkedLaunchTransaction({
      launch: async () => session,
      createTab: () => null,
      attach: vi.fn(),
      closeSession,
      rollbackTab,
    })).rejects.toThrow("Could not create a linked terminal tab");
    expect(closeSession).toHaveBeenCalledWith(session);
    expect(rollbackTab).not.toHaveBeenCalled();
  });

  it("closes the session and rolls back a tab when renderer attachment fails", async () => {
    const session = { sessionId: "session-failed-attach" };
    const tab = { tabId: "tab-failed-attach" };
    const order: string[] = [];
    await expect(completeLinkedLaunchTransaction({
      launch: async () => session,
      createTab: () => tab,
      attach: async () => {
        throw new Error("renderer failed");
      },
      closeSession: async () => {
        order.push("close");
      },
      rollbackTab: () => {
        order.push("rollback");
      },
    })).rejects.toThrow("renderer failed");
    expect(order).toEqual(["close", "rollback"]);
  });

  it("retains the staged tab when backend cleanup fails", async () => {
    const session = { sessionId: "session-cleanup-failed" };
    const tab = { tabId: "tab-cleanup-failed" };
    const rollbackTab = vi.fn();
    await expect(completeLinkedLaunchTransaction({
      launch: async () => session,
      createTab: () => tab,
      attach: async () => {
        throw new Error("renderer failed");
      },
      closeSession: async () => {
        throw new Error("close failed");
      },
      rollbackTab,
    })).rejects.toThrow("cleanup failed");
    expect(rollbackTab).not.toHaveBeenCalled();
  });

  it("rolls back a staged old-project launch after project disposal without persisting authority", async () => {
    const storage = new LinkedLaunchMemoryStorage();
    const ids = workspaceIDs();
    let alpha = createWorkspace(ids, {
      title: "Alpha shell",
      profileId: "shell-default",
      cwd: "/alpha",
    });
    const beta = createWorkspace(ids, {
      title: "Beta shell",
      profileId: "shell-default",
      cwd: "/beta",
    });
    expect(saveTerminalWorkspace(storage, "/alpha", alpha, 0.3)).toBe(true);
    expect(saveTerminalWorkspace(storage, "/beta", beta, 0.4)).toBe(true);

    const workspace = new WorkspaceController();
    workspace.publish({ status: "open", generation: 1 });
    const launchTicket = workspace.capture();
    const session = {
      generation: 1,
      sessionId: "alpha-live-session",
      profileId: "agent-beta",
      cwd: "/alpha",
      streamUrl: "ws://alpha?token=STREAM_AUTHORITY_CANARY",
      context: "LAUNCH_CONTEXT_CANARY",
      environment: { PTRACK_CAPABILITY_TOKEN: "CAPABILITY_AUTHORITY_CANARY" },
    };
    let stagedTabId = "";
    let disposed = false;
    const persistenceStage = new LinkedLaunchPersistenceStage();
    let releasePersistenceStage: (() => void) | null = null;
    let releaseAttach = () => {};
    const attachBlocked = new Promise<void>((resolve) => { releaseAttach = resolve; });
    let notifyAttachStarted = () => {};
    const attachStarted = new Promise<void>((resolve) => { notifyAttachStarted = resolve; });
    const closeSession = vi.fn(async () => {});

    const operation = completeLinkedLaunchTransaction({
      launch: async () => session,
      createTab: () => {
        releasePersistenceStage = persistenceStage.begin(() => {
          saveTerminalWorkspace(storage, "/alpha", alpha, 0.3);
        });
        const prior = new Set(alpha.tabs.map((tab) => tab.id));
        alpha = reduceWorkspace(alpha, {
          type: "create-tab",
          title: "Linked alpha agent",
          profileId: "agent-beta",
          cwd: "/alpha",
          association: { version: 1, planId: 1, taskId: 1 },
        }, ids);
        stagedTabId = alpha.tabs.find((tab) => !prior.has(tab.id))?.id ?? "";
        // Exercise the exact callback used by TerminalDock's persistence
        // scheduler while the linked tab is tentative.
        persistUnlessLinkedLaunchStaged(persistenceStage, () => {
          saveTerminalWorkspace(storage, "/alpha", alpha, 0.3);
        });
        return stagedTabId === "" ? null : stagedTabId;
      },
      attach: async () => {
        notifyAttachStarted();
        await attachBlocked;
        if (disposed || !workspace.accepts(launchTicket, session.generation)) {
          throw new Error("old project terminal dock was disposed");
        }
      },
      closeSession,
      rollbackTab: (tabId) => {
        alpha = reduceWorkspace(alpha, { type: "close-tab", tabId }, ids);
      },
    });
    const transaction = operation.finally(() => {
      releasePersistenceStage?.();
      persistUnlessLinkedLaunchStaged(persistenceStage, () => {
        saveTerminalWorkspace(storage, "/alpha", alpha, 0.3);
      });
    });

    await attachStarted;
    const transition = workspace.beginTransition();
    disposed = true;
    expect(workspace.publish({ status: "open", generation: 2 }, transition)).toBe(true);
    releaseAttach();
    await expect(transaction).rejects.toThrow("disposed");
    expect(closeSession).toHaveBeenCalledOnce();
    expect(closeSession).toHaveBeenCalledWith(session);
    expect(persistenceStage.suppressed).toBe(false);

    expect(loadTerminalWorkspace(storage, "/alpha").workspace).toEqual(
      expect.objectContaining({
        tabs: [expect.objectContaining({ title: "Alpha shell" })],
      }),
    );
    expect(loadTerminalWorkspace(storage, "/beta").workspace).toEqual(beta);
    const persisted = [
      ...storage.writes,
      storage.getItem(terminalWorkspaceStorageKey("/alpha")),
      storage.getItem(terminalWorkspaceStorageKey("/beta")),
    ].join("\n");
    for (const forbidden of [
      stagedTabId,
      session.sessionId,
      "STREAM_AUTHORITY_CANARY",
      "LAUNCH_CONTEXT_CANARY",
      "CAPABILITY_AUTHORITY_CANARY",
      "PTRACK_CAPABILITY_TOKEN",
    ]) expect(persisted).not.toContain(forbidden);
  });
});
