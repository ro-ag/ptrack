import { describe, expect, it, vi } from "vitest";

import { installWindowChrome } from "./window-chrome";

function button(dataset, rect = { left: 0, bottom: 0 }) {
  const listeners = [];
  return {
    dataset,
    setAttribute: vi.fn(),
    addEventListener: (_type, listener) => listeners.push(listener),
    click: () => listeners.forEach((listener) => listener()),
    getBoundingClientRect: () => rect,
  };
}

function page(chrome) {
  const buttons = {
    menu: button({ appMenu: "" }, { left: 12.4, bottom: 40.2 }),
    minimize: button({ windowControl: "minimize" }),
    maximize: button({ windowControl: "toggle-maximize" }),
    close: button({ windowControl: "close" }),
  };
  const root = { dataset: chrome ? { windowChrome: chrome } : {} };
  const target = {
    document: {
      documentElement: root,
      querySelectorAll: (selector) => {
        if (selector === "[data-app-menu]") return [buttons.menu];
        if (selector === '[data-window-control="toggle-maximize"]') return [buttons.maximize];
        return [buttons.minimize, buttons.maximize, buttons.close];
      },
    },
    addEventListener: vi.fn(),
    setTimeout,
    clearTimeout,
    console: { error: vi.fn() },
  };
  return { buttons, root, target };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("frameless Windows and Linux title bars", () => {
  it("stays inert where the platform keeps its native chrome", () => {
    const { buttons, target } = page("");
    const invoke = vi.fn();
    expect(installWindowChrome(target, { invoke })).toBe(false);
    buttons.close.click();
    expect(invoke).not.toHaveBeenCalled();
    expect(target.addEventListener).not.toHaveBeenCalled();
  });

  it.each(["custom", "linux"])("sends %s controls to the shell and tracks the maximized state", async (chrome) => {
    const { buttons, root, target } = page(chrome);
    let maximized = false;
    const invoke = vi.fn(async (_command, { action }) => {
      if (action === "toggle-maximize") maximized = !maximized;
      return maximized;
    });
    expect(installWindowChrome(target, { invoke })).toBe(true);
    await settle();
    expect(invoke).toHaveBeenCalledWith("window_chrome", { action: "state" });
    expect(root.dataset.windowMaximized).toBe("false");

    buttons.maximize.click();
    await settle();
    expect(root.dataset.windowMaximized).toBe("true");
    expect(buttons.maximize.setAttribute).toHaveBeenCalledWith("aria-label", "Restore");

    buttons.minimize.click();
    buttons.close.click();
    await settle();
    expect(invoke.mock.calls.map(([, payload]) => payload.action)).toEqual([
      "state",
      "toggle-maximize",
      "minimize",
      "close",
    ]);
    // A double-click or snap maximizes without a click, so resizes re-read it.
    expect(target.addEventListener).toHaveBeenCalledWith("resize", expect.any(Function));
  });

  it("opens the app menu just under its button", async () => {
    const { buttons, target } = page("custom");
    const invoke = vi.fn(async () => false);
    installWindowChrome(target, { invoke });
    buttons.menu.click();
    await settle();
    expect(invoke).toHaveBeenLastCalledWith("window_chrome", { action: "menu", x: 12, y: 44 });
  });

  it("opens the selected Linux submenu at the clicked label", async () => {
    const { buttons, target } = page("linux");
    buttons.menu.dataset.appMenu = "File";
    const invoke = vi.fn(async () => false);
    installWindowChrome(target, { invoke });
    buttons.menu.click();
    await settle();
    expect(invoke).toHaveBeenLastCalledWith("window_chrome", {
      action: "menu", menuLabel: "File", x: 12, y: 44,
    });
  });

  it("keeps the page running when a control is refused", async () => {
    const { buttons, target } = page("custom");
    const invoke = vi.fn(async () => {
      throw new Error("custom window chrome requires Windows or Linux");
    });
    installWindowChrome(target, { invoke });
    buttons.close.click();
    await settle();
    expect(target.console.error).toHaveBeenCalledWith(expect.stringContaining("requires Windows or Linux"));
  });
});
