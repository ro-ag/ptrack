import {
  canOpenPreservedFirstRunProject,
  completedInitializationWorkspaceMatches,
  initialFirstRunState,
  initializationFailureMessage,
  initializationStatusMatchesOperation,
  projectGuideCommitFields,
  projectGuideStatusResolution,
} from "./first-run";
import {
  commitInitialization,
  initializeProjectRequest,
  openExactProject as runExactProjectOpen,
  readInitializationStatus,
  resumeInitialization,
  validateInitializationTarget,
} from "./first-run-journey";
import { messageFrom } from "./format";
import type { AppContext } from "./app-context";
import { element } from "./dom";
import type {
  FirstRunState,
  InitializationStatus,
  ProjectTargetValidation,
} from "./first-run";
import type { WorkspaceStateResponse } from "./snapshot-types";

export interface InitializeTargetOptions {
  durable?: boolean;
  expectedOperationId?: string;
}

// How long an in-progress initialization is polled before the window asks
// the user to resume it explicitly.
const INITIALIZATION_POLL_LIMIT = 20;
const INITIALIZATION_POLL_DELAY = 250;

export function createProjectLifecycle(ctx: AppContext) {
  const { api, setStatus, showError, workspaceController } = ctx;
  const elements = {
    closeProject: element("#close-project-button", HTMLButtonElement),
    openProject: element("#open-project-button", HTMLButtonElement),
    setupError: element("#setup-error", HTMLParagraphElement),
    stateInitialize: element("#state-initialize-project-button", HTMLButtonElement),
    stateOpen: element("#state-open-project-button", HTMLButtonElement),
    switchProject: element("#switch-project-button", HTMLButtonElement),
  };

  async function recoverWorkspaceState(error: unknown): Promise<void> {
    showError(error);
    try {
      const state = await api().GetWorkspaceState();
      workspaceController.publish({
        status: state.status,
        generation: Number(state.generation || 0),
      });
      ctx.shell.renderWorkspaceState(state, true);
      if (state.status === "open") await ctx.snapshot.loadSnapshot(ctx.state.board?.planId || 0);
    } catch (stateError) {
      workspaceController.publish({ status: "error", generation: 0 });
      ctx.shell.renderWorkspaceState(
        { status: "error", generation: 0, error: messageFrom(stateError) },
        true,
      );
    }
  }

  async function chooseProjectDirectory(purpose: string): Promise<string> {
    const path = await api().PickProjectDirectory(purpose);
    return typeof path === "string" ? path : "";
  }

  async function openExactProject(root: string): Promise<WorkspaceStateResponse | false | undefined> {
    await ctx.state.terminalHandle?.flushPending();
    let transition = ctx.shell.beginWorkspaceTransition();
    const outcome = await runExactProjectOpen(
      api(),
      root,
      async (result) => {
        if (!ctx.shell.publishBackendState(result.state, transition, false, true)) {
          return "abort";
        }
        return await ctx.recent.showWorkspaceConfirmation(
          "switch",
          result.activeResources ?? { terminals: 0, agentRuns: 0 },
        )
          ? "confirm"
          : "cancel";
      },
      () => {
        transition = ctx.shell.beginWorkspaceTransition();
      },
    );
    if (outcome.kind === "aborted") return;
    if (outcome.kind === "cancelled") {
      ctx.shell.renderWorkspaceState(outcome.result.state, true);
      return false;
    }
    const result = outcome.result;
    if (!ctx.shell.publishBackendState(result.state, transition, true)) return false;
    if (result.warning) showError(result.warning);
    return result.state;
  }

  // The picker is unavailable: the step it was opened from stays on screen,
  // saying why.
  function restorePickerCancelState(pickerCancelState: FirstRunState, error: unknown): void {
    ctx.state.firstRunState = {
      ...pickerCancelState,
      message: `The folder picker is unavailable: ${messageFrom(error)}`,
    };
    ctx.firstRun.renderFirstRunFlow(true);
    elements.setupError.textContent =
      `The folder picker is unavailable: ${messageFrom(error)}`;
  }

  async function requestOpenProject(
    selectedPath = "",
    returnFocus: HTMLElement | null = null,
    pickerCancelState: FirstRunState | null = null,
  ): Promise<void> {
    if (ctx.recent.recentProjectOperationActive()) return;
    const hadOpenWorkspace = ctx.state.workspaceState.status === "open";
    const focusId = returnFocus?.id || (hadOpenWorkspace
      ? "switch-project-button"
      : "state-open-project-button");
    let path = selectedPath;
    try {
      ctx.firstRun.setFirstRunState({
        type: pickerCancelState ? "repick" : "pick",
        intent: "open",
        returnFocusId: focusId,
      });
      path ||= await chooseProjectDirectory("open");
      if (!path) {
        if (pickerCancelState) {
          ctx.firstRun.setFirstRunState({ type: "pickerCancelled", restore: pickerCancelState });
        } else {
          ctx.firstRun.setFirstRunState({ type: "pickerCancelled" });
        }
        if (hadOpenWorkspace) requestAnimationFrame(() => returnFocus?.focus());
        else if (pickerCancelState) requestAnimationFrame(() => returnFocus?.focus());
        else ctx.firstRun.renderFirstRunFlow(true);
        return;
      }
      ctx.firstRun.setFirstRunState({ type: "validate" }, !hadOpenWorkspace);
      if (hadOpenWorkspace) setStatus("Validating the selected project folder…");
      const validation = await validateInitializationTarget(api(), path);
      if (validation.kind !== "existing") {
        const message = validation.kind === "new"
          ? "This folder is not an initialized p-track project."
          : validation.reason || "This folder requires recovery before it can be opened.";
        if (hadOpenWorkspace) {
          ctx.firstRun.setFirstRunState({ type: "reset", focusId });
          setStatus("Project unchanged");
          showError(new Error(message));
        } else {
          ctx.firstRun.setFirstRunState({
            type: validation.kind === "recovery-required" ? "recovery" : "failed",
            canonicalRoot: validation.canonicalRoot,
            message,
          }, true);
        }
        return;
      }
      ctx.firstRun.setFirstRunState({ type: "reset", focusId });
      await openExactProject(validation.canonicalRoot);
    } catch (error) {
      if (pickerCancelState && !path) {
        restorePickerCancelState(pickerCancelState, error);
        return;
      }
      if (hadOpenWorkspace) {
        ctx.firstRun.setFirstRunState({ type: "reset", focusId });
        await recoverWorkspaceState(error);
      } else {
        ctx.firstRun.setFirstRunState({
          type: "failed",
          canonicalRoot: path,
          operationId: "",
          message: messageFrom(error),
        }, true);
      }
    }
  }

  async function requestInitializeProject(
    returnFocus: HTMLElement | null = elements.stateInitialize,
    pickerCancelState: FirstRunState | null = null,
  ): Promise<void> {
    if (ctx.recent.recentProjectOperationActive()) return;
    const focusId = returnFocus?.id || "state-initialize-project-button";
    let path = "";
    try {
      ctx.firstRun.setFirstRunState({
        type: pickerCancelState ? "repick" : "pick",
        intent: "initialize",
        returnFocusId: focusId,
      });
      path = await chooseProjectDirectory("initialize");
      if (!path) {
        if (pickerCancelState) {
          ctx.firstRun.setFirstRunState({ type: "pickerCancelled", restore: pickerCancelState });
          requestAnimationFrame(() => returnFocus?.focus());
        } else {
          ctx.firstRun.setFirstRunState({ type: "pickerCancelled" });
          ctx.firstRun.renderFirstRunFlow(true);
        }
        return;
      }
      await validateInitializeTarget(path);
    } catch (error) {
      if (pickerCancelState && !path) {
        restorePickerCancelState(pickerCancelState, error);
        return;
      }
      ctx.firstRun.setFirstRunState({
        type: "failed",
        canonicalRoot: path,
        operationId: "",
        message: messageFrom(error),
      }, true);
    }
  }

  async function validateInitializeTarget(
    path: string,
    { durable = false, expectedOperationId = "" }: InitializeTargetOptions = {},
    observedValidation: ProjectTargetValidation | null = null,
  ): Promise<void> {
    const durableCheckpoint = ctx.state.firstRunState.checkpoint;
    const durableErrorKind = ctx.state.firstRunState.errorKind;
    try {
      ctx.firstRun.setFirstRunState({ type: "validate" }, true);
      const validation = observedValidation ||
        await validateInitializationTarget(api(), path);
      if (durable && validation.kind === "recovery-required") {
        ctx.firstRun.setFirstRunState({
          type: "recovery",
          canonicalRoot: validation.canonicalRoot,
          operationId: expectedOperationId,
          message: validation.reason ||
            "This preserved project setup cannot be resumed automatically.",
          checkpoint: durableCheckpoint,
          errorKind: durableErrorKind,
          durable: true,
          resumable: false,
        }, true);
        return;
      }
      if (durable && (
        validation.canonicalRoot !== path ||
        validation.kind !== "new" ||
        !validation.resume ||
        validation.operationId !== expectedOperationId
      )) {
        ctx.firstRun.setFirstRunState({
          type: "recovery",
          canonicalRoot: path,
          operationId: expectedOperationId,
          message:
            "The preserved initialization operation changed during revalidation and cannot be resumed automatically.",
          checkpoint: durableCheckpoint,
          errorKind: durableErrorKind,
          durable: true,
          resumable: false,
        }, true);
        return;
      }
      if (validation.resume) {
        ctx.firstRun.setFirstRunState({
          type: "resume",
          canonicalRoot: validation.canonicalRoot,
          operationId: validation.operationId,
          goal: validation.resume.goal,
          guideChoice: validation.resume.guideChoice,
          initialization: validation.resume.initialization,
        }, true);
        if (
          validation.resume.initialization.outcome === "complete" &&
          validation.resume.initialization.checkpoint === "desktop-bound"
        ) {
          await applyInitializationStatus(
            validation.operationId,
            validation.canonicalRoot,
            validation.resume.initialization,
          );
        }
        return;
      }
      if (validation.kind === "existing") {
        ctx.firstRun.setFirstRunState({ type: "existing", canonicalRoot: validation.canonicalRoot }, true);
        return;
      }
      if (validation.kind === "recovery-required") {
        ctx.firstRun.setFirstRunState({
          type: "recovery",
          canonicalRoot: validation.canonicalRoot,
          operationId: "",
          message: validation.reason || "This folder contains project state that cannot be changed safely.",
          checkpoint: durableCheckpoint,
          errorKind: durableErrorKind,
          durable,
        }, true);
        return;
      }
      ctx.firstRun.setFirstRunState({
        type: "new",
        canonicalRoot: validation.canonicalRoot,
        operationId: validation.operationId,
      }, true);
    } catch (error) {
      ctx.firstRun.setFirstRunState({
        type: durable ? "recovery" : "failed",
        canonicalRoot: path,
        operationId: durable ? expectedOperationId : "",
        message: durable
          ? `p-track could not revalidate the preserved operation: ${messageFrom(error)}`
          : messageFrom(error),
        checkpoint: durableCheckpoint,
        errorKind: durableErrorKind,
        durable,
      }, true);
    }
  }

  let publishedInitializationOperationId = "";

  function workspaceControllerMatches(state: WorkspaceStateResponse): boolean {
    return workspaceController.state.status === "open" &&
      workspaceController.state.generation === Number(state?.generation || 0);
  }

  async function rebindCompletedInitializationWorkspace(canonicalRoot: string): Promise<WorkspaceStateResponse> {
    let openError: unknown = new Error("The completed project could not be opened in this window.");
    try {
      const opened = await openExactProject(canonicalRoot);
      if (
        completedInitializationWorkspaceMatches(opened, canonicalRoot) &&
        workspaceControllerMatches(opened)
      ) return opened;
    } catch (error) {
      openError = error;
    }
    try {
      const refreshed = await api().GetWorkspaceState();
      workspaceController.publish({
        status: refreshed.status,
        generation: Number(refreshed.generation || 0),
      });
      ctx.shell.renderWorkspaceState(refreshed, false);
      if (
        completedInitializationWorkspaceMatches(refreshed, canonicalRoot) &&
        workspaceControllerMatches(refreshed)
      ) return refreshed;
    } catch {
      // The exact operation remains recoverable through its durable status.
    }
    throw openError;
  }

  // A complete initialization binds this window to the new project once, then
  // hands over to the first-plan onboarding.
  async function completeInitialization(
    operationId: string,
    canonicalRoot: string,
    status: InitializationStatus,
    state: unknown,
  ): Promise<void> {
    if (status.checkpoint !== "desktop-bound") {
      throw new Error("Initialization completed before the desktop workspace was bound.");
    }
    const reported: unknown = state || await api().GetWorkspaceState();
    let workspace: WorkspaceStateResponse;
    let rebound = false;
    if (completedInitializationWorkspaceMatches(reported, canonicalRoot)) {
      workspace = reported;
    } else {
      try {
        workspace = await rebindCompletedInitializationWorkspace(canonicalRoot);
        rebound = true;
      } catch (error) {
        ctx.firstRun.setFirstRunState({
          type: "recovery",
          canonicalRoot,
          operationId,
          message:
            `Initialization is complete, but this window could not open the project: ${messageFrom(error)}`,
          checkpoint: status.checkpoint,
          errorKind: status.errorKind,
          durable: true,
        }, true);
        return;
      }
    }
    if (publishedInitializationOperationId === operationId) return;
    if (!rebound) {
      ctx.state.firstRunState = { ...initialFirstRunState };
      if (!ctx.shell.publishBackendState(workspace, undefined, false, true)) return;
    } else if (!workspaceControllerMatches(workspace)) {
      return;
    }
    publishedInitializationOperationId = operationId;
    ctx.firstPlan.beginFirstPlanOnboarding(Number(workspace.generation));
  }

  // An initialization still running is polled a bounded number of times;
  // past that, resuming it is the user's explicit choice.
  async function awaitNextCheckpoint(
    operationId: string,
    canonicalRoot: string,
    status: InitializationStatus,
    pollAttempt: number,
  ): Promise<void> {
    if (pollAttempt >= INITIALIZATION_POLL_LIMIT) {
      ctx.firstRun.setFirstRunState({
        type: "recovery",
        message:
          "Initialization did not reach another checkpoint. Resume Setup will revalidate the preserved operation before continuing.",
        checkpoint: status.checkpoint,
        errorKind: status.errorKind,
        durable: true,
      }, true);
      return;
    }
    await new Promise((resolve) => window.setTimeout(resolve, INITIALIZATION_POLL_DELAY));
    const next = await readInitializationStatus(api(), operationId);
    await applyInitializationStatus(
      operationId,
      canonicalRoot,
      next,
      null,
      pollAttempt + 1,
    );
  }

  async function applyInitializationStatus(
    operationId: string,
    canonicalRoot: string,
    status: InitializationStatus,
    state: unknown = null,
    pollAttempt = 0,
  ): Promise<void> {
    if (
      ctx.state.firstRunState.operationId !== operationId ||
      ctx.state.firstRunState.canonicalRoot !== canonicalRoot
    ) return;
    if (!initializationStatusMatchesOperation(status, operationId, canonicalRoot)) {
      throw new Error("Initialization status does not match the committed operation.");
    }
    const guide = projectGuideStatusResolution(status.errorKind, status);
    if (guide.kind === "unknown-checkpoint") throw new Error(guide.message);
    if (guide.kind === "stale") {
      ctx.firstRun.setFirstRunState(guide.event, true);
      return;
    }
    if (status.outcome === "complete") {
      await completeInitialization(operationId, canonicalRoot, status, state);
      return;
    }
    if (status.outcome === "in-progress") {
      await awaitNextCheckpoint(operationId, canonicalRoot, status, pollAttempt);
      return;
    }
    if (status.outcome === "recovery-required") {
      ctx.firstRun.setFirstRunState({
        type: "recovery",
        message: "Project setup made a durable change and now requires recovery before continuing.",
        checkpoint: status.checkpoint,
        errorKind: status.errorKind,
        durable: true,
      }, true);
      return;
    }
    ctx.firstRun.setFirstRunState({
      type: "failed",
      message: initializationFailureMessage(status.errorKind),
      checkpoint: status.checkpoint,
      errorKind: status.errorKind,
    }, true);
  }

  async function reconcileInitializationStatus(
    error: unknown,
    operationId: string,
    canonicalRoot: string,
    observedStatus: InitializationStatus | null = null,
  ): Promise<void> {
    try {
      const status = observedStatus ||
        await readInitializationStatus(api(), operationId);
      // The call's own error can name a guide failure the status does not; a
      // checkpoint that does not fit it falls through to the status itself.
      const guide = projectGuideStatusResolution(error, status);
      if (guide.kind === "stale") {
        if (!initializationStatusMatchesOperation(status, operationId, canonicalRoot)) {
          throw new Error("Initialization status does not match the committed operation.");
        }
        ctx.firstRun.setFirstRunState(guide.event, true);
        return;
      }
      await applyInitializationStatus(operationId, canonicalRoot, status);
    } catch (statusError) {
      if (
        ctx.state.firstRunState.operationId !== operationId ||
        ctx.state.firstRunState.canonicalRoot !== canonicalRoot
      ) return;
      ctx.firstRun.setFirstRunState({
        type: "uncertain",
        message: `Initialization status is uncertain: ${messageFrom(statusError || error)}`,
        checkpoint: ctx.state.firstRunState.checkpoint,
      }, true);
    }
  }

  async function commitFirstRunProject(): Promise<void> {
    if (ctx.state.firstRunState.phase !== "review") return;
    const operationId = ctx.state.firstRunState.operationId;
    const canonicalRoot = ctx.state.firstRunState.canonicalRoot;
    const goal = ctx.state.firstRunState.goal;
    const guide = projectGuideCommitFields(ctx.state.firstRunState);
    const request = initializeProjectRequest(
      operationId,
      canonicalRoot,
      goal,
      guide,
    );
    ctx.firstRun.setFirstRunState({ type: "commit" }, true);
    const outcome = await commitInitialization(api(), request);
    if (outcome.kind === "status") {
      await reconcileInitializationStatus(
        outcome.error,
        operationId,
        canonicalRoot,
        outcome.status,
      );
      return;
    }
    if (outcome.kind === "uncertain") {
      if (
        ctx.state.firstRunState.operationId !== operationId ||
        ctx.state.firstRunState.canonicalRoot !== canonicalRoot
      ) return;
      ctx.firstRun.setFirstRunState({
        type: "uncertain",
        message:
          `Initialization status is uncertain: ${messageFrom(outcome.statusError || outcome.error)}`,
        checkpoint: ctx.state.firstRunState.checkpoint,
      }, true);
      return;
    }
    try {
      await applyInitializationStatus(
        operationId,
        canonicalRoot,
        outcome.result.status,
        outcome.result.state,
      );
    } catch (error) {
      await reconcileInitializationStatus(error, operationId, canonicalRoot);
    }
  }

  async function retryInitializationStatus(): Promise<void> {
    if (ctx.state.firstRunState.phase !== "uncertain") return;
    const operationId = ctx.state.firstRunState.operationId;
    const canonicalRoot = ctx.state.firstRunState.canonicalRoot;
    ctx.firstRun.setFirstRunState({ type: "reconcile" }, true);
    await reconcileInitializationStatus(
      new Error("Initialization status remains unavailable."),
      operationId,
      canonicalRoot,
    );
  }

  async function openExistingFromSetup(): Promise<void> {
    if (ctx.state.firstRunState.phase !== "existing" || !ctx.state.firstRunState.canonicalRoot) return;
    const root = ctx.state.firstRunState.canonicalRoot;
    ctx.firstRun.setFirstRunState({ type: "reset", focusId: "state-open-project-button" });
    try {
      await openExactProject(root);
    } catch (error) {
      await recoverWorkspaceState(error);
    }
  }

  async function resumeFirstRunSetup(): Promise<void> {
    if (
      ctx.state.firstRunState.phase !== "recovery" ||
      ctx.state.firstRunState.recoveryMode !== "durable" ||
      !ctx.state.firstRunState.canonicalRoot
    ) return;
    const operationId = ctx.state.firstRunState.operationId;
    const canonicalRoot = ctx.state.firstRunState.canonicalRoot;
    const checkpoint = ctx.state.firstRunState.checkpoint;
    ctx.firstRun.setFirstRunState({ type: "reconcile" }, true);
    try {
      const outcome = await resumeInitialization(
        api(),
        operationId,
        canonicalRoot,
      );
      if (outcome.kind === "status") {
        if (!initializationStatusMatchesOperation(
          outcome.status,
          operationId,
          canonicalRoot,
        )) {
          throw new Error("Initialization status does not match the preserved operation.");
        }
        await applyInitializationStatus(operationId, canonicalRoot, outcome.status);
        return;
      }
      await validateInitializeTarget(canonicalRoot, {
        durable: true,
        expectedOperationId: operationId,
      }, outcome.validation);
    } catch (error) {
      if (
        ctx.state.firstRunState.operationId !== operationId ||
        ctx.state.firstRunState.canonicalRoot !== canonicalRoot
      ) return;
      ctx.firstRun.setFirstRunState({
        type: "uncertain",
        message: `p-track could not confirm the preserved operation: ${messageFrom(error)}`,
        checkpoint,
      }, true);
    }
  }

  async function openProjectFromRecovery(): Promise<void> {
    if (
      !canOpenPreservedFirstRunProject(ctx.state.firstRunState) ||
      !ctx.state.firstRunState.canonicalRoot
    ) return;
    const recovery = { ...ctx.state.firstRunState };
    try {
      const opened = await rebindCompletedInitializationWorkspace(
        recovery.canonicalRoot,
      );
      if (recovery.checkpoint === "desktop-bound") {
        publishedInitializationOperationId = recovery.operationId;
        ctx.firstPlan.beginFirstPlanOnboarding(Number(opened.generation));
      }
    } catch (error) {
      ctx.state.firstRunState = {
        ...recovery,
        message: `The preserved project could not be opened: ${messageFrom(error)}`,
      };
      ctx.firstRun.renderFirstRunFlow(true);
    }
  }

  // After a close, a follow-up read settles what the window shows, and only
  // while nothing moved the workspace in the meantime: a project opened within
  // these 350 ms owns the window.
  function confirmClosedWorkspace(): void {
    const closed = workspaceController.capture();
    window.setTimeout(async () => {
      if (!workspaceController.isCurrent(closed)) return;
      try {
        const state = await api().GetWorkspaceState();
        if (!workspaceController.isCurrent(closed)) return;
        workspaceController.publish({
          status: state.status,
          generation: Number(state.generation || 0),
        });
        ctx.shell.renderWorkspaceState(state, true);
      } catch (error) {
        if (workspaceController.isCurrent(closed)) showError(error);
      }
    }, 350);
  }

  async function requestCloseProject(): Promise<void> {
    if (workspaceController.state.status !== "open") return;
    try {
      await ctx.state.terminalHandle?.flushPending();
      let transition = ctx.shell.beginWorkspaceTransition();
      let result = await api().CloseProject("");
      if (result.requiresConfirmation) {
        if (!ctx.shell.publishBackendState(result.state, transition, false, true)) return;
        const confirmed = await ctx.recent.showWorkspaceConfirmation("close", result.activeResources);
        if (!confirmed) {
          await api().CancelWorkspaceChange(result.confirmationToken);
          ctx.shell.renderWorkspaceState(result.state, true);
          return;
        }
        transition = ctx.shell.beginWorkspaceTransition();
        result = await api().CloseProject(result.confirmationToken);
      }
      if (!ctx.shell.publishBackendState(result.state, transition, true)) return;
      if (result.warning) showError(result.warning);
      if (result.state.status === "closed") confirmClosedWorkspace();
    } catch (error) {
      await recoverWorkspaceState(error);
    }
  }

  function bind(): void {
    elements.openProject.addEventListener("click", () =>
      void requestOpenProject("", elements.openProject),
    );
    elements.switchProject.addEventListener("click", () =>
      void requestOpenProject("", elements.switchProject),
    );
    elements.closeProject.addEventListener("click", () => void requestCloseProject());
    elements.stateInitialize.addEventListener("click", () =>
      void requestInitializeProject(elements.stateInitialize),
    );
    elements.stateOpen.addEventListener("click", () =>
      void requestOpenProject("", elements.stateOpen),
    );
  }

  return {
    bind,
    chooseProjectDirectory,
    requestOpenProject,
    requestInitializeProject,
    validateInitializeTarget,
    commitFirstRunProject,
    retryInitializationStatus,
    openExistingFromSetup,
    resumeFirstRunSetup,
    openProjectFromRecovery,
    requestCloseProject,
  };
}

export type ProjectLifecycle = ReturnType<typeof createProjectLifecycle>;
