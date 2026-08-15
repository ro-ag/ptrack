import { describe, expect, it } from "vitest";

import {
  diagnosticsRows,
  nextSettingsSectionIndex,
  resetApplicationStateConfirmation,
  resetApplicationStateMessage,
  resetWindowLayoutConfirmation,
  settingsPanelId,
  settingsSectionIndex,
  settingsSections,
  settingsTabId,
} from "./sections";

describe("settings sections", () => {
  it("pins the contract order", () => {
    expect(settingsSections.map((section) => section.id)).toEqual([
      "startup",
      "appearance",
      "terminal",
      "updates",
      "data",
    ]);
    expect(settingsTabId("terminal")).toBe("settings-tab-terminal");
    expect(settingsPanelId("terminal")).toBe("settings-panel-terminal");
  });

  it("falls back to the first section for an unknown id", () => {
    expect(settingsSectionIndex("updates")).toBe(3);
    expect(settingsSectionIndex("nonsense")).toBe(0);
  });

  it("wraps arrow traversal and jumps with Home and End", () => {
    expect(nextSettingsSectionIndex("ArrowDown", 0, 5)).toBe(1);
    expect(nextSettingsSectionIndex("ArrowDown", 4, 5)).toBe(0);
    expect(nextSettingsSectionIndex("ArrowUp", 0, 5)).toBe(4);
    expect(nextSettingsSectionIndex("ArrowRight", 1, 5)).toBe(2);
    expect(nextSettingsSectionIndex("ArrowLeft", 1, 5)).toBe(0);
    expect(nextSettingsSectionIndex("Home", 2, 5)).toBe(0);
    expect(nextSettingsSectionIndex("End", 0, 5)).toBe(4);
  });

  it("leaves other keys to the dialog", () => {
    expect(nextSettingsSectionIndex("Escape", 0, 5)).toBe(-1);
    expect(nextSettingsSectionIndex("Tab", 0, 5)).toBe(-1);
    expect(nextSettingsSectionIndex("ArrowDown", 0, 0)).toBe(-1);
  });
});

describe("reset confirmations", () => {
  it("states what each reset spares", () => {
    expect(resetWindowLayoutConfirmation.detail).toContain(
      "Settings, plans, tasks, notes, project databases, and Recent projects are not touched.",
    );
    expect(resetApplicationStateConfirmation.detail).toContain(
      "Network capability grants live in the project and are revoked in the open project, and must be granted again",
    );
    expect(resetApplicationStateConfirmation.detail).toContain(
      "Plans, tasks, notes, and Recent projects are not touched.",
    );
    expect(resetApplicationStateConfirmation.submit).toBe("Reset Application State");
  });

  // Revoking a grant writes DisableCapabilityV2 into the open project, so no
  // reset copy may promise the project database is untouched.
  it("never claims the project database survives a grant revocation", () => {
    expect(resetApplicationStateConfirmation.detail).not.toContain("project database");
    expect(resetApplicationStateMessage({ records: ["preferences"], capabilityGrants: 1 }))
      .not.toContain("project database");
  });

  it("reports the records and grants the runtime actually returned", () => {
    expect(
      resetApplicationStateMessage({
        records: ["preferences", "updates.auto-check", "window-state", "layout-state"],
        capabilityGrants: 2,
      }),
    ).toBe(
      "Cleared preferences, updates.auto-check, window-state, layout-state. 2 network capability grants were revoked and must be granted again. Plans, tasks, notes, and Recent projects were not touched.",
    );
    expect(resetApplicationStateMessage({ records: ["preferences"], capabilityGrants: 1 }))
      .toContain("1 network capability grant was revoked and must be granted again.");
  });

  it("claims nothing when the reply carries nothing", () => {
    const message = resetApplicationStateMessage({ records: ["", 7], capabilityGrants: "many" });
    expect(message).toContain("No stored records were cleared.");
    expect(message).toContain("No network capability grants were revoked.");
    expect(resetApplicationStateMessage(null)).toBe(message);
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
