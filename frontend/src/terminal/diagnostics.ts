export type TerminalDiagnosticStream =
  | "idle"
  | "connecting"
  | "connected"
  | "disconnected"
  | "failed";

export type TerminalDiagnosticRenderer =
  | "none"
  | "webgl"
  | "recovering"
  | "dom"
  | "fallback";

export type TerminalDiagnosticProcess =
  | "stopped"
  | "starting"
  | "running"
  | "stopping"
  | "exited"
  | "failed";

export type TerminalDiagnosticLayout =
  | "default"
  | "restored"
  | "repaired"
  | "discarded";

export interface TerminalDiagnosticInput {
  stream: TerminalDiagnosticStream;
  renderer: TerminalDiagnosticRenderer;
  process: TerminalDiagnosticProcess;
  layout: TerminalDiagnosticLayout;
  rendererAttempts: number;
  layoutRepairs: number;
  changedAt: number;
  hasSession: boolean;
  linked: boolean;
  busy: boolean;
  selected: boolean;
  visible: boolean;
}

export interface TerminalDiagnosticRow {
  key: "process" | "stream" | "renderer" | "layout" | "updated";
  label: string;
  value: string;
}

export interface TerminalDiagnosticView {
  rows: TerminalDiagnosticRow[];
  canRestart: boolean;
  canRetryRenderer: boolean;
  canForceStop: boolean;
  canResetLayout: boolean;
}

const streamLabels: Record<TerminalDiagnosticStream, string> = {
  idle: "Idle",
  connecting: "Connecting",
  connected: "Connected",
  disconnected: "Disconnected",
  failed: "Failed",
};

const rendererLabels: Record<TerminalDiagnosticRenderer, string> = {
  none: "Not created",
  webgl: "WebGL",
  recovering: "Recovering",
  dom: "DOM",
  fallback: "DOM fallback",
};

const processLabels: Record<TerminalDiagnosticProcess, string> = {
  stopped: "Stopped",
  starting: "Starting",
  running: "Running",
  stopping: "Stopping",
  exited: "Exited",
  failed: "Failed",
};

const layoutLabels: Record<TerminalDiagnosticLayout, string> = {
  default: "Default",
  restored: "Restored",
  repaired: "Repaired",
  discarded: "Discarded",
};

function boundedCount(value: number, maximum: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(maximum, Math.trunc(value)));
}

export function terminalDiagnosticView(
  input: TerminalDiagnosticInput,
): TerminalDiagnosticView {
  const attempts = boundedCount(input.rendererAttempts, 3);
  const repairs = boundedCount(input.layoutRepairs, 128);
  const changedAt = Number.isFinite(input.changedAt) && input.changedAt > 0
    ? new Date(input.changedAt).toISOString()
    : "Not recorded";
  const renderer = attempts > 0
    ? `${rendererLabels[input.renderer]} · ${attempts}/3 retries`
    : rendererLabels[input.renderer];
  const layout = repairs > 0
    ? `${layoutLabels[input.layout]} · ${repairs} repairs`
    : layoutLabels[input.layout];
  const recoverableProcess = input.process === "exited" || input.process === "failed";
  const recoverableStream = input.stream === "disconnected" || input.stream === "failed";

  return {
    rows: [
      { key: "process", label: "Process", value: processLabels[input.process] },
      { key: "stream", label: "Stream", value: streamLabels[input.stream] },
      { key: "renderer", label: "Renderer", value: renderer },
      { key: "layout", label: "Layout", value: layout },
      { key: "updated", label: "Updated", value: changedAt },
    ],
    canRestart: !input.linked && !input.busy && (recoverableProcess || recoverableStream),
    canRetryRenderer: input.renderer === "fallback" &&
      !input.busy && input.selected && input.visible,
    canForceStop: input.hasSession && !input.busy &&
      (input.process === "starting" || input.process === "running" ||
        input.process === "stopping" || input.process === "failed"),
    canResetLayout: input.layout === "repaired" || input.layout === "discarded",
  };
}
