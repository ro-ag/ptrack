import { describe, expect, it } from "vitest";

import {
  terminalDiagnosticView,
  type TerminalDiagnosticInput,
} from "./diagnostics";

const base: TerminalDiagnosticInput = {
  stream: "connected",
  renderer: "webgl",
  process: "running",
  layout: "restored",
  rendererAttempts: 0,
  layoutRepairs: 0,
  changedAt: Date.UTC(2026, 7, 11, 8, 0, 0),
  hasSession: true,
  linked: false,
  busy: false,
  selected: true,
  visible: true,
};

describe("terminalDiagnosticView", () => {
  it("maps bounded content-free state to stable labels", () => {
    expect(terminalDiagnosticView({
      ...base,
      stream: "failed",
      renderer: "fallback",
      process: "failed",
      layout: "repaired",
      rendererAttempts: 99,
      layoutRepairs: 999,
    })).toEqual({
      rows: [
        { key: "process", label: "Process", value: "Failed" },
        { key: "stream", label: "Stream", value: "Failed" },
        { key: "renderer", label: "Renderer", value: "DOM fallback · 3/3 retries" },
        { key: "layout", label: "Layout", value: "Repaired · 128 repairs" },
        { key: "updated", label: "Updated", value: "2026-08-11T08:00:00.000Z" },
      ],
      canRestart: true,
      canRetryRenderer: true,
      canForceStop: true,
      canResetLayout: true,
    });
  });

  it("denies stale, hidden, linked, and busy recovery actions", () => {
    const view = terminalDiagnosticView({
      ...base,
      stream: "disconnected",
      renderer: "fallback",
      process: "failed",
      layout: "discarded",
      linked: true,
      busy: true,
      selected: false,
      visible: false,
    });
    expect(view.canRestart).toBe(false);
    expect(view.canRetryRenderer).toBe(false);
    expect(view.canForceStop).toBe(false);
    expect(view.canResetLayout).toBe(true);
  });

  it("never carries extra secret-bearing fields into output", () => {
    const secret = "STREAM_AUTHORITY_CANARY";
    const input = {
      ...base,
      streamUrl: `ws://127.0.0.1/?token=${secret}`,
      command: secret,
      output: secret,
      environment: secret,
    } as TerminalDiagnosticInput;
    expect(JSON.stringify(terminalDiagnosticView(input))).not.toContain(secret);
  });
});
