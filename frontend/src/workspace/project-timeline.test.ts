import { describe, expect, it } from "vitest";

import {
  timelineBuckets,
  timelineMarkers,
  type ProjectTimeline,
} from "./project-timeline";

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
  const timeline = (overrides: Partial<ProjectTimeline> = {}): ProjectTimeline => ({
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
