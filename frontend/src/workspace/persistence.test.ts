import { describe, expect, it, vi } from "vitest";

import {
  createWorkspace,
  maximumCwdLength,
  maximumProfileIdLength,
  maximumWorkspaceIdLength,
  type IdFactory,
  type WorkspaceIdKind,
} from "./model";
import {
  clearTerminalWorkspace,
  clearTerminalWorkspaceAfterReplace,
  loadTerminalWorkspace,
  maximumTerminalWorkspaceBytes,
  saveTerminalWorkspace,
  repairWorkspaceDescriptors,
  savedWorkspaceCwds,
  serializeTerminalWorkspace,
  terminalWorkspaceStorageKey,
  type StorageLike,
  WorkspacePersistenceScheduler,
} from "./persistence";
import { reduceWorkspace } from "./reducer";

function ids(): IdFactory {
  let next = 0;
  return { next: (kind: WorkspaceIdKind) => `${kind}-${++next}` };
}

class MemoryStorage implements StorageLike {
  readonly values = new Map<string, string>();
  getItem(key: string) { return this.values.get(key) ?? null; }
  setItem(key: string, value: string) { this.values.set(key, value); }
  removeItem(key: string) { this.values.delete(key); }
}

class FakeTimerClock {
  now = 0;
  nextId = 0;
  readonly timers = new Map<number, { at: number; callback: () => void }>();

  setTimeout(callback: () => void, delay: number): number {
    const id = ++this.nextId;
    this.timers.set(id, { at: this.now + delay, callback });
    return id;
  }

  clearTimeout(handle: unknown): void {
    this.timers.delete(handle as number);
  }

  advance(milliseconds: number): void {
    const target = this.now + milliseconds;
    while (true) {
      const next = [...this.timers.entries()]
        .filter(([, timer]) => timer.at <= target)
        .sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
      if (!next) break;
      this.now = next[1].at;
      this.timers.delete(next[0]);
      next[1].callback();
    }
    this.now = target;
  }
}

describe("WorkspacePersistenceScheduler", () => {
  it("debounces mutations and enforces the unchanged maximum wait", () => {
    const clock = new FakeTimerClock();
    const write = vi.fn();
    const scheduler = new WorkspacePersistenceScheduler(clock, write);
    scheduler.markDirty();
    clock.advance(249);
    expect(write).not.toHaveBeenCalled();
    clock.advance(1);
    expect(write).toHaveBeenCalledOnce();

    for (let index = 0; index < 10; index += 1) {
      scheduler.markDirty();
      clock.advance(200);
    }
    expect(write).toHaveBeenCalledTimes(2);
  });

  it("flushes synchronously for pagehide-style flush and disposal", () => {
    const clock = new FakeTimerClock();
    const write = vi.fn();
    const scheduler = new WorkspacePersistenceScheduler(clock, write);
    scheduler.markDirty();
    expect(scheduler.flush()).toBe(true);
    expect(write).toHaveBeenCalledOnce();
    scheduler.markDirty();
    scheduler.dispose();
    expect(write).toHaveBeenCalledTimes(2);
    clock.advance(10_000);
    scheduler.markDirty();
    expect(write).toHaveBeenCalledTimes(2);
  });
});

describe("terminal workspace persistence", () => {
  it("uses a project key and serializes only descriptor allowlist fields", () => {
    const workspace = createWorkspace(ids(), { profileId: "shell", cwd: "/repo" });
    Object.assign(workspace as unknown as Record<string, unknown>, {
      sessionId: "runtime-session",
      activity: { unread: true },
    });
    Object.assign(workspace.tabs[0].root as unknown as Record<string, unknown>, {
      output: "secret terminal bytes",
      resources: { socket: true },
    });
    const raw = serializeTerminalWorkspace(workspace, 0.42)!;
    expect(terminalWorkspaceStorageKey("/tmp/a b")).toBe(
      "ptrack.terminal-workspace:%2Ftmp%2Fa%20b",
    );
    const parsed = JSON.parse(raw);
    expect(parsed.workspace).toEqual({
      version: workspace.version,
      activeTabId: workspace.activeTabId,
      tabs: workspace.tabs.map((tab) => ({
        id: tab.id,
        title: tab.title,
        activePaneId: tab.activePaneId,
        root: {
          kind: "terminal",
          paneId: "pane-2",
          profileId: "shell",
          cwd: "/repo",
        },
      })),
    });
    for (const forbidden of [
      "sessionId", "streamUrl", "token", "activity", "output", "resources", "epoch",
    ]) expect(raw).not.toContain(forbidden);
  });

  // Pins the descriptor allowlist: a widening of cloneWorkspaceForPersistence
  // would carry one of these fields through a save and reload.
  it("round-trips a leaky workspace without the fields outside the allowlist", () => {
    const storage = new MemoryStorage();
    const workspace = createWorkspace(ids(), { profileId: "shell", cwd: "/repo" });
    Object.assign(workspace as unknown as Record<string, unknown>, {
      secret: "sk-live-workspace",
      sessionId: "session-1",
    });
    Object.assign(workspace.tabs[0] as unknown as Record<string, unknown>, {
      token: "gh-token-tab",
      env: { AWS_SECRET_ACCESS_KEY: "leaked" },
      buffer: "scrollback bytes the user typed",
    });
    Object.assign(workspace.tabs[0].root as unknown as Record<string, unknown>, {
      sessionId: "session-2",
      env: { OPENAI_API_KEY: "leaked" },
      buffer: "pane scrollback",
    });

    expect(saveTerminalWorkspace(storage, "/repo", workspace, 0.3)).toBe(true);
    const loaded = loadTerminalWorkspace(storage, "/repo").workspace!;
    expect(Object.keys(loaded).sort()).toEqual(["activeTabId", "tabs", "version"]);
    expect(Object.keys(loaded.tabs[0]).sort()).toEqual([
      "activePaneId", "id", "root", "title",
    ]);
    expect(Object.keys(loaded.tabs[0].root).sort()).toEqual([
      "cwd", "kind", "paneId", "profileId",
    ]);
    const raw = storage.getItem(terminalWorkspaceStorageKey("/repo"))!;
    for (const forbidden of [
      "secret", "sk-live-workspace", "sessionId", "session-1", "token", "gh-token-tab",
      "env", "AWS_SECRET_ACCESS_KEY", "OPENAI_API_KEY", "buffer", "scrollback",
    ]) expect(raw).not.toContain(forbidden);
  });

  it("keeps project roots isolated in storage", () => {
    const storage = new MemoryStorage();
    const first = createWorkspace(ids(), { title: "Alpha", profileId: "shell" });
    const second = createWorkspace(ids(), { title: "Beta", profileId: "agent" });
    expect(saveTerminalWorkspace(storage, "/alpha", first, 0.25)).toBe(true);
    expect(saveTerminalWorkspace(storage, "/beta", second, 0.65)).toBe(true);
    expect(loadTerminalWorkspace(storage, "/alpha")).toMatchObject({
      workspace: first,
      dockRatio: 0.25,
    });
    expect(loadTerminalWorkspace(storage, "/beta")).toMatchObject({
      workspace: second,
      dockRatio: 0.65,
    });
  });

  it("migrates existing v1 data and persists only a plan/task pointer", () => {
    const storage = new MemoryStorage();
    const legacy = createWorkspace(ids(), { profileId: "shell", cwd: "/repo" });
    const key = terminalWorkspaceStorageKey("/repo");
    storage.setItem(key, JSON.stringify({ version: 1, workspace: legacy, dockRatio: 0.3 }));
    expect(loadTerminalWorkspace(storage, "/repo")).toMatchObject({
      workspace: legacy,
      invalidReason: null,
    });

    legacy.tabs[0].association = { version: 1, planId: 2, taskId: 9 };
    Object.assign(legacy.tabs[0].association as unknown as Record<string, unknown>, {
      generation: 7,
      sessionId: "runtime-session",
      token: "secret",
      environment: { SECRET: "value" },
      context: "hidden",
      output: "terminal bytes",
      authority: "network",
    });
    const raw = serializeTerminalWorkspace(legacy, 0.3)!;
    expect(JSON.parse(raw).workspace.tabs[0].association).toEqual({
      version: 1,
      planId: 2,
      taskId: 9,
    });
    for (const forbidden of [
      "generation", "sessionId", "token", "environment", "context", "output", "authority",
    ]) expect(raw).not.toContain(forbidden);
  });

  it("round trips valid state and treats storage exceptions as nonfatal", () => {
    const storage = new MemoryStorage();
    const workspace = createWorkspace(ids(), { profileId: "shell", cwd: "/repo" });
    expect(saveTerminalWorkspace(storage, "/repo", workspace, 0.4)).toBe(true);
    expect(loadTerminalWorkspace(storage, "/repo")).toMatchObject({
      workspace, dockRatio: 0.4, invalidReason: null,
    });
    const broken = {
      getItem: () => { throw new Error("blocked"); },
      setItem: () => { throw new Error("blocked"); },
      removeItem: () => { throw new Error("blocked"); },
    };
    expect(loadTerminalWorkspace(broken, "/repo").workspace).toBeNull();
    expect(saveTerminalWorkspace(broken, "/repo", workspace, 0.4)).toBe(false);
    expect(() => clearTerminalWorkspace(broken, "/repo")).not.toThrow();
  });

  it("round trips recursive ratios and the exact active pane", () => {
    const storage = new MemoryStorage();
    const factory = ids();
    let workspace = createWorkspace(factory, { profileId: "shell", cwd: "/repo" });
    const tabId = workspace.activeTabId;
    const first = workspace.tabs[0].activePaneId;
    workspace = reduceWorkspace(workspace, {
      type: "split-pane",
      tabId,
      paneId: first,
      direction: "horizontal",
      ratio: 0.37,
    }, factory);
    const second = workspace.tabs[0].activePaneId;
    workspace = reduceWorkspace(workspace, {
      type: "split-pane",
      tabId,
      paneId: second,
      direction: "vertical",
      ratio: 0.63,
    }, factory);
    workspace = reduceWorkspace(
      workspace,
      { type: "focus-pane", tabId, paneId: second },
      factory,
    );
    expect(saveTerminalWorkspace(storage, "/repo", workspace, 0.42)).toBe(true);
    expect(loadTerminalWorkspace(storage, "/repo")).toMatchObject({
      workspace,
      dockRatio: 0.42,
    });
  });

  it("clears project storage only after a successful controller replacement", () => {
    const storage = new MemoryStorage();
    const workspace = createWorkspace(ids());
    saveTerminalWorkspace(storage, "/repo", workspace, 0.3);
    const key = terminalWorkspaceStorageKey("/repo");
    expect(clearTerminalWorkspaceAfterReplace(storage, "/repo", null)).toBe(false);
    expect(storage.getItem(key)).not.toBeNull();
    expect(clearTerminalWorkspaceAfterReplace(storage, "/repo", workspace)).toBe(true);
    expect(storage.getItem(key)).toBeNull();
  });

  it.each([
    ["malformed", "{"],
    ["future", JSON.stringify({ version: 2, workspace: {}, dockRatio: 0.3 })],
    ["unknown", JSON.stringify({ version: 1, workspace: {}, dockRatio: 0.3, x: 1 })],
  ])("quarantines %s data with metadata only", (_name, raw) => {
    const storage = new MemoryStorage();
    const key = terminalWorkspaceStorageKey("/repo");
    storage.setItem(key, raw);
    const warn = vi.fn();
    expect(loadTerminalWorkspace(storage, "/repo", warn).workspace).toBeNull();
    expect(storage.getItem(key)).toBeNull();
    const metadata = JSON.parse(storage.getItem(`${key}:invalid`)!);
    expect(Object.keys(metadata).sort()).toEqual(["at", "bytes", "reason"]);
    expect(metadata).not.toHaveProperty("raw");
    expect(warn).toHaveBeenCalledOnce();
  });

  it("never retains attacker-controlled field names in quarantine metadata", () => {
    const storage = new MemoryStorage();
    const workspace = createWorkspace(ids());
    const secret = "SECRET_PROPERTY_NAME_CANARY";
    Object.assign(workspace.tabs[0].root as unknown as Record<string, unknown>, {
      [secret]: true,
    });
    const key = terminalWorkspaceStorageKey("/repo");
    storage.setItem(key, JSON.stringify({ version: 1, workspace, dockRatio: 0.3 }));
    const warnings: string[] = [];
    const result = loadTerminalWorkspace(storage, "/repo", (warning) => {
      warnings.push(warning);
    });
    const retained = JSON.stringify({
      result,
      warnings,
      quarantine: storage.getItem(`${key}:invalid`),
    });
    expect(retained).not.toContain(secret);
    expect(result.invalidReason).toBe("tabs[0].root contains unsupported fields");
  });

  it("rejects oversized, duplicate, unknown, and out-of-bound data", () => {
    const workspace = createWorkspace(ids(), { profileId: "shell", cwd: "/repo" });
    const invalid = [
      "x".repeat(maximumTerminalWorkspaceBytes + 1),
      JSON.stringify({ version: 1, dockRatio: 0.9, workspace }),
      JSON.stringify({ version: 1, dockRatio: 0.3, workspace: { ...workspace, x: 1 } }),
      JSON.stringify({ version: 1, dockRatio: 0.3, workspace: {
        ...workspace,
        tabs: [workspace.tabs[0], { ...workspace.tabs[0] }],
      } }),
    ];
    for (const raw of invalid) {
      const storage = new MemoryStorage();
      storage.setItem(terminalWorkspaceStorageKey("/repo"), raw);
      expect(loadTerminalWorkspace(storage, "/repo").workspace).toBeNull();
    }
  });

  it("enforces id, profile, cwd, and raw-size bounds", () => {
    const workspace = createWorkspace(ids());
    const pane = workspace.tabs[0].root;
    if (pane.kind !== "terminal") throw new Error("expected terminal");
    const invalid = [
      { ...workspace, activeTabId: "x".repeat(maximumWorkspaceIdLength + 1) },
      { ...workspace, tabs: [{ ...workspace.tabs[0], root: {
        ...pane, profileId: "x".repeat(maximumProfileIdLength + 1),
      } }] },
      { ...workspace, tabs: [{ ...workspace.tabs[0], root: {
        ...pane, cwd: "x".repeat(maximumCwdLength + 1),
      } }] },
    ];
    for (const value of invalid) expect(serializeTerminalWorkspace(value, 0.3)).toBeNull();
  });

  it("repairs profiles and explicit CWDs atomically while preserving blank roots", () => {
    const workspace = createWorkspace(ids(), { profileId: "missing", cwd: "/old" });
    expect(savedWorkspaceCwds(workspace)).toEqual(["/old"]);
    const repair = repairWorkspaceDescriptors(
      workspace,
      new Set(["shell"]),
      "shell",
      [{ requested: "/old", cwd: "/canonical", valid: true }],
    );
    expect(repair).toMatchObject({ repairedProfiles: 1, repairedCwds: 1 });
    expect(repair.workspace.tabs[0].root).toMatchObject({
      profileId: "shell", cwd: "/canonical",
    });
    expect(workspace.tabs[0].root).toMatchObject({
      profileId: "missing", cwd: "/old",
    });
    expect(repairWorkspaceDescriptors(
      workspace,
      new Set(["shell"]),
      "shell",
      [{ requested: "/old", cwd: "", valid: false }],
    ).workspace.tabs[0].root).toMatchObject({ profileId: "shell", cwd: "" });
    expect(repairWorkspaceDescriptors(
      workspace,
      new Set(["shell"]),
      "shell",
      null,
    ).workspace.tabs[0].root).toMatchObject({ profileId: "shell", cwd: "/old" });

    const blank = createWorkspace(ids(), { profileId: "shell", cwd: "" });
    expect(savedWorkspaceCwds(blank)).toEqual([]);
    expect(repairWorkspaceDescriptors(blank, new Set(["shell"]), "shell", []).workspace)
      .toBe(blank);
  });
});
