import { describe, expect, it } from "vitest";

import {
  terminalAssociationBadge,
  terminalStateLabel,
  terminalWorkingDirectoryText,
} from "./session-info";
import { initialShellState } from "./shell-integration";

describe("terminalStateLabel", () => {
  it("reads the dock's session states in its words", () => {
    const label = (state: Parameters<typeof terminalStateLabel>[0]["state"]) =>
      terminalStateLabel({ state, shell: null });
    expect(label("closed")).toBe("Closed");
    expect(label("opening")).toBe("Opening…");
    expect(label("running")).toBe("Running");
    expect(label("exited")).toBe("Exited");
    expect(label("failed")).toBe("Failed");
  });

  it("lets shell integration speak for a running pane only", () => {
    const prompt = { ...initialShellState, phase: "prompt" as const, lastExitCode: 0 };
    expect(terminalStateLabel({ state: "running", shell: prompt })).toBe("Prompt · last 0");
    expect(terminalStateLabel({ state: "exited", shell: prompt })).toBe("Exited");
    expect(terminalStateLabel({ state: "running", shell: initialShellState })).toBe("Running");
  });

  it("puts closing, a popped-out pane, and an activity signal first", () => {
    const shell = { ...initialShellState, phase: "executing" as const };
    expect(terminalStateLabel({ state: "running", closing: true, shell })).toBe("Closing…");
    expect(terminalStateLabel({ state: "running", poppedOut: true, shell })).toBe(
      "In its own window",
    );
    expect(terminalStateLabel({ state: "running", signal: "completed", shell })).toBe(
      "Completed",
    );
    expect(terminalStateLabel({ state: "exited", signal: "failed", shell: null })).toBe("Failed");
  });
});

describe("header facts", () => {
  it("states a linked plan or task, and nothing when unlinked", () => {
    expect(terminalAssociationBadge(undefined)).toBeNull();
    expect(terminalAssociationBadge({ version: 1, planId: 4 })).toBe("Linked · plan #4");
    expect(terminalAssociationBadge({ version: 1, planId: 4, taskId: 17 })).toBe(
      "Linked · plan #4 · task #17",
    );
  });

  it("marks a path for start truncation and names the project root", () => {
    expect(terminalWorkingDirectoryText("/srv/app")).toBe("‎/srv/app‎");
    expect(terminalWorkingDirectoryText("")).toBe("Project root");
  });
});
