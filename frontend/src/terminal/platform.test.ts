import { describe, expect, it, vi } from "vitest";

import { terminalPlatform } from "./platform";
import { terminalLinkActivation, terminalLinkHandler } from "./renderer";

describe("terminalPlatform", () => {
  it("treats every Apple platform string the same way", () => {
    // Keep iPad-reporting WebKit aligned with dock Mac shortcuts.
    for (const platform of ["MacIntel", "iPad", "iPhone"]) {
      expect(terminalPlatform(platform)).toBe("mac");
    }
    expect(terminalPlatform("Win32")).toBe("windows");
    expect(terminalPlatform("Linux x86_64")).toBe("linux");
    expect(terminalPlatform("")).toBe("linux");
  });
});

describe("terminalLinkActivation", () => {
  const click = (modifiers: { metaKey?: boolean; ctrlKey?: boolean }) => ({
    metaKey: false,
    ctrlKey: false,
    preventDefault: vi.fn(),
    ...modifiers,
  }) as unknown as MouseEvent;

  it("opens only with the platform modifier held", () => {
    const open = vi.fn(() => Promise.resolve());
    const mac = terminalLinkActivation({ onError: vi.fn(), open, platform: () => "mac" });
    mac(click({ ctrlKey: true }), "https://example.com");
    expect(open).not.toHaveBeenCalled();
    const event = click({ metaKey: true });
    mac(event, "https://example.com");
    expect(open).toHaveBeenCalledWith("https://example.com");
    expect(event.preventDefault).toHaveBeenCalled();

    const linux = terminalLinkActivation({ onError: vi.fn(), open, platform: () => "linux" });
    linux(click({ metaKey: true }), "https://example.org");
    expect(open).toHaveBeenCalledTimes(1);
    linux(click({ ctrlKey: true }), "https://example.org");
    expect(open).toHaveBeenLastCalledWith("https://example.org");
  });

  it("reports a link the system refused to open", async () => {
    const failure = new Error("no browser");
    const onError = vi.fn();
    const activate = terminalLinkActivation({
      onError,
      open: () => Promise.reject(failure),
      platform: () => "mac",
    });
    activate(click({ metaKey: true }), "https://example.com");
    await Promise.resolve();
    await Promise.resolve();
    expect(onError).toHaveBeenCalledWith(failure);
  });
});

describe("terminalLinkHandler", () => {
  const click = (modifiers: { metaKey?: boolean; ctrlKey?: boolean }) => ({
    metaKey: false,
    ctrlKey: false,
    preventDefault: vi.fn(),
    ...modifiers,
  }) as unknown as MouseEvent;

  it("routes OSC 8 activation through the modifier-gated opener", () => {
    const open = vi.fn(() => Promise.resolve());
    const handler = terminalLinkHandler({ onError: vi.fn(), open, platform: () => "mac" });
    handler.activate(click({ ctrlKey: true }), "https://example.com", {
      start: { x: 1, y: 1 },
      end: { x: 2, y: 1 },
    });
    expect(open).not.toHaveBeenCalled();
    handler.activate(click({ metaKey: true }), "https://example.com", {
      start: { x: 1, y: 1 },
      end: { x: 2, y: 1 },
    });
    expect(open).toHaveBeenCalledWith("https://example.com");
  });

  it("keeps non-http protocols away from activation", () => {
    expect(terminalLinkHandler({ onError: vi.fn() }).allowNonHttpProtocols).toBe(false);
  });
});
