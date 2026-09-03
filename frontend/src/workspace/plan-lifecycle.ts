export type PlanLifecycleAction =
  | "done"
  | "hold"
  | "resume"
  | "rename"
  | "delete"
  | "move"
  | "copy";

export interface PlanMenuItem {
  action: PlanLifecycleAction;
  label: string;
  destructive: boolean;
}

/** The plan context menu, identical for the sidebar and the board header. */
export function planMenuItems(plan: {
  status?: string;
  holdReason?: string | null;
} = {}): PlanMenuItem[] {
  const lifecycle: PlanMenuItem[] = plan.status === "done" || plan.status === "archived"
    ? []
    : [
        { action: "done", label: "Mark plan done…", destructive: false },
        plan.holdReason
          ? { action: "resume", label: "Resume plan", destructive: false }
          : { action: "hold", label: "Put plan on hold…", destructive: false },
      ];
  return [
    ...lifecycle,
    { action: "rename", label: "Rename", destructive: false },
    { action: "move", label: "Move to project…", destructive: false },
    { action: "copy", label: "Copy…", destructive: false },
    { action: "delete", label: "Delete…", destructive: true },
  ];
}

export interface PlanCompletionPrompt {
  heading: string;
  detail: string;
  action: string;
}

/** Replaces a full Done lane only while an open plan has completed every task. */
export function planCompletionPrompt(
  done: number,
  total: number,
  status = "active",
): PlanCompletionPrompt | null {
  if (status !== "active" || total === 0 || done !== total) return null;
  return {
    heading: "Every task is done",
    detail: "Close this plan to review the project checkpoint before continuing.",
    action: "Close plan…",
  };
}

export interface CheckpointSummary {
  goal?: string;
  summary?: string;
  openPlans?: { id: number; title: string }[];
  openIssues?: number;
  highIssues?: number;
  milestone?: { title: string; plansDone: number; plansTotal: number } | null;
}

/** Human checkpoint copy shown after the plan mutation commits. */
export function checkpointDialogText(checkpoint: CheckpointSummary): string {
  const plans = checkpoint.openPlans?.length
    ? checkpoint.openPlans.map((plan) => `#${plan.id} ${plan.title}`).join(", ")
    : "none";
  const lines = [
    `Goal: ${checkpoint.goal || "(not set)"}`,
    `Rolling summary: ${checkpoint.summary || "(not set)"}`,
    `Remaining open plans: ${plans}`,
    `Open issues: ${checkpoint.openIssues || 0} (${checkpoint.highIssues || 0} high)`,
  ];
  if (checkpoint.milestone) {
    lines.push(
      `Milestone: ${checkpoint.milestone.title} — ${checkpoint.milestone.plansDone}/${checkpoint.milestone.plansTotal} plans done`,
    );
  }
  lines.push(
    "",
    "Before continuing, re-evaluate whether the remaining roadmap still reaches the goal and update the rolling summary with what changed.",
  );
  return lines.join("\n");
}

/**
 * Keeps the context menu inside the viewport: a menu opened near the bottom
 * of the sidebar would otherwise render its destructive Delete item
 * off-screen and unreachable.
 */
export function clampMenuPosition(
  position: { x: number; y: number },
  menu: { width: number; height: number },
  viewport: { width: number; height: number },
  margin = 8,
): { x: number; y: number } {
  return {
    x: Math.max(margin, Math.min(position.x, viewport.width - menu.width - margin)),
    y: Math.max(margin, Math.min(position.y, viewport.height - menu.height - margin)),
  };
}

export interface DeletePreviewSummary {
  planId: number;
  title: string;
  tasks: number;
  notes: number;
  commits: number;
  detachedIssues: { id: number; title: string }[];
}

/** Human sentence for the delete confirmation body, from the preview call. */
export function deleteConfirmationText(summary: DeletePreviewSummary): string {
  const parts = [
    `${summary.tasks} task${summary.tasks === 1 ? "" : "s"}`,
    `${summary.notes} note${summary.notes === 1 ? "" : "s"}`,
  ];
  const issues = summary.detachedIssues.length;
  let text = `Deleting “${summary.title}” permanently removes ${parts.join(" and ")}.`;
  if (issues > 0) {
    text += ` ${issues} linked issue${issues === 1 ? "" : "s"} will be detached and kept.`;
  }
  if (summary.commits > 0) {
    text += ` ${summary.commits} commit record${summary.commits === 1 ? "" : "s"} stay as audit history with their links cleared.`;
  }
  return text;
}

export interface ProjectChoice {
  name: string;
  path: string;
  current: boolean;
}

export interface TransferDialogState {
  mode: "move" | "copy";
  projects: ProjectChoice[];
  targetPath: string; // "" until the user picks
  title: string; // optional new title field
}

/**
 * OK stays disabled until the dialog state is submittable:
 * - move: a non-current target must be chosen;
 * - copy: any target works, but landing in the current project (explicitly or
 *   by leaving the picker empty) requires a new title.
 */
export function transferSubmitDisabled(state: TransferDialogState): boolean {
  const target = state.projects.find((project) => project.path === state.targetPath);
  if (state.mode === "move") {
    return state.targetPath === "" || target === undefined || target.current;
  }
  const landsInCurrent = state.targetPath === "" || target === undefined || target.current;
  return landsInCurrent && state.title.trim() === "";
}
