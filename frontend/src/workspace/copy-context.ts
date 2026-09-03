interface ContextReference {
  id: number;
  title: string;
  status?: string;
}

export interface AgentContext {
  project: { name: string; root: string; goal?: string };
  plan?: ContextReference;
  task?: ContextReference & { latestNote?: string };
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
  }
  // Quotes protect paths containing spaces, apostrophes, or shell metacharacters.
  const root = "'" + project.root.replaceAll("'", "'\\''") + "'";
  lines.push("", "Read the current project records before continuing:", `cd -- ${root}`, "ptrack context");
  if (plan) lines.push(`ptrack plan show ${plan.id}`);
  if (task) lines.push(`ptrack task show ${task.id}`);
  lines.push("git status --short");
  return lines.join("\n");
}
