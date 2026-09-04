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

/// Splits the project's current plan out of the list so the sidebar can pin
/// it above the scrollable rows. The first `isActive` plan wins and the rest
/// keep their order; with no current plan everything stays in `rest`.
export function splitCurrentPlan<T extends SidebarPlan>(
  plans: T[],
): { current: T | undefined; rest: T[] } {
  const rest: T[] = [];
  let current: T | undefined;
  for (const plan of plans) {
    if (current === undefined && plan.isActive) {
      current = plan;
    } else {
      rest.push(plan);
    }
  }
  return { current, rest };
}
