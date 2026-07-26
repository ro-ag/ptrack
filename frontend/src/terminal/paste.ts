const maximumInputFrameBytes = 64 * 1024;
const defaultPreviewCharacters = 4_096;

export type TerminalPlatform = "mac" | "windows" | "linux";
export type TerminalShortcutAction =
  | "copy"
  | "paste"
  | "select-all"
  | "context-menu"
  | "ignore";

interface ShortcutEvent {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

export interface ClipboardPasteRequest {
  text: string;
  lineCount: number;
  preview: string;
  previewTruncated: boolean;
  requiresConfirmation: boolean;
}

export function binaryStringToBytes(input: string): Uint8Array {
  const bytes = new Uint8Array(input.length);
  for (let index = 0; index < input.length; index += 1) {
    bytes[index] = input.charCodeAt(index) & 0xff;
  }
  return bytes;
}

export function splitTerminalInput(input: Uint8Array): Uint8Array[] {
  const chunks: Uint8Array[] = [];
  for (let offset = 0; offset < input.byteLength; offset += maximumInputFrameBytes) {
    chunks.push(input.subarray(offset, offset + maximumInputFrameBytes));
  }
  return chunks;
}

export function prepareClipboardPaste(
  input: string,
  alternateScreen: boolean,
  maximumPreviewCharacters = defaultPreviewCharacters,
): ClipboardPasteRequest {
  const text = input.replace(/\r\n?/g, "\n");
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
    requiresConfirmation: !alternateScreen && text.includes("\n"),
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
