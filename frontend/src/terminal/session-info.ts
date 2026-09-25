import type { AssociationPointerV1 } from "../workspace/model";
import { runtimeAssociationLabel } from "../workspace/presentation";
import type { PaneActivitySignal } from "./activity";
import type { PaneRuntimeState } from "./runtime";
import { shellStatusLabel, type ShellState } from "./shell-integration";

// Shared active-pane labels for dock and detached window headers.

export interface TerminalStateLabelInput {
  state: PaneRuntimeState;
  closing?: boolean;
  /** The pane's session lives in a detached window (dock only). */
  poppedOut?: boolean;
  signal?: PaneActivitySignal;
  /** The pane's shell-integration state while it runs, or null without one. */
  shell: ShellState | null;
}

export function terminalStateLabel(input: TerminalStateLabelInput): string {
  if (input.closing) return "Closing…";
  if (input.poppedOut) return "In its own window";
  if (input.signal === "failed") return "Failed";
  if (input.signal === "completed") return "Completed";
  const shellLabel = input.state === "running" && input.shell
    ? shellStatusLabel(input.shell)
    : null;
  return ({
    closed: "Closed",
    opening: "Opening…",
    running: shellLabel ?? "Running",
    exited: "Exited",
    failed: "Failed",
  } satisfies Record<PaneRuntimeState, string>)[input.state];
}

/** The linked plan or task, as the header badge states it; null when unlinked. */
export function terminalAssociationBadge(
  pointer: AssociationPointerV1 | undefined,
): string | null {
  if (!pointer) return null;
  const label = runtimeAssociationLabel({
    planId: pointer.planId,
    ...(pointer.taskId === undefined ? {} : { taskId: pointer.taskId }),
  });
  return `Linked · ${label}`;
}

/**
 * Read-only path marked for start truncation in the right-to-left layout.
 */
export function terminalWorkingDirectoryText(cwd: string): string {
  return cwd === "" ? "Project root" : `‎${cwd}‎`;
}
