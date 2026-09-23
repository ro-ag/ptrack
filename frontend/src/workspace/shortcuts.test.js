// Window-wide keys: Escape closes the top overlay only, Tab stays inside a
// dialog, the ⌘ chords reach their views, and a terminal window keeps its
// keystrokes for the terminal.
import { afterEach, describe, expect, it } from "vitest";

import { bootApp } from "../test-support/app-harness";
import { shown } from "../test-support/journey";

describe("window shortcuts", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("closes only the top overlay on Escape, and nothing later sees that key", async () => {
    harness = await bootApp();
    await harness.click("#landing-settings-open");
    await harness.click("#settings-reset-window-layout");
    expect(harness.$("#workspace-confirm-modal").hidden).toBe(false);
    const later = [];
    harness.document.addEventListener("keydown", (event) => later.push(event.key));
    await harness.key(harness.document.activeElement, "Escape");
    expect(harness.$("#workspace-confirm-modal").hidden).toBe(true);
    expect(harness.$("#settings-modal").hidden).toBe(false);
    expect(harness.backend.names()).not.toContain("ResetWindowLayout");
    await harness.key(harness.document.activeElement, "Escape");
    expect(harness.$("#settings-modal").hidden).toBe(true);
    expect(later).toEqual([]);
  });

  it("keeps Tab inside the open dialog", async () => {
    harness = await bootApp();
    await harness.click("#landing-settings-open");
    await harness.click("#settings-reset-window-layout");
    const cancel = harness.$("#workspace-confirm-cancel");
    const submit = harness.$("#workspace-confirm-submit");
    submit.focus();
    const event = await harness.key(submit, "Tab");
    expect(event.defaultPrevented).toBe(true);
    expect(harness.document.activeElement).toBe(cancel);
    await harness.key(cancel, "Tab", { shiftKey: true });
    expect(harness.document.activeElement).toBe(submit);
  });

  it("reaches each view with ⌘1–3 and opens the palette with ⌘K in an open project", async () => {
    harness = await bootApp({ open: {}, responses: { SearchV2: [] } });
    const { body } = harness.document;
    await harness.key(body, "1", { metaKey: true });
    expect(shown(harness.$("#overview-page"))).toBe(true);
    await harness.key(body, "3", { metaKey: true });
    expect(shown(harness.$("#issues-page"))).toBe(true);
    await harness.key(body, "2", { metaKey: true });
    expect(shown(harness.$("#board"))).toBe(true);
    await harness.key(body, "k", { metaKey: true });
    expect(harness.$("#palette").hidden).toBe(false);
    await harness.key(body, "k", { metaKey: true });
    expect(harness.$("#palette").hidden).toBe(true);
  });

  it("has no palette without an open project", async () => {
    harness = await bootApp();
    await harness.key(harness.document.body, "k", { metaKey: true });
    expect(harness.$("#palette").hidden).toBe(true);
  });

  it("leaves a terminal window's keystrokes to the terminal", async () => {
    harness = await bootApp({ hash: "#terminal-window=terminal-2", responses: { GetTerminalWindowTab: null } });
    expect(harness.$("#terminal-window-status").textContent)
      .toBe("This window no longer shows a terminal. Close it.");
    const event = await harness.key(harness.document.body, "k", { metaKey: true });
    expect(event.defaultPrevented).toBe(false);
    expect(harness.$("#palette").hidden).toBe(true);
    const settings = await harness.key(harness.document.body, ",", { metaKey: true });
    expect(settings.defaultPrevented).toBe(false);
    expect(harness.$("#settings-modal").hidden).toBe(true);
  });

  it("sets the window behind an open dialog inert until the dialog closes", async () => {
    harness = await bootApp({ open: {} });
    const app = harness.$("#app");
    expect(app.inert).toBe(false);
    await harness.click("#settings-open");
    expect(app.inert).toBe(true);
    expect(app.getAttribute("aria-hidden")).toBe("true");
    expect(harness.$("#settings-modal").getAttribute("data-application-overlay-layer")).toBe("active");
    await harness.click("#settings-close");
    expect(app.inert).toBe(false);
    expect(app.hasAttribute("aria-hidden")).toBe(false);
  });
});
