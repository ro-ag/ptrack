import { animate } from "motion";
import { reducedMotionActive } from "../settings/preferences";
import type { RecentProjectEntry } from "./recent-projects";
import type { Overview, Summary } from "./overview";
import { filterRecentProjects } from "./overview";
import { relativeTime } from "./format";

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
        document.documentElement.dataset.reducedMotion || "system",
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
