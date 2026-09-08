// Shaping for the Overview's project history. Pure functions over the payload
// `GetProjectTimelineV1` returns, so the arithmetic that decides what the chart
// claims is testable without a DOM.

export interface ProjectTimeline {
  commits: number[];
  tags: { name: string; at: number }[];
  truncated: boolean;
  available: boolean;
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
  timeline: ProjectTimeline,
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

