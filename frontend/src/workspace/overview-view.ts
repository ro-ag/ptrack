import type { AppContext } from "./app-context";
import { RequestSequence } from "./controller";
import { element, emptyMemory, intelligenceItem, pill, statElement, svgElement } from "./dom";
import {
  compactAriaText,
  formatBytes,
  languageLabel,
  relativeTime,
  shortRelativeTime,
} from "./format";
import {
  driftPresentation,
  heatmapWeeks,
  repositoryChips,
  shortenSummaryHashes,
  stackLanguageRows,
  summaryShape,
  summaryShapeCaption,
  type HeatmapDay,
  type StackLanguageRow,
} from "./presentation";
import { timelineBuckets, timelineMarkers, type ProjectTimeline } from "./project-timeline";
import type {
  Board,
  BoardActivity,
  GitBranch,
  GitSection,
  StackProfile,
  WorkspaceSnapshot,
} from "./snapshot-types";
import { severityColors } from "./task-status";

// How many discovered projects the Repository panel lists before summarizing
// the rest. A Cargo workspace routinely discovers more than a dozen.
const STACK_PANEL_PROJECTS = 12;

const HISTORY_BUCKETS = 100;

type DriftFinding = ReturnType<typeof driftPresentation>["findings"][number];

// What each advisory drift finding means, in the words the Overview shows.
const driftCopy: Partial<Record<DriftFinding["kind"], readonly [string, string]>> = {
  checkoutChangedPath: ["Shared checkout change", "Project-level and unattributed"],
  untrackedFile: ["Untracked file", "Project-level and unattributed"],
  crossTaskPathOverlap: ["Possible cross-task path overlap", "Explicit owners on different tasks reported the same current path"],
  taskDriftSignal: ["Possible task drift", "Provider-neutral structured evidence indicates a current scope mismatch"],
};

/**
 * How old the rolling summary is, when the snapshot says. A summary that
 * predates the last release reads very differently from one written this
 * morning; with no summary, or no recorded write, there is nothing to date.
 */
export function summaryAgeLabel(
  summary: string,
  updatedAt: string | null | undefined,
  now = Date.now(),
): string {
  if (!summary || !updatedAt) return "";
  const age = relativeTime(updatedAt, "short", now);
  if (!age) return "";
  // A write under a minute old, or one a skewed clock dates ahead, is fresh.
  return age === "now" || age.startsWith("in ") ? "Updated just now" : `Updated ${age}`;
}

function evidenceSignals(count: number): string {
  return `${count} evidence signal${count === 1 ? "" : "s"}`;
}

// "Rust", "Rust and TypeScript", "Rust, TypeScript and Go" — a list a person
// reads, rather than one joined by separators.
export function languageSentence(languages: readonly string[]): string {
  if (languages.length <= 1) return languages[0] ?? "no known language";
  return `${languages.slice(0, -1).join(", ")} and ${languages[languages.length - 1]}`;
}

// The expanded breakdown: one row per language, largest first. The bar is
// sized by share, but the count sits beside it — a proportion is never the
// only thing rendered.
export function stackBreakdown(rows: readonly StackLanguageRow[]): HTMLDListElement {
  const list = document.createElement("dl");
  list.className = "stack-breakdown";
  list.id = "stack-breakdown";
  rows.forEach((row) => {
    const term = document.createElement("dt");
    term.className = "stack-breakdown-language";
    term.textContent = languageLabel(row.language);
    const detail = document.createElement("dd");
    detail.className = "stack-breakdown-detail";
    const bar = document.createElement("span");
    bar.className = "stack-breakdown-bar";
    bar.setAttribute("aria-hidden", "true");
    const fill = document.createElement("span");
    fill.style.width = `${Math.max(2, Math.round(row.share * 100))}%`;
    bar.append(fill);
    // Files, lines, and projects each get their own cell so the digits line up
    // down the list. A language with no counted lines still emits its cell,
    // empty, rather than shifting every column left on that row.
    const count = document.createElement("span");
    count.className = "stack-breakdown-count";
    count.textContent = `${row.files.toLocaleString()} file${row.files === 1 ? "" : "s"}`;
    const lines = document.createElement("span");
    lines.className = "stack-breakdown-lines";
    lines.textContent = row.lines > 0 ? `${row.lines.toLocaleString()} lines` : "";
    const scope = document.createElement("span");
    scope.className = "stack-breakdown-scope";
    scope.textContent = row.projects === 1 ? "1 project" : `${row.projects} projects`;
    detail.append(bar, count, lines, scope);
    list.append(term, detail);
  });
  return list;
}

export function activityElement(activity: BoardActivity, expanded = false): HTMLElement {
  const item = document.createElement("article");
  item.className = expanded ? "activity activity-expanded" : "activity";
  item.style.setProperty(
    "--activity-color",
    activity.kind === "commit" ? "var(--todo)" : "var(--accent)",
  );
  const title = document.createElement("p");
  title.className = "activity-title";
  title.textContent = activity.title;
  const detail = document.createElement("p");
  detail.className = "activity-detail";
  detail.textContent = activity.detail;
  const meta = document.createElement("span");
  meta.className = "activity-meta";
  meta.textContent = `${activity.kind} · ${activity.target} · ${shortRelativeTime(activity.occurredAt)}`;
  item.append(title, detail, meta);
  return item;
}

// The ring's arc carries a gradient along its own length, dim where the arc
// starts to bright where it ends, so the eye follows the direction of
// progress. The stop colours come from CSS so the ring re-tints with the
// theme; the sweep runs corner to corner because the arc starts at the top and
// travels clockwise.
function planRingGradient(): SVGElement {
  const defs = svgElement("defs");
  // The axis starts at twelve o'clock, where the arc starts, and runs to the
  // bottom right, so a short arc still travels most of the ramp instead of
  // sampling a sliver of it in the middle.
  const gradient = svgElement("linearGradient", {
    id: "plan-ring-sweep",
    x1: "0.5",
    y1: "0",
    x2: "1",
    y2: "1",
  });
  gradient.append(
    svgElement("stop", { offset: "0", class: "plan-ring-sweep-from" }),
    svgElement("stop", { offset: "0.35", class: "plan-ring-sweep-deep" }),
    svgElement("stop", { offset: "0.72", class: "plan-ring-sweep-mid" }),
    svgElement("stop", { offset: "1", class: "plan-ring-sweep-to" }),
  );
  defs.append(gradient);
  return defs;
}

/** The project-progress ring; `total` must be positive. */
export function planRingSvg(done: number, total: number): SVGElement {
  const radius = 34;
  const circumference = 2 * Math.PI * radius;
  const fraction = Math.min(1, done / total);
  const svg = svgElement("svg", {
    viewBox: "0 0 84 84",
    class: "plan-ring-svg",
    "aria-hidden": "true",
  });
  svg.append(
    planRingGradient(),
    svgElement("circle", { class: "plan-ring-track", cx: 42, cy: 42, r: radius }),
    svgElement("circle", {
      class: "plan-ring-value",
      cx: 42,
      cy: 42,
      r: radius,
      "stroke-dasharray": `${circumference}`,
      "stroke-dashoffset": `${circumference * (1 - fraction)}`,
      transform: "rotate(-90 42 42)",
    }),
  );
  const number = svgElement("text", {
    class: "plan-ring-number",
    x: 42,
    y: 40,
    "text-anchor": "middle",
  });
  number.textContent = `${Math.round(fraction * 100)}%`;
  const caption = svgElement("text", {
    class: "plan-ring-caption",
    x: 42,
    y: 54,
    "text-anchor": "middle",
  });
  caption.textContent = "tasks done";
  svg.append(number, caption);
  return svg;
}

/** The daily activity grid, its legend, and the totals row. */
export function heatmapChart(days: readonly HeatmapDay[]): [HTMLDivElement, HTMLDivElement] {
  const columns = heatmapWeeks([...days]);
  // The chart is read as a shape, not cell by cell, so the cells are sized for
  // the shape rather than for pointing at one day. Keeping the block short is
  // what lets Activity sit beside Status on a wide display instead of taking a
  // row of its own. The SVG scales, so retina sharpness is unaffected.
  const cell = 7;
  const pitch = 9;
  // Everything here is in viewBox units and scales with the drawing, labels
  // included, so the gutters are sized against the label size rather than
  // against pixels. The right margin exists because the last month label
  // starts on its column and runs past it.
  const left = 16;
  const top = 10;
  const right = 10;
  const width = left + columns.length * pitch + right;
  const height = top + 7 * pitch;
  const chart = document.createElement("div");
  chart.className = "heatmap-chart";
  const svg = svgElement("svg", {
    viewBox: `0 0 ${width} ${height}`,
    class: "heatmap-svg",
    role: "img",
    "aria-label": "Daily note and commit activity for the last 16 weeks",
  });
  const weekdays: ReadonlyArray<readonly [string, number]> = [["Mon", 1], ["Wed", 3], ["Fri", 5]];
  weekdays.forEach(([label, row]) => {
    const text = svgElement("text", { x: 0, y: top + row * pitch + cell, class: "heatmap-label" });
    text.textContent = label;
    svg.append(text);
  });
  let previousMonth = "";
  columns.forEach((column, x) => {
    const first = column.find((day) => day.date);
    const month = first?.date.slice(0, 7);
    if (first && month && month !== previousMonth) {
      const label = svgElement("text", { x: left + x * pitch, y: 6, class: "heatmap-label" });
      label.textContent = new Date(`${first.date}T12:00:00Z`).toLocaleDateString(undefined, { month: "short", timeZone: "UTC" });
      svg.append(label);
      previousMonth = month;
    }
    column.forEach((day, y) => {
      if (!day.date) return;
      const rect = svgElement("rect", {
        class: `heatmap-cell heatmap-level-${day.level}`,
        x: left + x * pitch,
        y: top + y * pitch,
        width: cell,
        height: cell,
        rx: 3,
      });
      const tip = svgElement("title");
      tip.textContent = `${day.count} ${day.count === 1 ? "item" : "items"} · ${day.date}`;
      rect.append(tip);
      svg.append(rect);
    });
  });
  const legend = document.createElement("div");
  legend.className = "heatmap-legend";
  legend.append("Less");
  for (let level = 0; level <= 4; level += 1) {
    const swatch = svgElement("svg", { viewBox: "0 0 12 12", "aria-hidden": "true" });
    swatch.append(svgElement("rect", { width: 12, height: 12, rx: 2, class: `heatmap-cell heatmap-level-${level}` }));
    legend.append(swatch);
  }
  legend.append("More");
  chart.append(svg, legend);
  const totals = document.createElement("div");
  totals.className = "activity-totals";
  const total = days.reduce((sum, day) => sum + day.count, 0);
  const active = days.filter((day) => day.count > 0).length;
  totals.append(
    statElement(total.toLocaleString(), "Notes + commits"),
    statElement(active, "Active days"),
    statElement(Math.max(...days.map((day) => day.count)), "Most in one day"),
    statElement((total / Math.max(1, days.length / 7)).toFixed(1), "Average per week"),
  );
  return [chart, totals];
}

// Heights are square-rooted before they are drawn. Commit activity is heavily
// skewed — one release day can carry fifty times a normal one — and against a
// raw maximum every other week collapses into a flat line at the bottom of the
// card, which is the opposite of a history. The transform is monotonic, so the
// busiest period is still unmistakably the tallest; it just stops erasing the
// rest of the project.
export function historyPath(
  buckets: readonly { count: number }[],
  width: number,
  floor: number,
  peak: number,
  close: boolean,
): string {
  const step = buckets.length > 1 ? width / (buckets.length - 1) : 0;
  const ceiling = Math.sqrt(peak) || 1;
  const points = buckets.map((bucket, index) => {
    const x = index * step;
    const y = floor - (Math.sqrt(bucket.count) / ceiling) * (floor - 8);
    return `${x.toFixed(1)} ${y.toFixed(1)}`;
  });
  const line = `M ${points.join(" L ")}`;
  return close ? `${line} L ${width} ${floor} L 0 ${floor} Z` : line;
}

// ------------------------------------------------------- project history
//
// The repository's whole life, read from git rather than from p-track's own
// commit records: those begin only when the hook was installed, so a chart
// built from them would draw a project that started the day tracking did.
//
// The shape is drawn by hand. A charting library was tried here and removed:
// at a hundred buckets across the card the curve smoothing it offered is not
// visible, and it cost a dependency and its own visual idiom.
export function historySvg(timeline: ProjectTimeline): SVGElement {
  const width = 720;
  const height = 96;
  const floor = height - 22;
  const buckets = timelineBuckets(timeline.commits, HISTORY_BUCKETS);
  const peak = buckets.reduce((top, bucket) => Math.max(top, bucket.count), 0) || 1;

  const svg = svgElement("svg", {
    viewBox: `0 0 ${width} ${height}`,
    class: "history-svg",
    role: "img",
    preserveAspectRatio: "none",
    "aria-label": `Git commit history, ${timeline.commits.length} Git commits`,
  });

  const defs = svgElement("defs");
  const gradient = svgElement("linearGradient", {
    id: "history-sweep",
    x1: "0",
    y1: "0",
    x2: "0",
    y2: "1",
  });
  gradient.append(
    svgElement("stop", { offset: "0", class: "history-stop-strong" }),
    svgElement("stop", { offset: "1", class: "history-stop-dim" }),
  );
  defs.append(gradient);
  svg.append(defs);

  svg.append(
    svgElement("path", {
      d: historyPath(buckets, width, floor, peak, true),
      class: "history-area",
      fill: "url(#history-sweep)",
    }),
    svgElement("path", { d: historyPath(buckets, width, floor, peak, false), class: "history-line" }),
    svgElement("line", { x1: 0, y1: floor, x2: width, y2: floor, class: "history-axis" }),
  );

  // Releases are the landmarks a reader navigates this by.
  timelineMarkers(timeline).forEach((marker) => {
    const at = marker.position * width;
    const rule = svgElement("line", { x1: at, y1: 4, x2: at, y2: floor, class: "history-marker" });
    const tip = svgElement("title");
    tip.textContent = `${marker.name} · ${new Date(marker.at * 1000).toLocaleDateString()}`;
    rule.append(tip);
    const label = svgElement("text", { x: at, y: height - 6, class: "history-marker-label" });
    label.textContent = marker.name;
    label.setAttribute("text-anchor", at > width - 40 ? "end" : at < 40 ? "start" : "middle");
    svg.append(rule, label);
  });
  return svg;
}

export function historyCaption(timeline: ProjectTimeline): string {
  const first = new Date(timeline.commits[0] * 1000);
  const last = new Date(timeline.commits[timeline.commits.length - 1] * 1000);
  const span = `${first.toLocaleDateString()} to ${last.toLocaleDateString()}`;
  return timeline.truncated
    ? `${timeline.commits.length.toLocaleString()} most recent Git commits, ${span}`
    : `${timeline.commits.length.toLocaleString()} Git commits, ${span}`;
}

function branchItem(branch: GitBranch): HTMLElement {
  const flags = [
    branch.current ? "current" : "",
    branch.remote ? "remote" : "local",
    branch.stale ? "stale signal" : "",
    branch.worktreePath ? `worktree ${branch.worktreePath}` : "",
  ].filter(Boolean);
  return intelligenceItem(branch.name, `${flags.join(" · ")} · ${shortRelativeTime(branch.lastCommitAt)}`, branch.stale ? "stale" : "");
}

export function createOverviewView(ctx: AppContext) {
  const { api, showError, workspaceController } = ctx;
  const elements = {
    activity: element("#activity-list", HTMLDivElement),
    activityMore: element("#activity-more", HTMLButtonElement),
    agentDrift: element("#agent-drift", HTMLDivElement),
    blockers: element("#overview-blockers", HTMLDivElement),
    gitBranches: element("#git-branches", HTMLDivElement),
    gitCommits: element("#git-commits", HTMLDivElement),
    gitCommitsDisclosure: element("#git-commits-disclosure", HTMLDetailsElement),
    gitRemotes: element("#git-remotes", HTMLDivElement),
    gitRemotesDisclosure: element("#git-remotes-disclosure", HTMLDetailsElement),
    gitState: element("#git-state", HTMLSpanElement),
    gitSummary: element("#git-summary", HTMLDivElement),
    goal: element("#goal", HTMLParagraphElement),
    heatmap: element("#activity-heatmap", HTMLDivElement),
    historyCaption: element("#history-caption", HTMLSpanElement),
    issueTotal: element("#issue-total", HTMLSpanElement),
    issues: element("#issue-list", HTMLDivElement),
    memoryDialogClose: element("#memory-dialog-close", HTMLButtonElement),
    memoryDialogList: element("#memory-dialog-list", HTMLDivElement),
    memoryModal: element("#memory-modal", HTMLDivElement),
    notes: element("#overview-notes", HTMLDivElement),
    overviewPage: element("#overview-page", HTMLElement),
    planRing: element("#plan-ring", HTMLDivElement),
    projectHistory: element("#project-history", HTMLDivElement),
    projectRoot: element("#project-root", HTMLParagraphElement),
    snapshotBounds: element("#snapshot-bounds", HTMLDivElement),
    stackEmpty: element("#stack-empty", HTMLParagraphElement),
    stackProjects: element("#stack-projects", HTMLDivElement),
    stackProjectsDisclosure: element("#stack-projects-disclosure", HTMLDetailsElement),
    stackProjectsSummary: element("#stack-projects-summary", HTMLElement),
    stackRescan: element("#stack-rescan", HTMLButtonElement),
    stackScanned: element("#stack-scanned", HTMLParagraphElement),
    stackSummary: element("#stack-summary", HTMLDivElement),
    stats: element("#project-stats", HTMLDivElement),
    storageStatus: element("#storage-status", HTMLParagraphElement),
    summary: element("#summary", HTMLParagraphElement),
    summaryAge: element("#summary-age", HTMLParagraphElement),
    summaryExpand: element("#summary-expand", HTMLButtonElement),
    summaryFlag: element("#summary-flag", HTMLSpanElement),
    summaryMetrics: element("#summary-metrics", HTMLSpanElement),
    summaryShapeRow: element("#summary-shape", HTMLParagraphElement),
  };

  const stackProfileRequests = new RequestSequence();
  const heatmapRequests = new RequestSequence();
  let memoryModalReturnFocus: HTMLElement | null = null;
  let heatmapRequested = false;
  // A forced rescan owns the stack panel until it answers; a plain re-read
  // started meanwhile would only race it with the stored, pre-rescan profile.
  let stackRescanInFlight = false;
  let stackProfile: StackProfile | null = null;
  let stackDetailExpanded = false;
  let projectHistory: ProjectTimeline | null = null;
  let projectHistoryRequested = false;

  // The Tracked files tile doubles as the breakdown's disclosure control: the
  // languages live one click away instead of crowding the tile row.
  //
  // This is a native <details>, not a button with a click handler. The browser
  // owns the open/closed state, so the interaction cannot be broken by anything
  // that happens during a re-render — and it keeps keyboard and screen-reader
  // behaviour for free.
  function stackDisclosure(profile: StackProfile, rows: readonly StackLanguageRow[]): HTMLDetailsElement {
    const details = document.createElement("details");
    details.className = "stat stack-details";
    details.open = stackDetailExpanded;

    const summary = document.createElement("summary");
    summary.className = "stack-details-summary";
    const label = document.createElement("span");
    label.className = "stat-label";
    // The tile column is narrow; a longer label truncates. The language count
    // lives in the tooltip and in the breakdown the tile opens.
    label.textContent = "Tracked files";
    const value = document.createElement("span");
    value.className = "stat-value";
    value.textContent = (profile.trackedFiles ?? 0).toLocaleString();
    const discovered = rows.length === 1 ? "1 language" : `${rows.length} languages`;
    summary.title = profile.linesCounted
      ? `${discovered}, ${(profile.lines ?? 0).toLocaleString()} lines`
      : discovered;
    summary.append(label, value);
    details.append(summary);

    if (rows.length) details.append(stackBreakdown(rows));
    // Remember the state so a snapshot refresh does not collapse the panel
    // under the reader.
    details.addEventListener("toggle", () => {
      stackDetailExpanded = details.open;
    });
    return details;
  }

  // Re-rendering the Overview empties several tall lists before refilling them,
  // and fitRecentMemory reads layout while they are empty. That clamps the
  // page's scrollTop to the momentarily shorter content, and refilling never
  // puts it back — so every snapshot, rescan and heatmap load threw the reader
  // back to the top of the page. Capture the offset around any re-render and
  // restore it once the DOM is whole, including the frame in which the
  // late-settling parts finish.
  function withOverviewScrollPreserved(render: () => void): void {
    const page = elements.overviewPage;
    if (page.hidden) {
      render();
      return;
    }
    const top = page.scrollTop;
    try {
      render();
    } finally {
      if (page.scrollTop !== top) page.scrollTop = top;
      requestAnimationFrame(() => {
        if (!page.hidden && page.scrollTop !== top) page.scrollTop = top;
      });
    }
  }

  function fitRecentMemory(): void {
    if (!ctx.state.board || elements.activity.children.length === 0) return;
    const items = Array.from(elements.activity.children).filter(
      (item): item is HTMLElement => item instanceof HTMLElement,
    );
    items.forEach((item) => {
      item.hidden = false;
    });
    elements.activityMore.hidden = true;
    if (elements.activity.scrollHeight <= elements.activity.clientHeight + 1) return;

    elements.activityMore.hidden = false;
    const available = elements.activity.clientHeight;
    let visible = 0;
    items.forEach((item, index) => {
      const fits = item.offsetTop + item.offsetHeight <= available;
      item.hidden = !fits && index > 0;
      if (!item.hidden) visible += 1;
    });
    const hidden = Math.max(0, items.length - visible);
    elements.activityMore.hidden = hidden === 0;
    elements.activityMore.setAttribute(
      "aria-label",
      hidden === 1 ? "Show 1 more memory item" : `Show ${hidden} more memory items`,
    );
  }

  // A summary that keeps to the guide is shown whole, as prose. One written as a
  // digest of notes cannot be made readable by styling it, so the card folds it
  // and says which rule it broke instead of pretending it reads.
  function renderSummary(board: Board): void {
    const text = board.summary ?? "";
    const display =
      text || "No rolling summary yet. Agents can update it with ptrack summary set.";
    // Every snapshot re-renders this card, and folding a summary the reader
    // deliberately opened is the same bug as scrolling them back to the top.
    // Only new text earns a fresh fold; the same text keeps the state it was
    // left in. Read before the write, or the comparison is against itself.
    const unchanged = elements.summary.dataset.source === display;
    const expanded = unchanged && elements.summary.dataset.expanded === "true";

    elements.summary.dataset.source = display;
    elements.summary.replaceChildren(
      ...shortenSummaryHashes(display).map((segment) => {
        if (!segment.full) return document.createTextNode(segment.text);
        const hash = document.createElement("abbr");
        hash.className = "summary-hash";
        hash.title = segment.full;
        hash.textContent = segment.text;
        return hash;
      }),
    );
    const age = summaryAgeLabel(text, board.summaryUpdatedAt);
    elements.summaryAge.hidden = !age;
    elements.summaryAge.textContent = age;
    const shape = text ? summaryShape(text) : null;
    const dense = Boolean(shape?.problem);
    elements.summary.dataset.dense = dense ? "true" : "false";
    elements.summary.dataset.expanded = expanded ? "true" : "false";
    elements.summaryFlag.hidden = !dense;
    elements.summaryShapeRow.hidden = !dense;
    elements.summaryExpand.textContent = expanded ? "Show less" : "Show all";
    elements.summaryMetrics.textContent = dense && shape ? summaryShapeCaption(shape) : "";
  }

  // The Overview is project-wide: totals never change with the selected plan
  // (the per-plan numbers stay on the board header).
  function renderProjectStats(board: Board): void {
    const progress = document.createElement("div");
    progress.className = "status-progress";
    const metrics: ReadonlyArray<readonly [number, number, string]> = [
      [board.stats.tasksDone, board.stats.tasks, "Tasks"],
      [board.stats.plansDone, board.stats.plans, "Plans"],
      [board.stats.milestonesDone, board.stats.milestones, "Milestones"],
    ];
    metrics.filter(([, total, label]) => total || label !== "Milestones").forEach(([done, total, label]) => {
      const metric = statElement(`${done}/${total}`, label);
      // Tasks get no bar: the ring beside this tile is already that bar, drawn
      // from the same two numbers. The fraction stays as the exact count the
      // ring rounds off.
      if (label !== "Tasks") {
        const bar = document.createElement("progress");
        bar.max = total || 1;
        bar.value = done;
        bar.setAttribute("aria-label", `${label}: ${done} of ${total} done`);
        metric.append(bar);
      }
      progress.append(metric);
    });
    const counts = document.createElement("div");
    counts.className = "status-counts";
    counts.append(
      statElement(board.stats.tasksOpen, "Open tasks"),
      statElement(board.stats.tasksBlocked, "Blocked"),
      statElement(board.stats.openIssues, "Open issues"),
      statElement(board.stats.notes, "Notes"),
      // Commits recorded against tasks, not the repository's Git history (the
      // Project history chart counts those).
      statElement(board.stats.commits, "Linked commits"),
    );
    // Tracked files, counted from the manifests git tracks. A line count is not
    // reported: one vendored directory or generated bundle outweighs the code
    // that defines the project. The tile expands into the per-language
    // breakdown rather than spending a tile on each language — a Cargo
    // workspace discovers a project per crate, and those tiles all read "Rust".
    if (stackProfile?.state === "ready") {
      counts.append(stackDisclosure(stackProfile, stackLanguageRows(stackProfile.projects ?? [])));
      if (stackProfile.incomplete) {
        counts.append(statElement("partial", "Scan truncated"));
      }
    }
    elements.stats.replaceChildren(progress, counts);
    renderPlanRing(board.stats.tasksDone, board.stats.tasks);
  }

  function renderOpenIssues(board: Board): void {
    elements.issueTotal.textContent = String(board.stats.openIssues);
    elements.issues.replaceChildren();
    if (board.openIssues.length === 0) {
      elements.issues.append(emptyMemory("No open issues. The path is clear."));
      return;
    }
    board.openIssues.forEach((issue) => {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "issue";
      item.style.setProperty("--issue-color", severityColors[issue.severity] || "var(--muted)");
      const marker = document.createElement("span");
      marker.className = "issue-marker";
      marker.setAttribute("aria-hidden", "true");
      const content = document.createElement("div");
      const title = document.createElement("p");
      title.className = "issue-title";
      title.textContent = issue.title;
      const meta = document.createElement("span");
      meta.className = "issue-meta";
      meta.textContent = `${issue.severity} · #${issue.id}${issue.taskId ? ` · task #${issue.taskId}` : ""}`;
      content.append(title, meta);
      item.append(marker, content);
      item.setAttribute(
        "aria-label",
        `Open issue #${issue.id}, ${issue.severity}: ${compactAriaText(issue.title)}`,
      );
      item.addEventListener("click", () => ctx.issues.openIssueDetail(issue.id, item));
      elements.issues.append(item);
    });
  }

  function renderRecentMemory(board: Board): void {
    elements.activity.replaceChildren();
    elements.memoryDialogList.replaceChildren();
    if (board.activity.length === 0) {
      const message = "Decisions and linked commits will appear here as the project evolves.";
      elements.activity.append(emptyMemory(message));
      elements.memoryDialogList.append(emptyMemory(message));
      elements.activityMore.hidden = true;
      return;
    }
    board.activity.forEach((activity) => {
      elements.activity.append(activityElement(activity));
      elements.memoryDialogList.append(activityElement(activity, true));
    });
    requestAnimationFrame(fitRecentMemory);
  }

  function renderMemory(): void {
    const board = ctx.state.board;
    if (!board) return;
    elements.goal.textContent = board.goal || "No north star set for this project.";
    renderSummary(board);
    renderProjectStats(board);
    renderOpenIssues(board);
    renderRecentMemory(board);
  }

  function renderPlanRing(done: number, total: number): void {
    elements.planRing.replaceChildren();
    if (!total) {
      elements.planRing.hidden = true;
      return;
    }
    elements.planRing.hidden = false;
    elements.planRing.setAttribute(
      "aria-label",
      `Project progress: ${done} of ${total} tasks done`,
    );
    elements.planRing.append(planRingSvg(done, total));
  }

  function renderHeatmap(days: readonly HeatmapDay[]): void {
    elements.heatmap.replaceChildren();
    if (!days.length) {
      elements.heatmap.append(emptyMemory("No activity recorded yet."));
      return;
    }
    elements.heatmap.append(...heatmapChart(days));
  }

  function renderProjectHistory(): void {
    const host = elements.projectHistory;
    host.replaceChildren();
    const timeline = projectHistory;
    if (!timeline?.available || timeline.commits.length === 0) {
      elements.historyCaption.textContent = "";
      host.append(
        emptyMemory("No repository history to draw. This reads git, not p-track's commit records."),
      );
      return;
    }
    host.append(historySvg(timeline));
    elements.historyCaption.textContent = historyCaption(timeline);
  }

  // Read once per project and re-read only when a snapshot lands, since the
  // backend reuses its answer until HEAD moves.
  async function loadProjectHistory(force = false): Promise<void> {
    if (workspaceController.state.status !== "open") return;
    if (projectHistoryRequested && !force) return;
    projectHistoryRequested = true;
    const ticket = workspaceController.capture();
    try {
      const response = await api().GetProjectTimelineV1();
      if (!workspaceController.accepts(ticket, ticket.generation)) return;
      projectHistory = response;
      withOverviewScrollPreserved(renderProjectHistory);
    } catch {
      if (!workspaceController.accepts(ticket, ticket.generation)) return;
      projectHistoryRequested = false;
      projectHistory = null;
      withOverviewScrollPreserved(renderProjectHistory);
    }
  }

  // The stack profile is re-read on every snapshot and when the Overview is
  // first shown. A plain read never forces a scan: the backend rescans on its
  // own only when HEAD moved since the stored profile, so a re-read is usually
  // a stored read. Only the Repository panel's Rescan control (`rescan`)
  // forces a full scan. A failed read shows the failure and waits for the next
  // snapshot's plain read; it never escalates to a forced scan.
  async function loadStackProfile(rescan = false): Promise<void> {
    if (workspaceController.state.status !== "open") return;
    if (stackRescanInFlight && !rescan) return;
    const ticket = workspaceController.capture();
    const request = stackProfileRequests.next();
    if (rescan) {
      stackRescanInFlight = true;
      stackProfile = { state: "scanning" };
      withOverviewScrollPreserved(renderStackProfile);
    }
    const current = () => stackProfileRequests.isCurrent(request) &&
      workspaceController.accepts(ticket, ticket.generation);
    try {
      const response = await api().GetStackProfileV1(rescan);
      if (!current()) return;
      stackProfile = response;
    } catch {
      if (!current()) return;
      stackProfile = { state: "failed" };
    } finally {
      if (rescan && stackProfileRequests.isCurrent(request)) stackRescanInFlight = false;
    }
    withOverviewScrollPreserved(() => {
      if (ctx.state.board) renderMemory();
      renderStackProfile();
    });
  }

  // A snapshot that lands while the Overview is shown re-reads the lazy panels
  // the Overview already asked for.
  function reloadRequestedOverviewPanels(): void {
    if (ctx.state.view === "overview" && heatmapRequested) void loadHeatmap(true);
    if (ctx.state.view === "overview" && projectHistoryRequested) void loadProjectHistory(true);
  }

  // Everything the Overview read for the project being left.
  function resetOverviewProjectData(): void {
    heatmapRequested = false;
    projectHistoryRequested = false;
    projectHistory = null;
    stackProfileRequests.invalidate();
    stackRescanInFlight = false;
    heatmapRequests.invalidate();
    stackProfile = null;
  }

  function clearOverviewCharts(): void {
    elements.heatmap.replaceChildren();
    elements.projectHistory.replaceChildren();
    elements.historyCaption.textContent = "";
  }

  // The heatmap is fetched lazily: only once the Overview is shown, and
  // again (forced) after a snapshot reload while it is visible.
  async function loadHeatmap(force = false): Promise<void> {
    if (workspaceController.state.status !== "open") return;
    if (heatmapRequested && !force) return;
    heatmapRequested = true;
    const ticket = workspaceController.capture();
    const request = heatmapRequests.next();
    const current = () => heatmapRequests.isCurrent(request) &&
      workspaceController.accepts(ticket, ticket.generation);
    try {
      const days = await api().GetActivityHeatmapV2(16);
      if (!current()) return;
      withOverviewScrollPreserved(() => renderHeatmap(days));
    } catch (error) {
      if (!current()) return;
      heatmapRequested = false;
      if (workspaceController.state.status === "open") showError(error);
    }
  }

  function renderProjectPanel(snapshot: WorkspaceSnapshot): void {
    const project = snapshot.project;
    const tracking = snapshot.tracking;
    elements.projectRoot.textContent = project.root;
    const storage = project.storage;
    elements.storageStatus.textContent = storage.exists
      ? `p-track format v${storage.formatVersion} · ${formatBytes(storage.sizeBytes)} · last written by ${storage.lastWriteVersion || "unknown"}`
      : storage.error || "p-track storage unavailable";
    elements.snapshotBounds.replaceChildren();
    for (const [label, bound] of Object.entries(tracking.bounds || {})) {
      elements.snapshotBounds.append(
        pill(label, bound.more ? `${bound.shown}/${bound.total}` : bound.total),
      );
    }

    elements.blockers.replaceChildren();
    if (tracking.blockers.length === 0) {
      elements.blockers.append(emptyMemory("No blocked tasks."));
    } else {
      tracking.blockers.slice(0, 10).forEach((task) => {
        elements.blockers.append(intelligenceItem(`Blocked · #${task.id}`, task.title, "error"));
      });
    }
    elements.notes.replaceChildren();
    tracking.notes.slice(0, 10).forEach((note) => {
      elements.notes.append(
        intelligenceItem(
          `${note.kind || "Note"} · ${note.target}${note.targetId ? ` #${note.targetId}` : ""}`,
          `${shortRelativeTime(note.occurredAt)} · ${note.body}`,
        ),
      );
    });
  }

  function renderIntelligence(): void {
    const snapshot = ctx.state.snapshot;
    if (!snapshot) return;
    renderProjectPanel(snapshot);
    renderGitIntelligence(snapshot.git);
    ctx.agentActivity.renderAgentActivity(snapshot.agentActivity);
    renderDrift(snapshot.drift);
  }

  function appendDriftFinding(finding: DriftFinding): void {
    const copy = driftCopy[finding.kind];
    if (!copy) return;
    const [title, meaning] = copy;
    const evidence = finding.path ||
      finding.runIds.map((runId) => runId.slice(0, 8)).join(", ") || "structured evidence";
    elements.agentDrift.append(
      intelligenceItem(
        title,
        `${meaning} · ${evidence} · ${evidenceSignals(finding.evidenceCount)}. This is advisory, not proof of drift.`,
        // Drift is advisory evidence, never an error: it reads in the info
        // colour whatever its severity.
        "advisory",
      ),
    );
  }

  function renderDrift(section: unknown): void {
    elements.agentDrift.replaceChildren();
    const drift = driftPresentation(section);
    if (drift.incomplete) {
      elements.agentDrift.append(
        intelligenceItem(
          "Work comparison incomplete",
          "Bounded Git or agent evidence was omitted. No missing warning should be treated as proof of alignment.",
          "advisory",
        ),
      );
    }
    drift.findings.filter((finding) => finding.severity === "warning").forEach(appendDriftFinding);
    if (drift.unlinkedCommits.length > 0) {
      const group = document.createElement("details");
      group.className = "drift-group";
      const summary = document.createElement("summary");
      summary.textContent = `${drift.unlinkedCommits.length} shown unlinked commit${drift.unlinkedCommits.length === 1 ? "" : "s"}`;
      group.append(summary);
      drift.unlinkedCommits.forEach((finding) => {
        group.append(
          intelligenceItem(
            finding.sha,
            `Exact SHA has no p-track commit link · ${evidenceSignals(finding.evidenceCount)}. This is advisory, not proof of drift.`,
          ),
        );
      });
      elements.agentDrift.append(group);
    }
    drift.findings.filter((finding) => finding.severity !== "warning").forEach(appendDriftFinding);
  }

  function renderStackProjects(projects: StackProfile["projects"] & object): void {
    if (!projects.length) {
      // Nothing to disclose: a control that opens onto an empty list is worse
      // than a sentence saying there is nothing there.
      elements.stackProjectsDisclosure.hidden = true;
      elements.stackEmpty.hidden = false;
      return;
    }
    elements.stackProjectsDisclosure.hidden = false;
    elements.stackEmpty.hidden = true;
    // A workspace discovers a project per crate, so the evidence rows are the
    // tallest thing on this page by a wide margin. The disclosure line carries
    // what a reader wants at a glance — how many, in what — and the rows stay
    // one click away.
    const languages = [
      ...new Set(projects.map((project) => languageLabel(project.language))),
    ];
    const count = projects.length;
    elements.stackProjectsSummary.textContent = `${count} discovered project${
      count === 1 ? "" : "s"
    } in ${languageSentence(languages)}`;
    // The panel is the evidence view, so it stays per-project — but a large
    // workspace discovers dozens, and the panel is not a place to scroll
    // through 64 rows. The Overview carries the per-language totals.
    const shown = projects.slice(0, STACK_PANEL_PROJECTS);
    const hidden = projects.length - shown.length;
    shown.forEach((project) => {
      elements.stackProjects.append(
        intelligenceItem(
          `${project.root || "."} · ${languageLabel(project.language)}`,
          `${project.files.toLocaleString()} tracked file${project.files === 1 ? "" : "s"} · from ${project.evidence.join(", ")}`,
        ),
      );
    });
    if (hidden > 0) {
      elements.stackProjects.append(
        emptyMemory(`+${hidden} more discovered project${hidden === 1 ? "" : "s"}`),
      );
    }
  }

  // The Repository panel's stack section. Every state is explicit: a project
  // that cannot be scanned says so rather than rendering an empty list that
  // reads as "no code here".
  function renderStackProfile(): void {
    elements.stackSummary.replaceChildren();
    elements.stackProjects.replaceChildren();
    elements.stackProjectsDisclosure.hidden = true;
    elements.stackEmpty.hidden = true;
    elements.stackScanned.textContent = "";
    elements.stackRescan.hidden = true;

    const profile = stackProfile;
    const state = profile?.state;
    if (!profile || state === "scanning") {
      elements.stackSummary.append(pill("Stack", state ? "scanning…" : "not scanned"));
      return;
    }
    if (state === "unavailable") {
      elements.stackSummary.append(pill("Stack", "not a git repository"));
      return;
    }
    if (state === "failed") {
      elements.stackSummary.append(pill("Stack", "scan failed", "error"));
      elements.stackRescan.hidden = false;
      elements.stackRescan.textContent = "Retry stack scan";
      return;
    }

    elements.stackSummary.append(pill("tracked files", (profile.trackedFiles ?? 0).toLocaleString()));
    if (profile.incomplete) {
      elements.stackSummary.append(pill("scan", "truncated at the path cap", "warning"));
    }
    renderStackProjects(profile.projects ?? []);
    elements.stackScanned.textContent = profile.scannedHead
      ? `Scanned at ${profile.scannedHead.slice(0, 8)}`
      : "";
    elements.stackRescan.hidden = false;
    elements.stackRescan.textContent = "Rescan stack";
  }

  function renderGitRemotesAndBranches(git: GitSection["snapshot"]): number {
    if (!git.remotes?.length) {
      elements.gitRemotes.append(emptyMemory("No remotes configured."));
    } else {
      git.remotes.forEach((remote) => {
        const fetch = remote.fetchUrls?.join(", ") || "none";
        const push = remote.pushUrls?.join(", ") || fetch;
        elements.gitRemotes.append(intelligenceItem(`Remote · ${remote.name}`, `fetch ${fetch} · push ${push}`));
      });
    }
    const branches = [...(git.localBranches || []), ...(git.remoteBranches || [])];
    branches.slice(0, 24).forEach((branch) => {
      elements.gitBranches.append(branchItem(branch));
    });
    if (branches.length === 0) {
      elements.gitBranches.append(emptyMemory("No branch refs found."));
    }
    return branches.length;
  }

  function renderGitCommits(git: GitSection["snapshot"]): void {
    (git.recentCommits || []).slice(0, 12).forEach((commit) => {
      const areas = commit.changedAreas?.map((area) => `${area.name} ${area.files}`).join(", ");
      const refs = commit.refs?.length ? ` · ${commit.refs.join(", ")}` : "";
      elements.gitCommits.append(
        intelligenceItem(
          `${commit.sha.slice(0, 8)} · ${commit.subject}`,
          `${commit.authorName} · ${shortRelativeTime(commit.date)} · ${commit.filesChanged} files${areas ? ` · ${areas}` : ""}${refs}`,
        ),
      );
    });
  }

  function renderGitIntelligence(section: GitSection): void {
    renderStackProfile();
    elements.gitSummary.replaceChildren();
    elements.gitRemotes.replaceChildren();
    elements.gitBranches.replaceChildren();
    elements.gitCommits.replaceChildren();
    elements.gitState.textContent = section.state;
    if (section.state !== "ready" && section.state !== "stale") {
      elements.gitState.textContent = "Error";
      elements.gitSummary.append(pill("Git", section.error || "unavailable", "error"));
      return;
    }
    if (section.state === "stale") {
      elements.gitSummary.append(
        pill("Git", `stale · ${section.error || "refresh unavailable"}`, "warning"),
      );
    }
    const git = section.snapshot;
    if (git.state === "notRepository") {
      elements.gitState.textContent = "No repository";
      elements.gitSummary.append(pill("Git", "not found"));
      return;
    }
    const status = git.status;
    elements.gitState.textContent = status.detached
      ? "Detached"
      : git.linkedWorktree
        ? "Worktree"
        : "Ready";
    for (const chip of repositoryChips(status, git.divergence, git.unpushedCommits?.length || 0)) {
      const item = document.createElement("span");
      item.className = "intelligence-pill";
      if (chip.tone) item.dataset.tone = chip.tone;
      item.textContent = chip.label;
      elements.gitSummary.append(item);
    }
    const branchCount = renderGitRemotesAndBranches(git);
    // Remotes and commits open on their first render with data, so the
    // Overview's last row is not two bare disclosure headers; after that the
    // reader's open/closed choice stands.
    const disclosures: ReadonlyArray<readonly [HTMLDetailsElement, boolean]> = [
      [elements.gitRemotesDisclosure, Boolean(git.remotes?.length || branchCount)],
      [elements.gitCommitsDisclosure, Boolean(git.recentCommits?.length)],
    ];
    for (const [disclosure, hasData] of disclosures) {
      if (hasData && disclosure.dataset.autoOpened !== "true") {
        disclosure.dataset.autoOpened = "true";
        disclosure.open = true;
      }
    }
    renderGitCommits(git);
  }

  function openMemoryHistory(): void {
    const active = document.activeElement;
    memoryModalReturnFocus = active instanceof HTMLElement ? active : null;
    elements.memoryModal.hidden = false;
    requestAnimationFrame(() => elements.memoryDialogClose.focus());
  }

  function closeMemoryHistory(): void {
    ctx.snapshot.hideApplicationOverlay(elements.memoryModal);
    memoryModalReturnFocus?.focus();
    memoryModalReturnFocus = null;
  }

  function bind(): void {
    elements.stackRescan.addEventListener("click", () => void loadStackProfile(true));
    elements.activityMore.addEventListener("click", openMemoryHistory);
    elements.summaryExpand.addEventListener("click", () => {
      const expanded = elements.summary.dataset.expanded === "true";
      elements.summary.dataset.expanded = expanded ? "false" : "true";
      elements.summaryExpand.textContent = expanded ? "Show all" : "Show less";
    });
    document.querySelectorAll("[data-close-memory-modal]").forEach((closer) => {
      closer.addEventListener("click", closeMemoryHistory);
    });
    elements.memoryDialogClose.addEventListener("click", closeMemoryHistory);
    if ("ResizeObserver" in window) {
      new ResizeObserver(() => requestAnimationFrame(fitRecentMemory)).observe(
        elements.activity,
      );
    }
  }

  return {
    bind,
    withOverviewScrollPreserved,
    fitRecentMemory,
    renderMemory,
    loadProjectHistory,
    loadStackProfile,
    reloadRequestedOverviewPanels,
    resetOverviewProjectData,
    clearOverviewCharts,
    loadHeatmap,
    renderIntelligence,
    closeMemoryHistory,
  };
}

export type OverviewView = ReturnType<typeof createOverviewView>;
