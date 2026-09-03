export const defaultSidebarWidth = 248;
export const minimumSidebarWidth = 180;
export const maximumSidebarWidth = 420;
export const sidebarWidthStorageKey = "ptrack-sidebar-width";
export const sidebarHiddenStorageKey = "ptrack-sidebar-hidden";

export function sidebarMaximumWidth(viewportWidth: number): number {
  return Math.max(
    minimumSidebarWidth,
    Math.min(maximumSidebarWidth, Math.floor(viewportWidth * 0.45)),
  );
}

export function clampSidebarWidth(width: number, viewportWidth: number): number {
  const responsiveMaximum = sidebarMaximumWidth(viewportWidth);
  const finiteWidth = Number.isFinite(width) ? width : defaultSidebarWidth;
  return Math.round(
    Math.max(minimumSidebarWidth, Math.min(finiteWidth, responsiveMaximum)),
  );
}

export function storedSidebarWidth(
  value: string | null,
  viewportWidth: number,
): number {
  if (value === null || value.trim() === "") {
    return clampSidebarWidth(defaultSidebarWidth, viewportWidth);
  }
  return clampSidebarWidth(Number(value), viewportWidth);
}

export function sidebarWidthFromKey(
  currentWidth: number,
  key: string,
  viewportWidth: number,
): number | null {
  if (key === "ArrowLeft") return clampSidebarWidth(currentWidth - 16, viewportWidth);
  if (key === "ArrowRight") return clampSidebarWidth(currentWidth + 16, viewportWidth);
  if (key === "PageDown") return clampSidebarWidth(currentWidth - 64, viewportWidth);
  if (key === "PageUp") return clampSidebarWidth(currentWidth + 64, viewportWidth);
  if (key === "Home") return minimumSidebarWidth;
  if (key === "End") return clampSidebarWidth(maximumSidebarWidth, viewportWidth);
  return null;
}

// Frontend view of the stored layout record. The desktop runtime is the
// authority; this module normalizes what it returns and builds the patch the
// single SetLayoutState command takes. The record's `usedAt` eviction counter
// is backend-owned, so it is never read here and can never be sent back.

export type LayoutView = "board" | "overview" | "issues";
export type LayoutStorageStatus = "ok" | "defaults" | "unreadable";

const layoutViews: readonly LayoutView[] = ["board", "overview", "issues"];
const boardLanes: readonly string[] = ["blocked", "doing", "done", "todo"];

export interface LayoutProjectState {
  view: LayoutView;
  planId: number;
  foldedLanes: string[];
}

export interface LayoutState {
  storage: LayoutStorageStatus;
  sidebar: { width: number; hidden: boolean };
  panels: { boardHidden: boolean; terminalHidden: boolean };
  projects: Record<string, LayoutProjectState>;
}

function layoutRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function layoutFlag(value: unknown): boolean {
  return value === true;
}

export function defaultLayoutState(): LayoutState {
  return {
    storage: "defaults",
    sidebar: { width: defaultSidebarWidth, hidden: false },
    panels: { boardHidden: false, terminalHidden: false },
    projects: {},
  };
}

export function layoutProjectState(
  state: LayoutState,
  projectRoot: string,
): LayoutProjectState {
  return state.projects[projectRoot] ??
    { view: "board", planId: 0, foldedLanes: [] };
}

function normalizeProject(value: unknown): LayoutProjectState {
  const entry = layoutRecord(value);
  const planId = Number(entry.planId);
  const lanes: unknown[] = Array.isArray(entry.foldedLanes) ? entry.foldedLanes : [];
  return {
    view: layoutViews.includes(entry.view as LayoutView)
      ? entry.view as LayoutView
      : "board",
    planId: Number.isFinite(planId) && planId > 0 ? Math.trunc(planId) : 0,
    foldedLanes: [
      ...new Set(lanes.filter((lane): lane is string =>
        typeof lane === "string" && boardLanes.includes(lane)
      )),
    ].sort(),
  };
}

// normalizeLayoutState is total: any shape reads as a complete record, and a
// reply without a status means the stored record was not read.
export function normalizeLayoutState(value: unknown): LayoutState {
  const document = layoutRecord(value);
  const sidebar = layoutRecord(document.sidebar);
  const panels = layoutRecord(document.panels);
  const width = Number(sidebar.width);
  const projects: Record<string, LayoutProjectState> = {};
  for (const [root, entry] of Object.entries(layoutRecord(document.projects))) {
    if (root !== "") projects[root] = normalizeProject(entry);
  }
  return {
    storage: ["ok", "defaults", "unreadable"].includes(document.storage as string)
      ? document.storage as LayoutStorageStatus
      : "unreadable",
    sidebar: {
      width: Number.isFinite(width) ? Math.round(width) : defaultSidebarWidth,
      hidden: layoutFlag(sidebar.hidden),
    },
    panels: {
      boardHidden: layoutFlag(panels.boardHidden),
      terminalHidden: layoutFlag(panels.terminalHidden),
    },
    projects,
  };
}

// layoutStatePatch is the single patch SetLayoutState takes. Only the open
// project is sent, so the bounded per-project map keeps every other entry.
export function layoutStatePatch(
  state: LayoutState,
  projectRoot: string,
): Record<string, unknown> {
  const entry = state.projects[projectRoot];
  return {
    sidebar: state.sidebar,
    panels: state.panels,
    ...(projectRoot === "" || !entry ? {} : { projects: { [projectRoot]: entry } }),
  };
}
