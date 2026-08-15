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
// non-destructive; Reset Application State names the capability grants because
// re-granting them is real work, and names what survives because a reset that
// reads as "erase everything" is one nobody dares run.
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
    "This clears your settings, the automatic update-check opt-in, the window and layout state, and every saved terminal workspace. Network capability grants live in the project and are revoked in the open project, and must be granted again before any network access works. Plans, tasks, notes, and Recent projects are not touched.",
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
  const count = Number(response.capabilityGrants);
  const grants = Number.isFinite(count) && count > 0 ? Math.trunc(count) : 0;
  const cleared = records.length === 0
    ? "No stored records were cleared"
    : `Cleared ${records.join(", ")}`;
  const revoked = grants === 0
    ? "No network capability grants were revoked"
    : `${grants} network capability grant${grants === 1 ? "" : "s"} ${
      grants === 1 ? "was" : "were"
    } revoked and must be granted again`;
  return `${cleared}. ${revoked}. Plans, tasks, notes, and Recent projects were not touched.`;
}

export interface DiagnosticsRow {
  label: string;
  value: string;
  copyable: boolean;
}

const diagnosticsLabels: Readonly<Record<string, string>> = {
  globalHome: "Global home",
  projectDatabase: "Project database",
  runtimeDirectory: "Runtime directory",
  updatesDirectory: "Updates directory",
  backups: "Backups",
  migration: "Migration",
  recovery: "Recovery",
  capabilities: "Capabilities",
};

function humanize(key: string): string {
  const labeled = diagnosticsLabels[key];
  if (labeled) return labeled;
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

// diagnosticsRows flattens the read-only report into labelled rows without
// assuming a field list, so a runtime that reports more detail still renders.
// Nesting deeper than a section and its members is summarized, never dropped.
export function diagnosticsRows(
  report: unknown,
  prefix = "",
  depth = 0,
): DiagnosticsRow[] {
  if (!report || typeof report !== "object") return [];
  const rows: DiagnosticsRow[] = [];
  for (const [key, value] of Object.entries(report as Record<string, unknown>)) {
    const label = prefix ? `${prefix} · ${humanize(key)}` : humanize(key);
    const leaf = leafValue(value);
    if (leaf !== null) {
      rows.push({ label, value: leaf, copyable: looksLikePath(leaf) });
      continue;
    }
    if (Array.isArray(value)) {
      const entries = value.map(leafValue).filter((entry): entry is string => entry !== null);
      if (entries.length === value.length && entries.length > 0) {
        rows.push({ label, value: entries.slice(0, 8).join(", "), copyable: false });
        continue;
      }
      // Element objects carry the real fields — backup paths and timestamps,
      // quarantine counts — so each one gets its own indexed rows instead of
      // a length that reads like a total.
      if (value.length > 0 && depth < 2) {
        value.slice(0, 8).forEach((entry, index) => {
          rows.push(...diagnosticsRows(entry, `${label} · ${index + 1}`, depth + 1));
        });
        continue;
      }
      rows.push({ label, value: `${value.length} recorded`, copyable: false });
      continue;
    }
    if (value && typeof value === "object" && depth < 2) {
      rows.push(...diagnosticsRows(value, label, depth + 1));
    }
  }
  return rows.slice(0, 64);
}
