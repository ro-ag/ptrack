export const defaultTerminalFontSize = 14;
export const minimumTerminalFontSize = 10;
export const maximumTerminalFontSize = 24;
export const terminalFontSizeStorageKey = "ptrack-terminal-font-size";

interface SettingStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export function clampTerminalFontSize(fontSize: number): number {
  const finiteSize = Number.isFinite(fontSize) ? fontSize : defaultTerminalFontSize;
  return Math.round(
    Math.max(minimumTerminalFontSize, Math.min(finiteSize, maximumTerminalFontSize)),
  );
}

export function storedTerminalFontSize(value: string | null): number {
  if (value === null || value.trim() === "") return defaultTerminalFontSize;
  return clampTerminalFontSize(Number(value));
}

export function readTerminalFontSize(storage: SettingStorage): number {
  try {
    return storedTerminalFontSize(storage.getItem(terminalFontSizeStorageKey));
  } catch {
    return defaultTerminalFontSize;
  }
}

export function writeTerminalFontSize(
  storage: SettingStorage,
  fontSize: number,
): void {
  try {
    storage.setItem(
      terminalFontSizeStorageKey,
      String(clampTerminalFontSize(fontSize)),
    );
  } catch {
    // Keep the live setting usable when WebView storage is unavailable.
  }
}

export function terminalZoomLabel(fontSize: number): string {
  return `${Math.round((clampTerminalFontSize(fontSize) / defaultTerminalFontSize) * 100)}%`;
}
