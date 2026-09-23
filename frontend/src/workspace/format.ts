// Shared display formatters. Every relative time and byte size the app shows
// goes through here, so the same instant or size never reads two ways.

export type RelativeTimeStyle = "short" | "long";

const relativeUnits: readonly [number, string, string][] = [
  // [seconds per unit, short suffix, long unit]
  [31_536_000, "y", "year"],
  [2_592_000, "mo", "month"],
  [86_400, "d", "day"],
  [3_600, "h", "hour"],
  [60, "m", "minute"],
];

function timestampOf(value: unknown): number {
  if (value instanceof Date) return value.getTime();
  if (typeof value === "number") return value;
  if (typeof value === "string" && value.trim() !== "") return new Date(value).getTime();
  return Number.NaN;
}

/**
 * Relative time from `value` (a Date, an ISO string, or epoch milliseconds)
 * to `now`. Both styles use the same thresholds and whole-unit flooring:
 * `short` ("3d ago", "in 5m", "now") fits dense metadata rows; `long`
 * ("3 days ago", "in 5 minutes", "just now") is for the landing page.
 */
export function relativeTime(
  value: unknown,
  style: RelativeTimeStyle = "short",
  now = Date.now(),
): string {
  const timestamp = timestampOf(value);
  const difference = now - timestamp;
  if (!Number.isFinite(difference)) return style === "long" ? "Date unavailable" : "";
  const seconds = Math.abs(difference) / 1000;
  const unit = relativeUnits.find(([size]) => seconds >= size);
  if (!unit) return style === "long" ? "just now" : "now";
  const [size, suffix, name] = unit;
  const amount = Math.floor(seconds / size);
  const phrase = style === "long"
    ? `${amount} ${name}${amount === 1 ? "" : "s"}`
    : `${amount}${suffix}`;
  return difference >= 0 ? `${phrase} ago` : `in ${phrase}`;
}

/** Binary byte size: "512 B", "1.5 KiB", "2.0 MiB", "1.2 GiB". */
export function formatBytes(value: unknown): string {
  const number = Number(value);
  const bytes = Number.isFinite(number) && number > 0 ? Math.trunc(number) : 0;
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KiB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MiB`;
  return `${(bytes / 1024 ** 3).toFixed(1)} GiB`;
}
