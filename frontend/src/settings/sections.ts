// Settings dialog sections: the frozen order of the roving-tabindex tablist,
// its keyboard traversal, and the read-only Data & Diagnostics rows.

export type SettingsSectionId = "appearance" | "terminal" | "updates" | "data";

export const settingsSections: ReadonlyArray<
  { readonly id: SettingsSectionId; readonly label: string }
> = [
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
