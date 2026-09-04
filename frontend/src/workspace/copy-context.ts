interface ContextReference {
  id: number;
  title: string;
  status?: string;
}

/** Extra task context the drawer has loaded; optional, brief stays light without it. */
export interface AgentTaskDetail {
  notes?: { body: string }[];
  issues?: { id: number; title: string; severity?: string; status?: string }[];
  commits?: { sha: string; subject: string }[];
}

export interface AgentContext {
  project: { name: string; root: string; goal?: string };
  plan?: ContextReference;
  task?: ContextReference & { latestNote?: string; detail?: AgentTaskDetail };
}

export function agentContextText({ project, plan, task }: AgentContext): string {
  if (!project.root) throw new Error("Open a project before copying context.");
  if (task && !plan) throw new Error("Task context requires its plan reference.");
  const lines = ["p-track project context", `Project: ${project.name}`, `Repository: ${project.root}`];
  if (project.goal) lines.push(`Goal: ${project.goal}`);
  if (plan) lines.push(`Plan #${plan.id}: ${plan.title}${plan.status ? ` (${plan.status})` : ""}`);
  if (task) {
    lines.push(`Task #${task.id}: ${task.title}${task.status ? ` (${task.status})` : ""}`);
    if (task.latestNote) lines.push(`Latest task note: ${task.latestNote}`);
    const detail = task.detail;
    if (detail?.notes?.length) {
      lines.push("", "Recent memories:");
      for (const note of detail.notes.slice(0, 3)) {
        lines.push(`- ${note.body.replace(/\s+/g, " ").trim()}`);
      }
    }
    if (detail?.issues?.length) {
      lines.push("", "Linked issues:");
      for (const issue of detail.issues.slice(0, 5)) {
        const bits = [issue.severity, issue.status].filter(Boolean).join(", ");
        lines.push(`- #${issue.id}${bits ? ` (${bits})` : ""}: ${issue.title}`);
      }
    }
    if (detail?.commits?.length) {
      lines.push("", "Linked commits:");
      for (const commit of detail.commits.slice(0, 5)) {
        lines.push(`- ${commit.sha.slice(0, 8)} ${commit.subject}`);
      }
    }
  }
  // Quotes protect paths containing spaces, apostrophes, or shell metacharacters.
  const root = "'" + project.root.replaceAll("'", "'\\''") + "'";
  lines.push("", "Read the current project records before continuing:", `cd -- ${root}`, "ptrack context");
  if (plan) lines.push(`ptrack plan show ${plan.id}`);
  if (task) lines.push(`ptrack task show ${task.id}`);
  lines.push("git status --short");
  return lines.join("\n");
}
