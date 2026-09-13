import { describe, expect, it } from "vitest";
import { filterRecentProjects, overviewActivity, overviewDistribution, overviewProgress, type Overview } from "./overview";

const counts = { activePlans: 1, openTasks: 2, doneTasks: 3, openIssues: 0 };
const now = 100 * 86400;
const overview: Overview = { trackedProjects: 2, summarizedProjects: 1, counts, projects: [{ root: "/project", syncedAt: now, counts, activity: [
  { kind: "task", id: 1, title: "Old", status: "done", updatedAt: now - 31 * 86400 },
  { kind: "issue", id: 2, title: "Boundary", status: "closed", updatedAt: now - 30 * 86400 },
  { kind: "task", id: 3, title: "Future", status: "todo", updatedAt: now + 1 },
] }] };
describe("overview", () => {
  it("filters record updates by an inclusive 30-day boundary, excluding future timestamps", () => {
    expect(overviewActivity(overview, 30, now * 1000).map((item) => item.id)).toEqual([2]);
    expect(overviewActivity(overview, null, now * 1000).map((item) => item.id)).toEqual([2, 1]);
  });
  it("searches recent project names and paths without changing their authorization fields", () => {
    const project = { name: "Example", canonicalPath: "/Work/Rust", entryId: "opaque", base: "token" };
    expect(filterRecentProjects([project], " rust ")).toEqual([project]);
    expect(filterRecentProjects([project], "EXAMPLE")[0]).toBe(project);
    expect(filterRecentProjects([project], "missing")).toEqual([]);
  });
});


it("reports task progress and cache coverage without inventing missing work", () => {
  expect(overviewProgress(overview)).toEqual({ totalTasks: 5, completion: 60, coverage: 50 });
  expect(overviewProgress({ ...overview, counts: { activePlans: 0, openTasks: 0, doneTasks: 0, openIssues: 0 }, trackedProjects: 0, summarizedProjects: 0 })).toEqual({ totalTasks: 0, completion: null, coverage: 0 });
});
it("bins all cached records by UTC calendar day, excluding old and future updates", () => {
  const recent = { ...overview, projects: [{ ...overview.projects[0], activity: Array.from({ length: 25 }, (_, id) => ({ kind: "task", id, title: "Task", status: "done", updatedAt: now - 1 })) }] };
  const bins = overviewDistribution(recent, now * 1000);
  expect(bins).toHaveLength(30);
  expect(bins.reduce((total, bin) => total + bin.count, 0)).toBe(25);
  expect(bins[28].count).toBe(25);
  expect(overviewDistribution(overview, now * 1000).every((bin) => bin.count === 0)).toBe(true);
});

import { overviewRefreshMessage } from "./overview";
it("reports partial refresh without implying skipped projects were updated", () => {
  expect(overviewRefreshMessage({ refreshedProjects: 2, skippedProjects: 1 })).toBe("2 projects refreshed; 1 unavailable. Existing summaries retained where available.");
  expect(overviewRefreshMessage({ refreshedProjects: 1, skippedProjects: 0 })).toBe("1 project refreshed.");
});
