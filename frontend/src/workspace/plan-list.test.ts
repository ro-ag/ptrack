import { describe, expect, it } from "vitest";
import { filterPlans } from "./plan-list";

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
