import { describe, expect, it } from "vitest";

import {
  applyPreferenceMirrors,
  defaultPreferences,
  normalizePreferences,
  preferenceMirrors,
  preferenceSaveMessage,
  preferencesFromMirrors,
  preferencesResponse,
  readTerminalPreferenceOverrides,
  reducedMotionActive,
  storageStatusNotice,
  terminalDefaultProfileMirrorKey,
  terminalFontFamilyMirrorKey,
  terminalFontSizeMirrorKey,
  terminalRendererMirrorKey,
  terminalScrollbackMirrorKey,
  terminalUnicodeMirrorKey,
  themeMirrorKey,
  webglPreferredByPreference,
} from "./preferences";

function stubStorage(initial: Record<string, string> = {}) {
  const entries = new Map(Object.entries(initial));
  return {
    entries,
    getItem: (key: string) => entries.get(key) ?? null,
    setItem: (key: string, value: string) => entries.set(key, value),
    removeItem: (key: string) => entries.delete(key),
  };
}

describe("preferences normalization", () => {
  it("reads an empty document as the documented defaults", () => {
    expect(normalizePreferences(undefined)).toEqual(defaultPreferences);
    expect(normalizePreferences("nonsense")).toEqual(defaultPreferences);
  });

  it("falls back on unknown enum values and wrong types", () => {
    const preferences = normalizePreferences({
      appearance: { theme: "sepia", density: 4, reducedMotion: null },
      terminal: { unicodeMode: "ancient", renderer: "metal", fontFamily: "   " },
    });

    expect(preferences.appearance).toEqual(defaultPreferences.appearance);
    expect(preferences.terminal.unicodeMode).toBe("modern");
    expect(preferences.terminal.renderer).toBe("auto");
    expect(preferences.terminal.fontFamily).toBe("monospace");
  });

  it("keeps the startup opt-in off until it is stored as true", () => {
    expect(defaultPreferences.startup).toEqual({
      restoreLastProject: false,
      lastProjectRoot: null,
    });
    expect(
      normalizePreferences({ startup: { restoreLastProject: "yes", lastProjectRoot: " " } })
        .startup,
    ).toEqual({ restoreLastProject: false, lastProjectRoot: null });
    expect(
      normalizePreferences({
        startup: { restoreLastProject: true, lastProjectRoot: "/work/app" },
      }).startup,
    ).toEqual({ restoreLastProject: true, lastProjectRoot: "/work/app" });
  });

  it("keeps every OS notification category independently opt-in", () => {
    expect(defaultPreferences.notifications).toEqual({
      handoffArrival: false,
      runFailureOrDrift: false,
      runCompletion: false,
    });
    expect(normalizePreferences({
      notifications: {
        handoffArrival: true,
        runFailureOrDrift: "yes",
        runCompletion: true,
      },
    }).notifications).toEqual({
      handoffArrival: true,
      runFailureOrDrift: false,
      runCompletion: true,
    });
  });

  it("clamps the documented ranges", () => {
    expect(normalizePreferences({ terminal: { fontSize: 2 } }).terminal.fontSize).toBe(10);
    expect(normalizePreferences({ terminal: { fontSize: 99 } }).terminal.fontSize).toBe(24);
    expect(normalizePreferences({ terminal: { scrollback: 4 } }).terminal.scrollback)
      .toBe(1_000);
    expect(
      normalizePreferences({ terminal: { scrollback: 900_000 } }).terminal.scrollback,
    ).toBe(200_000);
  });

  it("keeps a stored default profile id and reports a blank one as unset", () => {
    expect(
      normalizePreferences({ terminal: { defaultProfileId: "zsh" } })
        .terminal.defaultProfileId,
    ).toBe("zsh");
    expect(
      normalizePreferences({ terminal: { defaultProfileId: "  " } })
        .terminal.defaultProfileId,
    ).toBeNull();
  });

  it("splits the response document from its storage status", () => {
    const response = preferencesResponse({
      preferences: { appearance: { theme: "dark" } },
      storage: "unreadable",
    });

    expect(response.preferences.appearance.theme).toBe("dark");
    expect(response.storage).toBe("unreadable");
    expect(storageStatusNotice("unreadable")).toContain("could not be read");
    expect(storageStatusNotice("ok")).toBe("");
  });

  it("reads a reply without a storage status as unreadable", () => {
    const response = preferencesResponse({ appearance: { density: "compact" } });

    expect(response.preferences.appearance.density).toBe("compact");
    expect(response.storage).toBe("unreadable");
    expect(preferencesResponse(undefined).storage).toBe("unreadable");
  });

  it("states plainly that the runtime did not answer", () => {
    expect(storageStatusNotice("unavailable")).toContain("did not answer");
    expect(storageStatusNotice("defaults")).toBe("");
  });
});

describe("preference mirrors", () => {
  it("removes the theme key so the pre-paint guard follows the OS", () => {
    const storage = stubStorage({ [themeMirrorKey]: "light" });
    applyPreferenceMirrors(storage, defaultPreferences);

    expect(storage.getItem(themeMirrorKey)).toBeNull();
  });

  it("writes every cached terminal key from the stored record", () => {
    const storage = stubStorage();
    applyPreferenceMirrors(storage, {
      ...defaultPreferences,
      appearance: { ...defaultPreferences.appearance, theme: "dark" },
      terminal: {
        defaultProfileId: "agent-codex",
        fontFamily: "Fira Code",
        fontSize: 18,
        unicodeMode: "legacy",
        scrollback: 4_000,
        renderer: "dom",
      },
    });

    expect(storage.getItem(themeMirrorKey)).toBe("dark");
    expect(storage.getItem(terminalFontSizeMirrorKey)).toBe("18");
    expect(storage.getItem(terminalUnicodeMirrorKey)).toBe("false");
    expect(storage.getItem(terminalFontFamilyMirrorKey)).toBe("Fira Code");
    expect(storage.getItem(terminalScrollbackMirrorKey)).toBe("4000");
    expect(storage.getItem(terminalRendererMirrorKey)).toBe("dom");
    expect(storage.getItem(terminalDefaultProfileMirrorKey)).toBe("agent-codex");
  });

  it("mirrors modern Unicode as the true the terminal pane already reads", () => {
    expect(preferenceMirrors(defaultPreferences)).toContainEqual([
      terminalUnicodeMirrorKey,
      "true",
    ]);
  });

  it("rebuilds a record from the cache when the stored record cannot be read", () => {
    const storage = stubStorage({
      [themeMirrorKey]: "light",
      [terminalFontSizeMirrorKey]: "20",
      [terminalUnicodeMirrorKey]: "false",
      [terminalRendererMirrorKey]: "dom",
    });
    const preferences = preferencesFromMirrors(storage);

    expect(preferences.appearance.theme).toBe("light");
    expect(preferences.terminal.fontSize).toBe(20);
    expect(preferences.terminal.unicodeMode).toBe("legacy");
    expect(preferences.terminal.renderer).toBe("dom");
    expect(preferencesFromMirrors(stubStorage())).toEqual(defaultPreferences);
  });

  it("survives storage that throws", () => {
    const storage = {
      getItem: () => null,
      setItem: () => {
        throw new Error("denied");
      },
    };

    expect(() => applyPreferenceMirrors(storage, defaultPreferences)).not.toThrow();
  });
});

describe("terminal preference overrides", () => {
  it("reports unset overrides for a store with no record", () => {
    expect(readTerminalPreferenceOverrides(stubStorage())).toEqual({
      defaultProfileId: "",
      fontFamily: "",
      scrollback: 0,
      renderer: "auto",
    });
  });

  it("keeps the profile font family when the preference is the default stack", () => {
    const storage = stubStorage({ [terminalFontFamilyMirrorKey]: "monospace" });

    expect(readTerminalPreferenceOverrides(storage).fontFamily).toBe("");
  });

  it("clamps a mirrored scrollback and rejects unknown renderers", () => {
    const storage = stubStorage({
      [terminalScrollbackMirrorKey]: "900000",
      [terminalRendererMirrorKey]: "metal",
      [terminalDefaultProfileMirrorKey]: "zsh",
    });
    const overrides = readTerminalPreferenceOverrides(storage);

    expect(overrides.scrollback).toBe(200_000);
    expect(overrides.renderer).toBe("auto");
    expect(overrides.defaultProfileId).toBe("zsh");
  });

  it("allows the accelerated renderer only for auto and webgl", () => {
    expect(webglPreferredByPreference("auto")).toBe(true);
    expect(webglPreferredByPreference("webgl")).toBe(true);
    expect(webglPreferredByPreference("canvas")).toBe(false);
    expect(webglPreferredByPreference("dom")).toBe(false);
  });
});

describe("appearance behavior", () => {
  it("resolves reduced motion against the media query only for system", () => {
    expect(reducedMotionActive("system", true)).toBe(true);
    expect(reducedMotionActive("system", false)).toBe(false);
    expect(reducedMotionActive("always", false)).toBe(true);
    expect(reducedMotionActive("never", true)).toBe(false);
  });

  it("states plainly that a failed save left the record unchanged", () => {
    expect(preferenceSaveMessage("saving")).toBe("Saving settings…");
    expect(preferenceSaveMessage("saved")).toBe("Settings saved.");
    expect(preferenceSaveMessage("reset")).toBe("Settings reset to defaults.");
    expect(preferenceSaveMessage("failed")).toContain("stored record is unchanged");
  });
});
