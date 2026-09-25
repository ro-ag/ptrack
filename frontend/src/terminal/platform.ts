import type { TerminalPlatform } from "./paste";

/**
 * Shared platform detection for dock and detached terminal windows.
 */
export function terminalPlatform(
  platform: string = globalThis.navigator?.platform ?? "",
): TerminalPlatform {
  if (/Mac|iPhone|iPad/.test(platform)) return "mac";
  if (/Win/.test(platform)) return "windows";
  return "linux";
}
