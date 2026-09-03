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
      "notifications",
      "updates",
      "data",
    ]);
    expect(settingsTabId("terminal")).toBe("settings-tab-terminal");
    expect(settingsPanelId("terminal")).toBe("settings-panel-terminal");
  });

  it("falls back to the first section for an unknown id", () => {
    expect(settingsSectionIndex("updates")).toBe(4);
    expect(settingsSectionIndex("nonsense")).toBe(0);
  });

  it("wraps arrow traversal and jumps with Home and End", () => {
    expect(nextSettingsSectionIndex("ArrowDown", 0, 5)).toBe(1);
    expect(nextSettingsSectionIndex("ArrowDown", 5, 6)).toBe(0);
    expect(nextSettingsSectionIndex("ArrowUp", 0, 6)).toBe(5);
    expect(nextSettingsSectionIndex("ArrowRight", 1, 6)).toBe(2);
    expect(nextSettingsSectionIndex("ArrowLeft", 1, 6)).toBe(0);
    expect(nextSettingsSectionIndex("Home", 2, 6)).toBe(0);
    expect(nextSettingsSectionIndex("End", 0, 6)).toBe(5);
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
      "Plans, tasks, notes, and Recent projects are not touched.",
    );
    expect(resetApplicationStateConfirmation.submit).toBe("Reset Application State");
  });

  it("reports the records the runtime actually returned", () => {
    expect(
      resetApplicationStateMessage({
        records: ["preferences", "updates.auto-check", "window-state", "layout-state"],
      }),
    ).toBe(
      "Cleared preferences, updates.auto-check, window-state, layout-state. Plans, tasks, notes, and Recent projects were not touched.",
    );
  });

  it("claims nothing when the reply carries nothing", () => {
    const message = resetApplicationStateMessage({ records: ["", 7] });
    expect(message).toContain("No stored records were cleared.");
    expect(resetApplicationStateMessage(null)).toBe(message);
  });
});

// Every fixture below is shaped like DiagnosticsReportV1 in
// crates/ptrack-app/src/diagnostics_report.rs, so a backend field that moves
// breaks these rather than passing against a shape nothing emits.
describe("diagnostics rows", () => {
  it("labels the scalar sections and names what each copy control copies", () => {
    const rows = diagnosticsRows({
      paths: {
        globalHome: "/Users/dev/.ptrack",
        globalDatabase: "/Users/dev/.ptrack/global.redb",
      },
      runtime: { status: "active", detail: "" },
      capabilities: { granted: 2, total: 5 },
    });

    // The runtime still reports the deprecated capability broker, and the
    // dialog no longer renders it.
    expect(rows.map((row) => row.label)).not.toContain("Capabilities · Granted");

    expect(rows).toContainEqual({
      label: "Paths · Global home",
      value: "/Users/dev/.ptrack",
      copy: "Copy Paths · Global home",
    });
    expect(rows).toContainEqual({
      label: "Runtime · Status",
      value: "active",
      copy: null,
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
      copy: "Copy Paths · Project · Root",
    });
  });

  // `paths.globalDatabase` is null when the marker cannot be read, and
  // `paths.project` is null with no workspace open. A row that silently
  // disappeared would read as "there is nothing here" rather than "this could
  // not be reported".
  it("reports the sections the runtime nulls instead of dropping them", () => {
    const rows = diagnosticsRows({
      paths: { globalDatabase: null, project: null },
      capabilities: null,
    });

    expect(rows).toEqual([
      { label: "Paths · Global database", value: "Not available", copy: null },
      { label: "Paths · Project", value: "No project open", copy: null },
    ]);
  });

  it("renders the backup ledger entries the report emits", () => {
    const rows = diagnosticsRows({
      backups: {
        status: "available",
        entries: [
          {
            recordedAt: "2026-08-13T09:15:00.839591Z",
            project: "/work/app",
            path: "/Users/dev/.ptrack/backups/ptrack-20260813.redb",
            present: true,
          },
          {
            recordedAt: "2026-08-14T09:15:00.123Z",
            project: "/work/app",
            path: "/Users/dev/.ptrack/backups/ptrack-20260814.redb",
            present: false,
          },
        ],
      },
    });

    // Both entries still render, and each backup is one row rather than four.
    expect(rows).toHaveLength(2);
    expect(rows[0]).toEqual({
      label: "Backup",
      value: "/Users/dev/.ptrack/backups/ptrack-20260813.redb",
      detail: "2026-08-13 09:15:00 UTC · /work/app",
      copy: "Copy backup path ptrack-20260813.redb",
    });
    expect(rows[1].value).toBe("/Users/dev/.ptrack/backups/ptrack-20260814.redb");
    // A file the ledger names but that is gone says so, rather than spending a
    // row of its own on `Present: No`.
    expect(rows[1].detail).toBe("File missing · 2026-08-14 09:15:00 UTC · /work/app");
    expect(rows.map((row) => row.value)).not.toContain("2 recorded");
  });

  it("reports an unreadable or empty backup ledger honestly", () => {
    expect(diagnosticsRows({ backups: { status: "unavailable", entries: [] } }))
      .toEqual([{ label: "Backups", value: "Not available", copy: null }]);
    expect(diagnosticsRows({ backups: { status: "available", entries: [] } }))
      .toEqual([{ label: "Backups", value: "None recorded", copy: null }]);
  });

  // The runtime writes "" when it cannot format the recorded stamp at all.
  it("keeps an unformattable backup timestamp honest", () => {
    const rows = diagnosticsRows({
      backups: {
        status: "available",
        entries: [{ recordedAt: "", project: "", path: "/backups/a.redb", present: true }],
      },
    });

    expect(rows[0].detail).toBe("Unknown time");
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
      label: "Quarantine · Global",
      value: "0 records",
      copy: null,
    });
    expect(rows).toContainEqual({
      label: "Quarantine · Project",
      value: "3 records",
      copy: null,
    });
    expect(rows.map((row) => row.value)).not.toContain("2 recorded");
    expect(rows).toContainEqual({
      label: "Migration receipt 0001",
      value: "/Users/dev/.ptrack/migrations/0001/receipt.json",
      copy: "Copy migration receipt path 0001",
    });
  });

  // Every receipt the runtime emits is `<migrations>/<id>/receipt.json`
  // (RECEIPT_FILENAME in crates/ptrack-app/src/diagnostics_report.rs), so a
  // name taken from the file names all 25 buttons the same thing and the copy
  // confirmations that follow are identical too.
  it("tells one migration receipt apart from the next", () => {
    const rows = diagnosticsRows({
      migration: {
        quarantine: [],
        receipts: [
          "/Users/dev/.ptrack/migrations/0001-plans/receipt.json",
          "/Users/dev/.ptrack/migrations/0002-tasks/receipt.json",
        ],
      },
    });

    expect(rows.map((row) => row.copy)).toEqual([
      "Copy migration receipt path 0001-plans",
      "Copy migration receipt path 0002-tasks",
    ]);
    // The copy confirmation is announced as `${label} copied.`, so the labels
    // have to differ as well.
    expect(rows[0].label).not.toBe(rows[1].label);
    expect(new Set(rows.map((row) => row.copy)).size).toBe(rows.length);
  });

  // A store that could not be read has no count, which is not zero records.
  it("never reads an unreadable quarantine store as empty", () => {
    expect(diagnosticsRows({
      migration: {
        quarantine: [{ database: "project", status: "unavailable", count: null }],
        receipts: [],
      },
    })).toEqual([
      { label: "Quarantine · Project", value: "Not available", copy: null },
      { label: "Migration receipts", value: "None recorded", copy: null },
    ]);
  });

  it("summarizes a list it has no shape for instead of dropping it", () => {
    expect(diagnosticsRows({ audits: [{ id: 1 }, { id: 2 }] })).toContainEqual({
      label: "Audits",
      value: "2 recorded",
      copy: null,
    });
  });

  // Key order is the serializer's business; reading order is the dialog's.
  it("orders the report rather than trusting the serializer's key order", () => {
    const rows = diagnosticsRows({
      backups: { status: "available", entries: [] },
      runtime: { status: "active" },
      paths: { globalHome: "/Users/dev/.ptrack" },
    });

    expect(rows.map((row) => row.label)).toEqual([
      "Paths · Global home",
      "Runtime · Status",
      "Backups",
    ]);
  });

  // The row cap cuts from the end, so the two unbounded ledgers sit there and
  // the bounded sections sit above them. This fixture is the realistic worst
  // case plus four more path fields.
  it("keeps every section when the report outgrows the old 64-row cap", () => {
    const entries = Array.from({ length: 30 }, (_, index) => ({
      recordedAt: "2026-08-13T09:15:00Z",
      project: "/work/app",
      path: `/Users/dev/.ptrack/backups/ptrack-${index}.redb`,
      present: true,
    }));
    const rows = diagnosticsRows({
      paths: Object.fromEntries(
        Array.from({ length: 12 }, (_, index) => [`path${index}`, `/p/${index}`]),
      ),
      runtime: { status: "active" },
      backups: { status: "available", entries },
      migration: {
        quarantine: [
          { database: "global", status: "available", count: 0 },
          { database: "project", status: "available", count: 0 },
        ],
        receipts: Array.from(
          { length: 30 },
          (_, index) => `/Users/dev/.ptrack/migrations/${index}/receipt.json`,
        ),
      },
      capabilities: { granted: 1, total: 5 },
    });

    // 12 paths + 1 runtime + 25 backups + 2 quarantine + 25 receipts, with
    // both ledgers already capped at 25 by their own slices; the deprecated
    // `capabilities` section contributes nothing.
    expect(rows).toHaveLength(65);
    expect(rows.at(-1)?.label).toBe("Migration receipt 24");
  });

  it("ignores empty values and non-object reports", () => {
    expect(diagnosticsRows(null)).toEqual([]);
    expect(diagnosticsRows("report")).toEqual([]);
    expect(diagnosticsRows({ globalHome: "  " })).toEqual([]);
  });
});
