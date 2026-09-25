const maximumInputFrameBytes = 64 * 1024;
const defaultPreviewCharacters = 4_096;
const utf8Encoder = new TextEncoder();

export type TerminalPlatform = "mac" | "windows" | "linux";
export type TerminalShortcutAction =
  | "copy"
  | "paste"
  | "select-all"
  | "context-menu"
  | "search"
  | "zoom-in"
  | "zoom-out"
  | "zoom-reset"
  | "clear"
  | "ignore";

interface ShortcutEvent {
  key: string;
  code?: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

interface CompositionEvent {
  key: string;
  keyCode?: number;
  isComposing?: boolean;
}

export interface ClipboardPasteRequest {
  text: string;
  lineCount: number;
  preview: string;
  previewTruncated: boolean;
  /** Control characters and bracketed-paste markers removed from the text. */
  controlCharactersRemoved: number;
  requiresConfirmation: boolean;
}

/**
 * Paste context. Alternate-screen mode is trusted only with authenticated
 * shell integration that confirms a command is running.
 */
export interface PasteTarget {
  alternateScreen: boolean;
  shell?: { quality: string; phase: string } | null;
}

// Strip bracketed-paste markers to prevent pasted text ending the wrapper early.
const bracketedPasteMarkers = /\x1b\[20[01]~/g;
// Strip controls other than tab and newline; other controls are keystrokes.
const pasteControlCharacters = /[\x00-\x08\x0b-\x1f\x7f-\x9f]/g;

/** Whether a multi-line paste may skip review because a program owns the input. */
export function multiLinePasteReviewBypassed(target: PasteTarget): boolean {
  return target.alternateScreen &&
    target.shell?.quality === "rich" &&
    target.shell.phase === "executing";
}

/** One line of review detail shared by the dock's dialog and the terminal window. */
export function pasteReviewSummary(request: ClipboardPasteRequest): string {
  const parts = [`${request.lineCount} ${request.lineCount === 1 ? "line" : "lines"}`];
  if (request.previewTruncated) parts.push("preview truncated");
  if (request.controlCharactersRemoved > 0) {
    parts.push(
      `${request.controlCharactersRemoved} control ${
        request.controlCharactersRemoved === 1 ? "character" : "characters"
      } removed`,
    );
  }
  return parts.join(" · ");
}

export function binaryStringToBytes(input: string): Uint8Array<ArrayBuffer> {
  const bytes = new Uint8Array(input.length);
  for (let index = 0; index < input.length; index += 1) {
    bytes[index] = input.charCodeAt(index) & 0xff;
  }
  return bytes;
}

export function terminalTextToBytes(input: string): Uint8Array<ArrayBuffer> {
  return utf8Encoder.encode(input);
}

export function splitTerminalInput(input: Uint8Array<ArrayBuffer>): Uint8Array<ArrayBuffer>[] {
  const chunks: Uint8Array<ArrayBuffer>[] = [];
  for (let offset = 0; offset < input.byteLength; offset += maximumInputFrameBytes) {
    chunks.push(input.subarray(offset, offset + maximumInputFrameBytes));
  }
  return chunks;
}

export function prepareClipboardPaste(
  input: string,
  target: PasteTarget,
  maximumPreviewCharacters = defaultPreviewCharacters,
): ClipboardPasteRequest {
  const normalized = input.replace(/\r\n?/g, "\n");
  const unbracketed = normalized.replace(bracketedPasteMarkers, "");
  const text = unbracketed.replace(pasteControlCharacters, "");
  const controlCharactersRemoved = normalized.length - text.length;
  const previewCharacters: string[] = [];
  let previewTruncated = false;
  for (const character of text) {
    if (previewCharacters.length >= maximumPreviewCharacters) {
      previewTruncated = true;
      break;
    }
    previewCharacters.push(character);
  }
  const preview = previewTruncated ? `${previewCharacters.join("")}…` : text;
  let lineCount = text === "" ? 0 : 1;
  for (let index = 0; index < text.length; index += 1) {
    if (text.charCodeAt(index) === 10) lineCount += 1;
  }
  return {
    text,
    lineCount,
    preview,
    previewTruncated,
    controlCharactersRemoved,
    // Review text after stripping controls because input changed.
    requiresConfirmation: controlCharactersRemoved > 0 ||
      (!multiLinePasteReviewBypassed(target) && text.includes("\n")),
  };
}

export async function commitClipboardPaste(
  request: ClipboardPasteRequest,
  confirm: (request: ClipboardPasteRequest) => Promise<boolean>,
  paste: (text: string) => void,
): Promise<boolean> {
  if (request.text === "") return false;
  if (request.requiresConfirmation && !(await confirm(request))) return false;
  paste(request.text);
  return true;
}

export function isTerminalCompositionEvent(event: CompositionEvent): boolean {
  return event.isComposing === true || event.keyCode === 229 || event.key === "Process";
}

/**
 * The shortcut a key event means in a terminal, or null when xterm should
 * take it: input-method composition always belongs to the terminal.
 */
export function terminalKeyShortcut(
  event: ShortcutEvent & CompositionEvent,
  platform: TerminalPlatform,
  hasSelection: boolean,
): TerminalShortcutAction | null {
  if (isTerminalCompositionEvent(event)) return null;
  return terminalShortcutAction(event, platform, hasSelection);
}

export function terminalShortcutAction(
  event: ShortcutEvent,
  platform: TerminalPlatform,
  hasSelection: boolean,
): TerminalShortcutAction | null {
  if (event.altKey) return null;
  const key = event.key.toLowerCase();
  const nativeMac =
    platform === "mac" &&
    event.metaKey &&
    !event.ctrlKey &&
    !event.shiftKey;
  const commonTerminal =
    platform !== "mac" &&
    event.ctrlKey &&
    event.shiftKey &&
    !event.metaKey;
  const zoomModifier =
    platform === "mac"
      ? event.metaKey && !event.ctrlKey
      : event.ctrlKey && !event.metaKey;

  if (
    key === "f" &&
    ((platform === "mac" && zoomModifier && !event.shiftKey) ||
      (platform !== "mac" && zoomModifier && event.shiftKey))
  ) {
    return "search";
  }
  if (zoomModifier) {
    if (
      event.key === "+" || event.key === "=" ||
      event.code === "Equal" || event.code === "NumpadAdd"
    ) return "zoom-in";
    if (
      !event.shiftKey &&
      (event.key === "-" || event.code === "Minus" || event.code === "NumpadSubtract")
    ) return "zoom-out";
    if (
      !event.shiftKey &&
      (event.key === "0" || event.code === "Digit0" || event.code === "Numpad0")
    ) return "zoom-reset";
  }
  if (
    platform === "mac" &&
    key === "k" &&
    event.metaKey &&
    !event.ctrlKey &&
    !event.shiftKey
  ) {
    return "clear";
  }

  if (
    (event.key === "ContextMenu" &&
      !event.metaKey &&
      !event.ctrlKey &&
      !event.shiftKey) ||
    (event.key === "F10" &&
      event.shiftKey &&
      !event.metaKey &&
      !event.ctrlKey)
  ) {
    return "context-menu";
  }

  if (key === "c") {
    if (
      hasSelection &&
      (nativeMac ||
        (event.ctrlKey && !event.metaKey && !event.shiftKey) ||
        commonTerminal)
    ) {
      return "copy";
    }
    if (nativeMac || commonTerminal) return "ignore";
    return null;
  }
  if (
    event.key === "Insert" &&
    event.ctrlKey &&
    !event.metaKey &&
    !event.shiftKey
  ) {
    return hasSelection ? "copy" : "ignore";
  }
  if (
    key === "v" &&
    ((platform === "mac" &&
      event.metaKey &&
      !event.ctrlKey &&
      !event.shiftKey) ||
      (platform !== "mac" && event.ctrlKey && !event.metaKey))
  ) {
    return "paste";
  }
  if (event.key === "Insert" && event.shiftKey && !event.ctrlKey && !event.metaKey) {
    return "paste";
  }
  if (
    key === "a" &&
    ((platform === "mac" &&
      event.metaKey &&
      !event.ctrlKey &&
      !event.shiftKey) ||
      (platform !== "mac" &&
        event.ctrlKey &&
        event.shiftKey &&
        !event.metaKey))
  ) {
    return "select-all";
  }
  return null;
}
