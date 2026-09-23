import { describe, expect, it } from "vitest";
import { filterRecentProjects, overviewActivity, type Overview } from "./overview";

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



import { overviewRefreshMessage } from "./overview";
it("reports partial refresh without implying skipped projects were updated", () => {
  expect(overviewRefreshMessage({ refreshedProjects: 2, skippedProjects: 1 })).toBe("2 projects refreshed; 1 unavailable. Existing summaries retained where available.");
  expect(overviewRefreshMessage({ refreshedProjects: 1, skippedProjects: 0 })).toBe("1 project refreshed.");
});
