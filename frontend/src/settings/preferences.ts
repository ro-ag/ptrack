// Frontend view of the stored preferences record. The desktop runtime is the
// authority; this module normalizes what it returns, mirrors the parts the
// pre-paint guard and the terminal pane read out of localStorage, and keeps
// the same clamps so an optimistic render never disagrees with the store.

export type ThemePreference = "system" | "dark" | "light";
export type DensityPreference = "comfortable" | "compact";
export type ReducedMotionPreference = "system" | "always" | "never";
export type UnicodeModePreference = "modern" | "legacy";
export type RendererPreference = "auto" | "webgl" | "canvas" | "dom";
// "ok", "defaults", and "unreadable" are the statuses the runtime reports.
// "unavailable" is frontend-only: the command itself did not answer, so
// nothing about the stored record is known.
export type PreferencesStorageStatus =
  | "ok"
  | "defaults"
  | "unreadable"
  | "unavailable";

export interface AppearancePreferences {
  theme: ThemePreference;
  density: DensityPreference;
  reducedMotion: ReducedMotionPreference;
}

export interface TerminalPreferences {
  defaultProfileId: string | null;
  fontFamily: string;
  fontSize: number;
  unicodeMode: UnicodeModePreference;
  scrollback: number;
  renderer: RendererPreference;
}

export interface Preferences {
  version: number;
  appearance: AppearancePreferences;
  terminal: TerminalPreferences;
}

export const preferencesVersion = 1;

export const defaultPreferences: Preferences = {
  version: preferencesVersion,
  appearance: { theme: "system", density: "comfortable", reducedMotion: "system" },
  terminal: {
    defaultProfileId: null,
    fontFamily: "monospace",
    fontSize: 14,
    unicodeMode: "modern",
    scrollback: 25_000,
    renderer: "auto",
  },
};

export const terminalFontSizeRange = { minimum: 10, maximum: 24 } as const;
export const terminalScrollbackRange = { minimum: 1_000, maximum: 200_000 } as const;

// Storage keys the rest of the app already reads. They stay caches of the
// stored record so the first paint and the first terminal pane agree with it.
export const themeMirrorKey = "ptrack-theme";
export const terminalFontSizeMirrorKey = "ptrack-terminal-font-size";
export const terminalUnicodeMirrorKey = "ptrack-terminal-modern-unicode";
export const terminalFontFamilyMirrorKey = "ptrack-terminal-font-family";
export const terminalScrollbackMirrorKey = "ptrack-terminal-scrollback";
export const terminalRendererMirrorKey = "ptrack-terminal-renderer";
export const terminalDefaultProfileMirrorKey = "ptrack-terminal-default-profile";

interface MirrorStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem?(key: string): void;
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" ? value as Record<string, unknown> : {};
}

function enumeration<T extends string>(
  value: unknown,
  allowed: readonly T[],
  fallback: T,
): T {
  return allowed.includes(value as T) ? value as T : fallback;
}

function clamped(value: unknown, range: { minimum: number; maximum: number }, fallback: number) {
  const number = Number(value);
  if (!Number.isFinite(number)) return fallback;
  return Math.max(range.minimum, Math.min(range.maximum, Math.round(number)));
}

// normalizePreferences is total: any shape reads as a complete record, with
// unknown enum values, out-of-range numbers, and wrong types falling back to
// the documented defaults.
export function normalizePreferences(value: unknown): Preferences {
  const document = record(value);
  const appearance = record(document.appearance);
  const terminal = record(document.terminal);
  const profileId = terminal.defaultProfileId;
  return {
    version: preferencesVersion,
    appearance: {
      theme: enumeration(appearance.theme, ["system", "dark", "light"], "system"),
      density: enumeration(appearance.density, ["comfortable", "compact"], "comfortable"),
      reducedMotion: enumeration(
        appearance.reducedMotion,
        ["system", "always", "never"],
        "system",
      ),
    },
    terminal: {
      defaultProfileId: typeof profileId === "string" && profileId.trim() !== ""
        ? profileId
        : null,
      fontFamily: typeof terminal.fontFamily === "string" &&
          terminal.fontFamily.trim() !== ""
        ? terminal.fontFamily.trim()
        : defaultPreferences.terminal.fontFamily,
      fontSize: clamped(
        terminal.fontSize,
        terminalFontSizeRange,
        defaultPreferences.terminal.fontSize,
      ),
      unicodeMode: enumeration(terminal.unicodeMode, ["modern", "legacy"], "modern"),
      scrollback: clamped(
        terminal.scrollback,
        terminalScrollbackRange,
        defaultPreferences.terminal.scrollback,
      ),
      renderer: enumeration(
        terminal.renderer,
        ["auto", "webgl", "canvas", "dom"],
        "auto",
      ),
    },
  };
}

// preferencesResponse splits a GetPreferences/SetPreferences reply into the
// normalized record and the storage status the dialog states plainly.
export function preferencesResponse(
  value: unknown,
): { preferences: Preferences; storage: PreferencesStorageStatus } {
  const response = record(value);
  const document = response.preferences === undefined ? response : response.preferences;
  return {
    preferences: normalizePreferences(document),
    // A reply without a status is not a preferences document, so the stored
    // record was not read, whatever else came back.
    storage: enumeration(
      response.storage,
      ["ok", "defaults", "unreadable"],
      "unreadable",
    ),
  };
}

export function storageStatusNotice(status: PreferencesStorageStatus): string {
  if (status === "unreadable") {
    return "Stored settings could not be read, so defaults are shown. Changing a setting replaces the stored record.";
  }
  if (status === "unavailable") {
    return "Stored settings could not be read because p-track did not answer, so the values this window is already using are shown. Changes cannot be saved until it answers.";
  }
  return "";
}

// preferenceMirrors lists the localStorage cache writes for a record. A null
// value means the key is removed so the pre-paint guard falls back to the OS.
export function preferenceMirrors(
  preferences: Preferences,
): Array<readonly [string, string | null]> {
  return [
    [
      themeMirrorKey,
      preferences.appearance.theme === "system" ? null : preferences.appearance.theme,
    ],
    [terminalFontSizeMirrorKey, String(preferences.terminal.fontSize)],
    [
      terminalUnicodeMirrorKey,
      String(preferences.terminal.unicodeMode === "modern"),
    ],
    [terminalFontFamilyMirrorKey, preferences.terminal.fontFamily],
    [terminalScrollbackMirrorKey, String(preferences.terminal.scrollback)],
    [terminalRendererMirrorKey, preferences.terminal.renderer],
    [terminalDefaultProfileMirrorKey, preferences.terminal.defaultProfileId],
  ];
}

export function applyPreferenceMirrors(
  storage: MirrorStorage,
  preferences: Preferences,
): void {
  for (const [key, value] of preferenceMirrors(preferences)) {
    try {
      if (value === null) storage.removeItem?.(key);
      else storage.setItem(key, value);
    } catch {
      // Keep the live settings usable when WebView storage is unavailable.
    }
  }
}

// preferencesFromMirrors rebuilds a record out of the localStorage cache. It
// is the fallback when the runtime record cannot be read at all, so the
// dialog states what the window is actually using instead of clobbering the
// cache with defaults.
export function preferencesFromMirrors(storage: MirrorStorage): Preferences {
  const read = (key: string) => {
    try {
      return storage.getItem(key);
    } catch {
      return null;
    }
  };
  return normalizePreferences({
    appearance: { theme: read(themeMirrorKey) ?? "system" },
    terminal: {
      defaultProfileId: read(terminalDefaultProfileMirrorKey),
      fontFamily: read(terminalFontFamilyMirrorKey),
      fontSize: read(terminalFontSizeMirrorKey) ?? defaultPreferences.terminal.fontSize,
      unicodeMode: read(terminalUnicodeMirrorKey) === "false" ? "legacy" : "modern",
      scrollback: read(terminalScrollbackMirrorKey) ??
        defaultPreferences.terminal.scrollback,
      renderer: read(terminalRendererMirrorKey) ?? "auto",
    },
  });
}

export interface TerminalPreferenceOverrides {
  defaultProfileId: string;
  fontFamily: string;
  scrollback: number;
  renderer: RendererPreference;
}

// readTerminalPreferenceOverrides gives the terminal dock the stored terminal
// settings without another IPC round trip. Empty strings and zero mean "keep
// the profile's own value".
export function readTerminalPreferenceOverrides(
  storage: MirrorStorage,
): TerminalPreferenceOverrides {
  const read = (key: string) => {
    try {
      return storage.getItem(key);
    } catch {
      return null;
    }
  };
  const fontFamily = read(terminalFontFamilyMirrorKey);
  const scrollback = Number(read(terminalScrollbackMirrorKey));
  return {
    defaultProfileId: read(terminalDefaultProfileMirrorKey) || "",
    fontFamily: fontFamily && fontFamily !== defaultPreferences.terminal.fontFamily
      ? fontFamily
      : "",
    scrollback: Number.isFinite(scrollback) && scrollback > 0
      ? clamped(scrollback, terminalScrollbackRange, defaultPreferences.terminal.scrollback)
      : 0,
    renderer: enumeration(
      read(terminalRendererMirrorKey),
      ["auto", "webgl", "canvas", "dom"],
      "auto",
    ),
  };
}

// webglPreferredByPreference reports whether the stored renderer preference
// still allows the accelerated renderer. "canvas" has no installed addon, so
// it falls back to the DOM renderer exactly like "dom".
export function webglPreferredByPreference(renderer: RendererPreference): boolean {
  return renderer === "auto" || renderer === "webgl";
}

export function reducedMotionActive(
  preference: ReducedMotionPreference,
  prefersReducedMotion: boolean,
): boolean {
  if (preference === "always") return true;
  if (preference === "never") return false;
  return prefersReducedMotion;
}

export type PreferenceSavePhase = "saving" | "saved" | "failed" | "reset";

export function preferenceSaveMessage(phase: PreferenceSavePhase): string {
  if (phase === "saving") return "Saving settings…";
  if (phase === "saved") return "Settings saved.";
  if (phase === "reset") return "Settings reset to defaults.";
  return "Settings could not be saved. The stored record is unchanged.";
}
