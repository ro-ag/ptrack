import type { AssociationPointerV1 } from "../workspace/model";
import type { StreamState } from "./client";
import type { TerminalDiagnosticInput, TerminalDiagnosticProcess } from "./diagnostics";
import { terminalDiagnosticStream } from "./diagnostics-popover";
import type { PaneRuntimeState } from "./runtime";
import {
  terminalAssociationBadge,
  terminalStateLabel,
  terminalWorkingDirectoryText,
} from "./session-info";
import type { ShellState } from "./shell-integration";

// Read-only active-pane details; editing requires main-window commands.

export interface TerminalWindowInfoElements {
  state: HTMLElement;
  profile: HTMLElement;
  cwd: HTMLElement;
  association: HTMLElement;
  associationLabel: HTMLElement;
}

/** What a detached pane knows about itself. */
export interface DetachedPaneFacts {
  stream: StreamState;
  /** The shell's output ended or its exit arrived. */
  ended: boolean;
  /** The exit carried an error rather than an exit code. */
  failed: boolean;
  shell: ShellState | null;
  changedAt: number;
}

export interface TerminalWindowInfoState {
  pane: DetachedPaneFacts | null;
  profileName: string;
  cwd: string;
  association: AssociationPointerV1 | undefined;
}

/** The dock's session state for a pane that lives in a window. */
export function detachedPaneState(pane: DetachedPaneFacts | null): PaneRuntimeState {
  if (!pane) return "opening";
  if (pane.ended) return pane.failed ? "failed" : "exited";
  return "running";
}

/**
 * Diagnostic input for detached panes; actions remain in the main window.
 */
export function detachedDiagnosticInput(input: {
  pane: DetachedPaneFacts | null;
  linked: boolean;
  visible: boolean;
}): TerminalDiagnosticInput {
  const { pane } = input;
  const processState = ({
    closed: "stopped",
    opening: "starting",
    running: "running",
    exited: "exited",
    failed: "failed",
  } satisfies Record<PaneRuntimeState, TerminalDiagnosticProcess>)[detachedPaneState(pane)];
  return {
    stream: terminalDiagnosticStream(pane !== null, pane?.stream),
    renderer: pane ? "dom" : "none",
    process: processState,
    layout: "default",
    rendererAttempts: 0,
    layoutRepairs: 0,
    changedAt: pane?.changedAt ?? 0,
    hasSession: pane !== null,
    linked: input.linked,
    busy: false,
    selected: true,
    visible: input.visible,
  };
}

export class TerminalWindowInfo {
  readonly #elements: TerminalWindowInfoElements;

  constructor(elements: TerminalWindowInfoElements) {
    this.#elements = elements;
  }

  render(info: TerminalWindowInfoState): void {
    const { state, profile, cwd, association, associationLabel } = this.#elements;
    const label = terminalStateLabel({
      state: detachedPaneState(info.pane),
      shell: info.pane?.shell ?? null,
    });
    state.textContent = label;
    state.title = label;
    profile.textContent = info.profileName;
    profile.title = info.profileName;
    cwd.textContent = terminalWorkingDirectoryText(info.cwd);
    // The whole path, however much of it the row has room for.
    cwd.title = info.cwd || "Project root";
    const badge = terminalAssociationBadge(info.association);
    association.hidden = badge === null;
    associationLabel.textContent = badge ?? "";
    association.title = badge
      ? `${badge}. Relink it from the main window.`
      : "";
  }
}
