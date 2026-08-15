import { describe, expect, it } from "vitest";

import {
  focusAfterForgottenProject,
  initialRecentProjectsState,
  parseForgetRecentProjectResult,
  parseRecentProjectOpenResult,
  parseRecentProjectResolution,
  parseRecentProjects,
  preselectedRecentProject,
  RECENT_RELOCATION_UNCONFIRMED,
  recentProjectFocusKey,
  reduceRecentProjects,
} from "./recent-projects";

const entry = {
  entryId: "entry-a",
  base: "base-a",
  name: "Alpha",
  canonicalPath: "/work/alpha",
  lastOpenedAt: "2026-08-14T10:00:00Z",
  availability: "missing" as const,
};

describe("recent project recovery", () => {
  it("parses every typed availability and enforces newest-first bounded rows", () => {
    const states = ["available", "missing", "permission-required", "changed"];
    const projects = Array.from({ length: 20 }, (_, index) => ({
      ...entry,
      entryId: `entry-${index}`,
      base: `base-${index}`,
      availability: states[index % states.length],
      lastOpenedAt: new Date(Date.UTC(2026, 7, 14, 0, 20 - index)).toISOString(),
    }));
    const parsed = parseRecentProjects({ projects });
    expect(parsed).toHaveLength(20);
    expect(parsed[0].entryId).toBe("entry-0");
    expect(new Set(parsed.map((project) => project.availability))).toEqual(
      new Set(states),
    );
    expect(() => parseRecentProjects({ projects: [...projects, {
      ...entry,
      entryId: "entry-20",
    }] })).toThrow("20-entry limit");
    expect(() => parseRecentProjects({ projects: [...projects].reverse() }))
      .toThrow("newest first");
  });

  it("rejects malformed, duplicate, and string-inferred availability", () => {
    expect(() => parseRecentProjects({ projects: [{ ...entry, availability: "denied" }] }))
      .toThrow("availability");
    expect(() => parseRecentProjects({ projects: [entry, entry] }))
      .toThrow("unique");
  });

  it("keeps one scoped operation and preserves rows across failures", () => {
    const loading = reduceRecentProjects(initialRecentProjectsState, {
      type: "loadStarted",
    });
    const loaded = reduceRecentProjects(loading, {
      type: "loaded",
      projects: [entry],
    });
    const active = reduceRecentProjects(loaded, {
      type: "begin",
      operationId: 7,
      entry,
      intent: "locate",
    });
    expect(reduceRecentProjects(active, { type: "loadStarted" })).toBe(active);
    const failed = reduceRecentProjects(active, {
      type: "failed",
      message: "Could not confirm the selected folder.",
    });
    expect(failed).toMatchObject({
      phase: "error",
      operationId: 7,
      activeEntryId: "entry-a",
      activeBase: "base-a",
      projects: [entry],
    });
  });

  it("pins resolution, open, and Forget results to opaque entry identity", () => {
    expect(parseRecentProjectResolution({
      entryId: "entry-a",
      base: "base-a",
      canonicalRoot: "/work/beta",
      name: "Beta",
      resolution: "confirmation-required",
      confirmationToken: "confirm-a",
    }, entry).confirmationToken).toBe("confirm-a");
    expect(() => parseRecentProjectResolution({
      entryId: "entry-a",
      base: "new-base",
      canonicalRoot: "/work/beta",
      name: "Beta",
      resolution: "ready",
      confirmationToken: "",
    }, entry)).toThrow("no longer matches");
    expect(() => parseRecentProjectResolution({
      entryId: "entry-a",
      base: "base-a",
      canonicalRoot: "/work/beta",
      name: "Beta",
      resolution: "ready",
      confirmationToken: "unexpected",
    }, entry)).toThrow("no longer matches");

    expect(parseRecentProjectOpenResult({
      entryId: "entry-a",
      registryBase: "base-b",
      registryStatus: "relocated",
      open: {
        state: { status: "open", generation: 4 },
        requiresConfirmation: false,
      },
    }, entry).registryStatus).toBe("relocated");
    expect(parseRecentProjectOpenResult({
      entryId: "entry-a",
      registryBase: "base-a",
      registryStatus: "unchanged",
      open: {
        state: { status: "open", generation: 4 },
        requiresConfirmation: true,
        confirmationToken: "workspace-token",
        activeResources: { terminals: 1, agentRuns: 0, pendingAdmissions: 0 },
      },
    }, entry).open.requiresConfirmation).toBe(true);
    expect(() => parseRecentProjectOpenResult({
      entryId: "entry-a",
      registryBase: "base-a",
      registryStatus: "unchanged",
      open: {
        state: { status: "welcome", generation: 0 },
        requiresConfirmation: false,
      },
    }, entry)).toThrow("stale or invalid");
    expect(parseForgetRecentProjectResult({
      entryId: "entry-a",
      registryBase: "base-b",
      forgotten: true,
    }, entry).forgotten).toBe(true);
  });

  it("moves focus to the next row or heading after Forget", () => {
    const next = { ...entry, entryId: "entry-b", availability: "available" as const };
    expect(focusAfterForgottenProject([entry, next], entry.entryId)).toBe(
      recentProjectFocusKey(next.entryId, "open"),
    );
    expect(focusAfterForgottenProject([next, entry], entry.entryId)).toBe(
      recentProjectFocusKey(next.entryId, "open"),
    );
    expect(focusAfterForgottenProject([entry], entry.entryId)).toBe(
      "recent-project-heading",
    );
  });

  it("preselects the recorded last project only while the opt-in is on", () => {
    const moved = { ...entry, availability: "changed" as const };
    const other = {
      ...entry,
      entryId: "entry-b",
      canonicalPath: "/work/alpha-2",
      availability: "available" as const,
    };
    const on = { restoreLastProject: true, lastProjectRoot: "/work/alpha" };
    expect(preselectedRecentProject([other, moved], on)).toBe("entry-a");
    expect(preselectedRecentProject([other, moved], { ...on, restoreLastProject: false }))
      .toBe("");
    expect(preselectedRecentProject([other, moved], { ...on, lastProjectRoot: null }))
      .toBe("");
    expect(preselectedRecentProject([other], on)).toBe("");
    expect(preselectedRecentProject([], on)).toBe("");
  });

  it("treats relocation as unconfirmed after a lost response despite bounded reload", () => {
    expect(RECENT_RELOCATION_UNCONFIRMED).toContain("could not confirm");
    expect(RECENT_RELOCATION_UNCONFIRMED).toContain("bounded registry list");
    expect(RECENT_RELOCATION_UNCONFIRMED).toContain("without replaying");
  });
});
