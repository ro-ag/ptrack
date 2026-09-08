// Shaping for the Insights page. Everything here is a pure function over the
// payload `GetInsightsV1` returns, so the arithmetic that decides what a chart
// claims is testable without a DOM.
//
// The backend is deliberate about which series are exact and which are dated by
// `updated_at` on records that are currently done. That distinction survives
// into the names here — `completedByUpdate`, never `completed` — and into the
// captions the page prints, so nobody reads an estimate as a measurement.

export interface InsightsDay {
  date: string;
  notes: number;
  commits: number;
  tasksCreated: number;
  tasksCompletedByUpdate: number;
}

export interface InsightsWeek {
  week: string;
  created: number;
  completedByUpdate: number;
}

export interface InsightsBucket {
  label: string;
  count: number;
}

export interface InsightsPlan {
  id: number;
  title: string;
  status: string;
  total: number;
  done: number;
  blocked: number;
}

export interface InsightsSeverity {
  severity: string;
  open: number;
  closed: number;
}

export interface InsightsPunch {
  weekday: number;
  hour: number;
  count: number;
}

export interface InsightsTimeline {
  commits: number[];
  tags: { name: string; at: number }[];
  truncated: boolean;
  available: boolean;
}

export interface Insights {
  weeks: number;
  daily: InsightsDay[];
  cumulative: InsightsWeek[];
  leadTime: { buckets: InsightsBucket[]; counted: number; medianDays: number | null };
  plans: InsightsPlan[];
  issues: { bySeverity: InsightsSeverity[]; openAges: { id: number; severity: string; days: number }[] };
  punchcard: InsightsPunch[];
  totals: Record<string, number>;
  timeline: InsightsTimeline;
}

/// A day's total activity, which is what the momentum band is drawn from.
export function dayTotal(day: InsightsDay): number {
  return day.notes + day.commits;
}

// A rolling mean over the trailing `window` days, so a single busy afternoon
// does not read as a trend. Early days average over what exists rather than
// padding with zeroes, which would draw a slope that is an artefact of the
// window rather than of the project.
export function rollingMean(values: number[], window: number): number[] {
  if (window < 1) return values.slice();
  const result: number[] = [];
  let sum = 0;
  for (let index = 0; index < values.length; index += 1) {
    sum += values[index];
    if (index >= window) sum -= values[index - window];
    const counted = Math.min(index + 1, window);
    result.push(sum / counted);
  }
  return result;
}

export interface Momentum {
  current: number;
  /// The week before the current one, or `null` when the window does not
  /// reach back far enough to have one.
  previous: number | null;
  /// The change as a ratio, or `null` when there is nothing to divide by.
  change: number | null;
}

// Momentum compares the last week against the week before it. A ratio is only
// returned when there is a real week to compare against and it was not empty:
// "+100%" measured against nothing is not a fact about the project. The two
// reasons a ratio is missing are kept apart, because "the window is too short"
// and "last week was quiet" say very different things to a reader.
export function momentum(days: InsightsDay[]): Momentum {
  const totals = days.map(dayTotal);
  const current = totals.slice(-7).reduce((sum, value) => sum + value, 0);
  const earlier = totals.slice(-14, -7);
  if (earlier.length < 7) return { current, previous: null, change: null };
  const previous = earlier.reduce((sum, value) => sum + value, 0);
  if (previous === 0) return { current, previous, change: null };
  return { current, previous, change: (current - previous) / previous };
}

// Buckets commit timestamps into evenly spaced periods across the history, so
// the timeline draws a shape rather than thousands of overlapping ticks. The
// span is closed at both ends: the last bucket includes the newest commit.
export function timelineBuckets(
  commits: number[],
  buckets: number,
): { start: number; end: number; count: number }[] {
  if (commits.length === 0 || buckets < 1) return [];
  const first = commits[0];
  const last = commits[commits.length - 1];
  // A history entirely within one instant still deserves one bucket.
  if (last === first) return [{ start: first, end: first, count: commits.length }];
  const width = (last - first) / buckets;
  const result = Array.from({ length: buckets }, (_, index) => ({
    start: first + index * width,
    end: first + (index + 1) * width,
    count: 0,
  }));
  for (const at of commits) {
    const slot = Math.min(buckets - 1, Math.floor((at - first) / width));
    result[slot].count += 1;
  }
  return result;
}

// Tags that fall inside the drawn span, as a fraction across it, so the caller
// can place a marker without repeating the arithmetic. Tags outside the span
// are dropped rather than clamped to an edge, where they would claim a date
// they do not have.
export function timelineMarkers(
  timeline: InsightsTimeline,
): { name: string; at: number; position: number }[] {
  const { commits, tags } = timeline;
  if (commits.length === 0) return [];
  const first = commits[0];
  const last = commits[commits.length - 1];
  const span = last - first;
  return tags
    .filter((tag) => tag.at >= first && tag.at <= last)
    .map((tag) => ({
      name: tag.name,
      at: tag.at,
      position: span === 0 ? 0 : (tag.at - first) / span,
    }));
}

// The punchcard is a sparse list; the grid it draws is not. Filling it here
// keeps the drawing code from carrying lookup logic, and the maximum comes back
// with it because every cell is scaled against it.
export function punchcardGrid(cells: InsightsPunch[]): { grid: number[][]; max: number } {
  const grid = Array.from({ length: 7 }, () => new Array<number>(24).fill(0));
  let max = 0;
  for (const cell of cells) {
    if (cell.weekday < 0 || cell.weekday > 6 || cell.hour < 0 || cell.hour > 23) continue;
    grid[cell.weekday][cell.hour] += cell.count;
    max = Math.max(max, grid[cell.weekday][cell.hour]);
  }
  return { grid, max };
}

// Plans worth drawing, most complete first, with the ones carrying no tasks
// dropped: an empty bar says nothing about progress and costs a row.
export function planBars(plans: InsightsPlan[]): (InsightsPlan & { ratio: number })[] {
  return plans
    .filter((plan) => plan.total > 0)
    .map((plan) => ({ ...plan, ratio: plan.done / plan.total }))
    .sort((left, right) => right.ratio - left.ratio || right.total - left.total);
}

// Severity always reads in the same order regardless of what the project has,
// so the legend does not reshuffle between projects.
const SEVERITY_ORDER = ["critical", "high", "medium", "low"];

export function severityRows(rows: InsightsSeverity[]): InsightsSeverity[] {
  return SEVERITY_ORDER.map(
    (severity) =>
      rows.find((row) => row.severity === severity) ?? { severity, open: 0, closed: 0 },
  );
}

// Whether a section has anything to draw. Charts with no data render an empty
// state rather than empty axes, which read as a broken chart.
export function hasActivity(days: InsightsDay[]): boolean {
  return days.some((day) => dayTotal(day) > 0);
}

// A short, plain caption for a window, used in section headings.
export function windowLabel(weeks: number): string {
  if (weeks === 1) return "last week";
  if (weeks % 52 === 0) {
    const years = weeks / 52;
    return years === 1 ? "last year" : `last ${years} years`;
  }
  return `last ${weeks} weeks`;
}

// Formats the momentum change for display. Kept beside the calculation so the
// rounding that decides whether something reads as "no change" is testable.
export function momentumLabel(momentum: Momentum): string {
  const { previous, change } = momentum;
  if (previous === null) return "no earlier week recorded to compare";
  if (change === null) return "nothing recorded in the week before";
  const percent = Math.round(change * 100);
  if (percent === 0) return "level with the week before";
  return percent > 0
    ? `up ${percent}% on the week before`
    : `down ${Math.abs(percent)}% on the week before`;
}
