import { afterEach, describe, expect, it, vi } from "vitest";

import { board, bootApp, holdTimers, snapshot } from "../test-support/app-harness";
import { preferencesFromMirrors, storageStatusNotice } from "./preferences";
import { resetApplicationStateMessage } from "./sections";

const stored = (preferences = {}) => ({ storage: "ok", preferences });
const echo = (patch) => ({ storage: "ok", preferences: patch });

async function openSettings(responses = {}, options = {}) {
  const harness = await bootApp({
    responses: {
      SetPreferences: echo,
      GetDiagnosticsReport: { paths: { globalHome: "/Users/me/.ptrack" } },
      ...responses,
    },
    ...options,
  });
  await harness.click("#landing-settings-open");
  expect(harness.$("#settings-modal").hidden).toBe(false);
  return harness;
}

describe("Settings", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("applies the stored record to the window and the dialog", async () => {
    harness = await openSettings({
      GetPreferences: stored({ appearance: { theme: "dark", density: "compact", reducedMotion: "always" } }),
    });
    const root = harness.document.documentElement;
    expect(root.dataset.theme).toBe("dark");
    expect(root.dataset.density).toBe("compact");
    expect(root.dataset.reducedMotion).toBe("always");
    expect(harness.$("#settings-theme").value).toBe("dark");
    expect(harness.$("#settings-density").value).toBe("compact");
    expect(harness.$("#settings-storage-notice").hidden).toBe(true);
  });

  it("keeps the values in use and says so when the stored record cannot be read", async () => {
    harness = await openSettings({ GetPreferences: new Error("no answer") });
    expect(harness.$("#settings-storage-notice").hidden).toBe(false);
    expect(harness.$("#settings-storage-notice").textContent).toBe(storageStatusNotice("unavailable"));
  });

  it("writes one changed choice and clears its confirmation after a while", async () => {
    harness = await openSettings();
    const timers = holdTimers(harness);
    await harness.type("#settings-density", "compact", "change");
    expect(harness.backend.callsTo("SetPreferences")).toEqual([[{ appearance: { density: "compact" } }]]);
    const status = harness.$("#settings-save-status");
    expect(status.textContent).toBe("Settings saved.");
    expect(timers.map(({ delay }) => delay)).toEqual([6000]);
    timers[0].callback();
    expect(status.textContent).toBe("");
    // The live region stays in the document; only its text goes.
    expect(status.parentNode).not.toBeNull();
  });

  it("keeps a failed save on screen and shows the stored value again", async () => {
    harness = await openSettings({ SetPreferences: new Error("disk full") });
    await harness.type("#settings-density", "compact", "change");
    const status = harness.$("#settings-save-status");
    expect(status.textContent).toBe("Settings could not be saved. The stored record is unchanged.");
    expect(status.dataset.tone).toBe("error");
    expect(harness.$("#settings-density").value).toBe("comfortable");
  });

  it("writes the startup, notification, and theme-toggle settings through", async () => {
    harness = await openSettings();
    const box = harness.$("#settings-startup-restore");
    box.checked = true;
    await harness.type(box, box.value, "change");
    await harness.click("#theme-toggle");
    const patches = harness.backend.callsTo("SetPreferences").map(([patch]) => patch);
    expect(patches[0]).toEqual({ startup: { restoreLastProject: true } });
    expect(Object.keys(patches[1])).toEqual(["appearance"]);
    expect(["light", "dark"]).toContain(patches[1].appearance.theme);
  });

  it("names each diagnostics copy button and confirms the copy in the live region", async () => {
    harness = await openSettings();
    const copy = harness.$(".settings-diagnostic-copy");
    expect(copy.getAttribute("aria-label")).toBeTruthy();
    expect(copy.title).toBe(copy.getAttribute("aria-label"));
    expect(copy.textContent).toBe("");
    await harness.click(copy);
    expect(harness.copied).toEqual(["/Users/me/.ptrack"]);
    expect(harness.$("#settings-save-status").textContent).toBe("Home folder copied.");
  });

  it("confirms the layout and application resets and keeps their outcome on screen", async () => {
    const result = { cleared: ["layout", "preferences"] };
    harness = await openSettings({
      ResetWindowLayout: { storage: "defaults" },
      ResetApplicationState: result,
      GetPreferences: stored(),
    });
    const timers = holdTimers(harness);
    await harness.click("#settings-reset-window-layout");
    expect(harness.backend.names()).not.toContain("ResetWindowLayout");
    await harness.click("#workspace-confirm-submit");
    const status = harness.$("#settings-save-status");
    expect(status.textContent).toBe("Window layout reset to defaults.");
    expect(timers).toEqual([]);

    const reads = harness.backend.callsTo("GetPreferences").length;
    await harness.click("#settings-reset-application-state");
    await harness.click("#workspace-confirm-submit");
    expect(harness.backend.callsTo("ResetApplicationState")).toEqual([[]]);
    expect(harness.backend.callsTo("GetPreferences").length).toBeGreaterThan(reads);
    expect(status.textContent).toBe(resetApplicationStateMessage(result));
  });

  it("turns automatic update checks on through the updates service", async () => {
    harness = await openSettings({
      SetAutomaticUpdateChecks: (enabled) => ({ revision: 2, phase: "idle", currentVersion: "1.2.3", automaticChecks: enabled }),
    });
    const box = harness.$("#settings-updates-automatic");
    box.checked = true;
    await harness.type(box, box.value, "change");
    expect(harness.backend.callsTo("SetAutomaticUpdateChecks")).toEqual([[true]]);
    expect(harness.backend.names()).not.toContain("SetPreferences");
  });

  it("moves between sections with the arrow keys and closes on Escape", async () => {
    harness = await openSettings();
    const tabs = harness.$$('#settings-section-list [role="tab"]');
    expect(tabs[0].getAttribute("aria-selected")).toBe("true");
    await harness.key(tabs[0], "ArrowDown");
    expect(tabs[1].getAttribute("aria-selected")).toBe("true");
    expect(harness.$("#settings-panel-appearance").hidden).toBe(false);
    expect(harness.$("#settings-panel-startup").hidden).toBe(true);
    expect(harness.document.activeElement).toBe(tabs[1]);
    await harness.key(tabs[1], "Escape");
    expect(harness.$("#settings-modal").hidden).toBe(true);
    expect(harness.document.activeElement.id).toBe("landing-settings-open");
  });

  it("sends a Unicode change to the stored record and to the open dock", async () => {
    harness = await openSettings({}, {
      open: { snapshot: snapshot(board()) },
    });
    const setModernUnicode = vi.spyOn(harness.app.state.terminalHandle, "setModernUnicode");
    await harness.type("#settings-terminal-unicode", "modern", "change");
    expect(harness.backend.callsTo("SetPreferences")).toEqual([[{ terminal: { unicodeMode: "modern" } }]]);
    expect(setModernUnicode).toHaveBeenCalledWith(true);
  });

  it("opens the Help Center from the landing", async () => {
    harness = await bootApp();
    await harness.click("#landing-help-open");
    expect(harness.backend.callsTo("OpenHelpDestination")).toEqual([["help-center"]]);
  });

  it("mirrors the stored record into the pre-paint cache", async () => {
    harness = await openSettings({
      GetPreferences: stored({ appearance: { theme: "dark" }, terminal: { fontSize: 15, unicodeMode: "modern" } }),
    });
    const mirrored = preferencesFromMirrors(harness.dom.window.localStorage);
    expect(mirrored.appearance.theme).toBe("dark");
    expect(mirrored.terminal).toMatchObject({ fontSize: 15, unicodeMode: "modern" });
  });

  it("resets settings only after confirming, and says so", async () => {
    harness = await openSettings({ ResetPreferences: stored() });
    await harness.click("#settings-reset");
    expect(harness.backend.names()).not.toContain("ResetPreferences");
    await harness.click("#workspace-confirm-submit");
    expect(harness.backend.callsTo("ResetPreferences")).toEqual([[]]);
    expect(harness.$("#settings-save-status").textContent).toBe("Settings reset to defaults.");
  });
});
