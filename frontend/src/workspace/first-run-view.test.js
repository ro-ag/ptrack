// Drives the first-run and first-plan screens the way a person does — the
// folder picker, the setup steps, the onboarding forms — against a scripted
// backend, and checks what reaches the backend and what the window shows.
import { afterEach, describe, expect, it } from "vitest";

import { board, bootApp, snapshot } from "../test-support/app-harness";
import {
  canonicalRoot,
  complete,
  createdAt,
  goal,
  journeyCalls,
  journeyResponses,
  openState,
  operationId,
  reachReview,
  request,
  shown,
  visibleSteps,
} from "../test-support/journey";

describe("first-run window journey", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("reads the startup journal before choosing the first screen", async () => {
    harness = await bootApp();
    const names = harness.backend.names();
    expect(names.slice(names.indexOf("GetWorkspaceState"), names.indexOf("GetWorkspaceState") + 3))
      .toEqual(["GetWorkspaceState", "GetPendingInitializationV1", "GetWorkspaceState"]);
    expect(shown(harness.$("#project-state-card"))).toBe(true);
    expect(shown(harness.$("#setup-panel"))).toBe(false);
  });

  it("walks a new folder from the picker through setup, first plan, and first task", async () => {
    harness = await bootApp({ responses: journeyResponses() });
    await harness.click("#state-initialize-project-button");
    expect(visibleSteps(harness)).toContain("setup-goal-form");
    // Steps not on screen are out of the tab order too.
    expect(harness.$("#setup-review").inert).toBe(true);
    expect(harness.$("#setup-goal-form").inert).toBe(false);
    await harness.type("#setup-goal", goal);
    await harness.submit("#setup-goal-form");
    expect(visibleSteps(harness)).toContain("setup-guide");
    await harness.click("#setup-guide-skip");
    expect(visibleSteps(harness)).toContain("setup-review");
    expect(harness.$("#setup-review-goal").textContent).toBe(goal);
    await harness.click("#setup-commit");
    expect(visibleSteps(harness)).toContain("onboarding-plan-form");
    await harness.type("#onboarding-plan-title", "Launch plan");
    await harness.submit("#onboarding-plan-form");
    expect(visibleSteps(harness)).toContain("onboarding-task-form");
    await harness.type("#onboarding-task-title", "Ship the first slice");
    harness.$("#onboarding-start-now").checked = true;
    await harness.submit("#onboarding-task-form");

    expect(journeyCalls(harness)).toEqual([
      ["PickProjectDirectory", ["initialize"]],
      ["ValidateProjectTargetV1", [canonicalRoot]],
      ["InitializeProjectV1", [request]],
      ["CreateFirstPlanV1", [7, "Launch plan"]],
      ["CreateFirstTaskV1", [7, 11, "Ship the first slice"]],
      ["StartFirstTaskV1", [7, 21, createdAt]],
    ]);
    expect(shown(harness.$("#post-project-onboarding"))).toBe(false);
    expect(shown(harness.$("#board"))).toBe(true);
    expect(harness.toast()).toBe("");
  });

  it("opens an already initialized folder only through the explicit open action", async () => {
    harness = await bootApp({
      responses: journeyResponses({
        ValidateProjectTargetV1: { kind: "existing", canonicalRoot, operationId: "" },
        OpenProject: {
          state: openState(4),
          requiresConfirmation: false,
          confirmationToken: "",
        },
        GetWorkspaceSnapshot: (generation) => snapshot(board(), generation),
      }),
    });
    await harness.click("#state-initialize-project-button");
    expect(visibleSteps(harness)).toContain("setup-existing-actions");
    expect(journeyCalls(harness).map(([method]) => method))
      .toEqual(["PickProjectDirectory", "ValidateProjectTargetV1"]);
    await harness.click("#setup-open-existing");
    expect(journeyCalls(harness).at(-1)).toEqual(["OpenProject", [canonicalRoot, ""]]);
    expect(harness.backend.names()).not.toContain("InitializeProjectV1");
    expect(shown(harness.$("#board"))).toBe(true);
  });

  it("leaves nothing written when the folder picker is cancelled", async () => {
    harness = await bootApp({ responses: journeyResponses({ PickProjectDirectory: "" }) });
    await harness.click("#state-initialize-project-button");
    expect(journeyCalls(harness).map(([method]) => method)).toEqual(["PickProjectDirectory"]);
    expect(shown(harness.$("#setup-panel"))).toBe(false);
    expect(harness.document.activeElement.id).toBe("state-initialize-project-button");
  });

  it("reconciles a lost initialization reply by reading its status, never by committing again", async () => {
    let statusReads = 0;
    harness = await bootApp({
      responses: journeyResponses({
        InitializeProjectV1: new Error("initialization response was lost"),
        GetInitializationStatusV1: () => {
          statusReads += 1;
          if (statusReads === 1) throw new Error("status temporarily unavailable");
          return {
            operationId,
            canonicalRoot,
            outcome: "recovery-required",
            checkpoint: "project-committed",
            errorKind: "initialization-failed",
          };
        },
      }),
    });
    await reachReview(harness);
    await harness.click("#setup-commit");
    expect(visibleSteps(harness)).toContain("setup-uncertain-actions");
    await harness.click("#setup-check-status");
    expect(visibleSteps(harness)).toContain("setup-recovery-actions");
    expect(journeyCalls(harness).map(([method]) => method)).toEqual([
      "PickProjectDirectory",
      "ValidateProjectTargetV1",
      "InitializeProjectV1",
      "GetInitializationStatusV1",
      "GetInitializationStatusV1",
    ]);
  });

  it("revalidates a no-write failure, then retries the very same request", async () => {
    const noWrite = {
      operationId,
      canonicalRoot,
      outcome: "ready",
      checkpoint: "none",
      errorKind: "interrupted-before-commit",
    };
    let validations = 0;
    let commits = 0;
    harness = await bootApp({
      responses: journeyResponses({
        ValidateProjectTargetV1: () => {
          validations += 1;
          return validations === 1
            ? { kind: "new", canonicalRoot, operationId }
            : { kind: "new", canonicalRoot, operationId, initialization: noWrite, goal, guideChoice: "skip" };
        },
        InitializeProjectV1: () => {
          commits += 1;
          if (commits === 1) throw new Error("filesystem access changed before commit");
          return { initialization: complete, state: openState(9) };
        },
        GetInitializationStatusV1: noWrite,
      }),
    });
    await reachReview(harness);
    await harness.click("#setup-commit");
    expect(harness.$("#setup-retry").hidden).toBe(false);
    expect(harness.$("#setup-error").textContent)
      .toBe("No project files were written by this attempt. Retry checks the folder again before any write.");
    await harness.click("#setup-retry");
    expect(visibleSteps(harness)).toContain("setup-review");
    expect(harness.$("#setup-detail").textContent)
      .toBe("No project files were written. You can try again safely. Confirm to resume the same operation.");
    await harness.click("#setup-commit");
    const commitsSent = journeyCalls(harness).filter(([method]) => method === "InitializeProjectV1");
    expect(commitsSent).toEqual([["InitializeProjectV1", [request]], ["InitializeProjectV1", [request]]]);
    expect(journeyCalls(harness).map(([method]) => method)).toEqual([
      "PickProjectDirectory",
      "ValidateProjectTargetV1",
      "InitializeProjectV1",
      "GetInitializationStatusV1",
      "ValidateProjectTargetV1",
      "InitializeProjectV1",
    ]);
    expect(visibleSteps(harness)).toContain("onboarding-plan-form");
  });

  it("restores a resumable setup from the startup journal at its review step", async () => {
    const durable = {
      operationId,
      canonicalRoot,
      outcome: "in-progress",
      checkpoint: "project-committed",
      errorKind: "",
    };
    harness = await bootApp({
      responses: journeyResponses({
        GetPendingInitializationV1: {
          pending: true,
          initialization: durable,
          validation: { kind: "new", canonicalRoot, operationId, initialization: durable, goal, guideChoice: "skip" },
        },
      }),
    });
    expect(visibleSteps(harness)).toContain("setup-review");
    expect(harness.$("#setup-review-goal").textContent).toBe(goal);
    expect(harness.backend.names()).not.toContain("InitializeProjectV1");
    // Confirming resumes the same operation with the same request, once.
    await harness.click("#setup-commit");
    expect(journeyCalls(harness)).toEqual([["InitializeProjectV1", [request]]]);
    expect(visibleSteps(harness)).toContain("onboarding-plan-form");
  });

  it("shows a blocked setup from the startup journal as recovery", async () => {
    harness = await bootApp({
      responses: journeyResponses({
        GetPendingInitializationV1: {
          pending: true,
          initialization: {
            operationId,
            canonicalRoot,
            outcome: "recovery-required",
            checkpoint: "runtime-committed",
            errorKind: "recovery-required",
          },
          validation: { kind: "recovery-required", canonicalRoot, operationId: "", reason: "Storage needs manual recovery." },
        },
      }),
    });
    expect(visibleSteps(harness)).toContain("setup-recovery-actions");
    // A blocked setup cannot resume or open; it points at help and elsewhere.
    expect(shown(harness.$("#setup-resume"))).toBe(false);
    expect(shown(harness.$("#setup-open-recovery"))).toBe(false);
    expect(shown(harness.$("#setup-recovery-help"))).toBe(true);
    expect(shown(harness.$("#setup-recovery-choose"))).toBe(true);
    expect(harness.backend.names()).not.toContain("InitializeProjectV1");
  });

  it("previews the exact guide changes as text and commits the reviewed preview", async () => {
    const diff = "+<script>alert(1)</script>\n+Use ptrack for plans.";
    harness = await bootApp({
      responses: journeyResponses({
        PreviewProjectGuideV1: {
          available: true,
          message: "",
          previewToken: "preview-1",
          files: [
            { path: "AGENTS.md", action: "create", additions: 2, deletions: 0, diff },
            { path: "CLAUDE.md", action: "no-change", additions: 0, deletions: 0, diff: "" },
          ],
        },
      }),
    });
    await harness.click("#state-initialize-project-button");
    await harness.type("#setup-goal", goal);
    await harness.submit("#setup-goal-form");
    await harness.click("#setup-guide-preview-button");
    expect(harness.backend.callsTo("PreviewProjectGuideV1")).toEqual([[{ operationId, root: canonicalRoot }]]);
    const code = harness.$("#setup-guide-files code");
    expect(code.textContent).toBe(diff);
    expect(code.children).toHaveLength(0);
    expect(harness.$$("#setup-guide-files .setup-guide-counts").map((node) => node.textContent))
      .toEqual(["Create · +2 −0", "No change"]);
    await harness.click("#setup-guide-install");
    await harness.click("#setup-commit");
    expect(harness.backend.callsTo("InitializeProjectV1")).toEqual([[
      { ...request, guideChoice: "install", guidePreviewToken: "preview-1" },
    ]]);
  });

  it("keeps a typed goal when stepping back to the folder and forward again", async () => {
    harness = await bootApp({ responses: journeyResponses() });
    await harness.click("#state-initialize-project-button");
    await harness.type("#setup-goal", "Half-written goal");
    await harness.click("#setup-goal-back");
    expect(visibleSteps(harness)).toContain("setup-new-target-actions");
    await harness.click("#setup-new-target-continue");
    expect(harness.$("#setup-goal").value).toBe("Half-written goal");
  });

  it("returns to the step it came from when choosing another folder is cancelled", async () => {
    let picks = 0;
    harness = await bootApp({
      responses: journeyResponses({ PickProjectDirectory: () => (picks += 1) === 1 ? canonicalRoot : "" }),
    });
    await harness.click("#state-initialize-project-button");
    await harness.click("#setup-goal-back");
    await harness.click("#setup-new-target-choose");
    expect(picks).toBe(2);
    expect(visibleSteps(harness)).toContain("setup-new-target-actions");
    expect(harness.document.activeElement.id).toBe("setup-new-target-choose");
    expect(harness.backend.callsTo("ValidateProjectTargetV1")).toHaveLength(1);
  });

  it("cannot cancel setup while the commit is in flight", async () => {
    let finish;
    harness = await bootApp({
      responses: journeyResponses({
        InitializeProjectV1: () => new Promise((resolve) => { finish = resolve; }),
      }),
    });
    await reachReview(harness);
    await harness.click("#setup-commit");
    expect(harness.$("#setup-operation").getAttribute("aria-busy")).toBe("true");
    await harness.click("#setup-review-cancel");
    expect(shown(harness.$("#setup-panel"))).toBe(true);
    finish({ initialization: complete, state: openState(7) });
    await harness.settle();
    expect(visibleSteps(harness)).toContain("onboarding-plan-form");
  });

  it("keeps a completed setup recoverable when this window cannot open the project", async () => {
    let opens = 0;
    harness = await bootApp({
      responses: journeyResponses({
        InitializeProjectV1: { initialization: complete },
        OpenProject: () => {
          opens += 1;
          if (opens === 1) throw new Error("window is busy");
          return { state: openState(8), requiresConfirmation: false, confirmationToken: "" };
        },
      }),
    });
    await reachReview(harness);
    await harness.click("#setup-commit");
    expect(visibleSteps(harness)).toContain("setup-recovery-actions");
    expect(harness.$("#setup-detail").textContent)
      .toBe("Initialization is complete, but this window could not open the project: window is busy");
    await harness.click("#setup-recovery-help");
    expect(harness.backend.callsTo("OpenHelpDestination")).toEqual([["project-recovery"]]);
    expect(shown(harness.$("#setup-open-recovery"))).toBe(true);
    await harness.click("#setup-open-recovery");
    expect(harness.backend.callsTo("OpenProject").at(-1)).toEqual([canonicalRoot, ""]);
    expect(harness.backend.callsTo("InitializeProjectV1")).toHaveLength(1);
    expect(shown(harness.$("#setup-panel"))).toBe(false);
  });

  it("says the desktop startup state could not be read after its retries run out", async () => {
    harness = await bootApp({
      responses: { GetWorkspaceState: new Error("runtime is not up") },
      // The startup retries wait 100 ms apiece; run them at once.
      beforeStart: (dom) => {
        const run = dom.window.setTimeout;
        dom.window.setTimeout = (callback, delay) => run(callback, delay === 100 ? 0 : delay);
      },
    });
    await harness.settle(80);
    expect(harness.backend.callsTo("GetWorkspaceState")).toHaveLength(50);
    expect(harness.$("#project-state-card").textContent)
      .toContain("Could not load the desktop startup state: runtime is not up");
  });

  it("resumes a preserved setup by reading its status, never by committing again", async () => {
    let opens = 0;
    harness = await bootApp({
      responses: journeyResponses({
        InitializeProjectV1: { initialization: complete },
        OpenProject: () => {
          opens += 1;
          if (opens === 1) throw new Error("window is busy");
          return { state: openState(8), requiresConfirmation: false, confirmationToken: "" };
        },
      }),
    });
    await reachReview(harness);
    await harness.click("#setup-commit");
    expect(shown(harness.$("#setup-resume"))).toBe(true);
    const before = journeyCalls(harness).length;
    await harness.click("#setup-resume");
    expect(journeyCalls(harness).slice(before)[0]).toEqual(["GetInitializationStatusV1", [operationId]]);
    expect(harness.backend.callsTo("InitializeProjectV1")).toHaveLength(1);
    expect(shown(harness.$("#setup-panel"))).toBe(false);
  });

  it("treats a completion that never bound the desktop workspace as uncertain", async () => {
    harness = await bootApp({
      responses: journeyResponses({
        InitializeProjectV1: {
          initialization: { ...complete, checkpoint: "guide-applied" },
          state: openState(7),
        },
        GetInitializationStatusV1: { ...complete, checkpoint: "guide-applied" },
      }),
    });
    await reachReview(harness);
    await harness.click("#setup-commit");
    expect(visibleSteps(harness)).not.toContain("onboarding-plan-form");
    expect(visibleSteps(harness)).toContain("setup-uncertain-actions");
    expect(harness.$("#setup-heading").textContent).toBe("Keep this operation open");
    expect(harness.backend.callsTo("InitializeProjectV1")).toHaveLength(1);
  });
});

