import { describe, expect, it } from "vitest";
import {
  formatUpdateBytes,
  updatePresentation,
  updateProgress,
  updateStateIsNewer,
} from "./presentation";

describe("update presentation", () => {
  it.each([
    ["idle", "check", false],
    ["recovering", null, false],
    ["recovery-required", null, false],
    ["checking", null, true],
    ["current", "check", false],
    ["available", "download", false],
    ["downloading", null, true],
    ["ready", "apply", false],
    ["applying", null, true],
    ["canceling", null, false],
    ["action-required", null, false],
    ["installed", null, false],
    ["unavailable", null, false],
    ["error", "apply", false],
  ])("maps %s to a bounded action", (phase, primaryAction, cancel) => {
    const result = updatePresentation({
      phase,
      checksumVerified: true,
      error: phase === "recovery-required" ? "Manual recovery required." : "",
      release: { version: "1.2.4" },
    });
    expect(result.primaryAction).toBe(primaryAction);
    expect(result.cancel).toBe(cancel);
  });

  it("keeps checks explicit and recovery fail-closed", () => {
    expect(updatePresentation({ phase: "idle" })).toMatchObject({
      primaryAction: "check",
      primaryLabel: "Check for updates",
    });
    expect(updatePresentation({ phase: "recovering" })).toMatchObject({
      busy: true,
      primaryAction: null,
    });
    expect(updatePresentation({ phase: "recovery-required", error: "Manual cleanup required." })).toMatchObject({
      tone: "error",
      primaryAction: null,
      detail: "Manual cleanup required.",
    });
    expect(updatePresentation({ phase: "canceling" })).toMatchObject({
      busy: true,
      cancel: false,
      primaryAction: null,
    });
  });

  it("requires separate download and verified-install actions", () => {
    const release = { version: "1.2.4" };
    expect(updatePresentation({ phase: "available", release })).toMatchObject({
      primaryAction: "download",
      primaryLabel: "Download and verify",
    });
    expect(updatePresentation({ phase: "ready", release, checksumVerified: true })).toMatchObject({
      primaryAction: "apply",
      primaryLabel: "Install verified update…",
    });
    expect(updatePresentation({ phase: "ready", release, checksumVerified: false })).toMatchObject({
      primaryAction: null,
      tone: "error",
    });
  });

  it("does not call manual handoff installed", () => {
    expect(updatePresentation({
      phase: "action-required",
      applyAction: "revealed-verified-archive",
    })).toMatchObject({
      title: "Complete installation manually",
      detail: expect.stringContaining("Windows archive"),
    });
  });

  it("fences stale revisions and bounds progress", () => {
    expect(updateStateIsNewer({ revision: 3 }, { revision: 2 })).toBe(false);
    expect(updateStateIsNewer({ revision: 3 }, { revision: 3 })).toBe(true);
    expect(updateProgress({ downloadedBytes: 150, totalBytes: 100 }).percent).toBe(100);
    expect(updateProgress({ downloadedBytes: Number.NaN, totalBytes: -4 })).toEqual({
      downloaded: 0,
      total: 0,
      percent: 0,
    });
    expect(formatUpdateBytes(Number.POSITIVE_INFINITY)).toBe("0 B");
    expect(formatUpdateBytes(2 * 1024 * 1024)).toBe("2.0 MB");
  });

  it("fails closed for unknown backend phases", () => {
    expect(updatePresentation({ phase: "future-phase" })).toMatchObject({
      primaryAction: null,
      title: "Update status is unavailable",
    });
  });
});
