import type { AppContext } from "./app-context";
import { appendTextItems, element, setAriaBoolean, setFirstRunSectionVisible } from "./dom";
import {
  PROJECT_GUIDANCE_UNAVAILABLE,
  canOpenPreservedFirstRunProject,
  firstRunFocusTarget,
  parseProjectGuidePreview,
  pendingInitializationEvent,
  reduceFirstRun,
  validateNorthStarGoal,
  type FirstRunEvent,
  type FirstRunPhase,
  type FirstRunState,
  type PendingInitialization,
  type ProjectGuideFileAction,
  type ProjectGuidePreviewFile,
} from "./first-run";
import { messageFrom } from "./format";
import {
  durableProjectGuideReviewCopy,
  firstRunRecoveryActions,
  projectGuideRecoveryCopy,
  projectGuideReviewCopy,
} from "./presentation";

/** The words at the top of the setup card for one first-run step. */
export interface SetupContent {
  progress: string;
  eyebrow: string;
  heading: string;
  detail: string;
  status?: string;
  error?: string;
}

export function firstRunStoragePath(root: string): string {
  const separator = root.includes("\\") ? "\\" : "/";
  return `${root.replace(/[\\/]$/, "")}${separator}.ptrack${separator}ptrack.redb`;
}

function guideActionLabel(action: ProjectGuideFileAction): string {
  if (action === "create") return "Create";
  if (action === "update") return "Update";
  return "No change";
}

function resumingOperation(run: FirstRunState): boolean {
  return run.resumedOperation || run.guidePostCommit;
}

function stepEyebrow(run: FirstRunState): string {
  return run.intent === "initialize" ? "Initialize project" : "Open project";
}

// Steps whose copy never depends on more than the intent.
const fixedStepCopy: Partial<Record<FirstRunPhase, (run: FirstRunState) => SetupContent>> = {
  picking: (run) => ({
    progress: "Step 1 of 4",
    eyebrow: stepEyebrow(run),
    heading: "Choose a project folder",
    detail: "Use the native folder picker to continue.",
  }),
  validating: (run) => ({
    progress: "Step 1 of 4",
    eyebrow: stepEyebrow(run),
    heading: "Checking this folder…",
    detail: "p-track is resolving the canonical folder and checking project state without writing files.",
    status: "Validating the selected folder without making changes.",
  }),
  existing: () => ({
    progress: "Step 1 of 4",
    eyebrow: "Existing project found",
    heading: "This folder already has a p-track project",
    detail: "Open the existing project, or choose another folder to initialize.",
  }),
  "target-new": () => ({
    progress: "Step 1 of 4",
    eyebrow: "Initialize project",
    heading: "Continue with this folder?",
    detail:
      "Your north-star goal is preserved. Continue to edit it, choose another folder, or explicitly cancel setup.",
  }),
  goal: () => ({
    progress: "Step 2 of 4",
    eyebrow: "Project direction",
    heading: "Set the north-star goal",
    detail: "Describe the durable outcome this project is working toward.",
  }),
  "guide-previewing": () => ({
    progress: "Step 3 of 4",
    eyebrow: "Project guidance",
    heading: "Preparing the guide preview…",
    detail: "p-track is reading only AGENTS.md and CLAUDE.md and will not write files.",
    status: "Checking current guide files and generating a bounded diff.",
  }),
  reconciling: () => ({
    progress: "Checking status",
    eyebrow: "Initialization status",
    heading: "Checking the durable operation…",
    detail: "p-track is reading the saved operation status without replaying initialization.",
    status: "Project navigation remains locked until the operation reaches a definitive state.",
  }),
  committing: (run) => ({
    progress: "Step 4 of 4",
    eyebrow: "Initializing project",
    heading: resumingOperation(run)
      ? "Finishing project initialization…"
      : "Creating local project state…",
    detail: resumingOperation(run)
      ? run.storageAlreadyCreated
        ? "Private project storage is already durable. p-track is resuming the same operation without replaying completed steps."
        : "p-track is resuming the same durable operation before creating private project storage."
      : "Keep p-track open while it completes the recoverable initialization sequence.",
    status: "Initialization has started and can no longer be canceled.",
  }),
  uncertain: (run) => ({
    progress: "Status unavailable",
    eyebrow: "Initialization status",
    heading: "Keep this operation open",
    detail: run.message || "p-track could not confirm whether initialization is still running.",
    error: run.checkpoint && run.checkpoint !== "none"
      ? `Last durable checkpoint: ${run.checkpoint}`
      : "No definitive completion state is available yet.",
  }),
  recovery: (run) => ({
    progress: "Recovery required",
    eyebrow: "Project recovery",
    heading: run.recoveryMode === "durable"
      ? "Resume this project setup"
      : "This project needs recovery",
    detail: run.message || "This folder contains project state that cannot be changed safely.",
    error: run.checkpoint && run.checkpoint !== "none"
      ? `Last durable checkpoint: ${run.checkpoint}. The project and its setup choices are preserved.`
      : run.recoveryMode === "durable"
      ? "The initialization operation is preserved. Check its authoritative state before continuing."
      : run.recoveryMode === "blocked"
      ? "The preserved checkpoint cannot be resumed automatically. p-track will not repair or remove project files."
      : "p-track will not repair or remove project files automatically.",
  }),
  failed: (run) => ({
    progress: "Setup stopped",
    eyebrow: "Project setup",
    heading: run.intent === "open"
      ? "This project could not be opened"
      : run.errorKind === "project-not-found"
      ? "This folder is no longer available."
      : "This project was not initialized",
    detail: run.message || "p-track could not safely complete setup.",
    error: "No project files were written by this attempt. Retry checks the folder again before any write.",
  }),
};

export function guideStepCopy(run: FirstRunState, hasPreview: boolean): SetupContent {
  return {
    progress: "Step 3 of 4",
    eyebrow: "Project guidance",
    heading: hasPreview ? "Review exact guide changes" : "Choose project guidance",
    detail: hasPreview
      ? "The diff is read-only. Install only if every target and line is expected."
      : run.resumeNoWrite
      ? "No project files were written. You can try again safely."
      : !run.guideSkipAllowed
      ? run.guidePartiallyApplied
        ? projectGuideRecoveryCopy("partially-applied").detail
        : "This initialization operation already has durable progress. Preview the current guide files to resume safely."
      : "Skip Guide is selected by default. Previewing does not write files.",
    status: run.guideAvailable === false
      ? PROJECT_GUIDANCE_UNAVAILABLE
      : "",
    error: run.guideAvailable === null ? run.message : "",
  };
}

export function guideStaleCopy(run: FirstRunState): SetupContent {
  const recoveryCopy = projectGuideRecoveryCopy(
    run.guidePartiallyApplied ? "partially-applied" : "stale",
  );
  return {
    progress: "Guide review required",
    eyebrow: "Project guidance",
    heading: recoveryCopy.heading,
    detail: run.resumeNoWrite
      ? "No project files were written. You can try again safely."
      : run.guidePartiallyApplied
      ? recoveryCopy.detail
      : run.resumeLocked
      ? run.storageAlreadyCreated
        ? `Private project storage is already durable. Review the current guide files${run.guideSkipAllowed ? " or explicitly skip them" : ""} to finish initialization.`
        : "This initialization operation already has durable progress. Review the current guide files to resume safely."
      : "Nothing was written. Review the current guide files or explicitly skip them.",
    error: run.message,
  };
}

export function reviewStepCopy(run: FirstRunState): SetupContent {
  return {
    progress: "Step 4 of 4",
    eyebrow: "Review changes",
    heading: resumingOperation(run)
      ? run.resumeNoWrite
        ? "Resume this project initialization?"
        : "Finish this project initialization?"
      : "Initialize this project?",
    detail: run.resumeNoWrite
      ? "No project files were written. You can try again safely. Confirm to resume the same operation."
      : resumingOperation(run)
      ? run.storageAlreadyCreated
        ? "Project storage is already durable. Confirm the reviewed guide choice to resume the same operation."
        : "This operation already has durable progress. Confirm the reviewed guide choice to resume before project storage is created."
      : "Review every proposed change. Nothing is written until you confirm.",
  };
}

function guideFileElement(file: ProjectGuidePreviewFile): HTMLElement {
  const item = document.createElement("article");
  item.className = "setup-guide-file";
  const header = document.createElement("div");
  header.className = "setup-guide-file-header";
  const path = document.createElement("p");
  path.className = "setup-guide-path";
  path.textContent = file.path;
  const counts = document.createElement("p");
  counts.className = "setup-guide-counts";
  counts.textContent = file.action === "no-change"
    ? "No change"
    : `${guideActionLabel(file.action)} · +${file.additions} −${file.deletions}`;
  header.append(path, counts);
  item.append(header);
  if (file.diff) {
    const diff = document.createElement("pre");
    diff.className = "setup-guide-diff";
    diff.tabIndex = 0;
    diff.setAttribute("aria-label", `${file.path} bounded diff`);
    const code = document.createElement("code");
    code.textContent = file.diff;
    diff.append(code);
    item.append(diff);
  }
  return item;
}

export function createFirstRunView(ctx: AppContext) {
  const { api, openHelpDestination } = ctx;
  const elements = {
    closeProject: element("#close-project-button", HTMLButtonElement),
    openProject: element("#open-project-button", HTMLButtonElement),
    setupCheckStatus: element("#setup-check-status", HTMLButtonElement),
    setupCommit: element("#setup-commit", HTMLButtonElement),
    setupCompleteChanges: element("#setup-complete-changes", HTMLUListElement),
    setupDetail: element("#setup-detail", HTMLParagraphElement),
    setupError: element("#setup-error", HTMLParagraphElement),
    setupExistingActions: element("#setup-existing-actions", HTMLDivElement),
    setupExistingCancel: element("#setup-existing-cancel", HTMLButtonElement),
    setupExistingChoose: element("#setup-existing-choose", HTMLButtonElement),
    setupEyebrow: element("#setup-eyebrow", HTMLParagraphElement),
    setupGoal: element("#setup-goal", HTMLTextAreaElement),
    setupGoalBack: element("#setup-goal-back", HTMLButtonElement),
    setupGoalCancel: element("#setup-goal-cancel", HTMLButtonElement),
    setupGoalError: element("#setup-goal-error", HTMLParagraphElement),
    setupGoalForm: element("#setup-goal-form", HTMLFormElement),
    setupGuide: element("#setup-guide", HTMLElement),
    setupGuideBack: element("#setup-guide-back", HTMLButtonElement),
    setupGuideCancel: element("#setup-guide-cancel", HTMLButtonElement),
    setupGuideDefaultActions: element("#setup-guide-default-actions", HTMLDivElement),
    setupGuideDefaultChoice: element("#setup-guide-default-choice", HTMLDivElement),
    setupGuideFiles: element("#setup-guide-files", HTMLDivElement),
    setupGuideInstall: element("#setup-guide-install", HTMLButtonElement),
    setupGuideInstallActions: element("#setup-guide-install-actions", HTMLDivElement),
    setupGuidePreview: element("#setup-guide-preview", HTMLDivElement),
    setupGuidePreviewBack: element("#setup-guide-preview-back", HTMLButtonElement),
    setupGuidePreviewButton: element("#setup-guide-preview-button", HTMLButtonElement),
    setupGuidePreviewCancel: element("#setup-guide-preview-cancel", HTMLButtonElement),
    setupGuidePreviewSkip: element("#setup-guide-preview-skip", HTMLButtonElement),
    setupGuideReviewAgain: element("#setup-guide-review-again", HTMLButtonElement),
    setupGuideSkip: element("#setup-guide-skip", HTMLButtonElement),
    setupGuideStaleActions: element("#setup-guide-stale-actions", HTMLDivElement),
    setupGuideStaleBack: element("#setup-guide-stale-back", HTMLButtonElement),
    setupGuideStaleCancel: element("#setup-guide-stale-cancel", HTMLButtonElement),
    setupGuideStaleSkip: element("#setup-guide-stale-skip", HTMLButtonElement),
    setupHeading: element("#setup-heading", HTMLHeadingElement),
    setupNewTargetActions: element("#setup-new-target-actions", HTMLDivElement),
    setupNewTargetCancel: element("#setup-new-target-cancel", HTMLButtonElement),
    setupNewTargetChoose: element("#setup-new-target-choose", HTMLButtonElement),
    setupNewTargetContinue: element("#setup-new-target-continue", HTMLButtonElement),
    setupOpenExisting: element("#setup-open-existing", HTMLButtonElement),
    setupOpenRecovery: element("#setup-open-recovery", HTMLButtonElement),
    setupOperation: element("#setup-operation", HTMLDivElement),
    setupPanel: element("#setup-panel", HTMLElement),
    setupProgress: element("#setup-progress", HTMLParagraphElement),
    setupRecoveryActions: element("#setup-recovery-actions", HTMLDivElement),
    setupRecoveryChoose: element("#setup-recovery-choose", HTMLButtonElement),
    setupRecoveryHelp: element("#setup-recovery-help", HTMLButtonElement),
    setupResume: element("#setup-resume", HTMLButtonElement),
    setupRetry: element("#setup-retry", HTMLButtonElement),
    setupReturnWelcome: element("#setup-return-welcome", HTMLButtonElement),
    setupReview: element("#setup-review", HTMLElement),
    setupReviewBack: element("#setup-review-back", HTMLButtonElement),
    setupReviewCancel: element("#setup-review-cancel", HTMLButtonElement),
    setupReviewGoal: element("#setup-review-goal", HTMLParagraphElement),
    setupReviewGuideChanges: element("#setup-review-guide-changes", HTMLUListElement),
    setupReviewGuideChoice: element("#setup-review-guide-choice", HTMLParagraphElement),
    setupReviewGuideDetail: element("#setup-review-guide-detail", HTMLParagraphElement),
    setupStatus: element("#setup-status", HTMLParagraphElement),
    setupStorageSummary: element("#setup-storage-summary", HTMLParagraphElement),
    setupTarget: element("#setup-target", HTMLParagraphElement),
    setupTargetSummary: element("#setup-target-summary", HTMLDivElement),
    setupUncertainActions: element("#setup-uncertain-actions", HTMLDivElement),
    setupUntouchedRoot: element("#setup-untouched-root", HTMLParagraphElement),
    stateCard: element("#project-state-card", HTMLDivElement),
    stateOpen: element("#state-open-project-button", HTMLButtonElement),
    switchProject: element("#switch-project-button", HTMLButtonElement),
    welcomePanel: element("#welcome-panel", HTMLDivElement),
  };

  function setFirstRunState(event: FirstRunEvent, focus = false): void {
    ctx.state.firstRunState = reduceFirstRun(ctx.state.firstRunState, event);
    renderFirstRunFlow(focus);
  }

  function setSetupContent({ progress, eyebrow, heading, detail, status = "", error = "" }: SetupContent): void {
    elements.setupProgress.textContent = progress;
    elements.setupEyebrow.textContent = eyebrow;
    elements.setupHeading.textContent = heading;
    elements.setupDetail.textContent = detail;
    elements.setupStatus.textContent = status;
    elements.setupError.textContent = error;
  }

  function focusFirstRunTarget(): void {
    const target = document.getElementById(firstRunFocusTarget(ctx.state.firstRunState));
    requestAnimationFrame(() => target?.focus());
  }

  function renderProjectGuideFiles(files: readonly ProjectGuidePreviewFile[]): void {
    elements.setupGuideFiles.replaceChildren(...files.map(guideFileElement));
  }

  // Every step starts from the same closed card: no step's sections or
  // actions leak into the next.
  function resetSetupCard(run: FirstRunState): void {
    const idle = run.phase === "idle";
    ctx.updates.updateAboutUpdatesAvailability();
    elements.openProject.disabled = !idle;
    elements.switchProject.disabled = !idle;
    elements.closeProject.disabled = !idle;
    setFirstRunSectionVisible(elements.welcomePanel, idle);
    setFirstRunSectionVisible(elements.setupPanel, !idle);
    // Keep the focused heading and dedicated status/alert regions outside the
    // busy subtree so progress stays announceable while actions are locked.
    elements.stateCard.removeAttribute("aria-busy");
    setAriaBoolean(
      elements.setupOperation,
      "aria-busy",
      run.phase === "validating" ||
        run.phase === "guide-previewing" ||
        run.phase === "committing" ||
        run.phase === "reconciling",
    );
    elements.setupTargetSummary.hidden = true;
    setFirstRunSectionVisible(elements.setupGoalForm, false);
    setFirstRunSectionVisible(elements.setupGuide, false);
    setFirstRunSectionVisible(elements.setupReview, false);
    setFirstRunSectionVisible(elements.setupExistingActions, false);
    setFirstRunSectionVisible(elements.setupNewTargetActions, false);
    setFirstRunSectionVisible(elements.setupRecoveryActions, false);
    setFirstRunSectionVisible(elements.setupUncertainActions, false);
    elements.setupRetry.hidden = true;
    elements.setupResume.hidden = true;
    elements.setupOpenRecovery.hidden = true;
    elements.setupRecoveryHelp.hidden = true;
    elements.setupRecoveryChoose.hidden = false;
    elements.setupReturnWelcome.hidden = false;
    elements.setupReturnWelcome.textContent = "Return to Projects";
    elements.setupGoalError.textContent = "";
    elements.setupGoal.removeAttribute("aria-invalid");
  }

  function showTarget(run: FirstRunState): void {
    elements.setupTargetSummary.hidden = false;
    elements.setupTarget.textContent = run.canonicalRoot;
  }

  function showCommittedGuideRecoveryActions(run: FirstRunState): void {
    if (!run.resumeLocked || run.checkpoint === "none") return;
    setFirstRunSectionVisible(elements.setupRecoveryActions, true);
    elements.setupOpenRecovery.hidden = !canOpenPreservedFirstRunProject(run);
    elements.setupRecoveryHelp.hidden = false;
    elements.setupRecoveryChoose.hidden = true;
    elements.setupReturnWelcome.hidden = true;
  }

  function renderGoalStep(run: FirstRunState): void {
    setFirstRunSectionVisible(elements.setupGoalForm, true);
    elements.setupGoal.value = run.goal;
    elements.setupGoalError.textContent = run.goalError;
    setAriaBoolean(
      elements.setupGoal,
      "aria-invalid",
      Boolean(run.goalError),
    );
  }

  function renderGuideStep(run: FirstRunState): SetupContent {
    setFirstRunSectionVisible(elements.setupGuide, true);
    const hasPreview = run.guideAvailable === true &&
      run.guideFiles.length > 0;
    setFirstRunSectionVisible(elements.setupGuidePreview, hasPreview);
    setFirstRunSectionVisible(elements.setupGuideInstallActions, hasPreview);
    setFirstRunSectionVisible(elements.setupGuideDefaultActions, !hasPreview);
    setFirstRunSectionVisible(elements.setupGuideStaleActions, false);
    elements.setupGuideDefaultChoice.hidden = !run.guideSkipAllowed;
    elements.setupGuideSkip.hidden = !run.guideSkipAllowed;
    elements.setupGuidePreviewSkip.hidden = !run.guideSkipAllowed;
    elements.setupGuidePreviewButton.hidden = run.guideAvailable === false;
    elements.setupGuidePreviewBack.hidden = run.resumeLocked;
    elements.setupGuidePreviewCancel.hidden = run.resumeLocked;
    elements.setupGuideBack.hidden = run.resumeLocked;
    elements.setupGuideCancel.hidden = run.resumeLocked;
    showCommittedGuideRecoveryActions(run);
    if (hasPreview) renderProjectGuideFiles(run.guideFiles);
    return guideStepCopy(run, hasPreview);
  }

  function renderGuideStaleStep(run: FirstRunState): void {
    setFirstRunSectionVisible(elements.setupGuide, true);
    setFirstRunSectionVisible(elements.setupGuidePreview, false);
    setFirstRunSectionVisible(elements.setupGuideInstallActions, false);
    setFirstRunSectionVisible(elements.setupGuideDefaultActions, false);
    setFirstRunSectionVisible(elements.setupGuideStaleActions, true);
    elements.setupGuideDefaultChoice.hidden = true;
    elements.setupGuideStaleBack.hidden = run.resumeLocked;
    elements.setupGuideStaleCancel.hidden = run.resumeLocked;
    elements.setupGuideStaleSkip.hidden = !run.guideSkipAllowed;
    showCommittedGuideRecoveryActions(run);
  }

  function renderReviewStep(run: FirstRunState): void {
    setFirstRunSectionVisible(elements.setupReview, true);
    showCommittedGuideRecoveryActions(run);
    const storagePath = firstRunStoragePath(run.canonicalRoot);
    elements.setupStorageSummary.textContent = run.storageAlreadyCreated
      ? `${storagePath} is already durable for this operation.`
      : resumingOperation(run)
      ? `Resume this operation and create ${storagePath}.`
      : `Create ${storagePath}.`;
    elements.setupUntouchedRoot.textContent =
      `No files inside ${run.canonicalRoot} beyond this complete list will change.`;
    elements.setupReviewGoal.textContent = run.goal;
    const guideCopy = resumingOperation(run) && run.guideFiles.length === 0
      ? durableProjectGuideReviewCopy(run.guideChoice)
      : projectGuideReviewCopy(
        run.guideChoice,
        run.guideFiles,
      );
    elements.setupReviewGuideChoice.textContent = guideCopy.label;
    elements.setupReviewGuideDetail.textContent = guideCopy.detail;
    appendTextItems(elements.setupReviewGuideChanges, guideCopy.changes);
    appendTextItems(elements.setupCompleteChanges, [
      `${storagePath} · ${run.storageAlreadyCreated ? "already created" : "create"} private project database`,
      ...guideCopy.changes,
    ]);
    elements.setupReviewBack.hidden = run.resumeLocked;
    elements.setupReviewCancel.hidden = run.resumeLocked;
    elements.setupCommit.textContent = resumingOperation(run)
      ? run.resumeNoWrite ? "Resume Initialization" : "Finish Initialization"
      : "Initialize Project";
  }

  function renderRecoveryStep(run: FirstRunState): void {
    setFirstRunSectionVisible(elements.setupRecoveryActions, true);
    const recoveryActions = firstRunRecoveryActions(
      run.recoveryMode,
      run.checkpoint,
    );
    elements.setupResume.hidden = !recoveryActions.resume;
    elements.setupOpenRecovery.hidden = !recoveryActions.open;
    elements.setupRecoveryHelp.hidden = !recoveryActions.help;
    elements.setupRecoveryChoose.hidden = !recoveryActions.chooseAnother;
    elements.setupReturnWelcome.hidden = !recoveryActions.returnToWelcome;
  }

  function renderFailedStep(run: FirstRunState): void {
    setFirstRunSectionVisible(elements.setupRecoveryActions, true);
    elements.setupRetry.hidden = !run.canonicalRoot;
    elements.setupRecoveryChoose.hidden = !run.canonicalRoot;
    elements.setupReturnWelcome.textContent = run.errorKind === "project-not-found"
      ? "Cancel Setup"
      : "Return to Projects";
  }

  // The step's sections and actions, returning the copy the card shows when
  // it depends on more than the phase.
  function renderStep(run: FirstRunState): SetupContent | undefined {
    switch (run.phase) {
      case "existing":
        showTarget(run);
        setFirstRunSectionVisible(elements.setupExistingActions, true);
        return undefined;
      case "target-new":
        showTarget(run);
        setFirstRunSectionVisible(elements.setupNewTargetActions, true);
        return undefined;
      case "goal":
        showTarget(run);
        renderGoalStep(run);
        return undefined;
      case "guide":
        showTarget(run);
        return renderGuideStep(run);
      case "guide-previewing":
      case "committing":
      case "reconciling":
        showTarget(run);
        return undefined;
      case "guide-stale":
        showTarget(run);
        renderGuideStaleStep(run);
        return guideStaleCopy(run);
      case "review":
        showTarget(run);
        renderReviewStep(run);
        return reviewStepCopy(run);
      case "uncertain":
        showTarget(run);
        setFirstRunSectionVisible(elements.setupUncertainActions, true);
        return undefined;
      case "recovery":
        if (run.canonicalRoot) showTarget(run);
        renderRecoveryStep(run);
        return undefined;
      case "failed":
        if (run.canonicalRoot) showTarget(run);
        renderFailedStep(run);
        return undefined;
      default:
        return undefined;
    }
  }

  function renderFirstRunFlow(focus = false): void {
    const run = ctx.state.firstRunState;
    resetSetupCard(run);
    if (run.phase !== "idle") {
      const content = renderStep(run) ?? fixedStepCopy[run.phase]?.(run);
      if (content) setSetupContent(content);
    }
    if (focus) focusFirstRunTarget();
  }

  function hydratePendingInitialization(pending: PendingInitialization): boolean {
    const event = pendingInitializationEvent(pending);
    if (!event) return false;
    setFirstRunState({ type: "validate" });
    setFirstRunState(event, true);
    return true;
  }

  function submitFirstRunGoal(event: SubmitEvent): void {
    event.preventDefault();
    const validation = validateNorthStarGoal(elements.setupGoal.value);
    if (validation.error) {
      setFirstRunState({
        type: "goalInvalid",
        goal: elements.setupGoal.value,
        message: validation.error,
      }, true);
      return;
    }
    setFirstRunState({ type: "goalAccepted", goal: validation.value }, true);
  }

  function preserveFirstRunGoalDraft(): void {
    ctx.state.firstRunState = reduceFirstRun(ctx.state.firstRunState, {
      type: "goalDrafted",
      goal: elements.setupGoal.value,
    });
    elements.setupGoalError.textContent = "";
    elements.setupGoal.removeAttribute("aria-invalid");
  }

  function returnToSelectedFirstRunFolder(): void {
    preserveFirstRunGoalDraft();
    setFirstRunState({ type: "back" }, true);
  }

  async function previewFirstRunGuide(): Promise<void> {
    if (!(ctx.state.firstRunState.phase === "guide" || ctx.state.firstRunState.phase === "guide-stale")) return;
    const operationId = ctx.state.firstRunState.operationId;
    const canonicalRoot = ctx.state.firstRunState.canonicalRoot;
    const stillCurrent = () =>
      ctx.state.firstRunState.operationId === operationId &&
      ctx.state.firstRunState.canonicalRoot === canonicalRoot;
    setFirstRunState({ type: "guidePreviewStarted" }, true);
    try {
      const preview = parseProjectGuidePreview(
        await api().PreviewProjectGuideV1({ operationId, root: canonicalRoot }),
      );
      if (!stillCurrent()) return;
      setFirstRunState({ type: "guidePreviewed", preview }, true);
    } catch (error) {
      if (!stillCurrent()) return;
      setFirstRunState({
        type: "guidePreviewFailed",
        message: `Guide preview is unavailable: ${messageFrom(error)}`,
      }, true);
    }
  }

  function continueFirstRunWithoutGuide(): void {
    setFirstRunState({ type: "guideSkipped" }, true);
  }

  function continueFirstRunWithGuide(): void {
    setFirstRunState({ type: "guideInstalled" }, true);
  }

  function stepBack(): void {
    setFirstRunState({ type: "back" }, true);
  }

  function cancelFirstRunSetup(): void {
    if (
      ctx.state.firstRunState.resumeLocked ||
      ctx.state.firstRunState.recoveryMode === "durable" ||
      ["committing", "reconciling", "uncertain"].includes(ctx.state.firstRunState.phase)
    ) return;
    if (
      (ctx.state.firstRunState.goal || elements.setupGoal.value.trim()) &&
      !window.confirm("Cancel setup? Your project has not been initialized.")
    ) return;
    const focusId = ctx.state.firstRunState.returnFocusId;
    setFirstRunState({ type: "reset", focusId }, true);
  }

  function chooseAnotherFirstRunFolder(): void {
    if (ctx.state.firstRunState.recoveryMode === "durable") return;
    const pickerCancelState = { ...ctx.state.firstRunState };
    const returnFocus = ctx.state.firstRunState.phase === "existing"
      ? elements.setupExistingChoose
      : ctx.state.firstRunState.phase === "target-new"
      ? elements.setupNewTargetChoose
      : elements.setupRecoveryChoose;
    if (ctx.state.firstRunState.intent === "open") {
      void ctx.lifecycle.requestOpenProject("", returnFocus, pickerCancelState);
    } else {
      void ctx.lifecycle.requestInitializeProject(returnFocus, pickerCancelState);
    }
  }

  function retryFirstRunValidation(): void {
    if (ctx.state.firstRunState.phase !== "failed" || !ctx.state.firstRunState.canonicalRoot) return;
    const root = ctx.state.firstRunState.canonicalRoot;
    if (ctx.state.firstRunState.intent === "open") {
      void ctx.lifecycle.requestOpenProject(root, elements.stateOpen);
    } else {
      void ctx.lifecycle.validateInitializeTarget(root);
    }
  }

  function returnFirstRunToWelcome(): void {
    if (
      ctx.state.firstRunState.phase === "failed" ||
      (ctx.state.firstRunState.phase === "recovery" &&
        ["blocked", "ambiguous"].includes(ctx.state.firstRunState.recoveryMode))
    ) {
      const focusId = ctx.state.firstRunState.returnFocusId;
      setFirstRunState({ type: "reset", focusId }, true);
      return;
    }
    cancelFirstRunSetup();
  }

  function bindGuideSteps(): void {
    elements.setupGuidePreviewButton.addEventListener("click", () => void previewFirstRunGuide());
    elements.setupGuideReviewAgain.addEventListener("click", () => void previewFirstRunGuide());
    for (const skip of [elements.setupGuideSkip, elements.setupGuidePreviewSkip, elements.setupGuideStaleSkip]) {
      skip.addEventListener("click", continueFirstRunWithoutGuide);
    }
    elements.setupGuideInstall.addEventListener("click", continueFirstRunWithGuide);
    for (const back of [elements.setupGuideBack, elements.setupGuidePreviewBack, elements.setupGuideStaleBack]) {
      back.addEventListener("click", stepBack);
    }
    for (const cancel of [elements.setupGuideCancel, elements.setupGuidePreviewCancel, elements.setupGuideStaleCancel]) {
      cancel.addEventListener("click", cancelFirstRunSetup);
    }
  }

  function bind(): void {
    elements.setupGoalForm.addEventListener("submit", submitFirstRunGoal);
    elements.setupGoal.addEventListener("input", preserveFirstRunGoalDraft);
    elements.setupGoalBack.addEventListener("click", returnToSelectedFirstRunFolder);
    elements.setupGoalCancel.addEventListener("click", cancelFirstRunSetup);
    bindGuideSteps();
    elements.setupReviewBack.addEventListener("click", stepBack);
    elements.setupReviewCancel.addEventListener("click", cancelFirstRunSetup);
    elements.setupCommit.addEventListener("click", () => void ctx.lifecycle.commitFirstRunProject());
    elements.setupOpenExisting.addEventListener("click", () => void ctx.lifecycle.openExistingFromSetup());
    elements.setupExistingChoose.addEventListener("click", chooseAnotherFirstRunFolder);
    elements.setupExistingCancel.addEventListener("click", cancelFirstRunSetup);
    elements.setupNewTargetContinue.addEventListener("click", () =>
      setFirstRunState({ type: "continueToGoal" }, true)
    );
    elements.setupNewTargetChoose.addEventListener("click", chooseAnotherFirstRunFolder);
    elements.setupNewTargetCancel.addEventListener("click", cancelFirstRunSetup);
    elements.setupRetry.addEventListener("click", retryFirstRunValidation);
    elements.setupResume.addEventListener("click", () => void ctx.lifecycle.resumeFirstRunSetup());
    elements.setupOpenRecovery.addEventListener("click", () => void ctx.lifecycle.openProjectFromRecovery());
    elements.setupRecoveryHelp.addEventListener("click", () =>
      openHelpDestination("project-recovery")
    );
    elements.setupRecoveryChoose.addEventListener("click", chooseAnotherFirstRunFolder);
    elements.setupReturnWelcome.addEventListener("click", returnFirstRunToWelcome);
    elements.setupCheckStatus.addEventListener("click", () => void ctx.lifecycle.retryInitializationStatus());
  }

  return {
    bind,
    setFirstRunState,
    renderFirstRunFlow,
    hydratePendingInitialization,
  };
}

export type FirstRunView = ReturnType<typeof createFirstRunView>;
