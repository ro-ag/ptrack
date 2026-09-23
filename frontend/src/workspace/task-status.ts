// The four board lanes, in board order, with the colour and title each one
// shows wherever a task status is drawn.

export const statuses = ["todo", "doing", "blocked", "done"] as const;

export type TaskStatus = typeof statuses[number];

export function isTaskStatus(value: unknown): value is TaskStatus {
  return typeof value === "string" && (statuses as readonly string[]).includes(value);
}

export const laneColors: Record<TaskStatus, string> = {
  todo: "var(--todo)",
  doing: "var(--doing)",
  blocked: "var(--blocked)",
  done: "var(--done)",
};

export const severityColors: Record<string, string> = {
  low: "var(--text-soft)",
  medium: "var(--info)",
  high: "var(--doing)",
  critical: "var(--blocked)",
};

export const statusTitles: Record<TaskStatus, string> = {
  todo: "Todo",
  doing: "Doing",
  blocked: "Blocked",
  done: "Done",
};
