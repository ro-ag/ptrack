// Settings dialog sections: the frozen order of the roving-tabindex tablist,
// its keyboard traversal, and the read-only Data & Diagnostics rows.

export type SettingsSectionId =
  | "startup"
  | "appearance"
  | "terminal"
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

export const resetApplicationStateConfirmation: ResetConfirmationCopy = {
  eyebrow: "Application state",
  heading: "Reset all application state?",
  detail:
    "This clears your settings, the automatic update-check opt-in, the window and layout state, and every saved terminal workspace. Plans, tasks, notes, and Recent projects are not touched.",
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

export interface DiagnosticsRow {
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
    return [{ label: "Backups", value: "Not available", copy: null }];
  }
  const entries = Array.isArray(ledger.entries) ? ledger.entries : [];
  if (entries.length === 0) {
    return [{ label: "Backups", value: "None recorded", copy: null }];
  }
  return entries.slice(0, 25).map((entry) => {
    const backup = fields(entry);
    const path = text(backup.path);
    return {
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
  return (Array.isArray(value) ? value : []).map((entry) => {
    const row = fields(entry);
    const count = row.count;
    const counted = row.status !== "unavailable" && typeof count === "number";
    return {
      label: `Quarantine · ${humanize(text(row.database) || "unknown")}`,
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
    return [{ label: "Migration receipts", value: "None recorded", copy: null }];
  }
  // Every receipt is `<migrations>/<id>/receipt.json`, so the file name names
  // all 25 of them the same thing. The migration id — the parent directory —
  // is the only part that tells one row, one button, and one copy
  // confirmation apart from the next.
  return receipts.slice(0, 25).map((path) => {
    const id = segment(path, 1);
    return {
      label: `Migration receipt ${id}`,
      value: path,
      copy: `Copy migration receipt path ${id}`,
    };
  });
}

// flatten handles the sections that really are scalars — paths and runtime —
// without assuming a field list, so a runtime that reports more detail still
// renders. A list it has no shape for is summarized, never dropped.
function flatten(report: unknown, prefix = "", depth = 0): DiagnosticsRow[] {
  const rows: DiagnosticsRow[] = [];
  for (const [key, value] of Object.entries(fields(report))) {
    const label = prefix ? `${prefix} · ${humanize(key)}` : humanize(key);
    if (value === null) {
      rows.push({ label, value: absentValue(key), copy: null });
      continue;
    }
    const leaf = leafValue(value);
    if (leaf !== null) {
      rows.push({
        label,
        value: leaf,
        copy: looksLikePath(leaf) ? `Copy ${label}` : null,
      });
      continue;
    }
    if (Array.isArray(value)) {
      rows.push({ label, value: `${value.length} recorded`, copy: null });
      continue;
    }
    if (typeof value === "object" && depth < 2) {
      rows.push(...flatten(value, label, depth + 1));
    }
  }
  return rows;
}

// The report is read top to bottom, so where things are is a decision rather
// than whatever order the serializer happened to emit. A section the runtime
// adds later still renders, after the ones this dialog was designed around.
//
// The bounded sections come first and the two ledgers last, because the cap
// below cuts from the end: the only thing it may ever drop is the 26th backup,
// never a whole section nobody will notice is missing.
const reportOrder = ["paths", "runtime", "backups", "migration"];

// The realistic worst case is 61 rows — 8 paths, 1 runtime, 25 backups,
// 2 quarantine stores, 25 receipts — which left the old cap of 64 a few
// backend fields of headroom. The cap is a runaway guard, not a budget.
const maxDiagnosticsRows = 128;

export function diagnosticsRows(report: unknown): DiagnosticsRow[] {
  if (!report || typeof report !== "object") return [];
  const sections = report as Record<string, unknown>;
  const keys = [
    ...reportOrder.filter((key) => key in sections),
    ...Object.keys(sections).filter((key) => !reportOrder.includes(key)),
  ];
  // The runtime still reports the deprecated capability broker; the dialog no
  // longer displays it.
  const skipped = ["capabilities"];
  const rows: DiagnosticsRow[] = [];
  for (const key of keys) {
    if (skipped.includes(key)) continue;
    const value = sections[key];
    if (key === "backups") rows.push(...backupRows(value));
    else if (key === "migration") {
      const migration = fields(value);
      rows.push(...quarantineRows(migration.quarantine));
      rows.push(...receiptRows(migration.receipts));
    } else rows.push(...flatten({ [key]: value }));
  }
  return rows.slice(0, maxDiagnosticsRows);
}
