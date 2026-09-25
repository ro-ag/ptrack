export interface SidebarPlan {
  id: number;
  title: string;
  status: string;
  holdReason?: string | null;
  /** Present on workspace snapshot plans: the project's single current plan. */
  isActive?: boolean;
}

export function filterPlans<T extends SidebarPlan>(plans: T[], query: string, status: string): T[] {
  const search = query.trim().toLocaleLowerCase();
  return plans.filter((plan) => {
    const matchesStatus = status === "all" ||
      (status === "open" && plan.status === "active") ||
      (status === "held" && Boolean(plan.holdReason)) ||
      plan.status === status;
    return matchesStatus && `${plan.id} ${plan.title}`.toLocaleLowerCase().includes(search);
  });
}

/// Returns the current plan separately for the pinned sidebar card.
/// Falls back to `isActive` when no plan is shown; an unloaded selection pins nothing.
export function splitCurrentPlan<T extends SidebarPlan>(
  plans: T[],
  selectedPlanId: number | string = 0,
): { current: T | undefined; rest: T[] } {
  const selected = String(selectedPlanId);
  const isCurrent = selected !== "0" && selected !== ""
    ? (plan: T) => String(plan.id) === selected
    : (plan: T) => Boolean(plan.isActive);
  const rest: T[] = [];
  let current: T | undefined;
  for (const plan of plans) {
    if (current === undefined && isCurrent(plan)) {
      current = plan;
    } else {
      rest.push(plan);
    }
  }
  return { current, rest };
}

export interface CloseoutCandidate extends SidebarPlan {
  tasksDone?: number;
  tasksTotal?: number;
}

/// The inline call to action on the current-plan card once every task is
/// done but the plan is still active; `null` while there is work left, the
/// plan is held, or it is already closed.
export function currentPlanCloseoutLabel(plan: CloseoutCandidate | undefined): string | null {
  if (!plan || plan.status !== "active" || plan.holdReason) return null;
  const total = Number(plan.tasksTotal || 0);
  if (total === 0 || Number(plan.tasksDone || 0) !== total) return null;
  return `All ${total} ${total === 1 ? "task" : "tasks"} done · Close plan…`;
}

/// A done or archived plan can be opened on the board to read it, but it can
/// never become the current plan (the runtime refuses it), so opening one is
/// viewing it. The plan that is current stays current — unless it is this
/// one, completed while it was current.
export function planIsViewOnly(plan: SidebarPlan | undefined): boolean {
  if (!plan || plan.isActive) return false;
  return plan.status === "done" || plan.status === "archived";
}

/// The board heading's eyebrow while a view-only plan is open: which plan,
/// and why it is only being viewed.
export function planViewingLabel(plan: SidebarPlan): string {
  return `Viewing #${plan.id} (${plan.status})`;
}

/// The plan the sidebar pins as current: the board's plan, except while the
/// board shows a view-only plan — then the project's current plan stays
/// pinned, so the pinned card always names the plan work is added to.
export function pinnedPlanSelection<T extends SidebarPlan>(
  plans: T[],
  boardPlanId: number,
): number {
  const shown = plans.find((plan) => String(plan.id) === String(boardPlanId));
  return planIsViewOnly(shown) ? 0 : boardPlanId;
}
