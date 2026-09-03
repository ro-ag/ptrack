import { describe, expect, it } from "vitest";
import { agentContextText } from "./copy-context";

describe("copy context for an agent", () => {
  const project = { name: "Audio", root: "/projects/audio", goal: "Make playback reliable" };

  it("includes project, plan, and task references with commands for fresh records", () => {
    const text = agentContextText({
      project,
      plan: { id: 56, title: "Hardware settings", status: "active" },
      task: { id: 267, title: "Research device configuration", status: "todo", latestNote: "Check MIDI support" },
    });
    expect(text).toContain("Goal: Make playback reliable");
    expect(text).toContain("Plan #56: Hardware settings (active)");
    expect(text).toContain("Task #267: Research device configuration (todo)");
    expect(text).toContain("Latest task note: Check MIDI support");
    expect(text).toContain("ptrack plan show 56\nptrack task show 267");
  });

  it("copies project-only context without inventing plan or task references", () => {
    const text = agentContextText({ project });
    expect(text).toContain("ptrack context");
    expect(text).not.toMatch(/ptrack (plan|task) show/);
    expect(() => agentContextText({ project: { ...project, root: "" } })).toThrow();
  });

  it("quotes repository paths without executing shell substitutions", () => {
    const text = agentContextText({ project: { ...project, root: "/projects/O'Brien $(echo hi)" } });
    expect(text).toContain("cd -- '/projects/O'\\''Brien $(echo hi)'");
  });
});
