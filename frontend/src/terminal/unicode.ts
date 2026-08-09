export const modernUnicodeStorageKey = "ptrack-terminal-modern-unicode";

interface SettingStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
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

export function writeModernUnicodeSetting(
  storage: SettingStorage,
  enabled: boolean,
): void {
  try {
    storage.setItem(modernUnicodeStorageKey, String(enabled));
  } catch {
    // Keep the live setting usable when WebView storage is unavailable.
  }
}
