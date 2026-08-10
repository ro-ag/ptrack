import { describe, expect, it } from "vitest";
import {
  taskTransitionCanStart,
  taskTransitionConfirmationCopy,
  taskTransitionFocusIntent,
  taskTransitionResponseIsCurrent,
} from "./task-transition";

const expected = {
  generation: 7,
  taskId: 42,
  fromStatus: "doing",
  toStatus: "done",
};

describe("task transition response fencing", () => {
  it("permits only one task status transaction at a time", () => {
    expect(taskTransitionCanStart(false, false)).toBe(true);
    expect(taskTransitionCanStart(true, false)).toBe(false);
    expect(taskTransitionCanStart(false, true)).toBe(false);
    expect(taskTransitionCanStart(true, true)).toBe(false);
  });

  it("never restores focus into a hidden or switched drawer", () => {
    expect(taskTransitionFocusIntent("drawer-select", true, true)).toBe("drawer-select");
    expect(taskTransitionFocusIntent("drawer-select", false, false)).toBe("card-select");
    expect(taskTransitionFocusIntent("drawer-select", true, false)).toBe("none");
    expect(taskTransitionFocusIntent("card-select", false, false)).toBe("card-select");
    expect(taskTransitionFocusIntent("drag", false, false)).toBe("drag");
  });

  it("accepts only the exact applied transition", () => {
    expect(taskTransitionResponseIsCurrent({
      ...expected,
      applied: true,
      requiresConfirmation: false,
    }, expected)).toBe(true);
    for (const patch of [
      { generation: 8 },
      { taskId: 43 },
      { fromStatus: "todo" },
      { toStatus: "blocked" },
      { applied: true, requiresConfirmation: true },
    ]) {
      expect(taskTransitionResponseIsCurrent({
        ...expected,
        applied: true,
        requiresConfirmation: false,
        ...patch,
      }, expected)).toBe(false);
    }
  });

  it("requires a bounded content-free challenge shape", () => {
    const challenge = {
      ...expected,
      applied: false,
      requiresConfirmation: true,
      confirmation: {
        token: "opaque",
        expiresAt: "2026-08-10T12:00:00Z",
        activeTerminals: 2,
        activeAgents: 1,
      },
    };
    expect(taskTransitionResponseIsCurrent(challenge, expected)).toBe(true);
    expect(taskTransitionResponseIsCurrent({
      ...challenge,
      confirmation: { ...challenge.confirmation, token: "" },
    }, expected)).toBe(false);
    expect(taskTransitionResponseIsCurrent({
      ...challenge,
      confirmation: { ...challenge.confirmation, activeAgents: -1 },
    }, expected)).toBe(false);
  });

  it("presents exact counts without runtime identities", () => {
    const copy = taskTransitionConfirmationCopy(42, "Doing", "Done", {
      token: "must-not-render",
      expiresAt: "2026-08-10T12:00:00Z",
      activeTerminals: 2,
      activeAgents: 1,
    });
    expect(copy).toContain("2 active terminals and 1 active agent");
    expect(copy).toContain("processes, and capabilities stay unchanged");
    expect(copy).not.toContain("must-not-render");
    expect(copy).not.toContain("2026-");
  });
});
