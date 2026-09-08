import { describe, expect, it } from "vitest";

import {
  dayTotal,
  hasActivity,
  momentum,
  momentumLabel,
  planBars,
  punchcardGrid,
  rollingMean,
  severityRows,
  timelineBuckets,
  timelineMarkers,
  windowLabel,
  type InsightsDay,
  type InsightsTimeline,
} from "./insights";

function day(overrides: Partial<InsightsDay> = {}): InsightsDay {
  return {
    date: "2026-01-01",
    notes: 0,
    commits: 0,
    tasksCreated: 0,
    tasksCompletedByUpdate: 0,
    ...overrides,
  };
}

function days(totals: number[]): InsightsDay[] {
  return totals.map((count) => day({ notes: count }));
}

describe("rolling mean", () => {
  it("averages over what exists rather than padding the start with zeroes", () => {
    // Padding would draw a ramp out of the window itself, not the project.
    expect(rollingMean([3, 3, 3], 7)).toEqual([3, 3, 3]);
  });

  it("follows the trailing window once there is enough history", () => {
    expect(rollingMean([0, 0, 0, 4], 2)).toEqual([0, 0, 0, 2]);
    expect(rollingMean([1, 2, 3, 4, 5], 3)).toEqual([1, 1.5, 2, 3, 4]);
  });

  it("returns the values unchanged for a meaningless window", () => {
    expect(rollingMean([1, 2], 0)).toEqual([1, 2]);
  });
});

describe("momentum", () => {
  it("compares the last seven days against the seven before them", () => {
    const series = days([1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2]);
    expect(momentum(series)).toEqual({ current: 14, previous: 7, change: 1 });
  });

  it("reports no change rather than an invented one without an earlier week", () => {
    expect(momentum(days([5, 5, 5]))).toEqual({ current: 15, previous: null, change: null });
  });

  it("refuses to divide by an empty earlier week", () => {
    const series = days([0, 0, 0, 0, 0, 0, 0, 3, 3, 3, 3, 3, 3, 3]);
    // A jump from nothing is not "+infinity%", it is simply not comparable —
    // but the reader is told the week before was empty, not that the window is
    // too short.
    expect(momentum(series)).toEqual({ current: 21, previous: 0, change: null });
  });

  it("counts notes and commits together", () => {
    expect(dayTotal(day({ notes: 2, commits: 3 }))).toBe(5);
  });
});

describe("momentum label", () => {
  it("says what the number means in plain words", () => {
    expect(momentumLabel({ current: 3, previous: 2, change: 0.5 })).toBe(
      "up 50% on the week before",
    );
    expect(momentumLabel({ current: 3, previous: 4, change: -0.25 })).toBe(
      "down 25% on the week before",
    );
    expect(momentumLabel({ current: 3, previous: 3, change: 0 })).toBe(
      "level with the week before",
    );
  });

  it("keeps a short window and a quiet week apart", () => {
    expect(momentumLabel({ current: 3, previous: null, change: null })).toBe(
      "no earlier week recorded to compare",
    );
    expect(momentumLabel({ current: 3, previous: 0, change: null })).toBe(
      "nothing recorded in the week before",
    );
  });

  it("treats a rounding-level difference as level", () => {
    expect(momentumLabel({ current: 3, previous: 3, change: 0.001 })).toBe(
      "level with the week before",
    );
  });
});

describe("timeline buckets", () => {
  it("spreads commits across evenly spaced periods and closes the last one", () => {
    const buckets = timelineBuckets([0, 10, 20, 30, 40], 4);
    expect(buckets).toHaveLength(4);
    expect(buckets.map((bucket) => bucket.count)).toEqual([1, 1, 1, 2]);
    // The newest commit belongs to the final bucket, not to one past the end.
    expect(buckets[3].end).toBe(40);
  });

  it("handles a history recorded at a single instant", () => {
    expect(timelineBuckets([7, 7, 7], 5)).toEqual([{ start: 7, end: 7, count: 3 }]);
  });

  it("has nothing to draw for an empty history", () => {
    expect(timelineBuckets([], 10)).toEqual([]);
    expect(timelineBuckets([1, 2], 0)).toEqual([]);
  });
});

describe("timeline markers", () => {
  const timeline = (overrides: Partial<InsightsTimeline> = {}): InsightsTimeline => ({
    commits: [100, 200],
    tags: [],
    truncated: false,
    available: true,
    ...overrides,
  });

  it("places a tag as a fraction across the drawn span", () => {
    const markers = timelineMarkers(
      timeline({ tags: [{ name: "v1", at: 150 }] }),
    );
    expect(markers).toEqual([{ name: "v1", at: 150, position: 0.5 }]);
  });

  it("drops tags outside the span rather than pinning them to an edge", () => {
    // A tag older than the oldest drawn commit would otherwise claim a date it
    // does not have.
    const markers = timelineMarkers(
      timeline({ tags: [{ name: "ancient", at: 1 }, { name: "future", at: 9_999 }] }),
    );
    expect(markers).toEqual([]);
  });

  it("has nothing to place without commits", () => {
    expect(timelineMarkers(timeline({ commits: [], tags: [{ name: "v1", at: 5 }] }))).toEqual([]);
  });
});

describe("punchcard grid", () => {
  it("fills a full week-by-hour grid and reports the busiest cell", () => {
    const { grid, max } = punchcardGrid([
      { weekday: 0, hour: 9, count: 2 },
      { weekday: 0, hour: 9, count: 3 },
      { weekday: 6, hour: 23, count: 1 },
    ]);
    expect(grid).toHaveLength(7);
    expect(grid[0]).toHaveLength(24);
    expect(grid[0][9]).toBe(5);
    expect(grid[6][23]).toBe(1);
    expect(grid[3][3]).toBe(0);
    expect(max).toBe(5);
  });

  it("ignores cells outside the week", () => {
    const { grid, max } = punchcardGrid([
      { weekday: 7, hour: 0, count: 9 },
      { weekday: 0, hour: 24, count: 9 },
      { weekday: -1, hour: 1, count: 9 },
    ]);
    expect(max).toBe(0);
    expect(grid.flat().every((value) => value === 0)).toBe(true);
  });
});

describe("plan bars", () => {
  it("drops empty plans and orders by completion", () => {
    const bars = planBars([
      { id: 1, title: "half", status: "active", total: 4, done: 2, blocked: 0 },
      { id: 2, title: "empty", status: "active", total: 0, done: 0, blocked: 0 },
      { id: 3, title: "done", status: "done", total: 3, done: 3, blocked: 0 },
    ]);
    expect(bars.map((bar) => bar.title)).toEqual(["done", "half"]);
    expect(bars[0].ratio).toBe(1);
    expect(bars[1].ratio).toBe(0.5);
  });

  it("breaks a tie on completion by the larger plan", () => {
    const bars = planBars([
      { id: 1, title: "small", status: "active", total: 2, done: 1, blocked: 0 },
      { id: 2, title: "large", status: "active", total: 10, done: 5, blocked: 0 },
    ]);
    expect(bars.map((bar) => bar.title)).toEqual(["large", "small"]);
  });
});

describe("severity rows", () => {
  it("always reads in the same order and fills in what a project lacks", () => {
    const rows = severityRows([{ severity: "low", open: 1, closed: 2 }]);
    expect(rows.map((row) => row.severity)).toEqual(["critical", "high", "medium", "low"]);
    expect(rows[0]).toEqual({ severity: "critical", open: 0, closed: 0 });
    expect(rows[3]).toEqual({ severity: "low", open: 1, closed: 2 });
  });
});

describe("empty states", () => {
  it("knows when there is nothing to draw", () => {
    expect(hasActivity(days([0, 0, 0]))).toBe(false);
    expect(hasActivity(days([0, 1]))).toBe(true);
    expect(hasActivity([])).toBe(false);
  });
});

describe("window label", () => {
  it("reads as words rather than as a number of weeks", () => {
    expect(windowLabel(1)).toBe("last week");
    expect(windowLabel(16)).toBe("last 16 weeks");
    expect(windowLabel(52)).toBe("last year");
    expect(windowLabel(104)).toBe("last 2 years");
  });
});
