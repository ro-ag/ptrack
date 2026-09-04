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

  it("embeds drawer memories, issues, and commits when detail is available", () => {
    const text = agentContextText({
      project,
      plan: { id: 56, title: "Hardware settings", status: "active" },
      task: {
        id: 288,
        title: "Make lane extents per-system",
        status: "doing",
        detail: {
          notes: [
            { body: "Measured, not assumed:\nlanes are translation-invariant." },
            { body: "The real score-wide max is in autoplace::apply." },
          ],
          issues: [
            { id: 94, title: "lane extents lifts every system", severity: "high", status: "open" },
          ],
          commits: [{ sha: "eb39dd86abc", subject: "docs(research): reference policy (#288)" }],
        },
      },
    });
    expect(text).toContain("Recent memories:");
    expect(text).toContain("- Measured, not assumed: lanes are translation-invariant.");
    expect(text).toContain("Linked issues:");
    expect(text).toContain("- #94 (high, open): lane extents lifts every system");
    expect(text).toContain("Linked commits:");
    expect(text).toContain("- eb39dd86 docs(research): reference policy (#288)");
    expect(text).toContain("ptrack task show 288");
  });

  it("caps long detail lists and omits the sections entirely without detail", () => {
    const text = agentContextText({
      project,
      plan: { id: 56, title: "Hardware settings" },
      task: {
        id: 1,
        title: "Any task",
        detail: {
          notes: [{ body: "one" }, { body: "two" }, { body: "three" }, { body: "four" }],
          issues: [{ id: 7, title: "no severity or status" }],
          commits: [],
        },
      },
    });
    expect(text).toContain("- one");
    expect(text).not.toContain("- four");
    expect(text).toContain("- #7: no severity or status");
    expect(text).not.toContain("Linked commits:");
    const plain = agentContextText({
      project,
      plan: { id: 56, title: "Hardware settings" },
      task: { id: 1, title: "Any task" },
    });
    expect(plain).not.toContain("Recent memories:");
  });
});
