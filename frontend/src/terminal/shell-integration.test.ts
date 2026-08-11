import { describe, expect, it } from "vitest";

import {
  applyShellSignal,
  initialShellState,
  nextShellCWDValidation,
  parseShellOSC,
  safeShellCWD,
  shellStatusLabel,
} from "./shell-integration";

describe("terminal shell integration", () => {
  it("accepts exact nonce-bound 633 transitions and derives bounded completion metadata", () => {
    const nonce = "opaque-nonce";
    let state = initialShellState;
    for (const [payload, now] of [
      [`A;${nonce}`, 1],
      [`B;${nonce}`, 2],
      [`C;${nonce}`, 10],
      [`D;0;${nonce}`, 35],
    ] as const) {
      const signal = parseShellOSC(633, payload, nonce);
      expect(signal).not.toBeNull();
      state = applyShellSignal(state, signal!, now);
    }
    expect(state).toMatchObject({
      quality: "rich",
      phase: "completed",
      lastExitCode: 0,
      lastDurationMs: 25,
      sequence: 4,
    });
    expect(shellStatusLabel(state)).toBe("Command finished · 0");
    state = applyShellSignal(state, parseShellOSC(633, `A;${nonce}`, nonce)!, 36);
    expect(shellStatusLabel(state)).toBe("Prompt · last 0");
  });

  it("keeps standard 133 markers advisory and resets out-of-order sequences", () => {
    const start = parseShellOSC(133, "C")!;
    expect(applyShellSignal(initialShellState, start, 10)).toMatchObject({
      quality: "basic",
      phase: "unknown",
    });
    let state = applyShellSignal(initialShellState, parseShellOSC(133, "A")!, 1);
    state = applyShellSignal(state, parseShellOSC(133, "B")!, 2);
    state = applyShellSignal(state, parseShellOSC(133, "C")!, 3);
    state = applyShellSignal(state, parseShellOSC(133, "D;130")!, 8);
    expect(state).toMatchObject({ quality: "basic", phase: "completed", lastExitCode: 130 });
  });

  it("ignores advisory markers when an authenticated integration is active", () => {
    expect(parseShellOSC(133, "A", "nonce")).toBeNull();
    expect(parseShellOSC(633, "A", "nonce")).toBeNull();
    expect(parseShellOSC(633, "A;nonce", "nonce")).toEqual({
      kind: "prompt-start",
      authenticated: true,
    });
  });

  it("does not let unsigned output disrupt authenticated completion ordering", () => {
    const nonce = "nonce";
    let state = initialShellState;
    for (const payload of [`A;${nonce}`, `B;${nonce}`, `C;${nonce}`]) {
      state = applyShellSignal(state, parseShellOSC(633, payload, nonce)!, 10);
    }
    const unsigned = parseShellOSC(133, "D;99", nonce);
    expect(unsigned).toBeNull();
    expect(state.phase).toBe("executing");
    state = applyShellSignal(state, parseShellOSC(633, `D;7;${nonce}`, nonce)!, 25);
    expect(state).toMatchObject({ phase: "completed", lastExitCode: 7 });
  });

  it("rejects wrong nonces, malformed fields, integer overflow, and command text", () => {
    const invalid = [
      parseShellOSC(633, "A;wrong", "expected"),
      parseShellOSC(633, "D;0;wrong", "expected"),
      parseShellOSC(633, "D;2147483648;expected", "expected"),
      parseShellOSC(633, "A;expected;extra", "expected"),
      parseShellOSC(633, "E;printf secret;expected", "expected"),
      parseShellOSC(133, "D;0;extra"),
      parseShellOSC(133, "A\u0000"),
      parseShellOSC(133, "A".repeat(4097)),
    ];
    expect(invalid).toEqual(invalid.map(() => null));
  });

  it("parses only bounded absolute local working directories", () => {
    expect(safeShellCWD("file:///Users/test/a%20b")).toBe("/Users/test/a b");
    expect(safeShellCWD("file:///Users/test/semi%3B%20%C3%A9")).toBe("/Users/test/semi; é");
    expect(safeShellCWD("file://localhost/C:/Users/test")).toBe("C:/Users/test");
    expect(safeShellCWD("/home/test/project")).toBe("/home/test/project");
    expect(safeShellCWD("C:\\Users\\test")).toBe("C:\\Users\\test");
    for (const invalid of [
      "relative/path",
      "file://remote-host/home/test",
      "file:///home/test?query",
      "file:///home/test#fragment",
      "file:///home/%00test",
      "file:///home/%07test",
      "/home/test\nother",
    ]) expect(safeShellCWD(invalid)).toBeNull();
  });

  it("accepts nonce-bound CWD candidates without confusing them with state", () => {
    expect(parseShellOSC(633, "P;Cwd=/repo/subdir;nonce", "nonce")).toEqual({
      kind: "cwd",
      authenticated: true,
      cwd: "/repo/subdir",
    });
    expect(parseShellOSC(7, "file:///repo/subdir")).toEqual({
      kind: "cwd",
      authenticated: false,
      cwd: "/repo/subdir",
    });
  });

  it("invalidates pending CWD validation before deduplicating the latest candidate", () => {
    expect(nextShellCWDValidation(4, "/repo/a", "/repo/b")).toEqual({
      request: 5,
      validate: true,
    });
    expect(nextShellCWDValidation(5, "/repo/a", "/repo/a")).toEqual({
      request: 6,
      validate: false,
    });
  });
});
