import { describe, expect, it } from "vitest";

import { initTheme, nextTheme, resolveTheme, terminalThemeName, THEME_STORAGE_KEY } from "./theme";
import { terminalProfileTheme } from "./terminal/profile-settings";

function stubStorage(initial = null) {
  const entries = new Map(initial === null ? [] : [[THEME_STORAGE_KEY, initial]]);
  return {
    getItem: (key) => (entries.has(key) ? entries.get(key) : null),
    setItem: (key, value) => entries.set(key, value),
    removeItem: (key) => entries.delete(key),
  };
}

function stubMedia(prefersLight) {
  const listeners = [];
  return {
    matches: prefersLight,
    addEventListener: (_type, listener) => listeners.push(listener),
    emit() {
      for (const listener of listeners) listener();
    },
  };
}

describe("theme resolution", () => {
  it("prefers an explicitly stored theme over the OS", () => {
    expect(resolveTheme("light", false)).toBe("light");
    expect(resolveTheme("dark", true)).toBe("dark");
  });

  it("follows the OS preference when nothing is stored", () => {
    expect(resolveTheme(null, true)).toBe("light");
    expect(resolveTheme(null, false)).toBe("dark");
  });

  it("ignores unrecognized stored values", () => {
    expect(resolveTheme("sepia", true)).toBe("light");
  });

  it("alternates between themes", () => {
    expect(nextTheme("light")).toBe("dark");
    expect(nextTheme("dark")).toBe("light");
  });
});

describe("theme controller", () => {
  it("applies the OS theme on startup and tracks OS changes", () => {
    const root = { dataset: {} };
    const media = stubMedia(false);
    initTheme({ root, storage: stubStorage(), media });
    expect(root.dataset.theme).toBe("dark");

    media.matches = true;
    media.emit();
    expect(root.dataset.theme).toBe("light");
  });

  it("persists the toggled theme and stops following the OS", () => {
    const root = { dataset: {} };
    const storage = stubStorage();
    const media = stubMedia(false);
    const controller = initTheme({ root, storage, media });

    expect(controller.toggle()).toBe("light");
    expect(root.dataset.theme).toBe("light");
    expect(storage.getItem(THEME_STORAGE_KEY)).toBe("light");

    media.emit();
    expect(root.dataset.theme).toBe("light");
  });

  it("clears the explicit choice when the stored preference is system", () => {
    const root = { dataset: {} };
    const storage = stubStorage("light");
    const media = stubMedia(false);
    const controller = initTheme({ root, storage, media });

    expect(controller.setTheme("system")).toBe("dark");
    expect(storage.getItem(THEME_STORAGE_KEY)).toBeNull();

    media.matches = true;
    media.emit();
    expect(root.dataset.theme).toBe("light");
  });

  it("honors a stored theme on startup", () => {
    const root = { dataset: {} };
    const media = stubMedia(true);
    initTheme({ root, storage: stubStorage("dark"), media });
    expect(root.dataset.theme).toBe("dark");
  });

  it("reports applied themes through onChange", () => {
    const applied = [];
    const media = stubMedia(false);
    const controller = initTheme({
      root: { dataset: {} },
      storage: stubStorage(),
      media,
      onChange: (theme) => applied.push(theme),
    });
    controller.toggle();
    expect(applied).toEqual(["dark", "light"]);
  });
});

describe("terminal palette follows the app theme", () => {
  // CIE L* of an sRGB hex colour.
  function lightness(hex) {
    const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
      .map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
    const y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    return y > 216 / 24389 ? 116 * Math.cbrt(y) - 16 : (24389 / 27) * y;
  }

  it("draws the default profile light in the light theme and dark in the dark theme", () => {
    const light = terminalProfileTheme(terminalThemeName("default", "light"));
    const dark = terminalProfileTheme(terminalThemeName("default", "dark"));
    expect(lightness(light.background)).toBeGreaterThanOrEqual(90);
    expect(lightness(dark.background)).toBeLessThan(20);
    expect(terminalThemeName("", "light")).toBe("platinum");
  });

  it("keeps a palette the profile chose for itself", () => {
    expect(terminalThemeName("high-contrast", "light")).toBe("high-contrast");
    expect(terminalThemeName("platinum", "dark")).toBe("platinum");
  });
});
