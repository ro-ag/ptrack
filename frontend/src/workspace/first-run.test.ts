import { describe, expect, it, vi } from "vitest";

import {
  canOpenPreservedFirstRunProject,
  completedInitializationWorkspaceMatches,
  firstRunFocusTarget,
  initializationFailureMessage,
  initializationStatusMatchesOperation,
  initialFirstRunState,
  isProjectGuidePartiallyApplied,
  isProjectGuidePreviewStale,
  parseInitializeProjectResult,
  parseInitializationStatus,
  parsePendingInitialization,
  pendingInitializationEvent,
  parseProjectGuidePreview,
  parseProjectTargetValidation,
  PROJECT_GUIDANCE_UNAVAILABLE,
  projectGuideCommitFields,
  reduceFirstRun,
  resolveFirstRunStartupState,
  validateNorthStarGoal,
} from "./first-run";

describe("first-run setup policy", () => {
  it("restores the invoking action when a picker is cancelled", () => {
    const picking = reduceFirstRun(initialFirstRunState, {
      type: "pick",
      intent: "open",
      returnFocusId: "state-open-project-button",
    });
    const cancelled = reduceFirstRun(picking, { type: "pickerCancelled" });

    expect(cancelled.phase).toBe("idle");
    expect(firstRunFocusTarget(cancelled)).toBe("state-open-project-button");
  });

  it("keeps existing and new targets on distinct paths", () => {
    const validating = reduceFirstRun(initialFirstRunState, { type: "validate" });
    const existing = reduceFirstRun(validating, {
      type: "existing",
      canonicalRoot: "/projects/existing",
    });
    const fresh = reduceFirstRun(validating, {
      type: "new",
      canonicalRoot: "/projects/new",
      operationId: "operation-1",
    });

    expect(existing).toMatchObject({ phase: "existing", operationId: "" });
    expect(fresh).toMatchObject({ phase: "goal", operationId: "operation-1" });
  });

  it("backs into a reversible selected-folder step without losing the goal draft", () => {
    const goal = reduceFirstRun(initialFirstRunState, {
      type: "new",
      canonicalRoot: "/projects/new",
      operationId: "operation-1",
    });
    const drafted = reduceFirstRun(goal, {
      type: "goalDrafted",
      goal: "A partially entered north-star goal",
    });
    const selectedFolder = reduceFirstRun(drafted, { type: "back" });
    const continued = reduceFirstRun(selectedFolder, { type: "continueToGoal" });

    expect(selectedFolder).toMatchObject({
      phase: "target-new",
      canonicalRoot: "/projects/new",
      operationId: "operation-1",
      goal: "A partially entered north-star goal",
    });
    expect(firstRunFocusTarget(selectedFolder)).toBe("setup-heading");
    expect(continued).toMatchObject({
      phase: "goal",
      goal: "A partially entered north-star goal",
    });
    expect(firstRunFocusTarget(continued)).toBe("setup-heading");

    const pickingAnother = reduceFirstRun(selectedFolder, {
      type: "pick",
      intent: "initialize",
      returnFocusId: "setup-new-target-choose",
    });
    const pickerCancelled = reduceFirstRun(pickingAnother, {
      type: "pickerCancelled",
      restore: selectedFolder,
    });
    expect(pickerCancelled).toEqual(selectedFolder);
    expect(pickerCancelled.goal).toBe("A partially entered north-star goal");

    const repicking = reduceFirstRun(selectedFolder, {
      type: "repick",
      intent: "initialize",
      returnFocusId: "setup-new-target-choose",
    });
    const validatingReplacement = reduceFirstRun(repicking, { type: "validate" });
    const replacement = reduceFirstRun(validatingReplacement, {
      type: "new",
      canonicalRoot: "/projects/replacement",
      operationId: "operation-2",
    });
    const replacementFailure = reduceFirstRun(validatingReplacement, {
      type: "failed",
      canonicalRoot: "/projects/unavailable",
      operationId: "",
      message: "Validation unavailable.",
    });
    expect(repicking).toMatchObject({
      phase: "picking",
      canonicalRoot: "/projects/new",
      goal: "A partially entered north-star goal",
    });
    expect(replacement).toMatchObject({
      phase: "goal",
      canonicalRoot: "/projects/replacement",
      operationId: "operation-2",
      goal: "A partially entered north-star goal",
    });
    expect(replacementFailure).toMatchObject({
      phase: "failed",
      canonicalRoot: "/projects/unavailable",
      operationId: "",
      goal: "A partially entered north-star goal",
      recoveryMode: "no-write",
    });
  });

  it("moves from goal through explicit guide consent and makes commit noncancelable", () => {
    const goal = reduceFirstRun(initialFirstRunState, {
      type: "new",
      canonicalRoot: "/projects/new",
      operationId: "operation-1",
    });
    const guide = reduceFirstRun(goal, { type: "goalAccepted", goal: "Ship safely" });
    const previewing = reduceFirstRun(guide, { type: "guidePreviewStarted" });
    const previewed = reduceFirstRun(previewing, {
      type: "guidePreviewed",
      preview: guidePreview(),
    });
    const review = reduceFirstRun(previewed, { type: "guideInstalled" });
    const committing = reduceFirstRun(review, { type: "commit" });

    expect(guide).toMatchObject({
      phase: "guide",
      goal: "Ship safely",
      guideChoice: "skip",
    });
    expect(review).toMatchObject({
      phase: "review",
      guideChoice: "install",
      guidePreviewToken: "opaque-preview-token",
    });
    expect(committing.phase).toBe("committing");
    expect(committing.resumeLocked).toBe(true);
    expect(committing.recoveryMode).toBe("durable");
    expect(reduceFirstRun(committing, { type: "back" }).phase).toBe("committing");
  });

  it("keeps skip as the default and strips any preview token", () => {
    const previewed = {
      ...initialFirstRunState,
      phase: "guide" as const,
      canonicalRoot: "/project",
      operationId: "operation-1",
      goal: "Ship safely",
      guideAvailable: true,
      guidePreviewToken: "opaque-preview-token",
      guideFiles: guidePreview().files,
    };
    const review = reduceFirstRun(previewed, { type: "guideSkipped" });

    expect(review).toMatchObject({
      phase: "review",
      guideChoice: "skip",
      guidePreviewToken: "",
      guideFiles: [],
    });
    expect(projectGuideCommitFields(review)).toEqual({
      guideChoice: "skip",
      guidePreviewToken: "",
    });
  });

  it("requires a complete bounded preview before install", () => {
    expect(() => projectGuideCommitFields({
      ...initialFirstRunState,
      guideChoice: "install",
    })).toThrow("current preview");
    expect(projectGuideCommitFields({
      ...initialFirstRunState,
      guideChoice: "install",
      guidePreviewToken: "opaque-preview-token",
      guideFiles: guidePreview().files,
    })).toEqual({
      guideChoice: "install",
      guidePreviewToken: "opaque-preview-token",
    });
  });

  it("parses only the fixed bounded guide targets and renders unavailable as skip-only", () => {
    expect(parseProjectGuidePreview(guidePreview())).toMatchObject({
      available: true,
      previewToken: "opaque-preview-token",
      files: [
        { path: "AGENTS.md", action: "create", additions: 2, deletions: 0 },
        { path: "CLAUDE.md", action: "no-change", additions: 0, deletions: 0 },
      ],
    });
    expect(parseProjectGuidePreview({
      available: false,
      message: PROJECT_GUIDANCE_UNAVAILABLE,
      previewToken: "",
      files: [],
    })).toEqual({
      available: false,
      message: PROJECT_GUIDANCE_UNAVAILABLE,
      previewToken: "",
      files: [],
    });
    expect(() => parseProjectGuidePreview({
      ...guidePreview(),
      files: [{
        path: "../../AGENTS.md",
        action: "create",
        additions: 1,
        deletions: 0,
        diff: "+unsafe",
      }],
    })).toThrow();
    expect(() => parseProjectGuidePreview({
      ...guidePreview(),
      files: guidePreview().files.map((file) => ({
        ...file,
        diff: file.path === "AGENTS.md" ? "x".repeat(65_537) : file.diff,
      })),
    })).toThrow("unbounded diff");
  });

  it("preserves operation identity through noncancellable postcommit stale recovery", () => {
    const committing = {
      ...initialFirstRunState,
      phase: "committing" as const,
      canonicalRoot: "/project",
      operationId: "operation-1",
      goal: "Ship safely",
      guideChoice: "install" as const,
    };
    const stale = reduceFirstRun(committing, { type: "guideStale", postCommit: true });

    expect(stale).toMatchObject({
      phase: "guide-stale",
      canonicalRoot: "/project",
      operationId: "operation-1",
      goal: "Ship safely",
      guidePostCommit: true,
      guidePreviewToken: "",
    });
    expect(reduceFirstRun(stale, { type: "back" })).toEqual(stale);
    expect(isProjectGuidePreviewStale(new Error("project-guide-preview-stale"))).toBe(true);
  });

  it("never offers a skip after any guide file was durably applied", () => {
    const committing = {
      ...initialFirstRunState,
      phase: "committing" as const,
      canonicalRoot: "/project",
      operationId: "operation-1",
      goal: "Ship safely",
      guideChoice: "install" as const,
    };
    const partial = reduceFirstRun(committing, {
      type: "guideStale",
      postCommit: true,
      checkpoint: "project-committed",
      skipAllowed: false,
      partiallyApplied: true,
      message: "Project guidance was partially applied before setup stopped.",
    });
    const previewed = reduceFirstRun(
      reduceFirstRun(partial, { type: "guidePreviewStarted" }),
      { type: "guidePreviewed", preview: guidePreview() },
    );

    expect(partial).toMatchObject({
      phase: "guide-stale",
      operationId: "operation-1",
      goal: "Ship safely",
      guidePostCommit: true,
      guideSkipAllowed: false,
      guidePartiallyApplied: true,
    });
    expect(reduceFirstRun(partial, { type: "guideSkipped" })).toEqual(partial);
    expect(reduceFirstRun(previewed, { type: "guideSkipped" })).toEqual(previewed);
    expect(() => projectGuideCommitFields({
      ...partial,
      guideChoice: "skip",
    })).toThrow("cannot be skipped");
    expect(reduceFirstRun(previewed, { type: "guideInstalled" }).phase).toBe("review");
    const previewFailed = reduceFirstRun(
      reduceFirstRun(partial, { type: "guidePreviewStarted" }),
      { type: "guidePreviewFailed", message: "Preview is unavailable." },
    );
    expect(previewFailed).toMatchObject({
      phase: "guide",
      checkpoint: "project-committed",
      guideChoice: "install",
      guideSkipAllowed: false,
      resumeLocked: true,
    });
    expect(canOpenPreservedFirstRunProject(previewFailed)).toBe(true);
    expect(isProjectGuidePartiallyApplied(
      new Error("project-guide-partially-applied"),
    )).toBe(true);
  });

  it("restores every typed restart checkpoint without losing durable choices", () => {
    for (const checkpoint of [
      "none",
      "prepared",
      "runtime-committed",
      "project-committed",
      "guide-applied",
    ]) {
      const parsed = parseProjectTargetValidation(resumeValidation(checkpoint));
      expect(parsed.resume).toMatchObject({
        goal: "Ship safely",
        guideChoice: "install",
        initialization: {
          operationId: "operation-1",
          canonicalRoot: "/project",
          checkpoint,
        },
      });
    }
    expect(() => parseProjectTargetValidation({
      ...resumeValidation("project-committed"),
      guideChoice: undefined,
    })).toThrow("invalid durable setup choices");
    expect(() => parseProjectTargetValidation({
      ...resumeValidation("project-committed"),
      initialization: {
        ...resumeValidation("project-committed").initialization,
        operationId: "replacement-operation",
      },
    })).toThrow("changed operation identity");
  });

  it("locks restart resumes and sends durable GuideApplied choices without an old token", () => {
    const base = {
      ...initialFirstRunState,
      phase: "validating" as const,
      returnFocusId: "state-initialize-project-button",
    };
    const earlyInstall = reduceFirstRun(base, {
      type: "resume",
      canonicalRoot: "/project",
      operationId: "operation-1",
      goal: "Ship safely",
      guideChoice: "install",
      initialization: parseInitializationStatus(
        resumeValidation("prepared").initialization,
      ),
    });
    const guideApplied = reduceFirstRun(base, {
      type: "resume",
      canonicalRoot: "/project",
      operationId: "operation-1",
      goal: "Ship safely",
      guideChoice: "install",
      initialization: parseInitializationStatus(
        resumeValidation("guide-applied").initialization,
      ),
    });
    const durableSkip = reduceFirstRun(base, {
      type: "resume",
      canonicalRoot: "/project",
      operationId: "operation-1",
      goal: "Ship safely",
      guideChoice: "skip",
      initialization: parseInitializationStatus({
        ...resumeValidation("project-committed").initialization,
        errorKind: "",
      }),
    });

    expect(earlyInstall).toMatchObject({
      phase: "guide-stale",
      guidePostCommit: false,
      resumeLocked: true,
      guideSkipAllowed: false,
      storageAlreadyCreated: false,
      goal: "Ship safely",
      operationId: "operation-1",
    });
    expect(reduceFirstRun(earlyInstall, { type: "back" })).toEqual(earlyInstall);
    expect(guideApplied).toMatchObject({
      phase: "review",
      guidePostCommit: true,
      resumeLocked: true,
      storageAlreadyCreated: true,
      checkpoint: "guide-applied",
    });
    expect(projectGuideCommitFields(guideApplied)).toEqual({
      guideChoice: "install",
      guidePreviewToken: "",
    });
    expect(reduceFirstRun(guideApplied, { type: "commit" })).toMatchObject({
      phase: "committing",
      checkpoint: "guide-applied",
      resumeLocked: true,
    });
    expect(durableSkip).toMatchObject({
      phase: "review",
      guidePostCommit: true,
      resumeLocked: true,
      guideChoice: "skip",
      storageAlreadyCreated: true,
    });
    expect(projectGuideCommitFields(durableSkip)).toEqual({
      guideChoice: "skip",
      guidePreviewToken: "",
    });
  });

  it("keeps None/Ready no-write resumes cancellable but locks None/InProgress", () => {
    const base = {
      ...initialFirstRunState,
      phase: "validating" as const,
      returnFocusId: "state-initialize-project-button",
    };
    const resume = (outcome: "ready" | "in-progress") => reduceFirstRun(base, {
      type: "resume",
      canonicalRoot: "/project",
      operationId: "operation-1",
      goal: "Ship safely",
      guideChoice: "install",
      initialization: parseInitializationStatus({
        operationId: "operation-1",
        canonicalRoot: "/project",
        outcome,
        checkpoint: "none",
        errorKind: "project-guide-preview-stale",
      }),
    });
    const ready = resume("ready");
    const inProgress = resume("in-progress");

    expect(ready).toMatchObject({
      phase: "guide-stale",
      guidePostCommit: false,
      resumeLocked: false,
      resumeNoWrite: true,
      storageAlreadyCreated: false,
      operationId: "operation-1",
      goal: "Ship safely",
    });
    expect(reduceFirstRun(ready, { type: "back" })).toMatchObject({
      phase: "idle",
      operationId: "",
    });
    expect(inProgress).toMatchObject({
      phase: "guide-stale",
      guidePostCommit: false,
      resumeLocked: true,
      resumeNoWrite: false,
      operationId: "operation-1",
      goal: "Ship safely",
    });
    expect(reduceFirstRun(inProgress, { type: "back" })).toEqual(inProgress);

    const interrupted = reduceFirstRun(base, {
      type: "resume",
      canonicalRoot: "/project",
      operationId: "operation-1",
      goal: "Ship safely",
      guideChoice: "install",
      initialization: parseInitializationStatus({
        operationId: "operation-1",
        canonicalRoot: "/project",
        outcome: "ready",
        checkpoint: "none",
        errorKind: "interrupted-before-commit",
      }),
    });
    expect(interrupted).toMatchObject({
      phase: "guide-stale",
      resumeLocked: false,
      resumeNoWrite: true,
      guideSkipAllowed: true,
    });
    expect(reduceFirstRun(interrupted, { type: "guideSkipped" }).phase).toBe(
      "review",
    );
  });

  it("keeps uncertain operations bound while status is reconciled", () => {
    const committing = {
      ...initialFirstRunState,
      phase: "committing" as const,
      canonicalRoot: "/project",
      operationId: "operation-1",
    };
    const uncertain = reduceFirstRun(committing, {
      type: "uncertain",
      message: "Status unavailable",
      checkpoint: "prepared",
    });
    expect(uncertain).toMatchObject({
      phase: "uncertain",
      canonicalRoot: "/project",
      operationId: "operation-1",
    });
    expect(reduceFirstRun(uncertain, { type: "reconcile" }).phase).toBe("reconciling");
  });

  it("separates no-write failures, resumable checkpoints, and blocked recovery", () => {
    const operation = {
      ...initialFirstRunState,
      phase: "committing" as const,
      canonicalRoot: "/project",
      operationId: "operation-1",
      goal: "Ship safely",
    };
    const failed = reduceFirstRun(operation, {
      type: "failed",
      message: "This folder is no longer available.",
      checkpoint: "none",
      errorKind: "project-not-found",
    });
    const resumable = reduceFirstRun(operation, {
      type: "recovery",
      message: "Resume safely.",
      checkpoint: "runtime-committed",
      durable: true,
    });
    const blocked = reduceFirstRun(operation, {
      type: "recovery",
      message: "Manual recovery required.",
      checkpoint: "prepared",
      durable: true,
      resumable: false,
    });

    expect(failed).toMatchObject({
      phase: "failed",
      recoveryMode: "no-write",
      resumeLocked: false,
      resumeNoWrite: true,
      goal: "Ship safely",
      errorKind: "project-not-found",
    });
    expect(resumable).toMatchObject({
      phase: "recovery",
      recoveryMode: "durable",
      resumeLocked: true,
      checkpoint: "runtime-committed",
    });
    expect(blocked).toMatchObject({
      phase: "recovery",
      recoveryMode: "blocked",
      resumeLocked: true,
      checkpoint: "prepared",
    });
    expect(canOpenPreservedFirstRunProject({
      ...resumable,
      phase: "guide-stale",
      checkpoint: "project-committed",
    })).toBe(true);
    expect(canOpenPreservedFirstRunProject({
      ...resumable,
      phase: "guide",
      checkpoint: "project-committed",
    })).toBe(true);
    expect(canOpenPreservedFirstRunProject({
      ...resumable,
      phase: "review",
      checkpoint: "guide-applied",
    })).toBe(true);
    expect(canOpenPreservedFirstRunProject(resumable)).toBe(false);
    expect(canOpenPreservedFirstRunProject(blocked)).toBe(false);
  });

  it("validates trimmed goals at 1, 4096, and 4097 UTF-8 bytes", () => {
    expect(validateNorthStarGoal(" x ")).toMatchObject({ value: "x", byteLength: 1, error: "" });
    expect(validateNorthStarGoal("a".repeat(4_096)).error).toBe("");
    expect(validateNorthStarGoal("a".repeat(4_097)).error).toContain("4,096");
    expect(validateNorthStarGoal("é".repeat(2_048)).byteLength).toBe(4_096);
    expect(validateNorthStarGoal("é".repeat(2_049)).error).toContain("4,096");
  });

  it("maps backend failure categories to plain recovery copy", () => {
    expect(initializationFailureMessage("runtime-busy")).toContain("Another p-track process");
    expect(initializationFailureMessage("authority-shutdown-failed")).toContain("pause");
    expect(initializationFailureMessage("private-backend-token")).toBe(
      "Initialization stopped before making a durable change.",
    );
    expect(initializationFailureMessage("private-backend-token")).not.toContain(
      "private-backend-token",
    );
  });

  it("binds every initialization status to the committed operation and root", () => {
    const status = parseInitializationStatus({
      outcome: "in-progress",
      checkpoint: "prepared",
      operationId: "operation-1",
      canonicalRoot: "/project",
    });
    expect(initializationStatusMatchesOperation(status, "operation-1", "/project")).toBe(true);
    expect(initializationStatusMatchesOperation(status, "operation-2", "/project")).toBe(false);
    expect(initializationStatusMatchesOperation(status, "operation-1", "/replacement")).toBe(false);
  });

  it("rejects every checkpoint/outcome combination that could understate a write", () => {
    const legal = new Set([
      "none:ready",
      "none:in-progress",
      "prepared:in-progress",
      "prepared:recovery-required",
      "runtime-committed:in-progress",
      "runtime-committed:recovery-required",
      "project-committed:in-progress",
      "project-committed:recovery-required",
      "guide-applied:in-progress",
      "guide-applied:recovery-required",
      "desktop-bound:complete",
    ]);
    for (const checkpoint of [
      "none",
      "prepared",
      "runtime-committed",
      "project-committed",
      "guide-applied",
      "desktop-bound",
    ]) {
      for (const outcome of [
        "ready",
        "in-progress",
        "recovery-required",
        "complete",
      ]) {
        const parse = () => parseInitializationStatus({
          operationId: "operation-1",
          canonicalRoot: "/project",
          checkpoint,
          outcome,
          errorKind: "",
        });
        if (legal.has(`${checkpoint}:${outcome}`)) expect(parse).not.toThrow();
        else expect(parse).toThrow("inconsistent checkpoint outcome");
      }
    }
  });

  it("strictly restores one backend-owned pending initialization", () => {
    const initialization = {
      operationId: "operation-1",
      canonicalRoot: "/project",
      checkpoint: "project-committed",
      outcome: "recovery-required",
      errorKind: "project-guide-preview-stale",
    };
    expect(parsePendingInitialization({ pending: false })).toEqual({
      pending: false,
      initialization: null,
      validation: null,
    });
    const resumable = parsePendingInitialization({
      pending: true,
      initialization,
      validation: resumeValidation("project-committed"),
    });
    expect(resumable).toMatchObject({
      pending: true,
      initialization,
      validation: { operationId: "operation-1", resume: { goal: "Ship safely" } },
    });
    const blocked = parsePendingInitialization({
      pending: true,
      initialization,
      validation: {
        kind: "recovery-required",
        canonicalRoot: "/project",
        reason: "Manual recovery required.",
      },
    });
    expect(blocked).toMatchObject({
      pending: true,
      validation: { kind: "recovery-required" },
    });
    expect(reduceFirstRun(
      { ...initialFirstRunState, phase: "validating" },
      pendingInitializationEvent(resumable)!,
    )).toMatchObject({
      phase: "guide-stale",
      operationId: "operation-1",
      canonicalRoot: "/project",
      resumeLocked: true,
    });
    expect(reduceFirstRun(
      { ...initialFirstRunState, phase: "validating" },
      pendingInitializationEvent(blocked)!,
    )).toMatchObject({
      phase: "recovery",
      recoveryMode: "blocked",
      operationId: "operation-1",
      checkpoint: "project-committed",
    });
    expect(() => parsePendingInitialization({
      pending: false,
      initialization,
    })).toThrow("unexpected fields");
    expect(() => parsePendingInitialization({
      pending: true,
      initialization,
      validation: {
        ...resumeValidation("project-committed"),
        canonicalRoot: "/replacement",
        initialization: {
          ...initialization,
          canonicalRoot: "/replacement",
        },
      },
    })).toThrow("changed its canonical root");
  });

  it("refreshes a Welcome snapshot after pending discovery can auto-bind the workspace", async () => {
    const getWorkspaceState = vi.fn()
      .mockResolvedValueOnce({ status: "welcome", generation: 0 })
      .mockResolvedValueOnce({
        status: "open",
        generation: 9,
        project: { root: "/project" },
      });
    const getPendingInitialization = vi.fn().mockResolvedValue({ pending: false });

    const startup = await resolveFirstRunStartupState(
      getWorkspaceState,
      getPendingInitialization,
    );

    expect(getWorkspaceState).toHaveBeenCalledTimes(2);
    expect(getPendingInitialization).toHaveBeenCalledOnce();
    expect(startup).toEqual({
      state: {
        status: "open",
        generation: 9,
        project: { root: "/project" },
      },
      pending: { pending: false, initialization: null, validation: null },
    });
  });

  it("pins the canonical validation DTO", () => {
    expect(parseProjectTargetValidation({
      kind: "new",
      canonicalRoot: "/project",
      operationId: "operation-1",
      reason: "",
    })).toMatchObject({ kind: "new", operationId: "operation-1" });
    expect(() => parseProjectTargetValidation({
      outcome: "existing",
      canonicalRoot: "/existing",
    })).toThrow("unknown outcome");
  });

  it("requires a complete status to carry one open workspace", () => {
    expect(parseInitializationStatus({
      outcome: "complete",
      checkpoint: "desktop-bound",
      operationId: "operation-1",
      canonicalRoot: "/project",
    }).checkpoint).toBe("desktop-bound");
    expect(parseInitializationStatus({
      outcome: "in-progress",
      checkpoint: "guide-applied",
      operationId: "operation-1",
      canonicalRoot: "/project",
    }).checkpoint).toBe("guide-applied");
    expect(parseInitializeProjectResult({
      initialization: {
        outcome: "complete",
        checkpoint: "desktop-bound",
        operationId: "operation-1",
        canonicalRoot: "/project",
      },
      state: { status: "open", generation: 1 },
    }).state?.status).toBe("open");
    expect(() => parseInitializeProjectResult({
      initialization: {
        outcome: "complete",
        checkpoint: "desktop-bound",
        operationId: "operation-1",
        canonicalRoot: "/project",
      },
      state: null,
    })).toThrow(
      "open workspace",
    );
    expect(() => parseInitializeProjectResult({
      initialization: {
        outcome: "complete",
        checkpoint: "guide-applied",
        operationId: "operation-1",
        canonicalRoot: "/project",
      },
      state: { status: "open" },
    })).toThrow("inconsistent checkpoint outcome");
  });

  it("requires an exact-root, positive-generation workspace before completed onboarding", () => {
    expect(completedInitializationWorkspaceMatches({
      status: "open",
      generation: 7,
      project: { root: "/project" },
    }, "/project")).toBe(true);
    expect(completedInitializationWorkspaceMatches({
      status: "welcome",
      generation: 0,
    }, "/project")).toBe(false);
    expect(completedInitializationWorkspaceMatches({
      status: "open",
      generation: 7,
      project: { root: "/other" },
    }, "/project")).toBe(false);
    expect(completedInitializationWorkspaceMatches({
      status: "open",
      generation: 0,
      project: { root: "/project" },
    }, "/project")).toBe(false);
  });
});

function guidePreview() {
  return {
    available: true,
    message: "",
    previewToken: "opaque-preview-token",
    files: [
      {
        path: "AGENTS.md" as const,
        action: "create" as const,
        additions: 2,
        deletions: 0,
        diff: "@@ -0,0 +1,2 @@\n+Use p-track.\n+Keep tasks current.",
      },
      {
        path: "CLAUDE.md" as const,
        action: "no-change" as const,
        additions: 0,
        deletions: 0,
        diff: "",
      },
    ],
  };
}

function resumeValidation(checkpoint: string) {
  return {
    kind: "new",
    canonicalRoot: "/project",
    operationId: "operation-1",
    reason: "",
    initialization: {
      operationId: "operation-1",
      canonicalRoot: "/project",
      outcome: checkpoint === "project-committed"
        ? "recovery-required"
        : "in-progress",
      checkpoint,
      errorKind: checkpoint === "project-committed"
        ? "project-guide-preview-stale"
        : "",
    },
    goal: "Ship safely",
    guideChoice: "install",
  };
}
