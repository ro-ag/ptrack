import { describe, expect, it } from "vitest";

import {
  webglAttachAllowed,
  webglRecoveryDelay,
  type WebglRecoveryState,
} from "./renderer-recovery";

const recoverable: WebglRecoveryState = {
  disposed: false,
  attached: false,
  timerPending: false,
  attempts: 0,
  accepted: true,
  preferred: true,
  terminalHidden: false,
  documentHidden: false,
};

describe("terminal renderer recovery", () => {
  it("uses three bounded exponential retries before falling back to DOM", () => {
    expect([0, 1, 2, 3].map((attempts) =>
      webglRecoveryDelay({ ...recoverable, attempts }))).toEqual([250, 500, 1000, null]);
  });

  it("prevents policy rerenders from bypassing pending or exhausted recovery", () => {
    expect(webglAttachAllowed(recoverable, "policy")).toBe(true);
    expect(webglAttachAllowed({ ...recoverable, timerPending: true }, "policy")).toBe(false);
    expect(webglAttachAllowed({ ...recoverable, attempts: 3 }, "policy")).toBe(false);
    expect(webglAttachAllowed({ ...recoverable, attempts: 3 }, "retry")).toBe(true);
    expect(webglAttachAllowed({ ...recoverable, attempts: 4 }, "retry")).toBe(false);
  });

  it.each([
    ["disposed", { disposed: true }],
    ["already attached", { attached: true }],
    ["timer pending", { timerPending: true }],
    ["stale pane", { accepted: false }],
    ["not preferred", { preferred: false }],
    ["terminal hidden", { terminalHidden: true }],
    ["document hidden", { documentHidden: true }],
  ])("suppresses recovery while %s", (_name, override) => {
    expect(webglRecoveryDelay({ ...recoverable, ...override })).toBeNull();
  });
});
