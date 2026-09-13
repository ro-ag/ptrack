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

/** Distribution of cached record last-update timestamps, not a complete event history. */
export function overviewDistribution(overview: Overview, now = Date.now()) {
  const today = Math.floor(now / 86400000);
  const bins = Array.from({ length: 30 }, (_, index) => ({ day: today - 29 + index, count: 0 }));
  for (const project of overview.projects) {
    for (const item of project.activity) {
      if (item.updatedAt > now / 1000) continue;
      const index = Math.floor(item.updatedAt / 86400) - (today - 29);
      if (index >= 0 && index < bins.length) bins[index].count += 1;
    }
  }
  return bins;
}

export function overviewProgress(overview: Overview) {
  const totalTasks = overview.counts.openTasks + overview.counts.doneTasks;
  return {
    totalTasks,
    completion: totalTasks ? Math.round(overview.counts.doneTasks / totalTasks * 100) : null,
    coverage: overview.trackedProjects ? Math.round(overview.summarizedProjects / overview.trackedProjects * 100) : 0,
  };
}

export function overviewRefreshMessage(result: { refreshedProjects: number; skippedProjects: number }) {
  const refreshed = `${result.refreshedProjects} ${result.refreshedProjects === 1 ? "project" : "projects"} refreshed`;
  return result.skippedProjects ? `${refreshed}; ${result.skippedProjects} unavailable. Existing summaries retained where available.` : `${refreshed}.`;
}
