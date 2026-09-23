import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { animate } from "motion";
vi.mock("motion", () => ({ animate: vi.fn() }));
import { bindCoverDrag, boundedSelection, carouselGeometry, coverSummary, createCoverMotion, carouselPosition, horizontalGesture, landingProjects, selectedLandingProject } from "./landing";
import { relativeTime } from "./format";
import type { RecentProjectEntry } from "./recent-projects";
import type { Overview } from "./overview";
const a: RecentProjectEntry = { entryId: "a", base: "authorized-a", name: "Alpha", canonicalPath: "/alpha", lastOpenedAt: "2026-01-01T00:00:00Z", availability: "available" };
const b: RecentProjectEntry = { ...a, entryId: "b", name: "Beta", canonicalPath: "/beta", availability: "missing" };
const overview: Overview = { trackedProjects: 2, summarizedProjects: 1, counts: { activePlans: 1, openTasks: 2, doneTasks: 3, openIssues: 1 }, projects: [{ root: "/beta", syncedAt: 123, counts: { activePlans: 1, openTasks: 2, doneTasks: 3, openIssues: 1 }, activity: [] }] };
describe("landing carousel", () => {
  it("retains selected identity after reorder and falls back when filtered out", () => {
    expect(selectedLandingProject([b, a], "a")).toBe(a);
    expect(selectedLandingProject([b], "a")).toBe(b);
    expect(selectedLandingProject([], "a")).toBeNull();
  });
  it("filters actual availability and cache presence without inventing work status", () => {
    expect(landingProjects([a, b], overview, "", "available")).toEqual([a]);
    expect(landingProjects([a, b], overview, "", "synced")).toEqual([b]);
    expect(landingProjects([a, b], overview, " ALPHA ", "all")[0]).toBe(a);
    expect(landingProjects([a, b], null, "", "synced")).toEqual([]);
  });
  it("places cards on their actual side without wrapping at either end", () => {
    expect(carouselPosition(0, 0, 1)).toBe("pos-center");
    expect(carouselPosition(1, 0, 2)).toBe("pos-right");
    expect(carouselPosition(0, 1, 2)).toBe("pos-left");
    expect(carouselPosition(3, 0, 4)).toBe("pos-right");
    expect(carouselPosition(4, 0, 12)).toBe("pos-hidden");
    expect(carouselPosition(0, 0, 0)).toBe("pos-hidden");
  });
});

describe("cover flow navigation", () => {
  it("bounds zero, one, two and many projects without wrapping", () => {
    expect(boundedSelection(0, 1, 0)).toBe(-1);
    expect(boundedSelection(0, 1, 1)).toBe(0);
    expect(boundedSelection(0, -1, 2)).toBe(0);
    expect(boundedSelection(0, 1, 2)).toBe(1);
    expect(boundedSelection(1, 1, 2)).toBe(1);
    expect(boundedSelection(6, -1, 12)).toBe(5);
    expect(boundedSelection(11, 1, 12)).toBe(11);
  });
  it("moves the same album from angled depth to a front-facing center", () => {
    expect(carouselGeometry(1, 0)).toEqual({ x: 64, z: -140, angle: -58 });
    expect(carouselGeometry(1, 1)).toEqual({ x: 0, z: 36, angle: 0 });
    expect(carouselGeometry(1, 2)).toEqual({ x: -64, z: -140, angle: 58 });
    expect(carouselGeometry(5, 1).z).toBeLessThan(carouselGeometry(2, 1).z);
  });
  it("leaves vertical scrolling alone and requires deliberate horizontal travel", () => {
    const gesture = horizontalGesture();
    expect(gesture(10, 80, 0)).toBe(0);
    expect(gesture(15, 0, 250)).toBe(0);
    expect(gesture(15, 0, 260)).toBe(0);
    expect(gesture(15, 0, 270)).toBe(1);
  });
  it("consumes only one step through an inertial tail, then allows a fresh gesture", () => {
    const gesture = horizontalGesture();
    expect(gesture(60, 0, 0)).toBe(1);
    expect(gesture(100, 0, 50)).toBe(0);
    expect(gesture(50, 0, 160)).toBe(0);
    expect(gesture(5, 0, 280)).toBe(0);
    expect(gesture(-50, 0, 500)).toBe(-1);
  });
});

describe("Motion cover animation", () => {
  let card: HTMLButtonElement;
  const stop = vi.fn();
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(animate).mockReturnValue({ stop } as unknown as ReturnType<typeof animate>);
    card = { style: {} } as HTMLButtonElement;
    vi.stubGlobal("document", { documentElement: { dataset: { reducedMotion: "system" } } });
    vi.stubGlobal("window", { matchMedia: () => ({ matches: false }) });
  });
  afterEach(() => vi.unstubAllGlobals());
  it("sets first position directly then interpolates yaw, depth and horizontal position with Motion", () => {
    const motion = createCoverMotion(card);
    motion.move(1, 0);
    expect(card.style.transform).toContain("rotateY(-58deg)");
    expect(animate).not.toHaveBeenCalled();
    motion.move(1, 1);
    expect(animate).toHaveBeenCalledWith(0, 1, expect.objectContaining({ duration: 0.62, ease: [0.2, 0.8, 0.2, 1], onUpdate: expect.any(Function) }));
    const tick = vi.mocked(animate).mock.calls[0][2] as { onUpdate: (value: number) => void };
    tick.onUpdate(0.5);
    expect(card.style.transform).toBe("translate(-50%, -50%) perspective(760px) translate3d(32%, 0, -52px) rotateY(-29deg)");
    motion.move(1, 2);
    expect(stop).toHaveBeenCalledOnce();
    const retarget = vi.mocked(animate).mock.calls[1][2] as { onUpdate: (value: number) => void };
    retarget.onUpdate(0);
    expect(card.style.transform).toContain("translate3d(32%, 0, -52px)");
    retarget.onUpdate(1);
    expect(card.style.transform).toBe("translate(-50%, -50%) perspective(760px) translate3d(-64%, 0, -140px) rotateY(58deg)");
    motion.stop();
    expect(stop).toHaveBeenCalledTimes(2);
  });
  it("does not restart unchanged cards on summary refresh", () => {
    const motion = createCoverMotion(card);
    motion.move(1, 0); motion.move(1, 1); motion.move(1, 1);
    expect(animate).toHaveBeenCalledOnce();
    expect(stop).not.toHaveBeenCalled();
  });
  it("honors reduced motion and the app override, stopping active movement", () => {
    const motion = createCoverMotion(card);
    window.matchMedia = (() => ({ matches: true })) as typeof window.matchMedia;
    motion.move(1, 0); motion.move(1, 1);
    expect(animate).not.toHaveBeenCalled();
    document.documentElement.dataset.reducedMotion = "never";
    motion.move(1, 2);
    expect(animate).toHaveBeenCalledOnce();
    document.documentElement.dataset.reducedMotion = "always";
    motion.move(1, 2);
    expect(stop).toHaveBeenCalledOnce();
    expect(card.style.transform).toContain("rotateY(58deg)");
  });
});

describe("cover information", () => {
  it("shows actual counts and a labeled latest cached record, without inventing a goal", () => {
    const summary = { ...overview.projects[0], activity: [
      { kind: "task", id: 1, title: "Older work", status: "done", updatedAt: 100 },
      { kind: "issue", id: 2, title: "Fix reconnect", status: "open", updatedAt: 200 },
      { kind: "task", id: 3, title: "Future timestamp", status: "open", updatedAt: 400 },
    ] };
    expect(coverSummary(summary, 300000)).toEqual({
      context: "Latest issue: Fix reconnect",
      metrics: [{ value: 3, label: "done tasks" }, { value: 2, label: "open tasks" }, { value: 1, label: "active plans" }, { value: 1, label: "open issues" }],
      syncedAt: 123,
    });
    expect(summary.activity[0].id).toBe(1);
  });
  it("distinguishes missing summaries from synced projects with no recent records", () => {
    expect(coverSummary()).toEqual({ context: "Summary unavailable", metrics: [], syncedAt: null });
    expect(coverSummary(overview.projects[0]).context).toBe("No cached updates yet");
    expect(coverSummary({ ...overview.projects[0], syncedAt: NaN }).syncedAt).toBeNull();
  });
});

describe("pointer drag navigation", () => {
  let handlers: Record<string, (event: unknown) => void>;
  let follow: ReturnType<typeof vi.fn>, finish: ReturnType<typeof vi.fn>, capture: ReturnType<typeof vi.fn>;
  const event = (x: number, y = 0, extra = {}) => ({ clientX: x, clientY: y, pointerId: 1, button: 0, isPrimary: true, preventDefault: vi.fn(), ...extra });
  beforeEach(() => {
    handlers = {}; follow = vi.fn(); finish = vi.fn(); capture = vi.fn();
    const stage = { clientWidth: 500, addEventListener: (name: string, handler: (event: unknown) => void) => { handlers[name] = handler; },
      setPointerCapture: capture, hasPointerCapture: () => true, releasePointerCapture: vi.fn(), classList: { add: vi.fn(), remove: vi.fn() } } as unknown as HTMLElement;
    bindCoverDrag(stage, { enabled: () => true, follow, finish });
  });
  it("follows a horizontal pointer before release and snaps to the next cover", () => {
    handlers.pointerdown(event(100));
    handlers.pointermove(event(96));
    expect(capture).not.toHaveBeenCalled();
    handlers.pointermove(event(60));
    expect(capture).toHaveBeenCalledWith(1);
    expect(follow).toHaveBeenLastCalledWith(0.25);
    expect(finish).not.toHaveBeenCalled();
    handlers.pointerup(event(60));
    expect(finish).toHaveBeenCalledWith(1);
  });
  it("leaves vertical gestures alone and ignores secondary mouse buttons", () => {
    handlers.pointerdown(event(100));
    const vertical = event(98, 30);
    handlers.pointermove(vertical); handlers.pointerup(vertical);
    expect(vertical.preventDefault).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
    expect(finish).not.toHaveBeenCalled();
    handlers.pointerdown(event(100, 0, { button: 2 })); handlers.pointermove(event(20));
    expect(follow).not.toHaveBeenCalled();
  });
  it("snaps back below threshold or after cancellation while still suppressing the release click", () => {
    handlers.pointerdown(event(100)); handlers.pointermove(event(90)); handlers.pointerup(event(90));
    expect(finish).toHaveBeenLastCalledWith(0);
    handlers.pointerdown(event(100)); handlers.pointermove(event(20)); handlers.pointercancel(event(20));
    expect(finish).toHaveBeenLastCalledWith(0);
  });
  it("interpolates the exact center and side geometry continuously while dragging", () => {
    expect(carouselGeometry(1, 0.5)).toEqual({ x: 32, z: -52, angle: -29 });
    expect(Math.abs(carouselGeometry(1, 0.9999).x)).toBeLessThan(0.01);
  });
});

describe("relative project freshness", () => {
  const now = Date.UTC(2026, 8, 13, 12);
  it("uses readable units without implying old summaries are current", () => {
    expect(relativeTime(now - 10000, "long", now)).toBe("just now");
    expect(relativeTime(now - 120000, "long", now)).toBe("2 minutes ago");
    expect(relativeTime(now - 3600000, "long", now)).toBe("1 hour ago");
    expect(relativeTime(now - 86400000 * 4, "long", now)).toBe("4 days ago");
    expect(relativeTime(now - 86400000 * 60, "long", now)).toBe("2 months ago");
  });
  it("handles future clock skew and missing dates honestly", () => {
    expect(relativeTime(now + 120000, "long", now)).toBe("in 2 minutes");
    expect(relativeTime(NaN, "long", now)).toBe("Date unavailable");
  });
});
