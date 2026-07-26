import { describe, expect, it, vi } from "vitest";

import {
  commitClipboardPaste,
  prepareClipboardPaste,
  terminalShortcutAction,
} from "./paste";

describe("prepareClipboardPaste", () => {
  it.each([
    ["", "", 0, false],
    ["one line", "one line", 1, false],
    ["one\r\ntwo", "one\ntwo", 2, true],
    ["one\rtwo", "one\ntwo", 2, true],
    ["one\ntwo", "one\ntwo", 2, true],
    ["one\n", "one\n", 2, true],
  ])(
    "normalizes and classifies %j",
    (input, text, lineCount, requiresConfirmation) => {
      expect(prepareClipboardPaste(input, false)).toMatchObject({
        text,
        lineCount,
        requiresConfirmation,
      });
    },
  );

  it("bypasses multiline confirmation in the alternate screen", () => {
    expect(prepareClipboardPaste("one\ntwo", true).requiresConfirmation).toBe(false);
  });

  it("bounds a large Unicode preview without breaking the full paste", () => {
    const input = `${"😀".repeat(20)}\nsecond line`;
    const request = prepareClipboardPaste(input, false, 12);

    expect(Array.from(request.preview)).toHaveLength(13);
    expect(request.preview.endsWith("…")).toBe(true);
    expect(request.previewTruncated).toBe(true);
    expect(request.text).toBe(input);
  });

  it("preserves whitespace and keeps markup literal in the preview", () => {
    const input = "  <script>alert('&')</script>  ";
    const request = prepareClipboardPaste(input, false);

    expect(request.text).toBe(input);
    expect(request.preview).toBe(input);
    expect(request.lineCount).toBe(1);
  });
});

describe("commitClipboardPaste", () => {
  it("does nothing for blank clipboard input", async () => {
    const confirm = vi.fn();
    const paste = vi.fn();

    await expect(
      commitClipboardPaste(prepareClipboardPaste("", false), confirm, paste),
    ).resolves.toBe(false);
    expect(confirm).not.toHaveBeenCalled();
    expect(paste).not.toHaveBeenCalled();
  });

  it("pastes one line without confirmation", async () => {
    const confirm = vi.fn();
    const paste = vi.fn();

    await expect(
      commitClipboardPaste(prepareClipboardPaste("echo safe", false), confirm, paste),
    ).resolves.toBe(true);
    expect(confirm).not.toHaveBeenCalled();
    expect(paste).toHaveBeenCalledWith("echo safe");
  });

  it("cancels or confirms multiline input without executing lines itself", async () => {
    const request = prepareClipboardPaste("echo one\r\necho two", false);
    const cancelPaste = vi.fn();
    const confirmPaste = vi.fn();

    await expect(
      commitClipboardPaste(request, async () => false, cancelPaste),
    ).resolves.toBe(false);
    expect(cancelPaste).not.toHaveBeenCalled();

    await expect(
      commitClipboardPaste(request, async () => true, confirmPaste),
    ).resolves.toBe(true);
    expect(confirmPaste).toHaveBeenCalledWith("echo one\necho two");
  });

  it("delegates bracketed-paste behavior to xterm's public paste API", async () => {
    const terminal = {
      modes: { bracketedPasteMode: true },
      paste: vi.fn(),
    };

    await commitClipboardPaste(
      prepareClipboardPaste("one\ntwo", true),
      async () => {
        throw new Error("alternate screen should bypass confirmation");
      },
      (text) => terminal.paste(text),
    );

    expect(terminal.modes.bracketedPasteMode).toBe(true);
    expect(terminal.paste).toHaveBeenCalledWith("one\ntwo");
  });
});

describe("terminalShortcutAction", () => {
  const key = (
    value: string,
    modifiers: Partial<{
      metaKey: boolean;
      ctrlKey: boolean;
      shiftKey: boolean;
      altKey: boolean;
    }> = {},
  ) => ({
    key: value,
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    ...modifiers,
  });

  it("maps native and terminal copy shortcuts without stealing SIGINT", () => {
    expect(terminalShortcutAction(key("c", { ctrlKey: true }), "linux", true)).toBe(
      "copy",
    );
    expect(terminalShortcutAction(key("c", { ctrlKey: true }), "linux", false)).toBe(
      null,
    );
    expect(
      terminalShortcutAction(key("c", { ctrlKey: true, shiftKey: true }), "linux", true),
    ).toBe("copy");
    expect(terminalShortcutAction(key("c", { metaKey: true }), "mac", true)).toBe(
      "copy",
    );
    expect(terminalShortcutAction(key("c", { ctrlKey: true }), "mac", true)).toBe(
      "copy",
    );
    expect(terminalShortcutAction(key("c", { metaKey: true }), "mac", false)).toBe(
      "ignore",
    );
  });

  it("maps paste and select-all shortcuts without stealing readline Ctrl+A", () => {
    expect(terminalShortcutAction(key("v", { metaKey: true }), "mac", false)).toBe(
      "paste",
    );
    expect(terminalShortcutAction(key("v", { ctrlKey: true }), "mac", false)).toBe(
      null,
    );
    expect(terminalShortcutAction(key("v", { ctrlKey: true }), "linux", false)).toBe(
      "paste",
    );
    expect(
      terminalShortcutAction(key("Insert", { shiftKey: true }), "windows", false),
    ).toBe("paste");
    expect(terminalShortcutAction(key("a", { metaKey: true }), "mac", false)).toBe(
      "select-all",
    );
    expect(
      terminalShortcutAction(key("a", { ctrlKey: true, shiftKey: true }), "linux", false),
    ).toBe("select-all");
    expect(terminalShortcutAction(key("a", { ctrlKey: true }), "linux", false)).toBe(
      null,
    );
  });

  it("suppresses explicit copy shortcuts without a selection and opens the menu by keyboard", () => {
    expect(
      terminalShortcutAction(
        key("c", { ctrlKey: true, shiftKey: true }),
        "windows",
        false,
      ),
    ).toBe("ignore");
    expect(
      terminalShortcutAction(key("Insert", { ctrlKey: true }), "linux", false),
    ).toBe("ignore");
    expect(terminalShortcutAction(key("ContextMenu"), "mac", false)).toBe(
      "context-menu",
    );
    expect(
      terminalShortcutAction(key("F10", { shiftKey: true }), "windows", false),
    ).toBe("context-menu");
  });

  it("leaves control commands, navigation, and function keys to xterm", () => {
    expect(terminalShortcutAction(key("z", { ctrlKey: true }), "linux", false)).toBe(
      null,
    );
    expect(terminalShortcutAction(key("ArrowUp"), "mac", false)).toBe(null);
    expect(terminalShortcutAction(key("F5"), "windows", false)).toBe(null);
    expect(
      terminalShortcutAction(
        key("v", { ctrlKey: true, altKey: true }),
        "windows",
        false,
      ),
    ).toBe(null);
  });
});
