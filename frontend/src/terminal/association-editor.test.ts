import { describe, expect, it, vi } from "vitest";

import {
  commitTerminalAssociationMutation,
  terminalHasLinkedOrigin,
  type ActiveTerminalAssociation,
} from "./association-editor";

function state(): ActiveTerminalAssociation {
  return {
    generation: 7,
    tabId: "tab-linked",
    paneId: "pane-linked",
    sessionId: "opaque-session",
    revision: 3,
    pointer: { version: 1, planId: 2, taskId: 9 },
  };
}

describe("terminal association mutation", () => {
  it("retains linked restrictions after the persisted pointer is detached", () => {
    expect(terminalHasLinkedOrigin(
      { version: 1, planId: 2, taskId: 9 },
      false,
      false,
    )).toBe(true);
    expect(terminalHasLinkedOrigin(undefined, true, false)).toBe(true);
    expect(terminalHasLinkedOrigin(undefined, false, true)).toBe(true);
    expect(terminalHasLinkedOrigin(undefined, false, false)).toBe(false);
  });

  it("commits a validated relink only after backend success", async () => {
    let current = state();
    const persist = vi.fn();
    const create = vi.fn();
    const close = vi.fn();
    const socket = vi.fn();
    const next = await commitTerminalAssociationMutation({
      expected: state(),
      pointer: { version: 1, planId: 2 },
      current: () => current,
      mutate: async (sessionId, revision, pointer) => {
        expect({ sessionId, revision, pointer }).toEqual({
          sessionId: "opaque-session",
          revision: 3,
          pointer: { version: 1, planId: 2 },
        });
        return {
          generation: 7,
          sessionId,
          revision: 4,
          detached: false,
          pointer,
        };
      },
      commit: (committed) => {
        current = committed;
        persist(committed.pointer);
      },
    });
    expect(next).toEqual({
      ...state(),
      revision: 4,
      pointer: { version: 1, planId: 2 },
    });
    expect(persist).toHaveBeenCalledOnce();
    expect(create).not.toHaveBeenCalled();
    expect(close).not.toHaveBeenCalled();
    expect(socket).not.toHaveBeenCalled();
  });

  it("removes only the authority-free pointer after detach", async () => {
    let current = state();
    const persist = vi.fn();
    const next = await commitTerminalAssociationMutation({
      expected: state(),
      current: () => current,
      mutate: async (sessionId) => ({
        generation: 7,
        sessionId,
        revision: 4,
        detached: true,
      }),
      commit: (committed) => {
        current = committed;
        persist(committed.pointer);
      },
    });
    expect(next.pointer).toBeUndefined();
    expect(next.revision).toBe(4);
    expect(persist).toHaveBeenCalledWith(undefined);
  });

  it("retains the pointer on backend failure", async () => {
    const current = state();
    const persist = vi.fn();
    await expect(commitTerminalAssociationMutation({
      expected: state(),
      pointer: { version: 1, planId: 2 },
      current: () => current,
      mutate: async () => {
        throw new Error("stale revision");
      },
      commit: persist,
    })).rejects.toThrow("stale revision");
    expect(current.pointer).toEqual({ version: 1, planId: 2, taskId: 9 });
    expect(persist).not.toHaveBeenCalled();
  });

  it("discards results after a tab, session, revision, or generation change", async () => {
    const changes: Array<Partial<ActiveTerminalAssociation> | null> = [
      { tabId: "other-tab" },
      { sessionId: "other-session" },
      { revision: 4 },
      { generation: 8 },
      null,
    ];
    for (const change of changes) {
      let current: ActiveTerminalAssociation | null = state();
      const commit = vi.fn();
      await expect(commitTerminalAssociationMutation({
        expected: state(),
        pointer: { version: 1, planId: 2 },
        current: () => current,
        mutate: async () => {
          current = change === null ? null : { ...state(), ...change };
          return {
            generation: 7,
            sessionId: "opaque-session",
            revision: 4,
            detached: false,
            pointer: { version: 1, planId: 2 },
          };
        },
        commit,
      })).rejects.toThrow("Stale terminal association response ignored");
      expect(commit).not.toHaveBeenCalled();
    }
  });

  it("rejects mismatched response targets and revisions", async () => {
    const current = state();
    const commit = vi.fn();
    await expect(commitTerminalAssociationMutation({
      expected: state(),
      pointer: { version: 1, planId: 2 },
      current: () => current,
      mutate: async () => ({
        generation: 7,
        sessionId: "opaque-session",
        revision: 5,
        detached: false,
        pointer: { version: 1, planId: 2, taskId: 9 },
      }),
      commit,
    })).rejects.toThrow("Stale or invalid");
    expect(commit).not.toHaveBeenCalled();
  });
});
