// About and updates driven through the window: when the dialog may open,
// what its primary action asks the updater, and how a failure reads.
import { afterEach, describe, expect, it } from "vitest";

import { bootApp } from "../test-support/app-harness";
import { journeyResponses, reachReview } from "../test-support/journey";
import { projectLinkAllowed, releasePageAllowed } from "./controller";
import { updateActionFailureMessage } from "./presentation";

const idle = { revision: 1, phase: "idle", currentVersion: "1.2.3" };

describe("About link allowlists", () => {
  it("accepts this project's repository and release pages", () => {
    expect(projectLinkAllowed("https://github.com/ro-ag/ptrack")).toBe(true);
    expect(projectLinkAllowed("https://github.com/ro-ag/ptrack/")).toBe(true);
    expect(projectLinkAllowed("https://github.com/ro-ag/ptrack/blob/main/LICENSE")).toBe(true);
    expect(releasePageAllowed("https://github.com/ro-ag/ptrack/releases/tag/v0.41.3")).toBe(true);
  });

  it("rejects a link that escapes the scope through dot segments", () => {
    expect(releasePageAllowed("https://github.com/ro-ag/ptrack/releases/tag/../../..//evil/x"))
      .toBe(false);
    expect(projectLinkAllowed("https://github.com/ro-ag/ptrack/../evil/x")).toBe(false);
  });

  it("rejects a lookalike repository that only shares the prefix", () => {
    expect(projectLinkAllowed("https://github.com/ro-ag/ptrack-evil")).toBe(false);
    expect(releasePageAllowed("https://github.com/ro-ag/ptrack-evil/releases/tag/v1")).toBe(false);
  });

  it("rejects other origins, encoded separators, and malformed links", () => {
    expect(projectLinkAllowed("https://evil.example/ro-ag/ptrack")).toBe(false);
    expect(projectLinkAllowed("http://github.com/ro-ag/ptrack")).toBe(false);
    expect(projectLinkAllowed("https://github.com/ro-ag/ptrack%2f..%2fevil")).toBe(false);
    expect(projectLinkAllowed("https://github.com/ro-ag/ptrack/a%2f%2e%2e%2fevil")).toBe(false);
    expect(releasePageAllowed("https://github.com/ro-ag/ptrack/releases")).toBe(false);
    expect(projectLinkAllowed("not a url")).toBe(false);
    expect(releasePageAllowed("")).toBe(false);
  });
});

describe("About and updates", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("opens from the version button and reads the updater's state", async () => {
    harness = await bootApp();
    const reads = harness.backend.callsTo("GetUpdateState").length;
    await harness.click("#app-version");
    expect(harness.$("#updates-modal").hidden).toBe(false);
    expect(harness.backend.callsTo("GetUpdateState").length).toBeGreaterThan(reads);
    await harness.key(harness.document.body, "Escape");
    expect(harness.$("#updates-modal").hidden).toBe(true);
    expect(harness.document.activeElement.id).toBe("app-version");
  });

  it("opens the same About dialog from the native menu without checking for updates", async () => {
    harness = await bootApp();
    await harness.emit("about:open-requested");
    expect(harness.$("#updates-modal").hidden).toBe(false);
    expect(harness.backend.callsTo("CheckForUpdates")).toEqual([]);
    await harness.key(harness.document.body, "Escape");
    expect(harness.$("#updates-modal").hidden).toBe(true);
    expect(harness.document.activeElement.id).toBe("app-version");
  });

  it("stays closed, with its entry points disabled, while project setup runs", async () => {
    harness = await bootApp({ responses: journeyResponses() });
    await reachReview(harness);
    expect(harness.$("#app-version").disabled).toBe(true);
    expect(harness.$("#landing-settings-open").disabled).toBe(true);
    harness.app.updates.openAboutUpdates();
    await harness.emit("about:open-requested");
    await harness.emit("update:open-requested");
    expect(harness.$("#updates-modal").hidden).toBe(true);
    expect(harness.backend.names()).not.toContain("CheckForUpdates");
  });

  it("checks from the native menu and says what failed", async () => {
    harness = await bootApp({
      responses: { CheckForUpdates: new Error("the release feed timed out") },
    });
    await harness.emit("update:open-requested");
    expect(harness.$("#updates-modal").hidden).toBe(false);
    expect(harness.backend.callsTo("CheckForUpdates")).toEqual([[]]);
    expect(harness.toast()).toBe(
      updateActionFailureMessage("check", new Error("the release feed timed out")),
    );
  });

  it("sends Report an issue to its help destination", async () => {
    harness = await bootApp({ responses: { GetUpdateState: idle } });
    await harness.click("#app-version");
    await harness.click("#about-report");
    expect(harness.backend.callsTo("OpenHelpDestination")).toEqual([["report-issue"]]);
  });
});
