export const modernUnicodeStorageKey = "ptrack-terminal-modern-unicode";

// The key is a mirror of the stored preferences record, written only by
// applyPreferenceMirrors, so this module reads and never writes it.
interface SettingStorage {
  getItem(key: string): string | null;
}

export function modernUnicodeEnabled(storedValue: string | null): boolean {
  return storedValue !== "false";
}

export function readModernUnicodeSetting(storage: SettingStorage): boolean {
  try {
    return modernUnicodeEnabled(storage.getItem(modernUnicodeStorageKey));
  } catch {
    return true;
  }
}
