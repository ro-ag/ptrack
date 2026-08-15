import { describe, expect, it } from "vitest";

import {
  diagnosticsRows,
  nextSettingsSectionIndex,
  settingsPanelId,
  settingsSectionIndex,
  settingsSections,
  settingsTabId,
} from "./sections";

describe("settings sections", () => {
  it("pins the contract order", () => {
    expect(settingsSections.map((section) => section.id)).toEqual([
      "appearance",
      "terminal",
      "updates",
      "data",
    ]);
    expect(settingsTabId("terminal")).toBe("settings-tab-terminal");
    expect(settingsPanelId("terminal")).toBe("settings-panel-terminal");
  });

  it("falls back to the first section for an unknown id", () => {
    expect(settingsSectionIndex("updates")).toBe(2);
    expect(settingsSectionIndex("nonsense")).toBe(0);
  });

  it("wraps arrow traversal and jumps with Home and End", () => {
    expect(nextSettingsSectionIndex("ArrowDown", 0, 4)).toBe(1);
    expect(nextSettingsSectionIndex("ArrowDown", 3, 4)).toBe(0);
    expect(nextSettingsSectionIndex("ArrowUp", 0, 4)).toBe(3);
    expect(nextSettingsSectionIndex("ArrowRight", 1, 4)).toBe(2);
    expect(nextSettingsSectionIndex("ArrowLeft", 1, 4)).toBe(0);
    expect(nextSettingsSectionIndex("Home", 2, 4)).toBe(0);
    expect(nextSettingsSectionIndex("End", 0, 4)).toBe(3);
  });

  it("leaves other keys to the dialog", () => {
    expect(nextSettingsSectionIndex("Escape", 0, 4)).toBe(-1);
    expect(nextSettingsSectionIndex("Tab", 0, 4)).toBe(-1);
    expect(nextSettingsSectionIndex("ArrowDown", 0, 0)).toBe(-1);
  });
});

describe("diagnostics rows", () => {
  it("labels known sections and marks paths copyable", () => {
    const rows = diagnosticsRows({
      globalHome: "/Users/dev/.ptrack",
      projectDatabase: "/work/app/.ptrack/ptrack.redb",
      recovery: { required: false },
      capabilities: { granted: 2, total: 5 },
    });

    expect(rows).toContainEqual({
      label: "Global home",
      value: "/Users/dev/.ptrack",
      copyable: true,
    });
    expect(rows).toContainEqual({
      label: "Recovery · Required",
      value: "No",
      copyable: false,
    });
    expect(rows).toContainEqual({
      label: "Capabilities · Granted",
      value: "2",
      copyable: false,
    });
  });

  it("keeps nested project paths that the report groups under a section", () => {
    const rows = diagnosticsRows({
      paths: {
        globalHome: "/Users/dev/.ptrack",
        project: { root: "/work/app", database: "/work/app/.ptrack/ptrack.redb" },
      },
    });

    expect(rows).toContainEqual({
      label: "Paths · Project · Root",
      value: "/work/app",
      copyable: true,
    });
  });

  it("renders the backup ledger entries the report emits", () => {
    const rows = diagnosticsRows({
      backups: {
        status: "available",
        entries: [
          {
            recordedAt: "2026-08-13T09:15:00Z",
            project: "ptrack",
            path: "/Users/dev/.ptrack/backups/ptrack-20260813.redb",
            present: true,
          },
          {
            recordedAt: "2026-08-14T09:15:00Z",
            project: "ptrack",
            path: "/Users/dev/.ptrack/backups/ptrack-20260814.redb",
            present: false,
          },
        ],
      },
    });

    expect(rows).toContainEqual({
      label: "Backups · Entries · 2 · Path",
      value: "/Users/dev/.ptrack/backups/ptrack-20260814.redb",
      copyable: true,
    });
    expect(rows).toContainEqual({
      label: "Backups · Entries · 1 · Recorded at",
      value: "2026-08-13T09:15:00Z",
      copyable: false,
    });
    expect(rows).toContainEqual({
      label: "Backups · Entries · 2 · Present",
      value: "No",
      copyable: false,
    });
    expect(rows.map((row) => row.value)).not.toContain("2 recorded");
  });

  it("reports quarantine counts instead of the number of databases", () => {
    const rows = diagnosticsRows({
      migration: {
        quarantine: [
          { database: "global", status: "available", count: 0 },
          { database: "project", status: "available", count: 3 },
        ],
        receipts: ["/Users/dev/.ptrack/migrations/0001/receipt.json"],
      },
    });

    expect(rows).toContainEqual({
      label: "Migration · Quarantine · 2 · Database",
      value: "project",
      copyable: false,
    });
    expect(rows).toContainEqual({
      label: "Migration · Quarantine · 2 · Count",
      value: "3",
      copyable: false,
    });
    expect(rows).toContainEqual({
      label: "Migration · Quarantine · 1 · Count",
      value: "0",
      copyable: false,
    });
    expect(rows.map((row) => row.value)).not.toContain("2 recorded");
    expect(rows).toContainEqual({
      label: "Migration · Receipts",
      value: "/Users/dev/.ptrack/migrations/0001/receipt.json",
      copyable: false,
    });
  });

  it("summarizes a list it cannot expand instead of dropping it", () => {
    expect(diagnosticsRows({ backups: { status: "unavailable", entries: [] } }))
      .toContainEqual({
        label: "Backups · Entries",
        value: "0 recorded",
        copyable: false,
      });
  });

  it("ignores empty values and non-object reports", () => {
    expect(diagnosticsRows(null)).toEqual([]);
    expect(diagnosticsRows("report")).toEqual([]);
    expect(diagnosticsRows({ globalHome: "  " })).toEqual([]);
  });
});
