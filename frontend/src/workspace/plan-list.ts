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

/// Splits the current plan out of the list so the sidebar can pin it above
/// the scrollable rows. The current plan is the one the board shows: picking
/// a plan in the list makes it current, so the pinned card and the board
/// heading always name the same plan. With no plan on the board, the
/// project's `isActive` plan is current. A selected plan that is not among
/// the loaded rows pins nothing rather than a different plan. The rest keep
/// their order.
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
