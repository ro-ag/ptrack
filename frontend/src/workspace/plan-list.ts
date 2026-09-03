export interface SidebarPlan {
  id: number;
  title: string;
  status: string;
  holdReason?: string | null;
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
