import { describe, expect, it, vi } from "vitest";
import { bindProjectView } from "./project-view";
function fixture() {
  const buttons = ["carousel", "list"].map((view) => {
    const listeners = new Set<() => void>();
    return { dataset: { projectView: view }, pressed: "", setAttribute(_name: string, value: string) { this.pressed = value; }, addEventListener(_type: string, fn: () => void) { listeners.add(fn); }, removeEventListener(_type: string, fn: () => void) { listeners.delete(fn); }, click() { listeners.forEach((fn) => fn()); } };
  });
  const selected = { scrollIntoView: vi.fn() };
  const strip = { classList: { toggle: vi.fn() }, querySelector: () => selected };
  const stage = { hidden: false };
  const panel = { dataset: { projectView: "" }, querySelector: (selector: string) => selector === ".project-stage-wrap" ? stage : strip, querySelectorAll: () => buttons };
  return { root: { querySelector: () => panel } as unknown as ParentNode, panel, stage, strip, selected, buttons };
}
describe("project view", () => {
  it("switches only presentation and exposes pressed state, preserving selected button", () => {
    const f = fixture(); bindProjectView(f.root);
    f.buttons[1].click();
    expect(f.stage.hidden).toBe(true);
    expect(f.panel.dataset.projectView).toBe("list");
    expect(f.buttons.map((button) => button.pressed)).toEqual(["false", "true"]);
    expect(f.strip.querySelector()).toBe(f.selected);
    f.buttons[0].click();
    expect(f.stage.hidden).toBe(false);
    expect(f.buttons.map((button) => button.pressed)).toEqual(["true", "false"]);
  });
  it("releases click handlers and tolerates an absent landing panel", () => {
    const f = fixture(); const dispose = bindProjectView(f.root); dispose();
    f.buttons[1].click(); expect(f.stage.hidden).toBe(false);
    expect(() => bindProjectView({ querySelector: () => null } as unknown as ParentNode)()).not.toThrow();
  });
});
