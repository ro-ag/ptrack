import type { AssociationPointerV1 } from "../workspace/model";
import { runtimeAssociationLabel } from "../workspace/presentation";
import type { PaneActivitySignal } from "./activity";
import type { PaneRuntimeState } from "./runtime";
import { shellStatusLabel, type ShellState } from "./shell-integration";

// The words both terminal surfaces put in their header for the active pane:
// the dock's toolbar and a detached window's info row read the same state the
// same way, so a shell that says "Prompt · last 0" in one never says
// "Running" in the other.

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
 * A working directory shown read-only. The start of a long path is the part
 * least worth keeping, so the element truncates from the start (see
 * `.terminal-window-cwd` in style.css); the marks pin the leading separator
 * to the start of the line under that right-to-left layout.
 */
export function terminalWorkingDirectoryText(cwd: string): string {
  return cwd === "" ? "Project root" : `‎${cwd}‎`;
}
