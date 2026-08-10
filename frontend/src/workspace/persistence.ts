import {
  validateWorkspace,
  type PaneNode,
  type Workspace,
  type WorkspaceTab,
} from "./model";

export const terminalWorkspaceStoragePrefix = "ptrack.terminal-workspace:";
export const maximumTerminalWorkspaceBytes = 64 * 1024;
export const minimumDockRatio = 0.1;
export const maximumDockRatio = 0.75;
export const defaultDockRatio = 0.3;

export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

export interface TerminalWorkspaceLoadResult {
  workspace: Workspace | null;
  dockRatio: number;
  invalidReason: string | null;
}

export interface TerminalCWDValidation {
  requested: string;
  cwd: string;
  valid: boolean;
}

export interface WorkspaceDescriptorRepair {
  workspace: Workspace;
  repairedProfiles: number;
  repairedCwds: number;
}

export interface PersistenceTimerClock {
  setTimeout(callback: () => void, delayMilliseconds: number): unknown;
  clearTimeout(handle: unknown): void;
}

export class WorkspacePersistenceScheduler {
  readonly #clock: PersistenceTimerClock;
  readonly #write: () => void;
  readonly #debounceMilliseconds: number;
  readonly #maximumWaitMilliseconds: number;
  #debounceTimer: unknown = null;
  #maximumTimer: unknown = null;
  #dirty = false;
  #disposed = false;

  constructor(
    clock: PersistenceTimerClock,
    write: () => void,
    debounceMilliseconds = 250,
    maximumWaitMilliseconds = 2_000,
  ) {
    this.#clock = clock;
    this.#write = write;
    this.#debounceMilliseconds = debounceMilliseconds;
    this.#maximumWaitMilliseconds = maximumWaitMilliseconds;
  }

  markDirty(): void {
    if (this.#disposed) return;
    this.#dirty = true;
    if (this.#debounceTimer !== null) this.#clock.clearTimeout(this.#debounceTimer);
    this.#debounceTimer = this.#clock.setTimeout(() => {
      this.#debounceTimer = null;
      this.flush();
    }, this.#debounceMilliseconds);
    if (this.#maximumTimer === null) {
      this.#maximumTimer = this.#clock.setTimeout(() => {
        this.#maximumTimer = null;
        this.flush();
      }, this.#maximumWaitMilliseconds);
    }
  }

  flush(): boolean {
    if (this.#debounceTimer !== null) {
      this.#clock.clearTimeout(this.#debounceTimer);
      this.#debounceTimer = null;
    }
    if (this.#maximumTimer !== null) {
      this.#clock.clearTimeout(this.#maximumTimer);
      this.#maximumTimer = null;
    }
    if (!this.#dirty) return false;
    this.#dirty = false;
    this.#write();
    return true;
  }

  dispose(): void {
    if (this.#disposed) return;
    this.flush();
    this.#disposed = true;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function exactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  return actual.length === expected.length &&
    actual.every((key, index) => key === expected[index]);
}

function rawBytes(value: string): number {
  return new TextEncoder().encode(value).byteLength;
}

function cloneNode(node: PaneNode): PaneNode {
  if (node.kind === "terminal") {
    return {
      kind: "terminal",
      paneId: node.paneId,
      profileId: node.profileId,
      cwd: node.cwd,
    };
  }
  return {
    kind: "split",
    splitId: node.splitId,
    direction: node.direction,
    ratio: node.ratio,
    first: cloneNode(node.first),
    second: cloneNode(node.second),
  };
}

export function terminalWorkspaceStorageKey(projectRoot: string): string {
  return `${terminalWorkspaceStoragePrefix}${encodeURIComponent(projectRoot)}`;
}

export function cloneWorkspaceForPersistence(workspace: Workspace): Workspace {
  return {
    version: 1,
    activeTabId: workspace.activeTabId,
    tabs: workspace.tabs.map((tab: WorkspaceTab) => ({
      id: tab.id,
      title: tab.title,
      activePaneId: tab.activePaneId,
      root: cloneNode(tab.root),
    })),
  };
}

export function savedWorkspaceCwds(workspace: Workspace): string[] {
  const result = new Set<string>();
  const visit = (node: PaneNode): void => {
    if (node.kind === "terminal") {
      if (node.cwd !== "") result.add(node.cwd);
      return;
    }
    visit(node.first);
    visit(node.second);
  };
  for (const tab of workspace.tabs) visit(tab.root);
  return [...result];
}

export function repairWorkspaceDescriptors(
  workspace: Workspace,
  validProfileIds: ReadonlySet<string>,
  defaultProfileId: string,
  cwdValidations: readonly TerminalCWDValidation[] | null,
): WorkspaceDescriptorRepair {
  const cwdByRequest = cwdValidations === null
    ? null
    : new Map(cwdValidations.map((item) => [item.requested, item]));
  let repairedProfiles = 0;
  let repairedCwds = 0;
  const repairNode = (node: PaneNode): PaneNode => {
    if (node.kind === "split") {
      return { ...node, first: repairNode(node.first), second: repairNode(node.second) };
    }
    let profileId = node.profileId;
    let cwd = node.cwd;
    if (!validProfileIds.has(profileId)) {
      profileId = defaultProfileId;
      repairedProfiles += 1;
    }
    if (cwd !== "" && cwdByRequest !== null) {
      const validation = cwdByRequest.get(cwd);
      const repaired = validation?.valid ? validation.cwd : "";
      if (repaired !== cwd) repairedCwds += 1;
      cwd = repaired;
    }
    return { kind: "terminal", paneId: node.paneId, profileId, cwd };
  };
  if (workspace.tabs.length === 0) {
    return { workspace, repairedProfiles, repairedCwds };
  }
  const repairedWorkspace: Workspace = {
    version: 1,
    activeTabId: workspace.activeTabId,
    tabs: workspace.tabs.map((tab) => ({ ...tab, root: repairNode(tab.root) })),
  };
  return {
    workspace: repairedProfiles === 0 && repairedCwds === 0
      ? workspace
      : repairedWorkspace,
    repairedProfiles,
    repairedCwds,
  };
}

export function normalizeDockRatio(ratio: number): number {
  if (!Number.isFinite(ratio)) return defaultDockRatio;
  return Math.round(Math.max(minimumDockRatio, Math.min(maximumDockRatio, ratio)) * 1_000) /
    1_000;
}

export function serializeTerminalWorkspace(
  workspace: Workspace,
  dockRatio: number,
): string | null {
  const clone = cloneWorkspaceForPersistence(workspace);
  if (!validateWorkspace(clone).valid) return null;
  const raw = JSON.stringify({
    version: 1,
    workspace: clone,
    dockRatio: normalizeDockRatio(dockRatio),
  });
  return rawBytes(raw) <= maximumTerminalWorkspaceBytes ? raw : null;
}

function invalidResult(
  storage: StorageLike,
  key: string,
  reason: string,
  bytes: number,
  warn: (message: string) => void,
): TerminalWorkspaceLoadResult {
  const boundedReason = reason.slice(0, 160);
  try {
    storage.removeItem(key);
    storage.setItem(`${key}:invalid`, JSON.stringify({
      at: Date.now(),
      reason: boundedReason,
      bytes: Math.max(0, Math.trunc(bytes)),
    }));
  } catch {
    // Persistence is optional; a stopped in-memory default remains safe.
  }
  warn(`Saved terminal workspace was ignored: ${boundedReason}`);
  return { workspace: null, dockRatio: defaultDockRatio, invalidReason: boundedReason };
}

export function loadTerminalWorkspace(
  storage: StorageLike,
  projectRoot: string,
  warn: (message: string) => void = () => {},
): TerminalWorkspaceLoadResult {
  const key = terminalWorkspaceStorageKey(projectRoot);
  let raw: string | null;
  try {
    raw = storage.getItem(key);
  } catch {
    return { workspace: null, dockRatio: defaultDockRatio, invalidReason: null };
  }
  if (raw === null) {
    return { workspace: null, dockRatio: defaultDockRatio, invalidReason: null };
  }
  const bytes = rawBytes(raw);
  if (bytes > maximumTerminalWorkspaceBytes) {
    return invalidResult(storage, key, "saved data exceeds 64 KiB", bytes, warn);
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return invalidResult(storage, key, "saved data is malformed JSON", bytes, warn);
  }
  if (!isRecord(parsed) || !exactKeys(parsed, ["version", "workspace", "dockRatio"])) {
    return invalidResult(storage, key, "envelope fields are invalid", bytes, warn);
  }
  if (parsed.version !== 1) {
    return invalidResult(storage, key, "envelope version is unsupported", bytes, warn);
  }
  if (typeof parsed.dockRatio !== "number" ||
    !Number.isFinite(parsed.dockRatio) ||
    parsed.dockRatio < minimumDockRatio || parsed.dockRatio > maximumDockRatio) {
    return invalidResult(storage, key, "dock ratio is outside its bounds", bytes, warn);
  }
  const validation = validateWorkspace(parsed.workspace);
  if (!validation.valid) {
    return invalidResult(
      storage,
      key,
      validation.errors[0] ?? "workspace is invalid",
      bytes,
      warn,
    );
  }
  return {
    workspace: cloneWorkspaceForPersistence(parsed.workspace as Workspace),
    dockRatio: parsed.dockRatio,
    invalidReason: null,
  };
}

export function saveTerminalWorkspace(
  storage: StorageLike,
  projectRoot: string,
  workspace: Workspace,
  dockRatio: number,
): boolean {
  const raw = serializeTerminalWorkspace(workspace, dockRatio);
  if (raw === null) return false;
  try {
    storage.setItem(terminalWorkspaceStorageKey(projectRoot), raw);
    return true;
  } catch {
    return false;
  }
}

export function clearTerminalWorkspace(storage: StorageLike, projectRoot: string): void {
  const key = terminalWorkspaceStorageKey(projectRoot);
  try {
    storage.removeItem(key);
    storage.removeItem(`${key}:invalid`);
  } catch {
    // Persistence is optional.
  }
}

export function clearTerminalWorkspaceAfterReplace(
  storage: StorageLike,
  projectRoot: string,
  replacement: Workspace | null,
): boolean {
  if (replacement === null) return false;
  clearTerminalWorkspace(storage, projectRoot);
  return true;
}
