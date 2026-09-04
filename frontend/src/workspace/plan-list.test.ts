import { describe, expect, it } from "vitest";
import { filterPlans, splitCurrentPlan } from "./plan-list";

const plans = [
  { id: 1, title: "Foundation", status: "done" },
  { id: 2, title: "Playback engine", status: "active" },
  { id: 3, title: "Playback export", status: "active", holdReason: "Waiting on engine" },
  { id: 4, title: "Prototype", status: "archived" },
];

describe("sidebar plan filters", () => {
  it("combines a case-insensitive title or number with lifecycle status", () => {
    expect(filterPlans(plans, " PLAYBACK ", "open").map((plan) => plan.id)).toEqual([2, 3]);
    expect(filterPlans(plans, "3", "held").map((plan) => plan.id)).toEqual([3]);
    expect(filterPlans(plans, "playback", "done")).toEqual([]);
    expect(filterPlans(plans, "", "archived").map((plan) => plan.id)).toEqual([4]);
  });

  it("restores all loaded plans without changing their order or records", () => {
    expect(filterPlans(plans, "", "all")).toEqual(plans);
    expect(filterPlans([], "", "all")).toEqual([]);
  });
});

describe("sidebar current-plan split", () => {
  it("lifts the isActive plan out and keeps the remaining order", () => {
    const withCurrent = [
      { id: 5, title: "First", status: "done" },
      { id: 6, title: "Current work", status: "active", isActive: true },
      { id: 7, title: "Third", status: "active" },
    ];
    const { current, rest } = splitCurrentPlan(withCurrent);
    expect(current?.id).toBe(6);
    expect(rest.map((plan) => plan.id)).toEqual([5, 7]);
  });

  it("keeps every plan in rest when nothing is current", () => {
    const { current, rest } = splitCurrentPlan(plans);
    expect(current).toBeUndefined();
    expect(rest).toEqual(plans);
  });

  it("keeps the list whole for an empty project", () => {
    expect(splitCurrentPlan([])).toEqual({ current: undefined, rest: [] });
  });
});
