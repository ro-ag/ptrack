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

/** The text an error, a rejected promise, or a bare string carries. */
export function messageFrom(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    const message = error.message;
    if (message) return String(message);
  }
  return "Something went wrong";
}

// Dense metadata rows use the short style of the shared formatter; the
// landing page uses its long style.
export function shortRelativeTime(value: unknown): string {
  return relativeTime(value, "short");
}

// Display names for the identifiers the backend can send. An unknown
// identifier renders as itself: a newer backend must never produce a blank
// label.
export const languageLabels: Readonly<Record<string, string>> = {
  rust: "Rust",
  go: "Go",
  javascript: "JavaScript",
  typescript: "TypeScript",
  python: "Python",
  swift: "Swift",
  java: "Java",
  kotlin: "Kotlin",
  csharp: "C#",
  ruby: "Ruby",
  php: "PHP",
  elixir: "Elixir",
  dart: "Dart",
  c: "C/C++",
  terraform: "Terraform",
  container: "Containers",
};

export function languageLabel(id: string): string {
  return languageLabels[id] || id;
}

// Long report titles (whole paragraphs, paths) must not become screen-reader
// labels: announcements stay to one line and the full text remains visible
// content on the button itself.
export function compactAriaText(text: unknown, max = 80): string {
  const flat = String(text).replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat;
}
