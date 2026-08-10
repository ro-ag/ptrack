import {
  collectWorkspaceIds,
  createSplitPane,
  createTerminalPane,
  createWorkspaceTab,
  defaultSplitRatio,
  findSplitPane,
  findTerminalPane,
  isWorkspace,
  maximumPaneDepth,
  maximumPanesPerTab,
  maximumWorkspaceTabs,
  nextWorkspaceId,
  normalizeSplitRatio,
  normalizeTabTitle,
  normalizeWorkspace,
  paneCount,
  type IdFactory,
  type PaneNode,
  type SplitDirection,
  type TerminalDescriptor,
  type TerminalPane,
  type Workspace,
  type WorkspaceTab,
} from "./model";

export type WorkspaceAction =
  | { type: "create-tab"; title?: string; profileId?: string; cwd?: string }
  | { type: "select-tab"; tabId: string }
  | { type: "rename-tab"; tabId: string; title: string }
  | { type: "reorder-tab"; tabId: string; toIndex: number }
  | { type: "duplicate-tab"; tabId: string }
  | { type: "close-tab"; tabId: string }
  | { type: "focus-pane"; tabId: string; paneId: string }
  | {
      type: "update-pane";
      tabId: string;
      paneId: string;
      changes: TerminalDescriptor;
    }
  | {
      type: "split-pane";
      tabId: string;
      paneId: string;
      direction: SplitDirection;
      ratio?: number;
      profileId?: string;
      cwd?: string;
    }
  | { type: "close-pane"; tabId: string; paneId: string }
  | { type: "resize-split"; tabId: string; splitId: string; ratio: number };

function updateTab(
  workspace: Workspace,
  tabId: string,
  update: (tab: WorkspaceTab) => WorkspaceTab | null,
): Workspace {
  const index = workspace.tabs.findIndex((tab) => tab.id === tabId);
  if (index < 0) return workspace;
  const replacement = update(workspace.tabs[index]);
  if (!replacement || replacement === workspace.tabs[index]) return workspace;
  const tabs = workspace.tabs.slice();
  tabs[index] = replacement;
  return { ...workspace, tabs };
}

function replaceNode(
  node: PaneNode,
  predicate: (candidate: PaneNode, depth: number) => boolean,
  replacement: (candidate: PaneNode, depth: number) => PaneNode,
  depth = 1,
): PaneNode {
  if (predicate(node, depth)) return replacement(node, depth);
  if (node.kind === "terminal") return node;
  const first = replaceNode(node.first, predicate, replacement, depth + 1);
  if (first !== node.first) return { ...node, first };
  const second = replaceNode(node.second, predicate, replacement, depth + 1);
  return second === node.second ? node : { ...node, second };
}

function firstPaneId(node: PaneNode): string {
  return node.kind === "terminal" ? node.paneId : firstPaneId(node.first);
}

function cloneNode(
  node: PaneNode,
  ids: IdFactory,
  usedIds: Set<string>,
  paneIdMap: Map<string, string>,
): PaneNode {
  if (node.kind === "terminal") {
    const paneId = nextWorkspaceId(ids, "pane", usedIds);
    paneIdMap.set(node.paneId, paneId);
    return { ...node, paneId };
  }
  return {
    kind: "split",
    splitId: nextWorkspaceId(ids, "split", usedIds),
    direction: node.direction,
    ratio: node.ratio,
    first: cloneNode(node.first, ids, usedIds, paneIdMap),
    second: cloneNode(node.second, ids, usedIds, paneIdMap),
  };
}

export function createTab(
  workspace: Workspace,
  ids: IdFactory,
  options: TerminalDescriptor & { title?: string } = {},
): Workspace {
  if (workspace.tabs.length >= maximumWorkspaceTabs) return workspace;
  try {
    const tab = createWorkspaceTab(ids, options, collectWorkspaceIds(workspace));
    return { ...workspace, activeTabId: tab.id, tabs: [...workspace.tabs, tab] };
  } catch {
    return workspace;
  }
}

export function selectTab(workspace: Workspace, tabId: string): Workspace {
  if (workspace.activeTabId === tabId) return workspace;
  return workspace.tabs.some((tab) => tab.id === tabId)
    ? { ...workspace, activeTabId: tabId }
    : workspace;
}

export function renameTab(
  workspace: Workspace,
  tabId: string,
  title: string,
): Workspace {
  if (typeof title !== "string" || title.trim().length === 0) return workspace;
  return updateTab(workspace, tabId, (tab) => {
    const nextTitle = normalizeTabTitle(title);
    return nextTitle === tab.title ? tab : { ...tab, title: nextTitle };
  });
}

export function reorderTab(
  workspace: Workspace,
  tabId: string,
  toIndex: number,
): Workspace {
  if (!Number.isFinite(toIndex)) return workspace;
  const fromIndex = workspace.tabs.findIndex((tab) => tab.id === tabId);
  if (fromIndex < 0) return workspace;
  const target = Math.max(0, Math.min(workspace.tabs.length - 1, Math.trunc(toIndex)));
  if (fromIndex === target) return workspace;
  const tabs = workspace.tabs.slice();
  const [tab] = tabs.splice(fromIndex, 1);
  tabs.splice(target, 0, tab);
  return { ...workspace, tabs };
}

export function duplicateTab(
  workspace: Workspace,
  ids: IdFactory,
  tabId: string,
): Workspace {
  if (workspace.tabs.length >= maximumWorkspaceTabs) return workspace;
  const index = workspace.tabs.findIndex((tab) => tab.id === tabId);
  if (index < 0) return workspace;
  try {
    const source = workspace.tabs[index];
    const usedIds = collectWorkspaceIds(workspace);
    const paneIdMap = new Map<string, string>();
    const duplicate: WorkspaceTab = {
      id: nextWorkspaceId(ids, "tab", usedIds),
      title: normalizeTabTitle(`${source.title} copy`),
      activePaneId: "",
      root: cloneNode(source.root, ids, usedIds, paneIdMap),
    };
    duplicate.activePaneId = paneIdMap.get(source.activePaneId) ?? firstPaneId(duplicate.root);
    const tabs = workspace.tabs.slice();
    tabs.splice(index + 1, 0, duplicate);
    return { ...workspace, activeTabId: duplicate.id, tabs };
  } catch {
    return workspace;
  }
}

export function closeTab(workspace: Workspace, tabId: string): Workspace {
  if (workspace.tabs.length <= 1) return workspace;
  const index = workspace.tabs.findIndex((tab) => tab.id === tabId);
  if (index < 0) return workspace;
  const tabs = workspace.tabs.filter((tab) => tab.id !== tabId);
  const activeTabId = workspace.activeTabId === tabId
    ? tabs[Math.min(index, tabs.length - 1)].id
    : workspace.activeTabId;
  return { ...workspace, activeTabId, tabs };
}

export function focusPane(
  workspace: Workspace,
  tabId: string,
  paneId: string,
): Workspace {
  if (workspace.activeTabId === tabId) {
    const tab = workspace.tabs.find((candidate) => candidate.id === tabId);
    if (tab?.activePaneId === paneId) return workspace;
  }
  const next = updateTab(workspace, tabId, (tab) =>
    findTerminalPane(tab.root, paneId) ? { ...tab, activePaneId: paneId } : null,
  );
  return next === workspace ? workspace : { ...next, activeTabId: tabId };
}

export function updatePane(
  workspace: Workspace,
  tabId: string,
  paneId: string,
  changes: TerminalDescriptor,
): Workspace {
  const hasProfile = typeof changes.profileId === "string";
  const hasCwd = typeof changes.cwd === "string";
  if (!hasProfile && !hasCwd) return workspace;
  return updateTab(workspace, tabId, (tab) => {
    const root = replaceNode(
      tab.root,
      (node) => node.kind === "terminal" && node.paneId === paneId,
      (node) => ({
        ...(node as TerminalPane),
        ...(hasProfile ? { profileId: changes.profileId as string } : {}),
        ...(hasCwd ? { cwd: changes.cwd as string } : {}),
      }),
    );
    return root === tab.root ? tab : { ...tab, root };
  });
}

export function splitPane(
  workspace: Workspace,
  ids: IdFactory,
  tabId: string,
  paneId: string,
  direction: SplitDirection,
  options: TerminalDescriptor & { ratio?: number } = {},
): Workspace {
  if (direction !== "horizontal" && direction !== "vertical") return workspace;
  const tab = workspace.tabs.find((candidate) => candidate.id === tabId);
  if (!tab || paneCount(tab.root) >= maximumPanesPerTab) return workspace;
  let activePaneId = tab.activePaneId;
  const usedIds = collectWorkspaceIds(workspace);
  try {
    const root = replaceNode(
      tab.root,
      (node, depth) =>
        node.kind === "terminal" && node.paneId === paneId && depth < maximumPaneDepth,
      (node) => {
        const terminal = node as TerminalPane;
        const second = createTerminalPane(
          ids,
          {
            profileId: options.profileId ?? terminal.profileId,
            cwd: options.cwd ?? terminal.cwd,
          },
          usedIds,
        );
        activePaneId = second.paneId;
        return createSplitPane(ids, terminal, second, {
          direction,
          ratio: options.ratio ?? defaultSplitRatio,
        }, usedIds);
      },
    );
    if (root === tab.root) return workspace;
    return updateTab(workspace, tabId, (current) => ({ ...current, root, activePaneId }));
  } catch {
    return workspace;
  }
}

function removePane(
  node: PaneNode,
  paneId: string,
): { root: PaneNode | null; fallbackPaneId: string | null } {
  if (node.kind === "terminal") {
    return node.paneId === paneId
      ? { root: null, fallbackPaneId: null }
      : { root: node, fallbackPaneId: null };
  }
  const first = removePane(node.first, paneId);
  if (!first.root) return { root: node.second, fallbackPaneId: firstPaneId(node.second) };
  if (first.root !== node.first) {
    return { root: { ...node, first: first.root }, fallbackPaneId: first.fallbackPaneId };
  }
  const second = removePane(node.second, paneId);
  if (!second.root) return { root: node.first, fallbackPaneId: firstPaneId(node.first) };
  if (second.root !== node.second) {
    return { root: { ...node, second: second.root }, fallbackPaneId: second.fallbackPaneId };
  }
  return { root: node, fallbackPaneId: null };
}

export function closePane(
  workspace: Workspace,
  tabId: string,
  paneId: string,
): Workspace {
  return updateTab(workspace, tabId, (tab) => {
    if (paneCount(tab.root) <= 1 || !findTerminalPane(tab.root, paneId)) return tab;
    const removed = removePane(tab.root, paneId);
    if (!removed.root) return tab;
    const activePaneId = tab.activePaneId === paneId
      ? removed.fallbackPaneId ?? firstPaneId(removed.root)
      : tab.activePaneId;
    return { ...tab, root: removed.root, activePaneId };
  });
}

export function resizeSplit(
  workspace: Workspace,
  tabId: string,
  splitId: string,
  ratio: number,
): Workspace {
  return updateTab(workspace, tabId, (tab) => {
    if (!findSplitPane(tab.root, splitId)) return tab;
    const normalized = normalizeSplitRatio(ratio);
    const root = replaceNode(
      tab.root,
      (node) => node.kind === "split" && node.splitId === splitId,
      (node) => node.kind === "split" && node.ratio === normalized
        ? node
        : ({ ...node, ratio: normalized }) as PaneNode,
    );
    return root === tab.root ? tab : { ...tab, root };
  });
}

export function reduceWorkspace(
  workspace: Workspace,
  action: WorkspaceAction,
  ids: IdFactory,
): Workspace {
  const current = isWorkspace(workspace)
    ? workspace
    : normalizeWorkspace(workspace, ids);
  let next = current;
  switch (action.type) {
    case "create-tab":
      next = createTab(current, ids, action);
      break;
    case "select-tab":
      next = selectTab(current, action.tabId);
      break;
    case "rename-tab":
      next = renameTab(current, action.tabId, action.title);
      break;
    case "reorder-tab":
      next = reorderTab(current, action.tabId, action.toIndex);
      break;
    case "duplicate-tab":
      next = duplicateTab(current, ids, action.tabId);
      break;
    case "close-tab":
      next = closeTab(current, action.tabId);
      break;
    case "focus-pane":
      next = focusPane(current, action.tabId, action.paneId);
      break;
    case "update-pane":
      next = updatePane(current, action.tabId, action.paneId, action.changes);
      break;
    case "split-pane":
      next = splitPane(current, ids, action.tabId, action.paneId, action.direction, action);
      break;
    case "close-pane":
      next = closePane(current, action.tabId, action.paneId);
      break;
    case "resize-split":
      next = resizeSplit(current, action.tabId, action.splitId, action.ratio);
      break;
  }
  return isWorkspace(next) ? next : current;
}
