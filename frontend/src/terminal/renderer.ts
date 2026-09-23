import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { UnicodeGraphemesAddon } from "@xterm/addon-unicode-graphemes";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal } from "@xterm/xterm";

import type { TerminalPlatform } from "./paste";
import { terminalPlatform } from "./platform";
import {
  terminalRendererOptions,
  type NormalizedTerminalProfileSettings,
} from "./profile-settings";

/**
 * Hands a link to the system browser. A runtime without the bridge (a plain
 * browser preview) has nowhere to send it and stays quiet; a bridge that
 * refuses reports it, so a click that did nothing is never silent.
 */
export function openExternalURL(uri: string): Promise<void> {
  const runtime = (globalThis as { runtime?: { BrowserOpenURL?: (uri: string) => unknown } })
    .runtime;
  if (typeof runtime?.BrowserOpenURL !== "function") return Promise.resolve();
  const open = runtime.BrowserOpenURL.bind(runtime);
  return Promise.resolve().then(() => open(uri)).then(() => undefined);
}

/**
 * A link in terminal output opens only with the platform's modifier held, so
 * an ordinary click can still select text.
 */
export function terminalLinkActivation(options: {
  onError(error: unknown): void;
  open?: (uri: string) => Promise<void>;
  platform?: () => TerminalPlatform;
}): (event: MouseEvent, uri: string) => void {
  const open = options.open ?? openExternalURL;
  const platform = options.platform ?? (() => terminalPlatform());
  return (event, uri) => {
    const modified = platform() === "mac" ? event.metaKey : event.ctrlKey;
    if (!modified) return;
    event.preventDefault();
    void open(uri).catch(options.onError);
  };
}

export interface TerminalRendererParts {
  terminal: Terminal;
  fit: FitAddon;
  search: SearchAddon;
  unicode: UnicodeGraphemesAddon | null;
}

/**
 * The renderer both the dock and the terminal window draw a session with:
 * the same options, the same add-ons, the same link rule. Mounting it and
 * wiring the stream stay with each surface.
 */
export function createTerminalRenderer(options: {
  settings: NormalizedTerminalProfileSettings;
  fontSize: number;
  modernUnicode: boolean;
  onLinkError(error: unknown): void;
}): TerminalRendererParts {
  const terminal = new Terminal({
    allowProposedApi: true,
    cursorBlink: true,
    rescaleOverlappingGlyphs: true,
    ...terminalRendererOptions(options.settings, options.fontSize),
  });
  const fit = new FitAddon();
  terminal.loadAddon(fit);
  let unicode: UnicodeGraphemesAddon | null = null;
  if (options.modernUnicode) {
    unicode = new UnicodeGraphemesAddon();
    terminal.loadAddon(unicode);
  }
  const search = new SearchAddon();
  terminal.loadAddon(search);
  terminal.loadAddon(
    new WebLinksAddon(terminalLinkActivation({ onError: options.onLinkError })),
  );
  return { terminal, fit, search, unicode };
}
