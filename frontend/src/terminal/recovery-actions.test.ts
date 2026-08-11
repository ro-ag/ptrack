import { describe, expect, it, vi } from "vitest";

import {
  forceStopTerminalRecovery,
  resetTerminalWorkspaceRecovery,
  restartTerminalRecovery,
  retryTerminalRendererRecovery,
} from "./recovery-actions";

describe("terminal recovery actions", () => {
  it("closes a failed session before opening a fresh one", async () => {
    const order: string[] = [];
    await expect(restartTerminalRecovery({
      linked: false,
      close: async () => { order.push("close"); },
      accepted: () => order.at(-1) === "close",
      open: async () => { order.push("open"); },
    })).resolves.toBe("restarted");
    expect(order).toEqual(["close", "open"]);

    await expect(restartTerminalRecovery({
      linked: true,
      close: vi.fn(),
      accepted: () => true,
      open: vi.fn(),
    })).rejects.toThrow("launched again from their plan or task");
  });

  it("force-stops only the confirmed, still-current session", async () => {
    const close = vi.fn(async () => {});
    let sessionId = "session-a";
    await expect(forceStopTerminalRecovery({
      capture: () => ({ epoch: 1 }),
      currentSessionId: () => sessionId,
      closing: () => false,
      confirm: async () => {
        sessionId = "session-b";
        return true;
      },
      accepted: () => true,
      close,
    })).resolves.toBe("stale");
    expect(close).not.toHaveBeenCalled();

    sessionId = "session-b";
    await expect(forceStopTerminalRecovery({
      capture: () => ({ epoch: 2 }),
      currentSessionId: () => sessionId,
      closing: () => false,
      confirm: async () => true,
      accepted: () => true,
      close,
    })).resolves.toBe("stopped");
    expect(close).toHaveBeenCalledOnce();
  });

  it("retries only an allowed, current renderer", () => {
    const order: string[] = [];
    const options = {
      allowed: true,
      capture: () => ({ epoch: 1 }),
      accepted: () => true,
      reset: () => { order.push("reset"); },
      refresh: () => { order.push("refresh"); },
      attach: () => { order.push("attach"); },
      fit: () => { order.push("fit"); },
      render: () => { order.push("render"); },
    };
    expect(retryTerminalRendererRecovery(options)).toBe(true);
    expect(order).toEqual(["reset", "refresh", "attach", "fit", "render"]);
    order.length = 0;
    expect(retryTerminalRendererRecovery({ ...options, allowed: false })).toBe(false);
    expect(order).toEqual([]);
  });

  it("clears corrupt persistence only after close and replacement", async () => {
    const order: string[] = [];
    await expect(resetTerminalWorkspaceRecovery({
      confirm: async () => true,
      close: async () => { order.push("close"); },
      accepted: () => true,
      replace: () => {
        order.push("replace");
        return { version: 1 };
      },
      clear: () => { order.push("clear"); },
    })).resolves.toBe("reset");
    expect(order).toEqual(["close", "replace", "clear"]);

    order.length = 0;
    await expect(resetTerminalWorkspaceRecovery({
      confirm: async () => true,
      close: async () => { order.push("close"); },
      accepted: () => true,
      replace: () => null,
      clear: () => { order.push("clear"); },
    })).resolves.toBe("unchanged");
    expect(order).toEqual(["close"]);

    order.length = 0;
    await expect(resetTerminalWorkspaceRecovery({
      confirm: async () => true,
      close: async () => { order.push("close"); },
      accepted: () => false,
      replace: () => {
        order.push("replace");
        return { version: 1 };
      },
      clear: () => { order.push("clear"); },
    })).resolves.toBe("stale");
    expect(order).toEqual(["close"]);
  });
});
