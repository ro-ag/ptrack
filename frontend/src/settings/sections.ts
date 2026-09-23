// Settings dialog sections: the frozen order of the roving-tabindex tablist,
// its keyboard traversal, and the read-only Data & Diagnostics rows.

export type SettingsSectionId =
  | "startup"
  | "appearance"
  | "terminal"
  | "notifications"
  | "updates"
  | "data";

// Startup leads: it is what happens before anything else is on screen, and it
// is neither an appearance choice nor part of the read-only report.
export const settingsSections: ReadonlyArray<
  { readonly id: SettingsSectionId; readonly label: string }
> = [
  { id: "startup", label: "Startup" },
  { id: "appearance", label: "Appearance" },
  { id: "terminal", label: "Terminal" },
  { id: "notifications", label: "Notifications" },
  { id: "updates", label: "Updates" },
  { id: "data", label: "Data & Diagnostics" },
];

export function settingsTabId(section: SettingsSectionId): string {
  return `settings-tab-${section}`;
}

export function settingsPanelId(section: SettingsSectionId): string {
  return `settings-panel-${section}`;
}

export function settingsSectionIndex(section: string): number {
  const index = settingsSections.findIndex((entry) => entry.id === section);
  return index < 0 ? 0 : index;
}

// nextSettingsSectionIndex moves the roving tabindex. Arrow keys wrap so a
// vertical tablist behaves like the platform list it looks like; -1 means the
// key belongs to the dialog, not the tablist.
export function nextSettingsSectionIndex(
  key: string,
  current: number,
  count: number,
): number {
  if (count <= 0) return -1;
  const index = Math.max(0, Math.min(current, count - 1));
  if (key === "ArrowDown" || key === "ArrowRight") return (index + 1) % count;
  if (key === "ArrowUp" || key === "ArrowLeft") return (index + count - 1) % count;
  if (key === "Home") return 0;
  if (key === "End") return count - 1;
  return -1;
}

export interface ResetConfirmationCopy {
  readonly eyebrow: string;
  readonly heading: string;
  readonly detail: string;
  readonly cancel: string;
  readonly submit: string;
}

// Both resets live only in Data & Diagnostics. Reset Window Layout is
// non-destructive; Reset Application State names what survives because a
// reset that reads as "erase everything" is one nobody dares run.
export const resetWindowLayoutConfirmation: ResetConfirmationCopy = {
  eyebrow: "Window layout",
  heading: "Reset the window layout?",
  detail:
    "The window size and position, the sidebar, and the board and terminal panels return to their defaults. Settings, plans, tasks, notes, project databases, and Recent projects are not touched.",
  cancel: "Keep Layout",
  submit: "Reset Layout",
};

// The footer reset covers every section of Settings, not just the one on
// screen, so it says so before it runs.
export const resetSettingsConfirmation: ResetConfirmationCopy = {
  eyebrow: "Settings",
  heading: "Reset all settings?",
  detail:
    "Every section of Settings returns to its default: startup, appearance, terminal, notifications, and updates. Plans, tasks, notes, window layout, and Recent projects are not touched.",
  cancel: "Keep Settings",
  submit: "Reset All Settings",
};

export const resetApplicationStateConfirmation: ResetConfirmationCopy = {
  eyebrow: "Application state",
  heading: "Reset all application state?",
  detail:
    "This clears your settings, notification and automatic update-check opt-ins, the window and layout state, and every saved terminal workspace. Plans, tasks, notes, and Recent projects are not touched.",
  cancel: "Keep Application State",
  submit: "Reset Application State",
};

// resetApplicationStateMessage reports what the runtime actually cleared, so a
// record it could not reach is never claimed as reset.
export function resetApplicationStateMessage(result: unknown): string {
  const response = result && typeof result === "object"
    ? result as Record<string, unknown>
    : {};
  const records = (Array.isArray(response.records) ? response.records : [])
    .filter((record): record is string => typeof record === "string" && record !== "");
  const cleared = records.length === 0
    ? "No stored records were cleared"
    : `Cleared ${records.join(", ")}`;
  return `${cleared}. Plans, tasks, notes, and Recent projects were not touched.`;
}

export type DiagnosticsGroup = "Global" | "This project";

export interface DiagnosticsRow {
  /** Where the row is shown: app-wide storage first, then the open project. */
  group: DiagnosticsGroup;
  label: string;
  value: string;
  /** Secondary text under the value: when and where, never the value again. */
  detail?: string;
  /** Accessible name for the copy control, or null when there is nothing to copy. */
  copy: string | null;
}

function humanize(key: string): string {
  const spaced = key.replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/[_-]+/g, " ");
  return spaced.charAt(0).toUpperCase() + spaced.slice(1).toLowerCase();
}

function looksLikePath(value: string): boolean {
  return value.startsWith("/") || /^[A-Za-z]:[\\/]/.test(value);
}

function leafValue(value: unknown): string | null {
  if (typeof value === "string") return value.trim() === "" ? null : value;
  if (typeof value === "number") return Number.isFinite(value) ? String(value) : null;
  if (typeof value === "boolean") return value ? "Yes" : "No";
  return null;
}

function fields(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" ? value as Record<string, unknown> : {};
}

function text(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

// The whole path is too long to hear read out on every copy control, so one
// segment of it distinguishes one otherwise identical button from the next.
function segment(path: string, fromEnd: number): string {
  const parts = path.split(/[\\/]/).filter((part) => part !== "");
  return parts[parts.length - 1 - fromEnd] ?? parts[parts.length - 1] ?? path;
}

// A null section is never dropped. Only `paths.project` states a reason, because
// it is the one field the report derives directly from the open workspace.
function absentValue(key: string): string {
  return key === "project" ? "No project open" : "Not available";
}

// The ledger carries RFC3339 with microsecond precision, which nobody reads as
// a date. Trimmed to the second in UTC — never rounded up to a precision the
// record does not have. An unparsable stamp is reported as unknown, not guessed.
function readableTime(value: string): string {
  if (value === "") return "Unknown time";
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return value;
  const iso = parsed.toISOString();
  return `${iso.slice(0, 10)} ${iso.slice(11, 19)} UTC`;
}

// A backup is one thing, so it is one row: the path is what anyone would act
// on, and when it was taken and which project it came from are context under
// it. A file the ledger names but that is gone says so before its own path.
function backupRows(value: unknown): DiagnosticsRow[] {
  const ledger = fields(value);
  if (ledger.status === "unavailable") {
    return [{ group: "Global", label: "Backups", value: "Not available", copy: null }];
  }
  const entries = Array.isArray(ledger.entries) ? ledger.entries : [];
  if (entries.length === 0) {
    return [{ group: "Global", label: "Backups", value: "None recorded", copy: null }];
  }
  return entries.slice(0, 25).map((entry) => {
    const backup = fields(entry);
    const path = text(backup.path);
    return {
      group: "Global",
      label: "Backup",
      value: path === "" ? "Not available" : path,
      detail: [
        backup.present === false ? "File missing" : "",
        readableTime(text(backup.recordedAt)),
        text(backup.project),
      ].filter((part) => part !== "").join(" · "),
      copy: path === "" ? null : `Copy backup path ${segment(path, 0)}`,
    };
  });
}

// One row per database reporting its count, not one row per field of it. A
// store that could not be read has no count, which is not the same as zero.
function quarantineRows(value: unknown): DiagnosticsRow[] {
  return (Array.isArray(value) ? value : []).map((entry): DiagnosticsRow => {
    const row = fields(entry);
    const count = row.count;
    const counted = row.status !== "unavailable" && typeof count === "number";
    const database = text(row.database) === "project" ? "project" : "global";
    return {
      group: "Global",
      label: `Quarantined records (${database} database)`,
      value: counted
        ? `${count} record${count === 1 ? "" : "s"}`
        : "Not available",
      copy: null,
    };
  });
}

function receiptRows(value: unknown): DiagnosticsRow[] {
  const receipts = (Array.isArray(value) ? value : []).map(text)
    .filter((path) => path !== "");
  if (receipts.length === 0) {
    return [{ group: "Global", label: "Migration receipts", value: "None recorded", copy: null }];
  }
  // Every receipt is `<migrations>/<id>/receipt.json`, so the file name names
  // all 25 of them the same thing. The migration id — the parent directory —
  // is the only part that tells one row, one button, and one copy
  // confirmation apart from the next.
  return receipts.slice(0, 25).map((path) => {
    const id = segment(path, 1);
    return {
      group: "Global",
      label: `Migration receipt ${id}`,
      value: path,
      copy: `Copy migration receipt path ${id}`,
    };
  });
}

// flatten handles the sections that really are scalars — paths and runtime —
// without assuming a field list, so a runtime that reports more detail still
// renders. A list it has no shape for is summarized, never dropped.
function flatten(
  report: unknown,
  prefix = "",
  depth = 0,
  group: DiagnosticsGroup = "Global",
): DiagnosticsRow[] {
  const rows: DiagnosticsRow[] = [];
  for (const [key, value] of Object.entries(fields(report))) {
    const label = prefix ? `${prefix} · ${humanize(key)}` : humanize(key);
    if (value === null) {
      rows.push({ group, label, value: absentValue(key), copy: null });
      continue;
    }
    const leaf = leafValue(value);
    if (leaf !== null) {
      rows.push({
        group,
        label,
        value: leaf,
        copy: looksLikePath(leaf) ? copyLabel(group, label) : null,
      });
      continue;
    }
    if (Array.isArray(value)) {
      rows.push({ group, label, value: `${value.length} recorded`, copy: null });
      continue;
    }
    if (typeof value === "object" && depth < 2) {
      rows.push(...flatten(value, label, depth + 1, group));
    }
  }
  return rows;
}

function copyLabel(group: DiagnosticsGroup, label: string): string {
  return `Copy ${group === "Global" ? "global" : "project"} ${label.toLowerCase()} path`;
}

// One named field: its human label, in the group it belongs to. A null field
// is reported as absent rather than dropped.
function fieldRow(
  group: DiagnosticsGroup,
  label: string,
  source: Record<string, unknown>,
  key: string,
): DiagnosticsRow[] {
  if (!(key in source)) return [];
  const value = source[key];
  if (value === null) return [{ group, label, value: absentValue(key), copy: null }];
  const leaf = leafValue(value);
  if (leaf === null) return [];
  return [{ group, label, value: leaf, copy: looksLikePath(leaf) ? copyLabel(group, label) : null }];
}

// The paths and runtime fields this dialog was designed around, with the
// label a person reads. Anything else the runtime adds still renders, after
// these, under a humanized key.
const globalPathLabels: Record<string, string> = {
  globalHome: "Home folder",
  globalDatabase: "Database",
  backupsDirectory: "Backups folder",
  migrationsDirectory: "Migrations folder",
  runtimeDirectory: "Runtime folder",
  updatesDirectory: "Updates folder",
};
const runtimeLabels: Record<string, string> = { status: "Runtime status", detail: "Runtime detail" };

// The report is read top to bottom, so where things are is a decision rather
// than whatever order the serializer happened to emit: Global (home, database,
// backups, migrations, runtime), then This project (root, database). A section
// the runtime adds later still renders, after the ones this dialog was
// designed around.
//
// The realistic worst case is 61 rows — 8 paths, 1 runtime, 25 backups,
// 2 quarantine stores, 25 receipts — which left the old cap of 64 a few
// backend fields of headroom. The cap is a runaway guard, not a budget.
const maxDiagnosticsRows = 128;

export function diagnosticsRows(report: unknown): DiagnosticsRow[] {
  if (!report || typeof report !== "object") return [];
  const sections = report as Record<string, unknown>;
  const paths = fields(sections.paths);
  const runtime = fields(sections.runtime);
  const migration = fields(sections.migration);
  const rows: DiagnosticsRow[] = [
    ...fieldRow("Global", globalPathLabels.globalHome, paths, "globalHome"),
    ...fieldRow("Global", globalPathLabels.globalDatabase, paths, "globalDatabase"),
    ...fieldRow("Global", globalPathLabels.backupsDirectory, paths, "backupsDirectory"),
    ...("backups" in sections ? backupRows(sections.backups) : []),
    ...fieldRow("Global", globalPathLabels.migrationsDirectory, paths, "migrationsDirectory"),
    ...("migration" in sections
      ? [...quarantineRows(migration.quarantine), ...receiptRows(migration.receipts)]
      : []),
    ...fieldRow("Global", globalPathLabels.runtimeDirectory, paths, "runtimeDirectory"),
    ...fieldRow("Global", runtimeLabels.status, runtime, "status"),
    ...fieldRow("Global", runtimeLabels.detail, runtime, "detail"),
    ...fieldRow("Global", globalPathLabels.updatesDirectory, paths, "updatesDirectory"),
  ];
  const extraPaths = Object.fromEntries(
    Object.entries(paths).filter(([key]) => !(key in globalPathLabels) && key !== "project"),
  );
  rows.push(...flatten(extraPaths));
  rows.push(...flatten(Object.fromEntries(
    Object.entries(runtime).filter(([key]) => !(key in runtimeLabels)),
  ), "Runtime"));
  // The runtime still reports the deprecated capability broker; the dialog no
  // longer displays it.
  const known = ["paths", "runtime", "backups", "migration", "capabilities"];
  for (const key of Object.keys(sections)) {
    if (!known.includes(key)) rows.push(...flatten({ [key]: sections[key] }));
  }
  if ("project" in paths) {
    const project = paths.project;
    if (project === null || typeof project !== "object") {
      rows.push({ group: "This project", label: "Project", value: absentValue("project"), copy: null });
    } else {
      const fieldsOf = fields(project);
      rows.push(
        ...fieldRow("This project", "Root folder", fieldsOf, "root"),
        ...fieldRow("This project", "Database", fieldsOf, "database"),
        ...flatten(Object.fromEntries(
          Object.entries(fieldsOf).filter(([key]) => key !== "root" && key !== "database"),
        ), "", 0, "This project"),
      );
    }
  }
  return rows.slice(0, maxDiagnosticsRows);
}
