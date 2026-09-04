export type PlanLifecycleAction =
  | "copy-context"
  | "rename"
  | "done"
  | "hold"
  | "resume"
  | "delete"
  | "move"
  | "copy";

export interface PlanLifecycleState {
  status?: string;
  holdReason?: string;
  tasksTotal?: number;
  tasksDone?: number;
}

export interface PlanMenuItem {
  action: PlanLifecycleAction;
  label: string;
  destructive: boolean;
}

/** The plan context menu, identical for the sidebar and the board header. */
export function planMenuItems(plan: PlanLifecycleState = {}): PlanMenuItem[] {
  const items: PlanMenuItem[] = [
    { action: "copy-context", label: "Copy context", destructive: false },
    { action: "rename", label: "Rename", destructive: false },
  ];
  if (!plan.status || plan.status === "active") {
    items.push(
      { action: "done", label: "Mark plan done…", destructive: false },
      plan.holdReason
        ? { action: "resume", label: "Resume", destructive: false }
        : { action: "hold", label: "Put on hold…", destructive: false },
    );
  }
  items.push(
    { action: "move", label: "Move to project…", destructive: false },
    { action: "copy", label: "Copy…", destructive: false },
    { action: "delete", label: "Delete…", destructive: true },
  );
  return items;
}

/** A non-empty, active, unheld plan should prompt as soon as every task is done. */
export function planReadyForCompletion(plan: PlanLifecycleState): boolean {
  const total = Number(plan.tasksTotal || 0);
  return plan.status === "active" && !plan.holdReason && total > 0 &&
    Number(plan.tasksDone || 0) === total;
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
