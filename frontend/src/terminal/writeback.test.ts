import { describe, expect, it } from "vitest";

import type { ActiveTerminalAssociation } from "./association-editor";
import {
  terminalWritebackContentPolicy,
  terminalWritebackMaximumBytes,
  terminalWritebackStateMatches,
  stableTerminalWritebackRequestID,
} from "./writeback";

function active(): ActiveTerminalAssociation {
  return {
    generation: 7,
    tabId: "tab-a",
    paneId: "pane-a",
    sessionId: "session-a",
    revision: 3,
    pointer: { version: 1, planId: 11, taskId: 13 },
  };
}

describe("terminal write-back policy", () => {
  it("normalizes explicit form content and counts UTF-8 bytes", () => {
    const result = terminalWritebackContentPolicy("  handoff 界\r\nnext  ");
    expect(result).toEqual({
      valid: true,
      normalized: "handoff 界\nnext",
      bytes: 16,
      message: `16 / ${terminalWritebackMaximumBytes} bytes`,
    });
  });

  it("rejects empty, huge multibyte, and overlong list content", () => {
    expect(terminalWritebackContentPolicy(" \n ").valid).toBe(false);
    expect(terminalWritebackContentPolicy("界".repeat(3_000)).valid).toBe(false);
    expect(terminalWritebackContentPolicy("line\n".repeat(129)).valid).toBe(false);
  });

  it("accepts only the exact generation, tab, pane, session, revision, and pointer", () => {
    const expected = active();
    expect(terminalWritebackStateMatches(expected, active())).toBe(true);
    for (const changed of [
      { generation: 8 },
      { tabId: "tab-b" },
      { paneId: "pane-b" },
      { sessionId: "session-b" },
      { revision: 4 },
      { pointer: { version: 1 as const, planId: 11 } },
      { pointer: undefined },
    ]) {
      expect(terminalWritebackStateMatches(expected, { ...active(), ...changed })).toBe(false);
    }
    expect(terminalWritebackStateMatches(expected, null)).toBe(false);
  });

  it("keeps the idempotency key across an unchanged re-preview", () => {
    let creates = 0;
    const create = () => `request-${++creates}`;
    const first = stableTerminalWritebackRequestID(null, create);
    const retry = stableTerminalWritebackRequestID(first, create);
    expect(first).toBe("request-1");
    expect(retry).toBe(first);
    expect(creates).toBe(1);
    expect(stableTerminalWritebackRequestID(null, create)).toBe("request-2");
  });
});
