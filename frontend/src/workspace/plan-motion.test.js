import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { animate } from "motion/mini";
import { bindPlanMotion } from "./plan-motion";

vi.mock("motion/mini", () => ({ animate: vi.fn() }));

describe("plan interaction motion", () => {
  let row;
  let title;
  let handlers;
  let stop;
  beforeEach(() => {
    handlers = {};
    row = { addEventListener: (name, handler) => { handlers[name] = handler; } };
    title = { style: {} };
    stop = vi.fn();
    animate.mockReturnValue({ stop });
    vi.stubGlobal("document", { documentElement: { dataset: {} } });
    vi.stubGlobal("window", { matchMedia: () => ({ matches: false }) });
    bindPlanMotion(row, title);
  });
  afterEach(() => vi.unstubAllGlobals());

  it("interrupts hover motion on press and resets on pointer exit", () => {
    handlers.pointerenter({ pointerType: "mouse" });
    handlers.pointerdown({ button: 0, target: { closest: () => null } });
    handlers.pointerleave();
    expect(animate.mock.calls.map((call) => call[1].transform)).toEqual([
      "translateX(4px)", "translateX(2px)", "translateX(0px)",
    ]);
    expect(stop).toHaveBeenCalledTimes(2);
  });

  it("does not animate hover for touch or presses on nested actions", () => {
    handlers.pointerenter({ pointerType: "touch" });
    handlers.pointerdown({ button: 0, target: { closest: () => ({}) } });
    expect(animate).not.toHaveBeenCalled();
  });

  it("honors system reduced motion and the app's explicit override", () => {
    window.matchMedia = () => ({ matches: true });
    handlers.pointerenter({ pointerType: "mouse" });
    expect(animate).not.toHaveBeenCalled();
    expect(title.style.transform).toBe("none");
    document.documentElement.dataset.reducedMotion = "never";
    handlers.pointerenter({ pointerType: "mouse" });
    expect(animate).toHaveBeenCalledTimes(1);
    document.documentElement.dataset.reducedMotion = "always";
    handlers.pointerleave();
    expect(stop).toHaveBeenCalledOnce();
    expect(animate).toHaveBeenCalledTimes(1);
    expect(title.style.transform).toBe("none");
  });
});
