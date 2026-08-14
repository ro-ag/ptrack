import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it, vi } from "vitest";

import { installTauriBridge } from "./tauri-bridge";
import {
  firstRunFocusTarget,
  initialFirstRunState,
  pendingInitializationEvent,
  projectGuideCommitFields,
  reduceFirstRun,
  resolveFirstRunStartupState,
} from "./workspace/first-run";
import {
  createFirstPlan,
  createFirstTask,
  initialFirstPlanState,
  reduceFirstPlan,
  startFirstTask,
} from "./workspace/first-plan";
import {
  commitInitialization,
  initializeProjectRequest,
  openExactProject,
  readInitializationStatus,
  resumeInitialization,
  validateInitializationTarget,
} from "./workspace/first-run-journey";

const frontendRoot = resolve(import.meta.dirname, "..");
const indexSource = readFileSync(resolve(frontendRoot, "index.html"), "utf8");
const appSource = readFileSync(resolve(frontendRoot, "src/app.js"), "utf8");
const operationId = "o".repeat(43);
const canonicalRoot = "/projects/alpha";
const createdAt = "2026-08-14T12:00:00Z";

function journeyBridge(resolveMethod) {
  const calls = [];
  const target = { __TAURI_INTERNALS__: {}, navigator: { clipboard: {} } };
  installTauriBridge(target, {
    invoke: vi.fn(async (command, payload) => {
      calls.push([command, payload]);
      if (command === "pick_project_directory") {
        return resolveMethod("PickProjectDirectory", [payload.purpose]);
      }
      if (command !== "gui_invoke") throw new Error(`unexpected ${command}`);
      return resolveMethod(payload.request.method, payload.request.arguments);
    }),
    listen: vi.fn(),
    clipboard: { readText: vi.fn(), writeText: vi.fn() },
  });
  return { api: target.go.gui.App, calls };
}

function methodNames(calls) {
  return calls.map(([command, payload]) =>
    command === "pick_project_directory"
      ? "PickProjectDirectory"
      : payload.request.method
  );
}

describe("first-run journey smoke coverage", () => {
  it("boots a true first launch through the bridge and renders the two primary journeys", async () => {
    let workspaceReads = 0;
    const { api, calls } = journeyBridge((method) => {
      if (method === "GetWorkspaceState") {
        workspaceReads += 1;
        return { status: "welcome", generation: 0 };
      }
      if (method === "GetPendingInitializationV1") return { pending: false };
      throw new Error(`unexpected ${method}`);
    });

    const startup = await resolveFirstRunStartupState(
      () => api.GetWorkspaceState(),
      () => api.GetPendingInitializationV1(),
    );

    expect(startup).toEqual({
      state: { status: "welcome", generation: 0 },
      pending: { pending: false, initialization: null, validation: null },
    });
    expect(workspaceReads).toBe(2);
    expect(methodNames(calls)).toEqual([
      "GetWorkspaceState",
      "GetPendingInitializationV1",
      "GetWorkspaceState",
    ]);
    expect(calls.map(([, payload]) => payload.request.arguments)).toEqual([
      [],
      [],
      [],
    ]);
    expect(indexSource.match(/class="state-card"/g)).toHaveLength(1);
    expect(indexSource).toMatch(
      /id="state-initialize-project-button"[^>]*>Initialize Project<\/button>[\s\S]*id="state-open-project-button"[\s\S]*>Open Project…<\/button>/,
    );
    expect(appSource).toMatch(
      /resolveFirstRunStartupState\([\s\S]*GetWorkspaceState\(\)[\s\S]*GetPendingInitializationV1\(\)[\s\S]*hydratePendingInitialization/,
    );
  });

  it("runs a new project from typed validation through first plan, task, and start receipts", async () => {
    const responses = {
      ValidateProjectTargetV1: {
        kind: "new",
        canonicalRoot,
        operationId,
      },
      InitializeProjectV1: {
        initialization: {
          operationId,
          canonicalRoot,
          outcome: "complete",
          checkpoint: "desktop-bound",
          errorKind: "",
        },
        state: {
          status: "open",
          generation: 7,
          project: { root: canonicalRoot, name: "alpha" },
        },
      },
      CreateFirstPlanV1: {
        plan: {
          id: 11,
          title: "Launch plan",
          status: "active",
          createdAt,
          updatedAt: createdAt,
        },
        state: { status: "open", generation: 7 },
      },
      CreateFirstTaskV1: {
        task: {
          id: 21,
          planId: 11,
          title: "Ship the first slice",
          status: "todo",
          createdAt,
          updatedAt: createdAt,
        },
        state: { status: "open", generation: 7 },
      },
      StartFirstTaskV1: {
        task: {
          id: 21,
          planId: 11,
          title: "Ship the first slice",
          status: "doing",
          createdAt,
          updatedAt: "2026-08-14T12:01:00Z",
        },
        state: { status: "open", generation: 7 },
      },
    };
    const { api, calls } = journeyBridge((method) => responses[method]);

    const validation = await validateInitializationTarget(api, canonicalRoot);
    let setup = reduceFirstRun(initialFirstRunState, { type: "validate" });
    setup = reduceFirstRun(setup, {
      type: "new",
      canonicalRoot: validation.canonicalRoot,
      operationId: validation.operationId,
    });
    setup = reduceFirstRun(setup, {
      type: "goalAccepted",
      goal: "Make first launch trustworthy",
    });
    setup = reduceFirstRun(setup, { type: "guideSkipped" });
    const guide = projectGuideCommitFields(setup);
    setup = reduceFirstRun(setup, { type: "commit" });
    const initializeRequest = initializeProjectRequest(
      setup.operationId,
      setup.canonicalRoot,
      setup.goal,
      guide,
    );
    const initialized = await commitInitialization(api, initializeRequest);

    expect(setup).toMatchObject({
      phase: "committing",
      canonicalRoot,
      operationId,
      goal: "Make first launch trustworthy",
      guideChoice: "skip",
      resumeLocked: true,
    });
    expect(initialized.kind).toBe("result");
    expect(initialized.result.status).toMatchObject({
      outcome: "complete",
      checkpoint: "desktop-bound",
    });

    let onboarding = reduceFirstPlan(initialFirstPlanState, {
      type: "begin",
      generation: 7,
    });
    onboarding = reduceFirstPlan(onboarding, {
      type: "createPlan",
      title: "Launch plan",
    });
    const plan = await createFirstPlan(
      api,
      7,
      "Launch plan",
    );
    onboarding = reduceFirstPlan(onboarding, {
      type: "planCreated",
      planId: plan.plan.id,
      title: plan.plan.title,
    });
    onboarding = reduceFirstPlan(onboarding, {
      type: "createTask",
      title: "Ship the first slice",
      startNow: true,
    });
    const task = await createFirstTask(
      api,
      7,
      plan.plan.id,
      "Ship the first slice",
    );
    onboarding = reduceFirstPlan(onboarding, {
      type: "taskCreated",
      taskId: task.task.id,
      updatedAt: task.task.updatedAt,
      status: task.task.status,
    });
    await startFirstTask(
      api,
      7,
      plan.plan.id,
      task.task.id,
      task.task.title,
      task.task.updatedAt,
    );
    onboarding = reduceFirstPlan(onboarding, { type: "taskStarted" });

    expect(onboarding.phase).toBe("complete");
    expect(methodNames(calls)).toEqual([
      "ValidateProjectTargetV1",
      "InitializeProjectV1",
      "CreateFirstPlanV1",
      "CreateFirstTaskV1",
      "StartFirstTaskV1",
    ]);
    expect(calls.map(([, payload]) => payload.request.arguments)).toEqual([
      [canonicalRoot],
      [{
        operationId,
        root: canonicalRoot,
        goal: "Make first launch trustworthy",
        guideChoice: "skip",
        guidePreviewToken: "",
      }],
      [7, "Launch plan"],
      [7, 11, "Ship the first slice"],
      [7, 21, createdAt],
    ]);
    expect(calls[1][1].request.arguments[0]).toEqual({
      operationId,
      root: canonicalRoot,
      goal: "Make first launch trustworthy",
      guideChoice: "skip",
      guidePreviewToken: "",
    });
    expect(indexSource).toMatch(
      /id="setup-goal-form"[\s\S]*id="setup-guide"[\s\S]*id="setup-review"[\s\S]*id="post-project-onboarding"[\s\S]*id="onboarding-plan-form"[\s\S]*id="onboarding-task-form"/,
    );
    expect(appSource).toMatch(
      /setupCommit\.addEventListener[\s\S]*onboardingPlanForm\.addEventListener[\s\S]*onboardingTaskForm\.addEventListener/,
    );
  });

  it("keeps existing-project discovery on the explicit open path", async () => {
    const selectedDescendant = `${canonicalRoot}/nested/folder`;
    const { api, calls } = journeyBridge((method) => {
      if (method === "ValidateProjectTargetV1") {
        return { kind: "existing", canonicalRoot, operationId: "" };
      }
      if (method === "OpenProject") {
        return {
          state: {
            status: "open",
            generation: 4,
            project: { root: canonicalRoot, name: "alpha" },
          },
          requiresConfirmation: false,
          confirmationToken: "",
        };
      }
      throw new Error(`unexpected ${method}`);
    });

    const validation = await validateInitializationTarget(
      api,
      selectedDescendant,
    );
    const existing = reduceFirstRun(
      reduceFirstRun(initialFirstRunState, { type: "validate" }),
      { type: "existing", canonicalRoot: validation.canonicalRoot },
    );
    const opened = await openExactProject(
      api,
      existing.canonicalRoot,
      async () => "abort",
      () => {},
    );

    expect(existing).toMatchObject({
      phase: "existing",
      canonicalRoot,
      operationId: "",
    });
    expect(opened).toMatchObject({
      kind: "opened",
      result: {
        requiresConfirmation: false,
        state: { status: "open", generation: 4, project: { root: canonicalRoot } },
      },
    });
    expect(methodNames(calls)).toEqual([
      "ValidateProjectTargetV1",
      "OpenProject",
    ]);
    expect(calls[1][1].request.arguments).toEqual([canonicalRoot, ""]);
    expect(calls[0][1].request.arguments).toEqual([selectedDescendant]);
    expect(indexSource).toMatch(
      /id="setup-existing-actions"[\s\S]*id="setup-open-existing"[^>]*>Open Existing Project<\/button>/,
    );
    expect(appSource).toMatch(
      /setupOpenExisting\.addEventListener\("click", \(\) => void openExistingFromSetup\(\)\)/,
    );
  });

  it("cancels stale open confirmations and re-prompts when authority changes", async () => {
    const firstToken = "confirmation-one";
    const secondToken = "confirmation-two";
    let openCalls = 0;
    const { api, calls } = journeyBridge((method) => {
      if (method === "OpenProject") {
        openCalls += 1;
        if (openCalls <= 2) {
          return {
            state: { status: "open", generation: openCalls },
            requiresConfirmation: true,
            confirmationToken: openCalls === 1 ? firstToken : secondToken,
            activeResources: { terminals: 1, agentRuns: 0, pendingAdmissions: 0 },
          };
        }
        return {
          state: { status: "open", generation: 3 },
          requiresConfirmation: false,
          confirmationToken: "",
        };
      }
      throw new Error(`unexpected ${method}`);
    });
    const decisions = [];
    let transitions = 0;
    const opened = await openExactProject(
      api,
      canonicalRoot,
      async (result) => {
        decisions.push(result.confirmationToken);
        return "confirm";
      },
      () => {
        transitions += 1;
      },
    );

    expect(opened).toMatchObject({
      kind: "opened",
      result: { requiresConfirmation: false, state: { generation: 3 } },
    });
    expect(decisions).toEqual([firstToken, secondToken]);
    expect(transitions).toBe(2);
    expect(calls.map(([, payload]) => payload.request.arguments)).toEqual([
      [canonicalRoot, ""],
      [canonicalRoot, firstToken],
      [canonicalRoot, secondToken],
    ]);

    const aborted = journeyBridge((method) => {
      if (method === "OpenProject") {
        return {
          state: { status: "open", generation: 4 },
          requiresConfirmation: true,
          confirmationToken: firstToken,
          activeResources: { terminals: 1, agentRuns: 0, pendingAdmissions: 0 },
        };
      }
      if (method === "CancelWorkspaceChange") return { cancelled: true };
      throw new Error(`unexpected ${method}`);
    });
    const abortedResult = await openExactProject(
      aborted.api,
      canonicalRoot,
      async () => "abort",
      () => {},
    );
    expect(abortedResult.kind).toBe("aborted");
    expect(methodNames(aborted.calls)).toEqual([
      "OpenProject",
      "CancelWorkspaceChange",
    ]);
    expect(aborted.calls[1][1].request.arguments).toEqual([firstToken]);

    const invalid = journeyBridge((method) => {
      if (method === "OpenProject") {
        return {
          state: { status: "open", generation: 4 },
          requiresConfirmation: true,
          confirmationToken: "",
          activeResources: { terminals: 1, agentRuns: 0, pendingAdmissions: 0 },
        };
      }
      throw new Error(`unexpected ${method}`);
    });
    await expect(openExactProject(
      invalid.api,
      canonicalRoot,
      async () => "confirm",
      () => {},
    )).rejects.toThrow("missing its token");

    let changingToken = 0;
    const changing = journeyBridge((method) => {
      if (method === "OpenProject") {
        changingToken += 1;
        return {
          state: { status: "open", generation: changingToken },
          requiresConfirmation: true,
          confirmationToken: `changing-${changingToken}`,
          activeResources: { terminals: 1, agentRuns: 0, pendingAdmissions: 0 },
        };
      }
      if (method === "CancelWorkspaceChange") return { cancelled: true };
      throw new Error(`unexpected ${method}`);
    });
    await expect(openExactProject(
      changing.api,
      canonicalRoot,
      async () => "confirm",
      () => {},
    )).rejects.toThrow("changed too many times");
    expect(methodNames(changing.calls)).toEqual([
      "OpenProject",
      "OpenProject",
      "OpenProject",
      "OpenProject",
      "CancelWorkspaceChange",
    ]);
    expect(changing.calls.at(-1)[1].request.arguments).toEqual(["changing-4"]);
  });

  it("hydrates resumable and blocked recovery from the authoritative startup journal", async () => {
    async function hydrate(pendingResponse) {
      const { api, calls } = journeyBridge((method) => {
        if (method === "GetWorkspaceState") {
          return { status: "welcome", generation: 0 };
        }
        if (method === "GetPendingInitializationV1") return pendingResponse;
        if (method === "GetInitializationStatusV1") {
          return pendingResponse.initialization;
        }
        if (method === "ValidateProjectTargetV1") {
          return pendingResponse.validation;
        }
        if (method === "InitializeProjectV1") {
          return {
            initialization: {
              ...pendingResponse.initialization,
              outcome: "complete",
              checkpoint: "desktop-bound",
              errorKind: "",
            },
            state: {
              status: "open",
              generation: 8,
              project: { root: canonicalRoot, name: "alpha" },
            },
          };
        }
        throw new Error(`unexpected ${method}`);
      });
      const startup = await resolveFirstRunStartupState(
        () => api.GetWorkspaceState(),
        () => api.GetPendingInitializationV1(),
      );
      const event = pendingInitializationEvent(startup.pending);
      return {
        state: reduceFirstRun(initialFirstRunState, event),
        api,
        calls,
      };
    }

    const durableStatus = {
      operationId,
      canonicalRoot,
      outcome: "in-progress",
      checkpoint: "project-committed",
      errorKind: "",
    };
    const resumable = await hydrate({
      pending: true,
      initialization: durableStatus,
      validation: {
        kind: "new",
        canonicalRoot,
        operationId,
        initialization: durableStatus,
        goal: "Make first launch trustworthy",
        guideChoice: "skip",
      },
    });
    const blockedStatus = {
      ...durableStatus,
      outcome: "recovery-required",
      checkpoint: "runtime-committed",
      errorKind: "recovery-required",
    };
    const blocked = await hydrate({
      pending: true,
      initialization: blockedStatus,
      validation: {
        kind: "recovery-required",
        canonicalRoot,
        operationId: "",
        reason: "Storage needs manual recovery.",
      },
    });

    expect(resumable.state).toMatchObject({
      phase: "review",
      canonicalRoot,
      operationId,
      checkpoint: "project-committed",
      goal: "Make first launch trustworthy",
      guideChoice: "skip",
      recoveryMode: "durable",
      resumeLocked: true,
    });
    expect(blocked.state).toMatchObject({
      phase: "recovery",
      canonicalRoot,
      operationId,
      checkpoint: "runtime-committed",
      recoveryMode: "blocked",
      resumeLocked: true,
    });
    const recovery = await resumeInitialization(
      resumable.api,
      operationId,
      canonicalRoot,
    );
    expect(recovery.kind).toBe("validation");
    expect(recovery.validation).toMatchObject({
      kind: "new",
      operationId,
      canonicalRoot,
      resume: {
        goal: "Make first launch trustworthy",
        guideChoice: "skip",
      },
    });
    const resumedGuide = projectGuideCommitFields(resumable.state);
    const resumedRequest = initializeProjectRequest(
      operationId,
      canonicalRoot,
      resumable.state.goal,
      resumedGuide,
    );
    const resumed = await commitInitialization(resumable.api, resumedRequest);
    expect(resumed.kind).toBe("result");
    expect(resumed.result.status).toMatchObject({
      outcome: "complete",
      checkpoint: "desktop-bound",
    });
    expect(methodNames(resumable.calls)).toEqual([
      "GetWorkspaceState",
      "GetPendingInitializationV1",
      "GetWorkspaceState",
      "GetInitializationStatusV1",
      "ValidateProjectTargetV1",
      "InitializeProjectV1",
    ]);
    expect(resumable.calls.slice(3).map(([, payload]) => payload.request.arguments))
      .toEqual([
        [operationId],
        [canonicalRoot],
        [{
          operationId,
          root: canonicalRoot,
          goal: "Make first launch trustworthy",
          guideChoice: "skip",
          guidePreviewToken: "",
        }],
      ]);
    expect(indexSource).toMatch(
      /id="setup-recovery-actions"[\s\S]*id="setup-resume"[\s\S]*id="setup-open-recovery"[\s\S]*id="setup-recovery-help"/,
    );
    expect(appSource).toMatch(
      /hydratePendingInitialization\(pending\)[\s\S]*pendingInitializationEvent\(pending\)/,
    );
  });

  it("keeps picker cancellation mutation-free and reconciles uncertain transport by status only", async () => {
    let statusReads = 0;
    const { api, calls } = journeyBridge((method) => {
      if (method === "PickProjectDirectory") return "";
      if (method === "InitializeProjectV1") {
        throw new Error("initialization response was lost");
      }
      if (method === "GetInitializationStatusV1") {
        statusReads += 1;
        if (statusReads === 1) throw new Error("status temporarily unavailable");
        return {
          operationId,
          canonicalRoot,
          outcome: "recovery-required",
          checkpoint: "project-committed",
          errorKind: "initialization-failed",
        };
      }
      throw new Error(`unexpected ${method}`);
    });

    let picker = reduceFirstRun(initialFirstRunState, {
      type: "pick",
      intent: "initialize",
      returnFocusId: "state-initialize-project-button",
    });
    const selected = await api.PickProjectDirectory("initialize");
    if (!selected) picker = reduceFirstRun(picker, { type: "pickerCancelled" });
    expect(picker.phase).toBe("idle");
    expect(firstRunFocusTarget(picker)).toBe("state-initialize-project-button");
    expect(methodNames(calls)).toEqual(["PickProjectDirectory"]);

    let setup = reduceFirstRun(initialFirstRunState, {
      type: "new",
      canonicalRoot,
      operationId,
    });
    setup = reduceFirstRun(setup, {
      type: "goalAccepted",
      goal: "Make first launch trustworthy",
    });
    setup = reduceFirstRun(setup, { type: "guideSkipped" });
    const guide = projectGuideCommitFields(setup);
    const request = initializeProjectRequest(
      operationId,
      canonicalRoot,
      setup.goal,
      guide,
    );
    setup = reduceFirstRun(setup, { type: "commit" });
    const commit = await commitInitialization(api, request);
    expect(commit.kind).toBe("uncertain");
    setup = reduceFirstRun(setup, {
      type: "uncertain",
      message: commit.statusError.message,
      checkpoint: setup.checkpoint,
    });
    expect(setup).toMatchObject({
      phase: "uncertain",
      operationId,
      canonicalRoot,
      resumeLocked: true,
    });

    setup = reduceFirstRun(setup, { type: "reconcile" });
    const status = await readInitializationStatus(api, operationId);
    setup = reduceFirstRun(setup, {
      type: "recovery",
      canonicalRoot,
      operationId,
      message: "Durable setup requires recovery.",
      checkpoint: status.checkpoint,
      errorKind: status.errorKind,
      durable: true,
    });

    expect(setup).toMatchObject({
      phase: "recovery",
      checkpoint: "project-committed",
      recoveryMode: "durable",
      resumeLocked: true,
    });
    expect(methodNames(calls)).toEqual([
      "PickProjectDirectory",
      "InitializeProjectV1",
      "GetInitializationStatusV1",
      "GetInitializationStatusV1",
    ]);
    expect(methodNames(calls).filter((method) => method === "InitializeProjectV1"))
      .toHaveLength(1);
    expect(calls[0]).toEqual([
      "pick_project_directory",
      { purpose: "initialize" },
    ]);
    expect(appSource).toContain("commitInitialization(api(), request)");
    expect(appSource).toMatch(
      /async function retryInitializationStatus\(\)[\s\S]*setFirstRunState\(\{ type: "reconcile" \}[\s\S]*reconcileInitializationStatus/,
    );
  });

  it("revalidates an authoritative no-write failure before retrying the same request", async () => {
    let validationReads = 0;
    let initializationCalls = 0;
    const noWriteStatus = {
      operationId,
      canonicalRoot,
      outcome: "ready",
      checkpoint: "none",
      errorKind: "interrupted-before-commit",
    };
    const completed = {
      initialization: {
        ...noWriteStatus,
        outcome: "complete",
        checkpoint: "desktop-bound",
        errorKind: "",
      },
      state: {
        status: "open",
        generation: 9,
        project: { root: canonicalRoot, name: "alpha" },
      },
    };
    const { api, calls } = journeyBridge((method) => {
      if (method === "ValidateProjectTargetV1") {
        validationReads += 1;
        if (validationReads === 1) {
          return { kind: "new", canonicalRoot, operationId };
        }
        return {
          kind: "new",
          canonicalRoot,
          operationId,
          initialization: noWriteStatus,
          goal: "Make first launch trustworthy",
          guideChoice: "skip",
        };
      }
      if (method === "InitializeProjectV1") {
        initializationCalls += 1;
        if (initializationCalls === 1) {
          throw new Error("filesystem access changed before commit");
        }
        return completed;
      }
      if (method === "GetInitializationStatusV1") return noWriteStatus;
      throw new Error(`unexpected ${method}`);
    });

    const fresh = await validateInitializationTarget(api, canonicalRoot);
    let setup = reduceFirstRun(initialFirstRunState, {
      type: "new",
      canonicalRoot: fresh.canonicalRoot,
      operationId: fresh.operationId,
    });
    setup = reduceFirstRun(setup, {
      type: "goalAccepted",
      goal: "Make first launch trustworthy",
    });
    setup = reduceFirstRun(setup, { type: "guideSkipped" });
    const firstRequest = initializeProjectRequest(
      operationId,
      canonicalRoot,
      setup.goal,
      projectGuideCommitFields(setup),
    );
    setup = reduceFirstRun(setup, { type: "commit" });
    const firstCommit = await commitInitialization(api, firstRequest);
    expect(firstCommit.kind).toBe("status");
    setup = reduceFirstRun(setup, {
      type: "failed",
      canonicalRoot,
      operationId,
      message: firstCommit.error.message,
      checkpoint: firstCommit.status.checkpoint,
      errorKind: firstCommit.status.errorKind,
    });
    expect(setup).toMatchObject({
      phase: "failed",
      operationId,
      canonicalRoot,
      goal: "Make first launch trustworthy",
      recoveryMode: "no-write",
      resumeNoWrite: true,
    });

    const revalidated = await validateInitializationTarget(api, canonicalRoot);
    setup = reduceFirstRun(setup, {
      type: "resume",
      canonicalRoot: revalidated.canonicalRoot,
      operationId: revalidated.operationId,
      goal: revalidated.resume.goal,
      guideChoice: revalidated.resume.guideChoice,
      initialization: revalidated.resume.initialization,
    });
    expect(setup).toMatchObject({
      phase: "review",
      operationId,
      canonicalRoot,
      goal: "Make first launch trustworthy",
      guideChoice: "skip",
      resumeNoWrite: true,
    });
    setup = reduceFirstRun(setup, { type: "commit" });
    const retryRequest = initializeProjectRequest(
      setup.operationId,
      setup.canonicalRoot,
      setup.goal,
      projectGuideCommitFields(setup),
    );
    const result = await commitInitialization(api, retryRequest);

    expect(result.kind).toBe("result");
    expect(result.result.status).toMatchObject({
      outcome: "complete",
      checkpoint: "desktop-bound",
    });
    expect(retryRequest).toEqual(firstRequest);
    expect(methodNames(calls)).toEqual([
      "ValidateProjectTargetV1",
      "InitializeProjectV1",
      "GetInitializationStatusV1",
      "ValidateProjectTargetV1",
      "InitializeProjectV1",
    ]);
    expect(calls[1][1].request.arguments).toEqual([firstRequest]);
    expect(calls[4][1].request.arguments).toEqual([firstRequest]);
    expect(appSource).toContain(
      'elements.setupRetry.addEventListener("click", retryFirstRunValidation)',
    );
  });
});
