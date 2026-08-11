export const workspaceVersion = 1 as const;
export const maximumWorkspaceTabs = 12;
export const maximumPanesPerTab = 8;
export const maximumPaneDepth = 6;
export const maximumTabTitleLength = 80;
export const maximumWorkspaceIdLength = 128;
export const maximumProfileIdLength = 128;
export const maximumCwdLength = 4096;
export const minimumSplitRatio = 0.1;
export const maximumSplitRatio = 0.9;
export const defaultSplitRatio = 0.5;

export type SplitDirection = "horizontal" | "vertical";
export type WorkspaceIdKind = "tab" | "pane" | "split";

export interface IdFactory {
  next(kind: WorkspaceIdKind): string;
}

export interface TerminalPane {
  kind: "terminal";
  paneId: string;
  profileId: string;
  cwd: string;
}

// AssociationPointerV1 is the only association metadata safe to persist with
// a tab. The backend resolves it against the current project generation before
// attaching context to any live terminal session or agent run.
export interface AssociationPointerV1 {
  version: 1;
  planId?: number;
  taskId?: number;
}

export interface SplitPane {
  kind: "split";
  splitId: string;
  direction: SplitDirection;
  ratio: number;
  first: PaneNode;
  second: PaneNode;
}

export type PaneNode = TerminalPane | SplitPane;

export interface WorkspaceTab {
  id: string;
  title: string;
  activePaneId: string;
  root: PaneNode;
  association?: AssociationPointerV1;
}

export type Terminal = TerminalPane;
export type Split = SplitPane;
export type Tab = WorkspaceTab;

export interface Workspace {
  version: typeof workspaceVersion;
  activeTabId: string;
  tabs: WorkspaceTab[];
}

export interface TerminalDescriptor {
  profileId?: string;
  cwd?: string;
}

export type WorkspaceTabOptions = TerminalDescriptor & {
  title?: string;
  association?: AssociationPointerV1;
};

export interface WorkspaceValidation {
  valid: boolean;
  errors: readonly string[];
}

interface NormalizationState {
  ids: IdFactory;
  usedIds: Set<string>;
  paneCount: number;
  seenObjects: WeakSet<object>;
  fallback: Required<TerminalDescriptor>;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function nonemptyId(value: unknown): value is string {
  return typeof value === "string" &&
    value.trim().length > 0 &&
    value.length <= maximumWorkspaceIdLength;
}

function positiveSafeId(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}

export function normalizeAssociationPointer(
  value: unknown,
): AssociationPointerV1 | undefined {
  if (!isRecord(value) || value.version !== 1) return undefined;
  const planId = value.planId;
  const taskId = value.taskId;
  if (planId !== undefined && !positiveSafeId(planId)) return undefined;
  if (taskId !== undefined && (!positiveSafeId(taskId) || planId === undefined)) {
    return undefined;
  }
  return {
    version: 1,
    ...(planId === undefined ? {} : { planId }),
    ...(taskId === undefined ? {} : { taskId }),
  };
}

function reportUnknownKeys(
  value: Record<string, unknown>,
  allowed: readonly string[],
  path: string,
  errors: string[],
): void {
  const allowedKeys = new Set(allowed);
  if (Object.keys(value).some((key) => !allowedKeys.has(key))) {
    errors.push(`${path} contains unsupported fields`);
  }
}

function allocateId(
  ids: IdFactory,
  kind: WorkspaceIdKind,
  usedIds: Set<string>,
): string {
  for (let attempt = 0; attempt < 64; attempt += 1) {
    const id = ids.next(kind);
    if (nonemptyId(id) && !usedIds.has(id)) {
      usedIds.add(id);
      return id;
    }
  }
  throw new Error(`IdFactory could not provide a unique ${kind} id`);
}

function retainOrAllocateId(
  value: unknown,
  kind: WorkspaceIdKind,
  state: NormalizationState,
): string {
  if (nonemptyId(value) && !state.usedIds.has(value)) {
    state.usedIds.add(value);
    return value;
  }
  return allocateId(state.ids, kind, state.usedIds);
}

export function normalizeTabTitle(value: unknown, fallback = "Terminal"): string {
  if (typeof value !== "string" || value.trim().length === 0) return fallback;
  return value.trim().slice(0, maximumTabTitleLength);
}

export function normalizeSplitRatio(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return defaultSplitRatio;
  }
  const clamped = Math.max(minimumSplitRatio, Math.min(maximumSplitRatio, value));
  return Math.round(clamped * 1_000) / 1_000;
}

function normalizeNode(
  value: unknown,
  depth: number,
  state: NormalizationState,
): PaneNode | null {
  if (
    !isRecord(value) ||
    state.seenObjects.has(value) ||
    state.paneCount >= maximumPanesPerTab
  ) return null;
  state.seenObjects.add(value);

  if (value.kind === "terminal") {
    state.paneCount += 1;
    return {
      kind: "terminal",
      paneId: retainOrAllocateId(value.paneId, "pane", state),
      profileId:
        typeof value.profileId === "string"
          ? value.profileId.slice(0, maximumProfileIdLength)
          : state.fallback.profileId,
      cwd: typeof value.cwd === "string"
        ? value.cwd.slice(0, maximumCwdLength)
        : state.fallback.cwd,
    };
  }

  if (value.kind !== "split") return null;
  if (depth >= maximumPaneDepth) return null;

  const first = normalizeNode(value.first, depth + 1, state);
  const second = normalizeNode(value.second, depth + 1, state);
  if (!first) return second;
  if (!second) return first;
  return {
    kind: "split",
    splitId: retainOrAllocateId(value.splitId, "split", state),
    direction:
      value.direction === "horizontal" || value.direction === "vertical"
        ? value.direction
        : "horizontal",
    ratio: normalizeSplitRatio(value.ratio),
    first,
    second,
  };
}

function terminalPaneIds(node: PaneNode, result: string[] = []): string[] {
  if (node.kind === "terminal") {
    result.push(node.paneId);
  } else {
    terminalPaneIds(node.first, result);
    terminalPaneIds(node.second, result);
  }
  return result;
}

export function paneIds(node: PaneNode): string[] {
  return terminalPaneIds(node);
}

export function paneCount(node: PaneNode): number {
  return node.kind === "terminal"
    ? 1
    : paneCount(node.first) + paneCount(node.second);
}

export function paneDepth(node: PaneNode): number {
  return node.kind === "terminal"
    ? 1
    : 1 + Math.max(paneDepth(node.first), paneDepth(node.second));
}

export function findTerminalPane(
  node: PaneNode,
  paneId: string,
): TerminalPane | null {
  if (node.kind === "terminal") return node.paneId === paneId ? node : null;
  return findTerminalPane(node.first, paneId) ??
    findTerminalPane(node.second, paneId);
}

export function findSplitPane(node: PaneNode, splitId: string): SplitPane | null {
  if (node.kind === "terminal") return null;
  if (node.splitId === splitId) return node;
  return findSplitPane(node.first, splitId) ?? findSplitPane(node.second, splitId);
}

export function collectWorkspaceIds(workspace: Workspace): Set<string> {
  const result = new Set<string>();
  const visit = (node: PaneNode): void => {
    if (node.kind === "terminal") {
      result.add(node.paneId);
      return;
    }
    result.add(node.splitId);
    visit(node.first);
    visit(node.second);
  };
  for (const tab of workspace.tabs) {
    result.add(tab.id);
    visit(tab.root);
  }
  return result;
}

export function nextWorkspaceId(
  ids: IdFactory,
  kind: WorkspaceIdKind,
  usedIds: Set<string>,
): string {
  return allocateId(ids, kind, usedIds);
}

export function createTerminalPane(
  ids: IdFactory,
  descriptor: TerminalDescriptor = {},
  usedIds = new Set<string>(),
): TerminalPane {
  return {
    kind: "terminal",
    paneId: allocateId(ids, "pane", usedIds),
    profileId: (descriptor.profileId ?? "").slice(0, maximumProfileIdLength),
    cwd: (descriptor.cwd ?? "").slice(0, maximumCwdLength),
  };
}

export function createSplitPane(
  ids: IdFactory,
  first: PaneNode,
  second: PaneNode,
  options: { direction: SplitDirection; ratio?: number },
  usedIds?: Set<string>,
): SplitPane {
  const occupied = usedIds ?? new Set<string>();
  if (!usedIds) {
    const visit = (node: PaneNode): void => {
      if (node.kind === "terminal") {
        occupied.add(node.paneId);
      } else {
        occupied.add(node.splitId);
        visit(node.first);
        visit(node.second);
      }
    };
    visit(first);
    visit(second);
  }
  return {
    kind: "split",
    splitId: allocateId(ids, "split", occupied),
    direction: options.direction,
    ratio: normalizeSplitRatio(options.ratio),
    first,
    second,
  };
}

export function createWorkspaceTab(
  ids: IdFactory,
  options: WorkspaceTabOptions = {},
  usedIds = new Set<string>(),
): WorkspaceTab {
  const id = allocateId(ids, "tab", usedIds);
  const root = createTerminalPane(ids, options, usedIds);
  const association = normalizeAssociationPointer(options.association);
  return {
    id,
    title: normalizeTabTitle(options.title, "Terminal"),
    activePaneId: root.paneId,
    root,
    ...(association === undefined ? {} : { association }),
  };
}

export function createWorkspace(
  ids: IdFactory,
  options: WorkspaceTabOptions = {},
): Workspace {
  const tab = createWorkspaceTab(ids, options);
  return {
    version: workspaceVersion,
    activeTabId: tab.id,
    tabs: [tab],
  };
}

export function normalizeWorkspace(
  value: unknown,
  ids: IdFactory,
  fallback: Required<TerminalDescriptor> = { profileId: "", cwd: "" },
): Workspace {
  const source = isRecord(value) ? value : {};
  const sourceTabs = Array.isArray(source.tabs) ? source.tabs : [];
  const usedIds = new Set<string>();
  const tabs: WorkspaceTab[] = [];

  for (const candidate of sourceTabs.slice(0, maximumWorkspaceTabs)) {
    if (!isRecord(candidate)) continue;
    const state: NormalizationState = {
      ids,
      usedIds,
      paneCount: 0,
      seenObjects: new WeakSet(),
      fallback: {
        profileId: fallback.profileId.slice(0, maximumProfileIdLength),
        cwd: fallback.cwd.slice(0, maximumCwdLength),
      },
    };
    const root = normalizeNode(candidate.root, 1, state);
    if (!root) continue;
    const paneIdList = terminalPaneIds(root);
    const association = normalizeAssociationPointer(candidate.association);
    tabs.push({
      id: retainOrAllocateId(candidate.id, "tab", state),
      title: normalizeTabTitle(candidate.title, `Terminal ${tabs.length + 1}`),
      activePaneId:
        nonemptyId(candidate.activePaneId) && paneIdList.includes(candidate.activePaneId)
          ? candidate.activePaneId
          : paneIdList[0],
      root,
      ...(association === undefined ? {} : { association }),
    });
  }

  if (tabs.length === 0) {
    const tab = createWorkspaceTab(ids, fallback, usedIds);
    tabs.push(tab);
  }

  const activeTabId =
    nonemptyId(source.activeTabId) && tabs.some((tab) => tab.id === source.activeTabId)
      ? source.activeTabId
      : tabs[0].id;
  return { version: workspaceVersion, activeTabId, tabs };
}

export function validateWorkspace(value: unknown): WorkspaceValidation {
  const errors: string[] = [];
  if (!isRecord(value)) return { valid: false, errors: ["workspace must be an object"] };
  reportUnknownKeys(value, ["version", "activeTabId", "tabs"], "workspace", errors);
  if (value.version !== workspaceVersion) errors.push("workspace version must be 1");
  if (!nonemptyId(value.activeTabId)) errors.push("activeTabId must be nonempty");
  if (!Array.isArray(value.tabs) || value.tabs.length === 0) {
    errors.push("workspace must contain at least one tab");
    return { valid: false, errors };
  }
  if (value.tabs.length > maximumWorkspaceTabs) errors.push("workspace has too many tabs");

  const ids = new Set<string>();
  const addId = (candidate: unknown, path: string): void => {
    if (!nonemptyId(candidate)) {
      errors.push(`${path} must be a nonempty id`);
    } else if (ids.has(candidate)) {
      errors.push(`${path} must be unique`);
    } else {
      ids.add(candidate);
    }
  };

  const validateNode = (
    candidate: unknown,
    path: string,
    depth: number,
    seen: WeakSet<object>,
  ): string[] => {
    if (depth > maximumPaneDepth) {
      errors.push(`${path} exceeds maximum depth`);
      return [];
    }
    if (!isRecord(candidate)) {
      errors.push(`${path} must be a pane node`);
      return [];
    }
    if (seen.has(candidate)) {
      errors.push(`${path} must not be cyclic`);
      return [];
    }
    seen.add(candidate);
    if (candidate.kind === "terminal") {
      reportUnknownKeys(candidate, ["kind", "paneId", "profileId", "cwd"], path, errors);
      addId(candidate.paneId, `${path}.paneId`);
      if (typeof candidate.profileId !== "string") {
        errors.push(`${path}.profileId must be a string`);
      } else if (candidate.profileId.length > maximumProfileIdLength) {
        errors.push(`${path}.profileId is too long`);
      }
      if (typeof candidate.cwd !== "string") {
        errors.push(`${path}.cwd must be a string`);
      } else if (candidate.cwd.length > maximumCwdLength) {
        errors.push(`${path}.cwd is too long`);
      }
      return nonemptyId(candidate.paneId) ? [candidate.paneId] : [];
    }
    if (candidate.kind !== "split") {
      errors.push(`${path}.kind is invalid`);
      return [];
    }
    reportUnknownKeys(
      candidate,
      ["kind", "splitId", "direction", "ratio", "first", "second"],
      path,
      errors,
    );
    addId(candidate.splitId, `${path}.splitId`);
    if (candidate.direction !== "horizontal" && candidate.direction !== "vertical") {
      errors.push(`${path}.direction is invalid`);
    }
    if (
      typeof candidate.ratio !== "number" ||
      !Number.isFinite(candidate.ratio) ||
      candidate.ratio < minimumSplitRatio ||
      candidate.ratio > maximumSplitRatio ||
      candidate.ratio !== normalizeSplitRatio(candidate.ratio)
    ) errors.push(`${path}.ratio is outside its finite bounds`);
    return [
      ...validateNode(candidate.first, `${path}.first`, depth + 1, seen),
      ...validateNode(candidate.second, `${path}.second`, depth + 1, seen),
    ];
  };

  const tabIds: string[] = [];
  for (const [index, candidate] of value.tabs.entries()) {
    const path = `tabs[${index}]`;
    if (!isRecord(candidate)) {
      errors.push(`${path} must be an object`);
      continue;
    }
    reportUnknownKeys(
      candidate,
      ["id", "title", "activePaneId", "root", "association"],
      path,
      errors,
    );
    addId(candidate.id, `${path}.id`);
    if (nonemptyId(candidate.id)) tabIds.push(candidate.id);
    if (typeof candidate.title !== "string" || candidate.title.trim().length === 0) {
      errors.push(`${path}.title must be nonempty`);
    } else if (candidate.title.length > maximumTabTitleLength) {
      errors.push(`${path}.title is too long`);
    }
    const paneIdList = validateNode(candidate.root, `${path}.root`, 1, new WeakSet());
    if (paneIdList.length === 0) errors.push(`${path} must contain at least one pane`);
    if (paneIdList.length > maximumPanesPerTab) errors.push(`${path} has too many panes`);
    if (!nonemptyId(candidate.activePaneId) || !paneIdList.includes(candidate.activePaneId)) {
      errors.push(`${path}.activePaneId must reference a pane in the tab`);
    }
    if (candidate.association !== undefined) {
      if (!isRecord(candidate.association)) {
        errors.push(`${path}.association must be an object`);
      } else {
        reportUnknownKeys(
          candidate.association,
          ["version", "planId", "taskId"],
          `${path}.association`,
          errors,
        );
        if (candidate.association.version !== 1) {
          errors.push(`${path}.association version must be 1`);
        }
        const planId = candidate.association.planId;
        const taskId = candidate.association.taskId;
        if (planId !== undefined && !positiveSafeId(planId)) {
          errors.push(`${path}.association.planId must be a positive safe integer`);
        }
        if (taskId !== undefined && !positiveSafeId(taskId)) {
          errors.push(`${path}.association.taskId must be a positive safe integer`);
        }
        if (taskId !== undefined && planId === undefined) {
          errors.push(`${path}.association.taskId requires planId`);
        }
      }
    }
  }
  if (!nonemptyId(value.activeTabId) || !tabIds.includes(value.activeTabId)) {
    errors.push("activeTabId must reference a tab");
  }
  return { valid: errors.length === 0, errors };
}

export function isWorkspace(value: unknown): value is Workspace {
  return validateWorkspace(value).valid;
}
