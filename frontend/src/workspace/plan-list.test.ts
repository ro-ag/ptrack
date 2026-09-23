import { describe, expect, it } from "vitest";
import {
  currentPlanCloseoutLabel,
  filterPlans,
  pinnedPlanSelection,
  planIsViewOnly,
  planViewingLabel,
  splitCurrentPlan,
} from "./plan-list";

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

describe("one current plan", () => {
  const loaded = [
    { id: 13, title: "Selected", status: "active" },
    { id: 29, title: "CLI active", status: "active", isActive: true },
    { id: 30, title: "Other", status: "active" },
  ];

  it("pins the plan the board shows, not a different active plan", () => {
    const { current, rest } = splitCurrentPlan(loaded, 13);
    expect(current?.id).toBe(13);
    expect(rest.map((plan) => plan.id)).toEqual([29, 30]);
    expect(splitCurrentPlan(loaded, "30").current?.id).toBe(30);
  });

  it("falls back to the active plan only when no plan is on the board", () => {
    expect(splitCurrentPlan(loaded, 0).current?.id).toBe(29);
  });

  it("pins nothing when the selected plan is not loaded", () => {
    const { current, rest } = splitCurrentPlan(loaded, 99);
    expect(current).toBeUndefined();
    expect(rest).toEqual(loaded);
  });
});

describe("current plan closeout call to action", () => {
  it("appears once every task of an active plan is done", () => {
    expect(currentPlanCloseoutLabel({ id: 29, title: "x", status: "active", tasksDone: 8, tasksTotal: 8 }))
      .toBe("All 8 tasks done · Close plan…");
    expect(currentPlanCloseoutLabel({ id: 1, title: "x", status: "active", tasksDone: 1, tasksTotal: 1 }))
      .toBe("All 1 task done · Close plan…");
  });

  it("stays hidden with work left, on hold, closed, empty, or no plan", () => {
    const base = { id: 1, title: "x", status: "active", tasksDone: 8, tasksTotal: 8 };
    expect(currentPlanCloseoutLabel({ ...base, tasksDone: 7 })).toBeNull();
    expect(currentPlanCloseoutLabel({ ...base, holdReason: "waiting" })).toBeNull();
    expect(currentPlanCloseoutLabel({ ...base, status: "done" })).toBeNull();
    expect(currentPlanCloseoutLabel({ ...base, tasksDone: 0, tasksTotal: 0 })).toBeNull();
    expect(currentPlanCloseoutLabel(undefined)).toBeNull();
  });
});

describe("view-only plans", () => {
  const board = [
    { id: 1, title: "Foundation", status: "done" },
    { id: 2, title: "Playback engine", status: "active", isActive: true },
    { id: 4, title: "Prototype", status: "archived" },
    { id: 5, title: "Wrapped up", status: "done", isActive: true },
  ];

  it("treats done and archived plans as viewable but never current", () => {
    expect(planIsViewOnly(board[0])).toBe(true);
    expect(planIsViewOnly(board[2])).toBe(true);
    expect(planIsViewOnly(board[1])).toBe(false);
    expect(planIsViewOnly(undefined)).toBe(false);
  });

  it("keeps a plan completed while current as the current plan", () => {
    expect(planIsViewOnly(board[3])).toBe(false);
  });

  it("names the viewed plan and its state on the board heading", () => {
    expect(planViewingLabel(board[0])).toBe("Viewing #1 (done)");
    expect(planViewingLabel(board[2])).toBe("Viewing #4 (archived)");
  });

  it("pins the current plan while a view-only plan is on the board", () => {
    expect(pinnedPlanSelection(board, 1)).toBe(0);
    expect(splitCurrentPlan(board, pinnedPlanSelection(board, 1)).current?.id).toBe(2);
    expect(pinnedPlanSelection(board, 2)).toBe(2);
    expect(pinnedPlanSelection(board, 5)).toBe(5);
    expect(pinnedPlanSelection(board, 99)).toBe(99);
  });
});
