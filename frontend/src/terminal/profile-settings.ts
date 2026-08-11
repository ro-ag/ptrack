import type { ITerminalOptions, ITheme } from "@xterm/xterm";

export type TerminalCWDPolicy = "requested" | "project" | "fixed";
export type TerminalExitBehavior = "keep" | "close-on-success" | "close";

export interface TerminalProfileSettings {
  theme?: string;
  fontFamily?: string;
  fontSize?: number;
  scrollback?: number;
  cwdPolicy?: TerminalCWDPolicy;
  exitBehavior?: TerminalExitBehavior;
}

export interface NormalizedTerminalProfileSettings {
  theme: string;
  fontFamily: string;
  fontSize: number;
  scrollback: number;
  cwdPolicy: TerminalCWDPolicy;
  exitBehavior: TerminalExitBehavior;
}

export const defaultTerminalProfileSettings: NormalizedTerminalProfileSettings = {
  theme: "default",
  fontFamily: "monospace",
  fontSize: 14,
  scrollback: 25_000,
  cwdPolicy: "requested",
  exitBehavior: "keep",
};

const defaultFontStack =
  '"SFMono-Regular", "SF Mono", Menlo, Monaco, Consolas, "Liberation Mono", "DejaVu Sans Mono", "Apple Color Emoji", "Segoe UI Emoji", monospace';

const themes: Record<string, ITheme> = {
  default: {
    background: "#090d12",
    foreground: "#e6e9f0",
    cursor: "#3dd6a3",
    selectionBackground: "#31594f",
  },
  platinum: {
    background: "#f6f7f9",
    foreground: "#202733",
    cursor: "#087f68",
    selectionBackground: "#b9e5d9",
  },
  "high-contrast": {
    background: "#000000",
    foreground: "#ffffff",
    cursor: "#ffff00",
    selectionBackground: "#185fff",
  },
};

function boundedInteger(value: number | undefined, fallback: number, min: number, max: number) {
  if (!Number.isFinite(value)) return fallback;
  return Math.max(min, Math.min(max, Math.round(value as number)));
}

export function normalizeTerminalProfileSettings(
  input: TerminalProfileSettings,
): NormalizedTerminalProfileSettings {
  const cwdPolicy = input.cwdPolicy === "project" || input.cwdPolicy === "fixed"
    ? input.cwdPolicy
    : "requested";
  const exitBehavior = input.exitBehavior === "close" ||
      input.exitBehavior === "close-on-success"
    ? input.exitBehavior
    : "keep";
  return {
    theme: typeof input.theme === "string" && input.theme.length > 0
      ? input.theme
      : defaultTerminalProfileSettings.theme,
    fontFamily: typeof input.fontFamily === "string" && input.fontFamily.trim().length > 0
      ? input.fontFamily
      : defaultTerminalProfileSettings.fontFamily,
    fontSize: boundedInteger(
      input.fontSize,
      defaultTerminalProfileSettings.fontSize,
      10,
      24,
    ),
    scrollback: boundedInteger(
      input.scrollback,
      defaultTerminalProfileSettings.scrollback,
      100,
      100_000,
    ),
    cwdPolicy,
    exitBehavior,
  };
}

export function terminalProfileTheme(name: string): ITheme {
  return { ...(themes[name] ?? themes.default) };
}

export function terminalProfileFontFamily(fontFamily: string): string {
  return fontFamily === "monospace" ? defaultFontStack : fontFamily;
}

export function terminalRendererOptions(
  settings: NormalizedTerminalProfileSettings,
  fontSize: number,
): Pick<
  ITerminalOptions,
  | "fontFamily"
  | "fontSize"
  | "minimumContrastRatio"
  | "screenReaderMode"
  | "scrollback"
  | "theme"
> {
  return {
    fontFamily: terminalProfileFontFamily(settings.fontFamily),
    fontSize,
    minimumContrastRatio: 4.5,
    screenReaderMode: true,
    scrollback: settings.scrollback,
    theme: terminalProfileTheme(settings.theme),
  };
}

export function terminalProfileClosesAfterExit(
  behavior: TerminalExitBehavior,
  exitCode: number,
): boolean {
  return behavior === "close" || (behavior === "close-on-success" && exitCode === 0);
}
