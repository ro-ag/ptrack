import { afterEach, describe, expect, it } from "vitest";

import { board, bootApp, snapshot } from "../test-support/app-harness";
import { shown } from "../test-support/journey";
import { historyCaption, summaryAgeLabel } from "./overview-view";

const now = Date.parse("2026-09-22T12:00:00Z");

describe("summaryAgeLabel", () => {
  it("dates a written summary with the shared short relative time", () => {
    expect(summaryAgeLabel("Shipped.", "2026-09-22T09:00:00Z", now)).toBe("Updated 3h ago");
    expect(summaryAgeLabel("Shipped.", "2026-09-15T12:00:00Z", now)).toBe("Updated 7d ago");
  });

  it("calls a write under a minute old, or one dated ahead, fresh", () => {
    expect(summaryAgeLabel("Shipped.", "2026-09-22T11:59:40Z", now)).toBe("Updated just now");
    expect(summaryAgeLabel("Shipped.", "2026-09-22T12:05:00Z", now)).toBe("Updated just now");
  });

  it("dates nothing without a summary or a recorded write", () => {
    expect(summaryAgeLabel("", "2026-09-22T09:00:00Z", now)).toBe("");
    expect(summaryAgeLabel("Shipped.", null, now)).toBe("");
    expect(summaryAgeLabel("Shipped.", undefined, now)).toBe("");
    expect(summaryAgeLabel("Shipped.", "not a date", now)).toBe("");
  });
});

describe("rolling summary card", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("shows when the summary was last written", async () => {
    const updatedAt = new Date(Date.now() - 2 * 3600 * 1000).toISOString();
    harness = await bootApp({
      open: { snapshot: snapshot(board({ summary: "Beta shipped.", summaryUpdatedAt: updatedAt })) },
    });
    expect(harness.$("#summary-age").hidden).toBe(false);
    expect(harness.$("#summary-age").textContent).toBe("Updated 2h ago");
  });

  it("stays quiet when the runtime has no write recorded", async () => {
    harness = await bootApp({
      open: { snapshot: snapshot(board({ summary: "Beta shipped.", summaryUpdatedAt: null })) },
    });
    expect(harness.$("#summary-age").hidden).toBe(true);
    expect(harness.$("#summary-age").textContent).toBe("");
  });
});

function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}

const ready = (fields = {}) => ({ state: "ready", trackedFiles: 12, projects: [], ...fields });

describe("Overview panels", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("says how to write the rolling summary when there is none", async () => {
    harness = await bootApp({ open: { snapshot: snapshot(board({ summary: "" })) } });
    expect(harness.$("#summary").textContent)
      .toBe("No rolling summary yet. Agents can update it with ptrack summary set.");
  });

  it("labels the linked commit count by its source", async () => {
    harness = await bootApp({ open: {} });
    const labels = harness.$$("#project-stats .stat-label").map((node) => node.textContent);
    expect(labels).toContain("Linked commits");
  });

  it("re-reads the stack on every snapshot without forcing a scan", async () => {
    harness = await bootApp({ open: {}, responses: { GetStackProfileV1: ready() } });
    await harness.emit("workspace:data-changed");
    const reads = harness.backend.callsTo("GetStackProfileV1");
    expect(reads.length).toBeGreaterThanOrEqual(2);
    expect(reads.every(([rescan]) => rescan === false)).toBe(true);
  });

  it("forces a scan only from Rescan, and skips plain reads while it runs", async () => {
    const scan = deferred();
    harness = await bootApp({
      open: {},
      responses: { GetStackProfileV1: (rescan) => (rescan ? scan.promise : ready()) },
    });
    const before = harness.backend.callsTo("GetStackProfileV1").length;
    await harness.click("#stack-rescan");
    await harness.emit("workspace:data-changed");
    expect(harness.backend.callsTo("GetStackProfileV1").slice(before)).toEqual([[true]]);
    scan.resolve(ready());
    await harness.settle();
    await harness.emit("workspace:data-changed");
    expect(harness.backend.callsTo("GetStackProfileV1").at(-1)).toEqual([false]);
  });

  it("keeps the newest of two overlapping stack reads", async () => {
    const reads = [];
    harness = await bootApp({
      open: {},
      responses: {
        GetStackProfileV1: () => {
          const read = deferred();
          reads.push(read);
          return read.promise;
        },
      },
    });
    await harness.emit("workspace:data-changed");
    expect(reads.length).toBeGreaterThanOrEqual(2);
    reads.at(-1).resolve(ready());
    await harness.settle();
    reads.at(-2).resolve(ready({ incomplete: true }));
    await harness.settle();
    const labels = harness.$$("#project-stats .stat-label").map((node) => node.textContent);
    expect(labels).not.toContain("Scan truncated");
  });

  it("reads the activity heatmap and history once the Overview is shown", async () => {
    harness = await bootApp({
      open: {},
      responses: {
        GetActivityHeatmapV2: [],
        GetProjectTimelineV1: { available: false, commits: [] },
      },
    });
    expect(harness.backend.names()).not.toContain("GetActivityHeatmapV2");
    await harness.click("#nav-overview");
    expect(shown(harness.$("#overview-page"))).toBe(true);
    expect(harness.backend.callsTo("GetActivityHeatmapV2")).toEqual([[16]]);
    expect(harness.backend.callsTo("GetProjectTimelineV1")).toEqual([[]]);
    expect(harness.$("#project-history").textContent)
      .toBe("No repository history to draw. This reads git, not p-track's commit records.");
    await harness.click("#nav-board");
    await harness.click("#nav-overview");
    expect(harness.backend.callsTo("GetActivityHeatmapV2")).toHaveLength(1);
  });
});

describe("historyCaption", () => {
  it("counts Git commits, not p-track's commit records", () => {
    const commits = [Date.parse("2026-01-01T12:00:00Z") / 1000, Date.parse("2026-03-01T12:00:00Z") / 1000];
    const caption = historyCaption({ commits, tags: [], truncated: false, available: true });
    expect(caption).toMatch(/^2 Git commits, /);
    expect(historyCaption({ commits, tags: [], truncated: true, available: true }))
      .toMatch(/^2 most recent Git commits, /);
  });
});

