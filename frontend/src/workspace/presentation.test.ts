import { describe, expect, it } from "vitest";

import {
  agentActivityAnnouncement,
  agentActivityPresentation,
  agentIntelligenceLabel,
  appVersionLabel,
  collapsedLaneStatuses,
  commandShortcut,
  confirmationCopy,
  focusCycleIndex,
  groupSearchResults,
  heatLevel,
  heatmapWeeks,
  handoffPreviewResponseIsCurrent,
  linkedTaskRuntimePresentation,
  mutationFocusFallback,
  paletteStatusPresentation,
  paletteTarget,
  preserveSectionOnError,
  postProjectOnboardingActions,
  projectGuideRecoveryCopy,
  projectGuideReviewCopy,
  runtimeAssociationLabel,
  runtimeCountLabel,
  runtimeEventIsCurrent,
  shortcutIntent,
  workflowMutationFocusKey,
  worktreeSelectionForRerender,
  workspaceStateCopy,
  driftPresentation,
  durableProjectGuideReviewCopy,
  firstRunRecoveryActions,
} from "./presentation";

describe("workspace presentation policy", () => {
  it("formats release versions without inventing a development release", () => {
    expect(appVersionLabel("1.2.3")).toBe("v1.2.3");
    expect(appVersionLabel("v2.0.0")).toBe("v2.0.0");
    expect(appVersionLabel("dev")).toBe("dev");
    expect(appVersionLabel(" ")).toBe("dev");
  });

  it("defines distinct copy for every workspace state", () => {
    for (const state of ["welcome", "loading", "open", "error", "closed"] as const) {
      const copy = workspaceStateCopy(state, state === "error" ? "broken" : "");
      expect(copy.heading.length).toBeGreaterThan(0);
      expect(copy.detail.length).toBeGreaterThan(0);
    }
    expect(workspaceStateCopy("welcome")).toEqual({
      eyebrow: "p-track projects",
      heading: "Start with a project",
      detail: "Initialize p-track in a folder, or open a project you already use.",
    });
    expect(workspaceStateCopy("error", "broken").detail).toBe("broken");
  });

  it("summarizes only the explicit guide choice and complete preview", () => {
    expect(projectGuideReviewCopy("skip")).toEqual({
      label: "Skip Guide",
      detail: "No guide files will change.",
      changes: [],
    });
    expect(projectGuideReviewCopy("install", [
      { path: "AGENTS.md", action: "create", additions: 4, deletions: 0 },
      { path: "CLAUDE.md", action: "no-change", additions: 0, deletions: 0 },
    ])).toEqual({
      label: "Install Guide",
      detail: "Only the previewed guide changes will be applied.",
      changes: ["AGENTS.md · create · +4 −0", "CLAUDE.md · no change"],
    });
  });

  it("makes partial guide recovery noncancellable without suggesting skip", () => {
    const copy = projectGuideRecoveryCopy("partially-applied");

    expect(copy.heading).toBe("Review the applied guide changes");
    expect(copy.detail).toContain("already durable");
    expect(copy.error).toContain("partially applied");
    expect(`${copy.heading} ${copy.detail} ${copy.error}`).not.toContain("skip");
  });

  it("offers only checkpoint-safe recovery exits", () => {
    expect(firstRunRecoveryActions("durable", "runtime-committed")).toEqual({
      resume: true,
      open: false,
      help: true,
      chooseAnother: false,
      returnToWelcome: false,
    });
    expect(firstRunRecoveryActions("durable", "project-committed")).toMatchObject({
      resume: true,
      open: true,
      help: true,
    });
    expect(firstRunRecoveryActions("blocked", "prepared")).toEqual({
      resume: false,
      open: false,
      help: true,
      chooseAnother: true,
      returnToWelcome: true,
    });
  });

  it("describes durable restart choices without replaying guide changes", () => {
    expect(durableProjectGuideReviewCopy("skip")).toEqual({
      label: "Skip Guide",
      detail: "Skip Guide is already durable for this initialization operation.",
      changes: [],
    });
    expect(durableProjectGuideReviewCopy("install")).toEqual({
      label: "Install Guide",
      detail: "The durable guide step is complete and will not be replayed.",
      changes: ["AGENTS.md and CLAUDE.md · guide step already applied"],
    });
  });

  it("keeps post-project recovery actions explicit and non-rollback", () => {
    expect(postProjectOnboardingActions("plan-failed")).toEqual({
      primary: "Try Again",
      secondary: "Skip for Now",
    });
    expect(postProjectOnboardingActions("task-create-failed")).toEqual({
      primary: "Try Again",
      secondary: "Finish with Plan",
    });
    expect(postProjectOnboardingActions("task-start-failed")).toEqual({
      primary: "Try Starting Again",
      secondary: "Finish Setup",
    });
    expect(JSON.stringify(postProjectOnboardingActions("task-start-failed")))
      .not.toMatch(/cancel|rollback/i);
  });

  it("describes only explicitly counted active resources", () => {
    expect(confirmationCopy("switch", 1, 2)).toEqual({
      heading: "Switch projects?",
      submit: "Switch project",
      detail: expect.stringContaining("1 active terminal and 2 registered agent runs"),
    });
  });

  it("describes pending resource operations separately from terminals", () => {
    expect(confirmationCopy("switch", 0, 0, 1).detail).toContain(
      "1 resource operation still finishing",
    );
    expect(confirmationCopy("switch", 0, 0, 1).detail).toContain(
      "0 active terminals",
    );
  });

  it("presents linked task state without conflating terminals and agents", () => {
    expect(linkedTaskRuntimePresentation({
      terminals: 1,
      liveTerminals: 1,
      agents: 2,
      liveAgents: 1,
      terminalBackedRuns: 1,
      externalRuns: 1,
    })).toEqual({
      compact: "Live · 1T 2A",
      detail: "1 terminal · 2 agents · 2 live · 1 historical",
      state: "live",
    });
    expect(linkedTaskRuntimePresentation({
      terminals: 1,
      agents: 1,
    })?.state).toBe("historical");
    expect(linkedTaskRuntimePresentation(undefined)).toBeNull();
    expect(linkedTaskRuntimePresentation({ truncated: true })).toEqual({
      compact: "Runtime capped",
      detail: "Linked runtime may be omitted because the project candidate bound was reached",
      state: "historical",
    });
  });

  it("accepts handoff previews only for the exact visible task association", () => {
    const association = { planId: 2, taskId: 7, revision: 3 };
    expect(handoffPreviewResponseIsCurrent(7, association, association, 7)).toBe(true);
    expect(handoffPreviewResponseIsCurrent(7, association, { ...association, taskId: 8 }, 7)).toBe(false);
    expect(handoffPreviewResponseIsCurrent(7, association, { ...association, revision: 4 }, 7)).toBe(false);
    expect(handoffPreviewResponseIsCurrent(7, association, association, 8)).toBe(false);
    expect(handoffPreviewResponseIsCurrent(7, association, null, 7)).toBe(false);
  });

  it("labels exact runtime targets and separate live resource counts", () => {
    expect(runtimeAssociationLabel({ planId: 2, taskId: 9 })).toBe(
      "plan #2 · task #9",
    );
    expect(runtimeAssociationLabel({ planId: 2 })).toBe("plan #2");
    expect(runtimeAssociationLabel({})).toBe("project");
    expect(runtimeAssociationLabel(null)).toBe("unlinked");
    expect(runtimeCountLabel(
      [{ live: true }, { live: false }],
      [{ live: true }, { live: false }, { live: false }],
    )).toEqual({
      compact: "1T · 1A",
      detail: "1/2 live terminals · 1/3 live agents",
    });
  });

  it("presents only allowlisted content-free agent intelligence", () => {
    expect(agentIntelligenceLabel({
      state: "waiting",
      confidence: "medium",
      eventCount: 1,
    })).toBe("intelligence waiting · medium confidence · 1 structured event");
    expect(agentIntelligenceLabel({
      state: "potentiallyDrifting",
      confidence: "high",
      eventCount: -4,
    })).toBe(
      "intelligence potentiallyDrifting · high confidence · 0 structured events",
    );
    expect(agentIntelligenceLabel({
      state: "<script>alert(1)</script>",
      confidence: "very",
      eventCount: "many",
    })).toBe("");
  });

  it("does not reannounce unchanged heartbeat-only activity", () => {
    const initial = agentActivityAnnouncement({
      items: [{ runId: "run-1", state: "running", lastEventAt: "first" }],
      notifications: [{ id: "notice-1" }],
    });
    expect(initial?.text).toContain("1 running");
    expect(agentActivityAnnouncement({
      items: [{ runId: "run-1", state: "running", lastEventAt: "later" }],
      notifications: [{ id: "notice-1" }],
    }, initial?.key || "")).toBeNull();
    expect(agentActivityAnnouncement({
      items: [{ runId: "run-1", state: "waiting" }],
      notifications: [{ id: "notice-1" }],
    }, initial?.key || "")?.text).toContain("1 waiting");
    expect(agentActivityAnnouncement({
      items: [{ runId: "run-1", state: "running" }],
      notifications: [{ id: "notice-2" }],
    }, initial?.key || "")).not.toBeNull();
    expect(agentActivityAnnouncement({
      items: [],
      notifications: [],
      registeredTotal: 3,
    })?.text).toContain("3 registered agents; detailed states unavailable");
  });

  it("keeps workflow actions focus-distinct with a safe removed-row fallback", () => {
    const approve = workflowMutationFocusKey("approve", "proposal-1");
    const dismiss = workflowMutationFocusKey("dismiss", "proposal-1");
    expect(approve).toBe("workflow:approve:proposal-1");
    expect(dismiss).toBe("workflow:dismiss:proposal-1");
    expect(approve).not.toBe(dismiss);
    expect(mutationFocusFallback(approve)).toBe("workflowPrepare");
    expect(mutationFocusFallback(dismiss)).toBe("workflowPrepare");
    expect(mutationFocusFallback("ownership:run-1")).toBe("");
  });

  it("preserves only an available focused worktree choice across rerender", () => {
    const options = ["/repo", "/sibling"];
    expect(worktreeSelectionForRerender(options, "/repo", null, "run-1")).toBe("/repo");

    const unsubmitted = { runId: "run-1", value: "/sibling" };
    expect(worktreeSelectionForRerender(
      options, "/repo", unsubmitted, "run-1",
    )).toBe("/sibling");
    expect(worktreeSelectionForRerender(
      options, "/repo", unsubmitted, "run-2",
    )).toBe("/repo");
    expect(worktreeSelectionForRerender(
      ["/repo"], "/repo", unsubmitted, "run-1",
    )).toBe("/repo");
    expect(worktreeSelectionForRerender(
      options, "/sibling", null, "run-1",
    )).toBe("/sibling");
  });

  it("presents bounded unified agent activity with explicit unknown state", () => {
    expect(agentActivityPresentation({
      items: [
        { runId: "run-1", state: "running" },
        { runId: "run-2", state: "waiting" },
        { runId: "run-3", state: "invented" },
      ],
      bounds: { shown: 3, total: 5, more: 2 },
    })).toEqual({
      items: [
        { runId: "run-1", state: "running" },
        { runId: "run-2", state: "waiting" },
        { runId: "run-3", state: "unknown" },
      ],
      counts: [
        { state: "running", count: 1 },
        { state: "waiting", count: 1 },
        { state: "unknown", count: 1 },
      ],
      conflicts: [],
      analysisIncomplete: false,
      notifications: [],
      notificationsIncomplete: false,
      handoffs: { items: [], incomplete: false },
      worktrees: [],
		worktreesIncomplete: false,
		workflows: { items: [], incomplete: false },
		workflowTargets: [],
		workflowTargetsIncomplete: false,
      registeredTotal: 5,
      liveCount: 0,
      canHandoff: false,
      canPrepareWorkflow: false,
      compact: "3/5",
      detail: "5 registered agents · 2 older entries omitted",
    });
  });

  it("derives real agent action availability from registered and live counts", () => {
    expect(agentActivityPresentation({ bounds: { total: 0 } })).toMatchObject({
      registeredTotal: 0,
      liveCount: 0,
      canHandoff: false,
      canPrepareWorkflow: false,
    });
    expect(agentActivityPresentation({
      items: [{ runId: "run-1", state: "running", live: true }],
      bounds: { total: 1 },
    })).toMatchObject({
      registeredTotal: 1,
      liveCount: 1,
      canHandoff: false,
      canPrepareWorkflow: true,
    });
    expect(agentActivityPresentation({
      items: [
        { runId: "run-1", state: "running", live: true },
        { runId: "run-2", state: "waiting", live: true },
      ],
    })).toMatchObject({
      registeredTotal: 2,
      liveCount: 2,
      canHandoff: true,
      canPrepareWorkflow: true,
    });
    expect(agentActivityPresentation({ bounds: { total: 3 } })).toMatchObject({
      registeredTotal: 3,
      liveCount: 0,
      canHandoff: false,
      canPrepareWorkflow: false,
    });
  });

  it("allowlists unified activity fields and bounds malformed rows", () => {
    const presented = agentActivityPresentation({
      items: [{
        runId: "run-1",
        state: "running",
        live: true,
        association: { planId: 5, taskId: 37, revision: 2, prompt: "private" },
        terminalId: "terminal-private",
        projectRoot: "/private/repo",
        cwd: "/private/repo",
        summary: "private",
      }],
    });
    expect(presented.items).toEqual([{
      runId: "run-1",
      state: "running",
      live: true,
      association: { planId: 5, taskId: 37, revision: 2 },
    }]);
    expect(JSON.stringify(presented.items)).not.toContain("private");
  });

	it("allowlists bounded existing worktree metadata", () => {
    const activity = agentActivityPresentation({
      items: [{
        runId: "run-1",
        state: "running",
        worktree: {
          identity: {
            root: "/repo", branch: "main", head: "a".repeat(40),
            gitDir: "/private/git", commonGitDir: "/private/common",
          },
          verified: true,
          isolated: true,
          cwdMatches: true,
        },
      }],
      worktrees: [
        { root: "/repo", branch: "main", head: "a".repeat(40), remote: "secret" },
        { root: "/bad", branch: "bad", head: "not-a-sha" },
      ],
      worktreeBounds: { more: 1 },
	});

    expect(activity.worktrees).toEqual([
      { root: "/repo", branch: "main", head: "a".repeat(40) },
    ]);
    expect(activity.items[0].worktree).toEqual({
      identity: {
        root: "/repo", branch: "main", head: "a".repeat(40), linked: false,
      },
      verified: true,
      isolated: true,
      cwdMatches: true,
    });
    expect(JSON.stringify(activity.items[0])).not.toContain("private");
    expect(agentActivityPresentation({ worktreesIncomplete: true }).worktreesIncomplete).toBe(true);
  });

	it("allowlists closed no-execution workflow proposals", () => {
		const presented = agentActivityPresentation({
			workflows: { items: [
				{ id: "wf-1", runId: "run-1", kind: "pullRequest", state: "proposed", branch: "feature", head: "b".repeat(40), targetBranch: "main", targetHead: "d".repeat(40), status: { staged: 1, untracked: 2 }, command: "git push --force" },
				{ id: "wf-2", runId: "run-1", kind: "deploy", state: "approved", branch: "feature", head: "c".repeat(40) },
			], incomplete: true },
			workflowTargets: ["main", "bad\nbranch"],
		});
		expect(presented.workflows).toEqual({
			items: [{
				id: "wf-1", runId: "run-1", kind: "pullRequest", state: "proposed",
				branch: "feature", head: "b".repeat(40), targetBranch: "main", targetHead: "d".repeat(40),
				status: { staged: 1, unstaged: 0, untracked: 2, conflicted: 0, ahead: 0, behind: 0 },
			}],
			incomplete: true,
		});
		expect(presented.workflowTargets).toEqual(["main"]);
	});

  it("preserves bounded ownership conflicts and incomplete analysis", () => {
    expect(agentActivityPresentation({
      items: [{
        runId: "run-1",
        state: "running",
        ownership: { planId: 5, taskId: 38, associationRevision: 2 },
      }],
      conflicts: [{
        planId: 5,
        taskId: 38,
        agentCount: 3,
        ownerCount: 1,
        runIds: ["run-1", "run-2", "run-3"],
      }],
      conflictBounds: { shown: 1, total: 2, more: 1 },
      analysisIncomplete: true,
    })).toMatchObject({
      items: [{
        runId: "run-1",
        state: "running",
        ownership: { planId: 5, taskId: 38, associationRevision: 2 },
      }],
      conflicts: [{
        planId: 5,
        taskId: 38,
        agentCount: 3,
        ownerCount: 1,
        runIds: ["run-1", "run-2", "run-3"],
      }],
      analysisIncomplete: true,
    });
  });

  it("presents only closed content-free agent notifications", () => {
    expect(agentActivityPresentation({
      notifications: [
        { id: "n-1", runId: "run-1", kind: "approvalRequested", observedAt: "2026-08-10T20:00:00Z", terminalBacked: true },
        { id: "n-2", runId: "run-1", kind: "question", observedAt: "2026-08-10T20:01:00Z" },
        { id: "n-3", runId: "run-1", kind: "failure", observedAt: "2026-08-10T20:02:00Z" },
        { id: "n-4", runId: "run-1", kind: "completion", observedAt: "2026-08-10T20:03:00Z" },
        { id: "n-5", runId: "run-1", kind: "approvalGranted", observedAt: "2026-08-10T20:04:00Z", text: "secret" },
      ],
      notificationsIncomplete: true,
    })).toMatchObject({
      notifications: [
        { id: "n-1", runId: "run-1", kind: "approvalRequested", observedAt: "2026-08-10T20:00:00Z", terminalBacked: true },
        { id: "n-2", runId: "run-1", kind: "question", observedAt: "2026-08-10T20:01:00Z", terminalBacked: false },
        { id: "n-3", runId: "run-1", kind: "failure", observedAt: "2026-08-10T20:02:00Z", terminalBacked: false },
        { id: "n-4", runId: "run-1", kind: "completion", observedAt: "2026-08-10T20:03:00Z", terminalBacked: false },
      ],
      notificationsIncomplete: true,
    });
  });

  it("allowlists bounded immutable handoff proposals", () => {
    expect(agentActivityPresentation({
      handoffs: {
        items: [{
          id: "handoff-1",
          sourceRunId: "source-1",
          targetRunId: "target-1",
          createdAt: "2026-08-10T20:00:00Z",
          expiresAt: "2026-08-10T20:30:00Z",
          preview: {
            text: "Agent run state: working.",
            includedEventIds: Array.from({ length: 12 }, (_, index) => `event-${index}`),
          },
        }, {
          id: "forged",
          sourceRunId: "same",
          targetRunId: "same",
          createdAt: "now",
          expiresAt: "later",
          preview: { text: "forged" },
        }],
        incomplete: true,
      },
    }).handoffs).toEqual({
      items: [{
        id: "handoff-1",
        sourceRunId: "source-1",
        targetRunId: "target-1",
        createdAt: "2026-08-10T20:00:00Z",
        expiresAt: "2026-08-10T20:30:00Z",
        preview: {
          text: "Agent run state: working.",
          includedEventIds: Array.from({ length: 8 }, (_, index) => `event-${index}`),
          truncated: false,
        },
      }],
      incomplete: true,
    });
  });

  it("allowlists bounded conservative drift findings", () => {
    expect(driftPresentation({
      findings: [
        { kind: "untrackedFile", severity: "warning", scope: "projectUnattributed", path: "frontend/new.ts", evidenceCount: 1 },
        { kind: "unlinkedCommit", severity: "info", scope: "projectUnattributed", sha: "abcdef0123456789", evidenceCount: 1 },
        { kind: "crossTaskPathOverlap", severity: "warning", scope: "taskComparison", path: "internal/shared.go", runIds: ["one", "two"], evidenceCount: 2 },
        { kind: "certainDrift", severity: "critical", scope: "agent", path: "/secret", evidenceCount: 99 },
      ],
      incomplete: true,
    })).toEqual({
      findings: [
        { kind: "untrackedFile", severity: "warning", scope: "projectUnattributed", path: "frontend/new.ts", sha: "", runIds: [], evidenceCount: 1 },
        { kind: "crossTaskPathOverlap", severity: "warning", scope: "taskComparison", path: "internal/shared.go", sha: "", runIds: ["one", "two"], evidenceCount: 2 },
      ],
      unlinkedCommits: [
        { kind: "unlinkedCommit", severity: "info", scope: "projectUnattributed", path: "", sha: "abcdef0123456789", runIds: [], evidenceCount: 1 },
      ],
      incomplete: true,
    });
  });

  it("groups every valid unlinked commit while preserving other drift rows", () => {
    const drift = driftPresentation({
      findings: [
        { kind: "unlinkedCommit", severity: "info", scope: "projectUnattributed", sha: "abcdef0", evidenceCount: 1 },
        { kind: "untrackedFile", severity: "warning", scope: "projectUnattributed", path: "new.rs", evidenceCount: 2 },
        { kind: "unlinkedCommit", severity: "info", scope: "projectUnattributed", sha: "1234567", evidenceCount: 1 },
      ],
      bounds: { more: 1 },
    });
    expect(drift.findings.map((finding) => finding.kind)).toEqual(["untrackedFile"]);
    expect(drift.unlinkedCommits.map((finding) => finding.sha)).toEqual([
      "abcdef0", "1234567",
    ]);
    expect(drift.incomplete).toBe(true);
  });

  it("accepts runtime refresh events only for the open generation", () => {
    expect(runtimeEventIsCurrent(7, 7, true)).toBe(true);
    expect(runtimeEventIsCurrent(6, 7, true)).toBe(false);
    expect(runtimeEventIsCurrent(7, 7, false)).toBe(false);
    expect(runtimeEventIsCurrent("not-a-generation", 7, true)).toBe(false);
  });

  it("cycles focus in both directions", () => {
    expect(focusCycleIndex(3, 2, false)).toBe(0);
    expect(focusCycleIndex(3, 0, true)).toBe(2);
    expect(focusCycleIndex(3, 1, false)).toBe(2);
    expect(focusCycleIndex(2, 1, false)).toBe(0);
    expect(focusCycleIndex(2, 0, true)).toBe(1);
    expect(focusCycleIndex(3, -1, false)).toBe(0);
  });

  it("retains a successful section as stale when a partial refresh fails", () => {
    const previous = { state: "ready", snapshot: { branch: "main" } };
    expect(
      preserveSectionOnError(previous, { state: "error", error: "timed out" }),
    ).toEqual({
      state: "stale",
      snapshot: { branch: "main" },
      error: "timed out",
    });
  });

  it("suppresses shortcuts during composition, modifiers, and repeats", () => {
    expect(shortcutIntent({ key: "r" })).toBe("refresh");
    expect(shortcutIntent({ key: "/" })).toBe("addTask");
    expect(shortcutIntent({ key: "r", composing: true })).toBeNull();
    expect(shortcutIntent({ key: "/", ctrl: true })).toBeNull();
    expect(shortcutIntent({ key: "r", repeat: true })).toBeNull();
  });

  it("routes primary-modifier chords to commands", () => {
    expect(commandShortcut({ key: "k", meta: true })).toBe("palette");
    expect(commandShortcut({ key: "K", ctrl: true })).toBe("palette");
    expect(commandShortcut({ key: "1", meta: true })).toBe("board");
    expect(commandShortcut({ key: "2", meta: true })).toBe("overview");
    expect(commandShortcut({ key: "3", meta: true })).toBeNull();
    expect(commandShortcut({ key: ",", meta: true })).toBe("settings");
    expect(commandShortcut({ key: ",", ctrl: true })).toBe("settings");
    expect(commandShortcut({ key: "n", meta: true })).toBe("addTask");
  });

  it("ignores command chords without a primary modifier or with extras", () => {
    expect(commandShortcut({ key: "k" })).toBeNull();
    expect(commandShortcut({ key: "1", shift: true })).toBeNull();
    expect(commandShortcut({ key: "k", meta: true, alt: true })).toBeNull();
    expect(commandShortcut({ key: "k", meta: true, repeat: true })).toBeNull();
    expect(commandShortcut({ key: "k", meta: true, prevented: true })).toBeNull();
    expect(commandShortcut({ key: "x", meta: true })).toBeNull();
  });

  it("groups palette results in plans, tasks, notes order", () => {
    const groups = groupSearchResults([
      { kind: "note", id: 9, planId: 0, title: "Task note", snippet: "…" },
      { kind: "task", id: 4, planId: 2, title: "Card", snippet: "" },
      { kind: "plan", id: 2, planId: 2, title: "Board", snippet: "" },
    ]);
    expect(groups.map((group) => group.label)).toEqual(["Plans", "Tasks", "Notes"]);
    expect(groups[0].items[0].title).toBe("Board");
    expect(groupSearchResults([])).toEqual([]);
  });

  it("maps palette result statuses to canonical accessible glyphs", () => {
    expect(paletteStatusPresentation({ kind: "plan", status: "active" })).toEqual({
      glyph: "●", label: "Active", tone: "active",
    });
    expect(paletteStatusPresentation({ kind: "plan", status: "done" })?.glyph).toBe("✓");
    expect(paletteStatusPresentation({ kind: "plan", status: "archived" })?.glyph).toBe("—");
    expect(paletteStatusPresentation({ kind: "task", status: "todo" })?.glyph).toBe("○");
    expect(paletteStatusPresentation({ kind: "task", status: "doing" })?.glyph).toBe("◐");
    expect(paletteStatusPresentation({ kind: "task", status: "blocked" })?.glyph).toBe("✗");
    expect(paletteStatusPresentation({ kind: "task", status: "done" })?.glyph).toBe("✓");
    expect(paletteStatusPresentation({ kind: "note", status: "done" })).toBeNull();
    expect(paletteStatusPresentation({ kind: "task", status: "invented" })).toBeNull();
    expect(paletteStatusPresentation({ kind: "plan" })).toBeNull();
  });

  it("maps palette results to their activation targets", () => {
    expect(
      paletteTarget({ kind: "plan", id: 3, planId: 3, title: "P", snippet: "" }),
    ).toEqual({ view: "board", planId: 3, taskId: 0 });
    expect(
      paletteTarget({ kind: "task", id: 7, planId: 3, title: "T", snippet: "" }),
    ).toEqual({ view: "board", planId: 3, taskId: 7 });
    expect(
      paletteTarget({ kind: "note", id: 1, planId: 3, title: "N", snippet: "" }),
    ).toEqual({ view: "overview", planId: 0, taskId: 0 });
  });

  it("collapses empty lanes unless re-expanded or all lanes are empty", () => {
    const lanes = [
      { status: "todo", taskCount: 2 },
      { status: "doing", taskCount: 0 },
      { status: "blocked", taskCount: 0 },
      { status: "done", taskCount: 5 },
    ];
    expect(collapsedLaneStatuses(lanes, new Set())).toEqual(["doing", "blocked"]);
    expect(collapsedLaneStatuses(lanes, new Set(["doing"]))).toEqual(["blocked"]);
    const allEmpty = lanes.map((lane) => ({ ...lane, taskCount: 0 }));
    expect(collapsedLaneStatuses(allEmpty, new Set())).toEqual([]);
    const allBusy = lanes.map((lane) => ({ ...lane, taskCount: 1 }));
    expect(collapsedLaneStatuses(allBusy, new Set())).toEqual([]);
    // Populated lanes fold only when the user collapsed them manually.
    expect(collapsedLaneStatuses(lanes, new Set(), new Set(["done"]))).toEqual([
      "doing",
      "blocked",
      "done",
    ]);
    expect(collapsedLaneStatuses(lanes, new Set(), new Set(["doing"]))).toEqual([
      "doing",
      "blocked",
    ]);
  });

  it("scales heat levels against the series maximum", () => {
    expect(heatLevel(0, 10)).toBe(0);
    expect(heatLevel(1, 10)).toBe(1);
    expect(heatLevel(5, 10)).toBe(2);
    expect(heatLevel(10, 10)).toBe(4);
    expect(heatLevel(3, 0)).toBe(0);
  });

  it("buckets days into Sunday-first week columns with padding", () => {
    // 2026-07-21 is a Tuesday, so the first column gets two padding cells.
    const days = [
      { date: "2026-07-21", count: 3 },
      { date: "2026-07-22", count: 0 },
      { date: "2026-07-23", count: 9 },
    ];
    const columns = heatmapWeeks(days);
    expect(columns).toHaveLength(1);
    expect(columns[0].map((cell) => cell.date)).toEqual([
      "",
      "",
      "2026-07-21",
      "2026-07-22",
      "2026-07-23",
    ]);
    expect(columns[0][2].level).toBe(2);
    expect(columns[0][4].level).toBe(4);
    expect(heatmapWeeks([])).toEqual([]);
  });
});
