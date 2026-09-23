import { animate } from "motion";
import {
  preferenceChoice,
  reducedMotionActive,
  reducedMotionPreferences,
} from "../settings/preferences";
import type { RecentProjectEntry } from "./recent-projects";
import type { Overview, Summary } from "./overview";
import { filterRecentProjects } from "./overview";
import { messageFrom, relativeTime } from "./format";
import type { AppContext } from "./app-context";
import { element } from "./dom";
import { overviewActivity, overviewRefreshMessage } from "./overview";
import { bindProjectView } from "./project-view";
import { preselectedRecentProject, recentProjectPrimaryAction } from "./recent-projects";

export type LandingFilter = "all" | "available" | "synced";
export function landingProjects(projects: RecentProjectEntry[], overview: Overview | null, query: string, filter: LandingFilter) {
  return filterRecentProjects(projects, query).filter((project) => filter === "all" ||
    (filter === "available" ? project.availability === "available" : overview?.projects.some((summary) => summary.root === project.canonicalPath)));
}
export function selectedLandingProject(projects: RecentProjectEntry[], selectedId: string) {
  return projects.find((project) => project.entryId === selectedId) ?? projects[0] ?? null;
}
export function carouselPosition(index: number, selected: number, length: number) {
  if (index < 0 || index >= length || Math.abs(index - selected) > 3) return "pos-hidden";
  return index === selected ? "pos-center" : index < selected ? "pos-left" : "pos-right";
}
export function carouselGeometry(index: number, selected: number) {
  const offset = index - selected;
  const distance = Math.abs(offset);
  return { x: offset === 0 ? 0 : Math.sign(offset) * (distance < 1 ? distance * 64 : 64 + (distance - 1) * 22),
    z: distance < 1 ? 36 - distance * 176 : -140 - (distance - 1) * 55,
    angle: offset === 0 ? 0 : -Math.sign(offset) * 58 * Math.min(1, distance) };
}
/** Motion's numeric JS tween updates the matrix; native WebKit WAAPI loses these 3D updates. */
export function createCoverMotion(card: HTMLElement) {
  let animation: { stop: () => void } | undefined;
  let current: ReturnType<typeof carouselGeometry> | undefined;
  let target = "";
  const apply = (geometry: ReturnType<typeof carouselGeometry>) => {
    current = geometry;
    card.style.transform = `translate(-50%, -50%) perspective(760px) translate3d(${geometry.x}%, 0, ${geometry.z}px) rotateY(${geometry.angle}deg)`;
  };
  return {
    move(index: number, selected: number, followingPointer = false) {
      const geometry = carouselGeometry(index, selected);
      const identity = `${geometry.x},${geometry.z},${geometry.angle}`;
      const reduced = reducedMotionActive(
        preferenceChoice(document.documentElement.dataset.reducedMotion ?? "", reducedMotionPreferences) ?? "system",
        window.matchMedia("(prefers-reduced-motion: reduce)").matches,
      );
      if (target === identity && !reduced) return;
      animation?.stop();
      animation = undefined;
      if (!current || reduced || followingPointer) apply(geometry);
      else {
        const from = current;
        animation = animate(0, 1, {
          duration: 0.62,
          ease: [0.2, 0.8, 0.2, 1],
          onUpdate: (progress) => apply({
            x: from.x + (geometry.x - from.x) * progress,
            z: from.z + (geometry.z - from.z) * progress,
            angle: from.angle + (geometry.angle - from.angle) * progress,
          }),
        });
      }
      target = identity;
    },
    stop() { animation?.stop(); animation = undefined; },
  };
}
export function boundedSelection(index: number, direction: number, length: number) {
  return length ? Math.max(0, Math.min(length - 1, index + direction)) : -1;
}
/** One intentional step per horizontal gesture, including its inertial tail. */
export function horizontalGesture() {
  let last = -Infinity, distance = 0, consumed = false;
  return (x: number, y: number, time: number) => {
    if (time - last > 180) { distance = 0; consumed = false; }
    last = time;
    if (Math.abs(x) <= Math.abs(y) * 1.3) return 0;
    distance += x;
    if (consumed || Math.abs(distance) < 42) return 0;
    consumed = true;
    return Math.sign(distance);
  };
}
export function coverSummary(summary?: Summary, now = Date.now()) {
  const latest = summary?.activity.filter((item) => Number.isFinite(item.updatedAt) && item.updatedAt > 0 && item.updatedAt <= now / 1000)
    .sort((a, b) => b.updatedAt - a.updatedAt)[0];
  return {
    context: latest ? `Latest ${latest.kind}: ${latest.title}` : summary ? "No cached updates yet" : "Summary unavailable",
    metrics: summary ? [
      { value: summary.counts.doneTasks, label: "done tasks" },
      { value: summary.counts.openTasks, label: "open tasks" },
      { value: summary.counts.activePlans, label: "active plans" },
      { value: summary.counts.openIssues, label: "open issues" },
    ] : [],
    syncedAt: summary && Number.isFinite(summary.syncedAt) && summary.syncedAt > 0 ? summary.syncedAt : null,
  };
}
/** "/Users/ana/dev/app" → "~/dev/app"; paths outside a home folder stay whole. */
export function shortenHomePath(path: string): string {
  const match = /^(?:\/Users|\/home)\/[^/]+(?=\/|$)/.exec(path);
  return match ? `~${path.slice(match[0].length)}` : path;
}
/** The facts a list row shows beside the project name. */
export function landingRowDetails(project: RecentProjectEntry, summary?: Summary, now = Date.now()) {
  const counts = summary ? [
    `${summary.counts.openTasks} open ${summary.counts.openTasks === 1 ? "task" : "tasks"}`,
    `${summary.counts.openIssues} open ${summary.counts.openIssues === 1 ? "issue" : "issues"}`,
  ] : [];
  return {
    path: shortenHomePath(project.canonicalPath),
    counts,
    opened: `Opened ${relativeTime(new Date(project.lastOpenedAt).getTime(), "long", now)}`,
  };
}
/** Edge fades tell the reader the chip strip scrolls further in that direction. */
export function stripOverflow(scrollLeft: number, scrollWidth: number, clientWidth: number) {
  return { start: scrollLeft > 1, end: scrollLeft + clientWidth < scrollWidth - 1 };
}
function syncStripOverflow(strip: HTMLElement) {
  const list = strip.classList.contains("is-list");
  const overflow = stripOverflow(strip.scrollLeft, strip.scrollWidth, strip.clientWidth);
  strip.dataset.overflowStart = String(!list && overflow.start);
  strip.dataset.overflowEnd = String(!list && overflow.end);
}
function timeLabel(className: string, prefix: string, timestamp: number) {
  const element = node("time", className, `${prefix}${relativeTime(timestamp, "long")}`);
  if (Number.isFinite(timestamp)) {
    const date = new Date(timestamp);
    element.dateTime = date.toISOString(); element.title = date.toLocaleString();
  }
  return element;
}
const colors = ["#5FAFFF", "#3DD6A3", "#AFA8FF"];
function node<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, text = "") {
  const element = document.createElement(tag);
  element.className = className;
  if (text) element.textContent = text;
  return element;
}
interface LandingRenderOptions {
  projects: RecentProjectEntry[];
  summaries: Summary[];
  selectedId: string;
  preselectedId: string;
  busy: boolean;
  loading: boolean;
  select: (id: string) => void;
  open: (project: RecentProjectEntry, invoker: HTMLElement) => void;
  actions: (host: HTMLElement, project: RecentProjectEntry, descriptionId: string) => void;
}
interface ProjectNodes { item: HTMLElement; card: HTMLButtonElement; stripItem: HTMLElement; thumbnail: HTMLButtonElement; motion: ReturnType<typeof createCoverMotion> }
interface CarouselState { options: LandingRenderOptions; entries: Map<string, ProjectNodes>; selectedId: string; suppressClickUntil: number }
const carousels = new WeakMap<HTMLElement, CarouselState>();
function navigate(state: CarouselState, direction: number, focus: boolean, strip = false) {
  const { options } = state;
  if (options.busy || options.projects.length < 2) return;
  const selected = selectedLandingProject(options.projects, options.selectedId)!;
  const index = options.projects.indexOf(selected);
  const next = options.projects[boundedSelection(index, direction, options.projects.length)];
  if (selected === next) return;
  options.select(next.entryId);
  if (focus) (strip ? state.entries.get(next.entryId)?.thumbnail : state.entries.get(next.entryId)?.card)?.focus({ preventScroll: true });
}
/** Capture horizontal drags only after intent is clear; native vertical scrolling stays available. */
export function bindCoverDrag(stage: HTMLElement, callbacks: { enabled: () => boolean; follow: (fraction: number) => void; finish: (direction: number) => void }) {
  let start: { x: number; y: number; id: number; dragging: boolean; fraction: number } | null = null;
  stage.addEventListener("pointerdown", (event) => {
    if (event.button !== 0 || !event.isPrimary || !callbacks.enabled()) return;
    start = { x: event.clientX, y: event.clientY, id: event.pointerId, dragging: false, fraction: 0 };
  });
  stage.addEventListener("pointermove", (event) => {
    if (!start || event.pointerId !== start.id) return;
    const x = start.x - event.clientX, y = start.y - event.clientY;
    if (!start.dragging) {
      if (Math.abs(y) > 8 && Math.abs(y) >= Math.abs(x)) { start = null; return; }
      if (Math.abs(x) < 8 || Math.abs(x) <= Math.abs(y) * 1.3) return;
      start.dragging = true;
      stage.setPointerCapture(event.pointerId);
      stage.classList.add("is-dragging");
    }
    event.preventDefault();
    start.fraction = Math.max(-1, Math.min(1, x / Math.max(120, stage.clientWidth * 0.32)));
    callbacks.follow(start.fraction);
  });
  const finish = (event: PointerEvent, cancelled: boolean) => {
    if (!start || event.pointerId !== start.id) return;
    const drag = start; start = null;
    stage.classList.remove("is-dragging");
    if (stage.hasPointerCapture(event.pointerId)) stage.releasePointerCapture(event.pointerId);
    if (drag.dragging) callbacks.finish(!cancelled && Math.abs(drag.fraction) >= 0.18 ? Math.sign(drag.fraction) : 0);
  };
  stage.addEventListener("pointerup", (event) => finish(event, false));
  stage.addEventListener("pointercancel", (event) => finish(event, true));
  stage.addEventListener("lostpointercapture", (event) => finish(event, true));
}
function initializeCarousel(stage: HTMLElement, strip: HTMLElement, options: LandingRenderOptions) {
  const state: CarouselState = { options, entries: new Map(), selectedId: "", suppressClickUntil: 0 };
  const wheelStep = horizontalGesture();
  for (const host of [stage, strip]) host.addEventListener("keydown", (event) => {
    const vertical = host === strip && strip.classList.contains("is-list");
    const previous = vertical ? "ArrowUp" : "ArrowLeft", next = vertical ? "ArrowDown" : "ArrowRight";
    if (event.key !== previous && event.key !== next) return;
    event.preventDefault(); event.stopPropagation();
    navigate(state, event.key === previous ? -1 : 1, true, host === strip);
  });
  stage.addEventListener("wheel", (event) => {
    if (event.ctrlKey || Math.abs(event.deltaX) <= Math.abs(event.deltaY) * 1.3 || state.options.projects.length < 2) return;
    event.preventDefault();
    const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? stage.clientWidth : 1;
    const direction = wheelStep(event.deltaX * unit, event.deltaY * unit, event.timeStamp);
    if (direction) navigate(state, direction, stage.contains(document.activeElement));
  }, { passive: false });
  bindCoverDrag(stage, {
    enabled: () => !state.options.busy && state.options.projects.length > 1,
    follow: (fraction) => {
      const projects = state.options.projects;
      const selected = selectedLandingProject(projects, state.options.selectedId)!;
      const index = projects.indexOf(selected);
      const position = Math.max(0, Math.min(projects.length - 1, index + fraction));
      projects.forEach((project, i) => state.entries.get(project.entryId)?.motion.move(i, position, true));
    },
    finish: (direction) => {
      state.suppressClickUntil = performance.now() + 350;
      if (direction) navigate(state, direction, stage.contains(document.activeElement));
      const projects = state.options.projects;
      const index = projects.indexOf(selectedLandingProject(projects, state.options.selectedId)!);
      projects.forEach((project, i) => state.entries.get(project.entryId)?.motion.move(i, index));
    },
  });
  strip.addEventListener("scroll", () => syncStripOverflow(strip), { passive: true });
  if ("ResizeObserver" in window) new ResizeObserver(() => syncStripOverflow(strip)).observe(strip);
  carousels.set(stage, state);
  return state;
}
function createProjectNodes(state: CarouselState, id: string): ProjectNodes {
  const card = node("button", "project-card"); card.type = "button"; card.dataset.orbitProjectId = id;
  card.addEventListener("click", () => {
    if (performance.now() < state.suppressClickUntil || state.options.busy) return;
    const project = state.options.projects.find((entry) => entry.entryId === id);
    if (!project) return;
    const selected = selectedLandingProject(state.options.projects, state.options.selectedId);
    if (selected === project && project.availability === "available") state.options.open(project, card);
    else state.options.select(id);
  });
  const item = node("div", "orbit-card-item"); item.setAttribute("role", "listitem"); item.append(card);
  const thumbnail = node("button", "strip-item"); thumbnail.type = "button"; thumbnail.dataset.orbitProjectId = id;
  thumbnail.addEventListener("click", () => state.options.select(id));
  const stripItem = node("div", "orbit-strip-item"); stripItem.setAttribute("role", "listitem"); stripItem.append(thumbnail);
  return { item, card, stripItem, thumbnail, motion: createCoverMotion(item) };
}
export function renderLandingProjects(options: LandingRenderOptions) {
  const { projects, summaries } = options;
  const stage = document.querySelector<HTMLElement>("#recent-project-list")!;
  const strip = document.querySelector<HTMLElement>("#orbit-project-strip")!;
  const detail = document.querySelector<HTMLElement>("#orbit-selected-project")!;
  const state = carousels.get(stage) ?? initializeCarousel(stage, strip, options);
  state.options = options;
  const focused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const focusSurface = focused && stage.contains(focused) ? "stage" : focused && strip.contains(focused) ? "strip" : null;
  const selected = selectedLandingProject(projects, options.selectedId);
  const selectedIndex = selected ? projects.indexOf(selected) : -1;
  document.querySelector<HTMLButtonElement>("#orbit-previous")!.disabled = options.busy || selectedIndex <= 0;
  document.querySelector<HTMLButtonElement>("#orbit-next")!.disabled = options.busy || selectedIndex >= projects.length - 1 || !selected;
  const identities = new Set(projects.map((project) => project.entryId));
  for (const [id, entry] of state.entries) if (!identities.has(id)) {
    entry.motion.stop(); entry.item.remove(); entry.stripItem.remove(); state.entries.delete(id);
  }
  stage.querySelector(".empty-state")?.remove();
  strip.querySelector(".empty-state")?.remove();
  detail.replaceChildren();
  if (!selected) {
    const empty = node("div", "empty-state", options.loading ? "Loading recent projects…" : "No projects match this view. Clear search or initialize a project.");
    empty.setAttribute("role", "listitem"); stage.append(empty);
    const indexEmpty = node("div", "empty-state index-empty-state", empty.textContent ?? "");
    indexEmpty.setAttribute("role", "listitem"); strip.append(indexEmpty);
    detail.append(node("p", "project-plan", "Select a project from the chooser to view its details."));
    state.selectedId = "";
    return;
  }
  projects.forEach((project, index) => {
    let entry = state.entries.get(project.entryId);
    if (!entry) { entry = createProjectNodes(state, project.entryId); state.entries.set(project.entryId, entry); }
    const { item, card, stripItem, thumbnail } = entry;
    // Keep existing buttons in place: transforms interpolate and keyboard focus survives selection.
    if (stage.children[index] !== item) stage.insertBefore(item, stage.children[index] ?? null);
    if (strip.children[index] !== stripItem) strip.insertBefore(stripItem, strip.children[index] ?? null);
    const position = carouselPosition(index, selectedIndex, projects.length);
    item.className = `orbit-card-item ${position}`;
    card.className = `project-card ${position}`;
    entry.motion.move(index, selectedIndex);
    item.style.setProperty("--cover-order", String(projects.length - Math.abs(index - selectedIndex)));
    const color = colors[Array.from(project.entryId).reduce((sum, letter) => sum + letter.charCodeAt(0), 0) % colors.length];
    card.style.setProperty("--accent", color);
    card.tabIndex = project === selected ? 0 : -1;
    card.setAttribute("aria-hidden", String(position === "pos-hidden"));
    card.setAttribute("aria-current", String(project === selected));
    card.disabled = options.busy;
    card.setAttribute("aria-label", `${project === selected && project.availability === "available" ? "Open" : "Select"} ${project.name}, ${index + 1} of ${projects.length}`);
    const top = node("div", "card-top");
    top.append(node("span", "status-pill", project.availability === "available" ? "Available" : "Locate project"));
    if (project.entryId === options.preselectedId) top.prepend(node("span", "last-opened-tag", "Last opened"));
    const summary = summaries.find((summary) => summary.root === project.canonicalPath);
    const info = coverSummary(summary);
    const body = node("div", "cover-identity");
    const contextText = summary ? info.context : project.canonicalPath;
    const context = node("p", "cover-context", contextText); context.title = contextText;
    body.append(node("h3", "", project.name), context);
    const metrics = node("div", "cover-metrics");
    for (const metric of info.metrics) {
      const field = node("span", "cover-metric");
      field.append(node("strong", "", String(metric.value)), node("small", "", metric.label));
      metrics.append(field);
    }
    const cache = node("div", "cover-cache-info");
    if (!summary) {
      const stack = project.stack;
      const languages = stack?.languages.filter((language) => language.files > 0).slice().sort((a, b) => b.files - a.files).slice(0, 2).map((language) => language.language).join(" · ");
      if (stack) cache.append(node("strong", "", "Folder contents"), node("span", "", `${languages ? `${languages} · ` : ""}${stack.trackedFiles} tracked files`));
    }
    const timestamp = info.syncedAt === null ? new Date(project.lastOpenedAt).getTime() : info.syncedAt * 1000;
    const freshness = timeLabel("cover-freshness", info.syncedAt === null ? "Opened " : "Synced ", timestamp);
    card.replaceChildren(top, body, summary ? metrics : cache, freshness);
    thumbnail.className = `strip-item${project === selected ? " active" : ""}`;
    thumbnail.disabled = options.busy;
    thumbnail.setAttribute("aria-current", String(project === selected));
    thumbnail.setAttribute("aria-label", `Select ${project.name}`);
    thumbnail.title = project.name;
    // One selection style: the active row. The last-opened project is a text
    // tag, never a second highlight.
    const row = landingRowDetails(project, summary);
    const name = node("strong", "", project.name);
    const meta = node("span", "strip-meta");
    const path = node("span", "strip-path", row.path); path.title = project.canonicalPath;
    meta.append(path, ...row.counts.map((count) => node("span", "", count)), node("span", "", row.opened));
    thumbnail.replaceChildren(name, meta);
    if (project.entryId === options.preselectedId) thumbnail.append(node("span", "last-opened-tag", "Last opened"));
  });
  if (state.selectedId !== selected.entryId) {
    const thumbnail = state.entries.get(selected.entryId)!.thumbnail;
    // Scroll only the horizontal index, never the page containing the chooser.
    if (strip.classList.contains("is-list")) {
      const top = thumbnail.offsetTop, bottom = top + thumbnail.offsetHeight;
      if (top < strip.scrollTop) strip.scrollTop = top;
      else if (bottom > strip.scrollTop + strip.clientHeight) strip.scrollTop = bottom - strip.clientHeight;
    } else {
      const left = thumbnail.offsetLeft, right = left + thumbnail.offsetWidth;
      if (left < strip.scrollLeft) strip.scrollLeft = left;
      else if (right > strip.scrollLeft + strip.clientWidth) strip.scrollLeft = right - strip.clientWidth;
    }
  }
  state.selectedId = selected.entryId;
  syncStripOverflow(strip);
  if (focusSurface && document.activeElement !== focused) {
    const fallback = state.entries.get(selected.entryId)!;
    (focused?.isConnected ? focused : focusSurface === "stage" ? fallback.card : fallback.thumbnail).focus({ preventScroll: true });
  }
  renderDetail(detail, selected, summaries.find((item) => item.root === selected.canonicalPath), options);
}
function renderDetail(detail: HTMLElement, selected: RecentProjectEntry, summary: Summary | undefined, options: LandingRenderOptions) {
  const copy = node("div", "selected-copy");
  const heading = node("div", "selected-heading");
  const title = node("h3", "", selected.name);
  const description = node("p", "selected-path", selected.canonicalPath);
  description.id = "orbit-selected-path"; description.title = selected.canonicalPath;
  const actions = node("div", "orbit-selected-actions"); options.actions(actions, selected, description.id);
  heading.append(title, actions);
  copy.append(heading, description);
  const updates = summary?.activity.filter((item) => Number.isFinite(item.updatedAt) && item.updatedAt > 0 && item.updatedAt <= Date.now() / 1000)
    .sort((a, b) => b.updatedAt - a.updatedAt).slice(0, 3) ?? [];
  if (updates.length) {
    const recent = node("div", "selected-updates");
    recent.append(node("h4", "", "Recent updates"));
    for (const update of updates) {
      const row = node("div", "selected-update");
      const meta = node("span", "selected-update-meta", `${update.kind} · ${update.status} · `);
      meta.append(timeLabel("", "", update.updatedAt * 1000));
      row.append(node("span", "selected-update-title", update.title), meta);
      recent.append(row);
    }
    copy.append(recent);
  } else if (summary) copy.append(node("p", "selected-empty-updates", "No recent updates."));
  const stack = selected.stack;
  if (stack) {
    const folder = node("div", "selected-folder-details");
    folder.append(node("h4", "", "Folder details"));
    for (const language of stack.languages.filter((item) => item.files > 0).slice().sort((a, b) => b.files - a.files).slice(0, 4)) {
      const row = node("div", "selected-language");
      row.append(node("span", "", language.language), node("span", "", `${language.files} files`));
      folder.append(row);
    }
    folder.append(node("p", "", `${stack.trackedFiles} tracked files`));
    copy.append(folder);
  }
  copy.append(timeLabel("selected-last-opened", "Last opened ", new Date(selected.lastOpenedAt).getTime()));
  detail.append(copy);
}

// --------------------------------------------------- landing controller

const landingFilters: readonly LandingFilter[] = ["all", "available", "synced"];

// What the landing status line says while a recent-project action runs.
const recentOperationStatus: Partial<Record<string, (name: string) => string>> = {
  picking: (name) => `Choose the folder for “${name}”.`,
  resolving: (name) => `Checking “${name}”…`,
  "confirming-relocation": (name) => `Waiting for confirmation for “${name}”.`,
  opening: (name) => `Opening “${name}”…`,
  "confirming-forget": (name) => `Waiting for confirmation to remove “${name}” from Recent projects.`,
  forgetting: (name) => `Removing “${name}” from Recent projects…`,
};

const overviewStatColors = ["#AFA8FF", "#5FAFFF", "#3DD6A3", "#AFA8FF"];

/** The four landing totals; a dash for anything no summary has counted yet. */
export function overviewTotals(overview: Overview | null): Array<[string, string | number]> {
  return [
    ["Tracked projects", overview?.trackedProjects ?? "—"],
    ["Active plans", overview?.summarizedProjects ? overview.counts.activePlans : "—"],
    ["Open tasks", overview?.summarizedProjects ? overview.counts.openTasks : "—"],
    ["Open issues", overview?.summarizedProjects ? overview.counts.openIssues : "—"],
  ];
}

export function createLandingController(ctx: AppContext) {
  const { api, workspaceController } = ctx;
  const elements = {
    recentError: element("#recent-project-error", HTMLParagraphElement),
    recentStatus: element("#recent-project-status", HTMLParagraphElement),
    recents: element("#recent-project-list", HTMLDivElement),
    stateInitialize: element("#state-initialize-project-button", HTMLButtonElement),
    stateOpen: element("#state-open-project-button", HTMLButtonElement),
    search: element("#recent-project-search", HTMLInputElement),
    projectCount: element("#overview-project-count", HTMLSpanElement),
    refreshButton: element("#overview-refresh-button", HTMLButtonElement),
    overviewCounts: element("#global-overview-counts", HTMLDivElement),
    overviewCoverage: element("#global-overview-coverage", HTMLParagraphElement),
    overviewActivity: element("#global-overview-activity", HTMLDivElement),
    syncStatus: element("#orbit-sync-status", HTMLElement),
    refreshStatus: element("#overview-refresh-status", HTMLParagraphElement),
    overviewPeriod: element("#global-overview-period", HTMLSelectElement),
    previous: element("#orbit-previous", HTMLButtonElement),
    next: element("#orbit-next", HTMLButtonElement),
    activityToggle: element("#orbit-activity-toggle", HTMLButtonElement),
    activityPanel: element("#orbit-activity-panel", HTMLElement),
    activityClose: element("#orbit-activity-close", HTMLButtonElement),
  };

  let globalOverview: Overview | null = null;
  let landingSelectedId = "";
  let landingFilter: LandingFilter = "all";
  let overviewError = "";
  let overviewRequest = 0;
  let overviewLoading = false;
  let overviewRefreshNotice = "";

  function visibleProjects(): RecentProjectEntry[] {
    return landingProjects(ctx.state.recentProjectsState.projects, globalOverview, elements.search.value, landingFilter);
  }

  function renderRecentStatus(projects: RecentProjectEntry[], preselectedEntryId: string, operationActive: boolean): void {
    const recent = ctx.state.recentProjectsState;
    const preselected = projects.find(
      (project) => project.entryId === preselectedEntryId,
    );
    elements.recentStatus.textContent = recent.announcement ||
      (preselected
        ? `“${preselected.name}” is preselected as the last project p-track recorded. Confirm it to continue.`
        : "");
    elements.recentError.textContent = recent.message ||
      recent.listError;
    const active = projects.find(
      (project) => project.entryId === recent.activeEntryId,
    );
    if (active && operationActive) {
      elements.recentStatus.textContent = recentOperationStatus[recent.phase]?.(active.name) || "";
    }
  }

  function renderProjectActions(host: HTMLElement, project: RecentProjectEntry, descriptionId: string): void {
    const primaryAction = recentProjectPrimaryAction(project.availability);
    const button = ctx.recent.recentProjectActionButton(project, primaryAction, ctx.recent.recentProjectPrimaryLabel(project.availability), descriptionId, () => {
      if (primaryAction === "open") void ctx.recent.openAvailableRecentProject(project);
      else if (primaryAction === "retry") void ctx.recent.retryRecentProject(project);
      else void ctx.recent.locateRecentProject(project);
    });
    button.className = "primary-button";
    host.append(button);
    if (project.availability !== "available") {
      const forget = ctx.recent.recentProjectActionButton(project, "forget", "Forget", descriptionId, () => void ctx.recent.forgetRecentProject(project));
      forget.className = "secondary-button";
      host.append(forget);
    }
  }

  function renderRecentProjects(): void {
    const recent = ctx.state.recentProjectsState;
    const projects = visibleProjects();
    const operationActive = ctx.recent.recentProjectOperationActive();
    elements.projectCount.textContent = `${projects.length} / ${recent.projects.length}`;
    ctx.updates.updateAboutUpdatesAvailability();
    elements.stateInitialize.disabled = operationActive;
    elements.stateOpen.disabled = operationActive;
    elements.recents.setAttribute(
      "aria-busy",
      String(recent.listLoading || operationActive),
    );
    // The opted-in last project that did not auto-open is pointed at rather than
    // opened: the row says so, the live region says so, and nothing takes focus.
    const preselectedEntryId = preselectedRecentProject(projects, ctx.state.preferences.startup);
    renderRecentStatus(projects, preselectedEntryId, operationActive);
    landingSelectedId = selectedLandingProject(projects, projects.some((project) => project.entryId === landingSelectedId) ? landingSelectedId : preselectedEntryId)?.entryId || "";
    renderLandingProjects({
      projects, summaries: globalOverview?.projects || [], selectedId: landingSelectedId, preselectedId: preselectedEntryId,
      busy: operationActive || recent.listLoading, loading: recent.listLoading,
      select: (id) => { landingSelectedId = id; renderRecentProjects(); },
      open: (entry) => void ctx.recent.openAvailableRecentProject(entry),
      actions: renderProjectActions,
    });
  }

  async function readGlobalOverview(refresh: boolean): Promise<{ overview: Overview; notice: string }> {
    if (!refresh) return { overview: await api().GetGlobalOverviewV1(), notice: "" };
    const result = await api().RefreshGlobalOverviewV1();
    return { overview: result.overview, notice: overviewRefreshMessage(result) };
  }

  function setRefreshButtonBusy(busy: boolean, refresh: boolean): void {
    elements.refreshButton.disabled = busy;
    elements.refreshButton.textContent = busy
      ? refresh ? "Refreshing…" : "Loading…"
      : "Refresh summaries";
  }

  async function loadGlobalOverview(refresh = false): Promise<void> {
    const request = ++overviewRequest;
    overviewLoading = true;
    setRefreshButtonBusy(true, refresh);
    const ticket = workspaceController.capture();
    const isCurrent = () => {
      const current = workspaceController.capture();
      return request === overviewRequest && current.epoch === ticket.epoch &&
        current.generation === ticket.generation &&
        !["open", "loading"].includes(workspaceController.state.status);
    };
    try {
      const { overview, notice } = await readGlobalOverview(refresh);
      if (!isCurrent()) return;
      globalOverview = overview;
      overviewRefreshNotice = notice;
      overviewError = "";
    } catch (error) {
      if (!isCurrent()) return;
      // Stated on the landing page itself (never a blocking dialog), with the
      // backend's reason so the failure can be acted on.
      const reason = messageFrom(error);
      if (!refresh) globalOverview = null;
      overviewRefreshNotice = refresh ? `Could not refresh summaries: ${reason}. Existing summaries are still available. Try again.` : "";
      overviewError = refresh ? "" : `Could not load summaries: ${reason}. You can still open a project.`;
    } finally {
      if (request === overviewRequest) {
        overviewLoading = false;
        setRefreshButtonBusy(false, refresh);
      }
    }
    renderGlobalOverview();
    renderRecentProjects();
  }

  function cancelGlobalOverviewRead(): void {
    overviewRequest += 1;
  }

  function overviewUpdateRow(update: ReturnType<typeof overviewActivity>[number]): HTMLElement {
    const row = document.createElement("div");
    row.setAttribute("role", "listitem");
    const entry = ctx.state.recentProjectsState.projects.find((project) => project.canonicalPath === update.root);
    row.className = "overview-update";
    const openable = entry?.availability === "available";
    const content = document.createElement(openable ? "button" : "div");
    content.className = "overview-update-content";
    const title = document.createElement("span");
    title.className = "overview-update-title";
    title.textContent = update.title;
    const metadata = document.createElement("span");
    metadata.className = "overview-update-meta";
    const status = document.createElement("span");
    status.className = "overview-status";
    status.dataset.status = update.status;
    status.textContent = update.status;
    const context = document.createElement("span");
    context.textContent = `${entry?.name || update.root.split(/[\\/]/).filter(Boolean).pop() || update.root} · ${update.kind} #${update.id}`;
    context.title = update.root;
    const time = document.createElement("time");
    time.dateTime = new Date(update.updatedAt * 1000).toISOString();
    time.textContent = relativeTime(time.dateTime, "long");
    time.title = `Updated ${new Date(update.updatedAt * 1000).toLocaleString()}`;
    metadata.append(status, context, time);
    content.append(title, metadata);
    if (entry && content instanceof HTMLButtonElement) {
      content.type = "button";
      content.title = `Open ${entry.name}`;
      content.addEventListener("click", () => void ctx.recent.openAvailableRecentProject(entry));
    }
    row.append(content);
    return row;
  }

  function renderOverviewTotals(overview: Overview | null): void {
    for (const [index, [label, value]] of overviewTotals(overview).entries()) {
      const card = document.createElement("div");card.className = "stat-card";
      card.style.setProperty("--stat-color", overviewStatColors[index]);
      const number = document.createElement("span");number.className = "stat-value";number.textContent = String(value);
      const title = document.createElement("span");title.className = "stat-label";title.textContent = label;
      card.append(number, title);elements.overviewCounts.append(card);
    }
  }

  function renderGlobalOverview(): void {
    elements.overviewCounts.replaceChildren();
    elements.overviewActivity.replaceChildren();
    elements.overviewCoverage.textContent = overviewError;
    const overview = globalOverview;
    renderOverviewTotals(overview);
    elements.syncStatus.textContent = overview ? `${overview.summarizedProjects} of ${overview.trackedProjects} projects summarized` : "Synced summaries unavailable";
    elements.refreshStatus.textContent = overviewRefreshNotice;
    if (!overview) return;
    const oldest = Math.min(...overview.projects.map((project) => project.syncedAt));
    elements.overviewCoverage.textContent = overview.projects.length
      ? `Totals count only projects with a summary. The oldest was updated ${relativeTime(oldest * 1000, "long")}.`
      : "No project has a summary yet. Open a project to add its work to these totals.";
    const updates = overviewActivity(overview, elements.overviewPeriod.value === "all" ? null : 30);
    if (!updates.length) elements.overviewActivity.textContent = "No synced record updates in this period.";
    for (const update of updates) elements.overviewActivity.append(overviewUpdateRow(update));
  }

  function moveLandingSelection(direction: number, focus = false): void {
    const projects = visibleProjects();
    if (projects.length < 2 || ctx.recent.recentProjectOperationActive() || ctx.state.recentProjectsState.listLoading) return;
    const selected = selectedLandingProject(projects, landingSelectedId);
    const index = selected ? projects.indexOf(selected) : -1;
    landingSelectedId = projects[Math.max(0, Math.min(projects.length - 1, index + direction))].entryId;
    renderRecentProjects();
    const target = [...document.querySelectorAll<HTMLElement>("[data-orbit-project-id]")].find((button) => button.dataset.orbitProjectId === landingSelectedId);
    target?.scrollIntoView({ block: "nearest", inline: "nearest" });
    if (focus) target?.focus();
  }

  function toggleLandingActivity(open: boolean): void {
    elements.activityPanel.hidden = !open;elements.activityToggle.setAttribute("aria-expanded", String(open));
    (open ? elements.activityClose : elements.activityToggle).focus();
  }

  function selectLandingFilter(button: HTMLElement): void {
    const filter = landingFilters.find((candidate) => candidate === button.dataset.orbitFilter);
    if (!filter) return;
    landingFilter = filter;
    for (const chip of document.querySelectorAll("[data-orbit-filter]")) {
      const selected = chip === button;
      chip.classList.toggle("active", selected);chip.setAttribute("aria-pressed", String(selected));
    }
    renderRecentProjects();
  }

  function bind(): void {
    elements.search.addEventListener("input", renderRecentProjects);
    elements.refreshButton.addEventListener("click", () => {
      if (!overviewLoading) void loadGlobalOverview(true);
    });
    elements.overviewPeriod.addEventListener("change", renderGlobalOverview);
    elements.previous.addEventListener("click", () => moveLandingSelection(-1));
    elements.next.addEventListener("click", () => moveLandingSelection(1));
    for (const button of document.querySelectorAll<HTMLElement>("[data-orbit-filter]")) {
      button.addEventListener("click", () => selectLandingFilter(button));
    }
    bindProjectView();
    elements.activityToggle.addEventListener("click", () => toggleLandingActivity(Boolean(elements.activityPanel.hidden)));
    elements.activityClose.addEventListener("click", () => toggleLandingActivity(false));
    elements.activityPanel.addEventListener("keydown", (event) => { if (event.key === "Escape") { event.preventDefault(); toggleLandingActivity(false); } });
  }

  return {
    bind,
    renderRecentProjects,
    loadGlobalOverview,
    cancelGlobalOverviewRead,
    renderGlobalOverview,
  };
}

export type LandingController = ReturnType<typeof createLandingController>;
