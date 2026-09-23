export interface Summary {
  root: string;
  syncedAt: number;
  counts: { activePlans: number; openTasks: number; doneTasks: number; openIssues: number };
  activity: { kind: string; id: number; title: string; status: string; updatedAt: number }[];
}
export interface Overview {
  trackedProjects: number;
  summarizedProjects: number;
  counts: Summary["counts"];
  projects: Summary[];
}
export function overviewActivity(overview: Overview, days: number | null, now = Date.now()) {
  const cutoff = days === null ? -Infinity : now / 1000 - days * 86400;
  return overview.projects.flatMap((project) => project.activity.map((item) => ({ ...item, root: project.root })))
    .filter((item) => item.updatedAt >= cutoff && item.updatedAt <= now / 1000)
    .sort((a, b) => b.updatedAt - a.updatedAt || a.root.localeCompare(b.root) || a.id - b.id)
    .slice(0, 20);
}
export function filterRecentProjects<T extends { name: string; canonicalPath: string }>(projects: T[], query: string) {
  const search = query.trim().toLocaleLowerCase();
  return projects.filter((project) => `${project.name} ${project.canonicalPath}`.toLocaleLowerCase().includes(search));
}

export function overviewRefreshMessage(result: { refreshedProjects: number; skippedProjects: number }) {
  const refreshed = `${result.refreshedProjects} ${result.refreshedProjects === 1 ? "project" : "projects"} refreshed`;
  return result.skippedProjects ? `${refreshed}; ${result.skippedProjects} unavailable. Existing summaries retained where available.` : `${refreshed}.`;
}
