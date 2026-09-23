import { describe, expect, it, vi } from "vitest";

import { terminalPlatform } from "./platform";
import { terminalLinkActivation } from "./renderer";

describe("terminalPlatform", () => {
  it("treats every Apple platform string the same way", () => {
    // The terminal window used to test /Mac/ alone, so an iPad-reporting
    // WebKit got Linux shortcuts there and Mac shortcuts in the dock.
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
