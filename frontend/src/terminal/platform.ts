import type { TerminalPlatform } from "./paste";

/**
 * The one platform test every terminal surface uses. The dock and the
 * terminal window each carried their own copy and had already drifted: one
 * treated an iPad-reporting WebKit as a Mac and the other did not, so the
 * same shortcut or link click behaved differently per window.
 */
export function terminalPlatform(
  platform: string = globalThis.navigator?.platform ?? "",
): TerminalPlatform {
  if (/Mac|iPhone|iPad/.test(platform)) return "mac";
  if (/Win/.test(platform)) return "windows";
  return "linux";
}
