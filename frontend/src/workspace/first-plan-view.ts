import type { AppContext } from "./app-context";
import type { WorkspaceTicket } from "./controller";
import { element, setAriaBoolean, setFirstRunSectionVisible } from "./dom";
import {
  createFirstPlan,
  createFirstTask,
  firstPlanExitFocusTarget,
  firstPlanFocusTarget,
  reduceFirstPlan,
  startFirstTask as runStartFirstTask,
  validateOnboardingTitle,
  type FirstPlanEvent,
  type FirstPlanPhase,
  type FirstPlanState,
} from "./first-plan";
import { messageFrom } from "./format";
import { postProjectOnboardingActions } from "./presentation";

export function createFirstPlanView(ctx: AppContext) {
  const { api, workspaceController } = ctx;
  const elements = {
    boardPanelToggle: element("#board-panel-toggle", HTMLButtonElement),
    closeProject: element("#close-project-button", HTMLButtonElement),
    issuesPage: element("#issues-page", HTMLElement),
    navBoard: element("#nav-board", HTMLButtonElement),
    navIssues: element("#nav-issues", HTMLButtonElement),
    navOverview: element("#nav-overview", HTMLButtonElement),
    onboarding: element("#post-project-onboarding", HTMLElement),
    onboardingActivePlan: element("#onboarding-active-plan", HTMLParagraphElement),
    onboardingCreatePlan: element("#onboarding-create-plan", HTMLButtonElement),
    onboardingCreateTask: element("#onboarding-create-task", HTMLButtonElement),
    onboardingDetail: element("#onboarding-detail", HTMLParagraphElement),
    onboardingError: element("#onboarding-error", HTMLParagraphElement),
    onboardingFinishSetup: element("#onboarding-finish-setup", HTMLButtonElement),
    onboardingFinishWithPlan: element("#onboarding-finish-with-plan", HTMLButtonElement),
    onboardingHeading: element("#onboarding-heading", HTMLHeadingElement),
    onboardingOperation: element("#onboarding-operation", HTMLDivElement),
    onboardingPlanError: element("#onboarding-plan-error", HTMLParagraphElement),
    onboardingPlanForm: element("#onboarding-plan-form", HTMLFormElement),
    onboardingPlanTitle: element("#onboarding-plan-title", HTMLInputElement),
    onboardingProgress: element("#onboarding-progress", HTMLParagraphElement),
    onboardingRetryStart: element("#onboarding-retry-start", HTMLButtonElement),
    onboardingSkipPlan: element("#onboarding-skip-plan", HTMLButtonElement),
    onboardingStartFailedActions: element("#onboarding-start-failed-actions", HTMLDivElement),
    onboardingStartNow: element("#onboarding-start-now", HTMLInputElement),
    onboardingStatus: element("#onboarding-status", HTMLParagraphElement),
    onboardingTaskError: element("#onboarding-task-error", HTMLParagraphElement),
    onboardingTaskForm: element("#onboarding-task-form", HTMLFormElement),
    onboardingTaskTitle: element("#onboarding-task-title", HTMLInputElement),
    overviewPage: element("#overview-page", HTMLElement),
    planAdd: element("#plan-add", HTMLButtonElement),
    planFilterToggle: element("#plan-filter-toggle", HTMLButtonElement),
    planList: element("#sidebar-plan-list", HTMLDivElement),
    setupPanel: element("#setup-panel", HTMLElement),
    sidebarResize: element("#sidebar-resize", HTMLDivElement),
    sidebarToggle: element("#sidebar-toggle", HTMLButtonElement),
    stateCard: element("#project-state-card", HTMLDivElement),
    stateScreen: element("#workspace-state-screen", HTMLElement),
    switchProject: element("#switch-project-button", HTMLButtonElement),
    terminalPanelToggle: element("#terminal-panel-toggle", HTMLButtonElement),
    welcomePanel: element("#welcome-panel", HTMLDivElement),
    workspace: element("#workspace", HTMLElement),
  };

  function setFirstPlanState(event: FirstPlanEvent, focus = false): void {
    ctx.state.firstPlanState = reduceFirstPlan(ctx.state.firstPlanState, event);
    renderFirstPlanOnboarding(focus);
  }

  function focusFirstPlanTarget(): void {
    const target = document.getElementById(firstPlanFocusTarget(ctx.state.firstPlanState));
    requestAnimationFrame(() => target?.focus());
  }

  // Onboarding owns the window until it finishes: the plan list, sidebar and
  // panels hold still while it asks for the first plan and task.
  function lockLayout(active: boolean): void {
    ctx.updates.updateAboutUpdatesAvailability();
    setFirstRunSectionVisible(elements.onboarding, active);
    elements.planList.inert = active;
    elements.planAdd.disabled = active || workspaceController.state.status !== "open";
    elements.planFilterToggle.disabled = active || workspaceController.state.status !== "open";
    elements.sidebarToggle.disabled = active;
    elements.sidebarResize.inert = active;
    if (ctx.state.terminalHandle) ctx.state.terminalHandle.setLayoutLocked(active);
    else if (active) {
      elements.boardPanelToggle.disabled = true;
      elements.terminalPanelToggle.disabled = true;
    }
  }

  function showOnboardingScreen(): void {
    elements.stateScreen.hidden = false;
    elements.workspace.hidden = true;
    elements.overviewPage.hidden = true;
    elements.issuesPage.hidden = true;
    elements.welcomePanel.hidden = true;
    elements.welcomePanel.inert = true;
    elements.setupPanel.hidden = true;
    elements.setupPanel.inert = true;
    elements.navBoard.disabled = true;
    elements.navOverview.disabled = true;
    elements.navIssues.disabled = true;
    elements.switchProject.disabled = true;
    elements.closeProject.disabled = true;
  }

  function resetOnboardingCard(plan: FirstPlanState): void {
    elements.stateCard.removeAttribute("aria-busy");
    setFirstRunSectionVisible(elements.onboardingPlanForm, false);
    setFirstRunSectionVisible(elements.onboardingTaskForm, false);
    setFirstRunSectionVisible(elements.onboardingStartFailedActions, false);
    elements.onboardingPlanError.textContent = "";
    elements.onboardingTaskError.textContent = "";
    elements.onboardingStatus.textContent = "";
    elements.onboardingError.textContent = "";
    elements.onboardingPlanTitle.removeAttribute("aria-invalid");
    elements.onboardingTaskTitle.removeAttribute("aria-invalid");
    setAriaBoolean(
      elements.onboardingOperation,
      "aria-busy",
      ["creating-plan", "creating-task", "starting-task"].includes(
        plan.phase,
      ),
    );
  }

  function renderPlanStep(plan: FirstPlanState): void {
    setFirstRunSectionVisible(elements.onboardingPlanForm, true);
    elements.onboardingProgress.textContent = "Next step · Plan";
    elements.onboardingHeading.textContent = "Create the first plan";
    elements.onboardingDetail.textContent =
      "Give this project an active plan, or skip for now and use the empty workspace.";
    elements.onboardingPlanTitle.value = plan.planTitle;
    elements.onboardingPlanError.textContent = plan.planError;
    setAriaBoolean(
      elements.onboardingPlanTitle,
      "aria-invalid",
      Boolean(plan.planError),
    );
    const actions = postProjectOnboardingActions(
      plan.phase === "plan-failed" ? "plan-failed" : "plan",
    );
    elements.onboardingCreatePlan.textContent = actions.primary;
    elements.onboardingSkipPlan.textContent = actions.secondary;
    elements.onboardingPlanForm.inert = plan.phase === "creating-plan";
    if (plan.phase === "creating-plan") {
      elements.onboardingStatus.textContent = "Saving or reconciling the first plan…";
    } else if (plan.phase === "plan-failed") {
      elements.onboardingError.textContent = plan.message;
    }
  }

  function renderTaskStep(plan: FirstPlanState): void {
    setFirstRunSectionVisible(elements.onboardingTaskForm, true);
    elements.onboardingProgress.textContent = "Next step · Task";
    elements.onboardingHeading.textContent = "Add the first task";
    elements.onboardingDetail.textContent =
      "The plan is active. Add one task, then choose whether to start it now.";
    elements.onboardingActivePlan.textContent = plan.activePlanTitle;
    elements.onboardingTaskTitle.value = plan.taskTitle;
    elements.onboardingStartNow.checked = plan.startNow;
    elements.onboardingTaskError.textContent = plan.taskError;
    setAriaBoolean(
      elements.onboardingTaskTitle,
      "aria-invalid",
      Boolean(plan.taskError),
    );
    const actions = postProjectOnboardingActions(
      plan.phase === "task-create-failed" ? "task-create-failed" : "task",
    );
    elements.onboardingCreateTask.textContent = actions.primary;
    elements.onboardingFinishWithPlan.textContent = actions.secondary;
    elements.onboardingTaskForm.inert = plan.phase === "creating-task";
    if (plan.phase === "creating-task") {
      elements.onboardingStatus.textContent = "Saving or reconciling the first task…";
    } else if (plan.phase === "task-create-failed") {
      elements.onboardingError.textContent = plan.message;
    }
  }

  function renderStartingStep(plan: FirstPlanState): void {
    elements.onboardingProgress.textContent = "Next step · Start task";
    elements.onboardingHeading.textContent = "Reconciling the requested start…";
    elements.onboardingDetail.textContent =
      `Task #${plan.taskId} is durable. p-track is reconciling the requested start.`;
    elements.onboardingStatus.textContent = "Checking the explicit start request…";
  }

  function renderStartFailedStep(plan: FirstPlanState): void {
    setFirstRunSectionVisible(elements.onboardingStartFailedActions, true);
    elements.onboardingProgress.textContent = "Task saved · Start stopped";
    const actions = postProjectOnboardingActions("task-start-failed");
    elements.onboardingHeading.textContent = "Check whether the task started";
    elements.onboardingDetail.textContent =
      `Task #${plan.taskId} and plan “${plan.activePlanTitle}” are durable. Retry safely to reconcile its status.`;
    elements.onboardingRetryStart.textContent = actions.primary;
    elements.onboardingFinishSetup.textContent = actions.secondary;
    elements.onboardingError.textContent = plan.message;
  }

  function renderFirstPlanOnboarding(focus = false): void {
    const plan = ctx.state.firstPlanState;
    const active = plan.phase !== "idle" &&
      workspaceController.state.status === "open";
    lockLayout(active);
    if (!active) return;
    showOnboardingScreen();
    resetOnboardingCard(plan);
    if (["plan", "creating-plan", "plan-failed"].includes(plan.phase)) {
      renderPlanStep(plan);
    } else if (["task", "creating-task", "task-create-failed"].includes(plan.phase)) {
      renderTaskStep(plan);
    } else if (plan.phase === "starting-task") {
      renderStartingStep(plan);
    } else if (plan.phase === "task-start-failed") {
      renderStartFailedStep(plan);
    }
    if (focus) focusFirstPlanTarget();
  }

  function beginFirstPlanOnboarding(generation: number): void {
    setFirstPlanState({ type: "begin", generation }, true);
  }

  async function finishFirstPlanOnboarding(planId = ctx.state.firstPlanState.planId): Promise<void> {
    setFirstPlanState({ type: "finish" });
    elements.stateCard.removeAttribute("aria-busy");
    elements.stateScreen.hidden = true;
    ctx.shell.setWorkspacePagesBusy(false);
    elements.navBoard.disabled = false;
    elements.navOverview.disabled = false;
    elements.navIssues.disabled = false;
    elements.switchProject.disabled = false;
    elements.closeProject.disabled = false;
    ctx.state.view = "board";
    ctx.shell.applyView();
    document.getElementById(
      firstPlanExitFocusTarget(planId, ctx.layout.sidebarHeadingUnavailableForFocus()),
    )?.focus();
    await ctx.snapshot.loadSnapshot(planId > 0 ? planId : 0);
  }

  function onboardingContextIsCurrent(ticket: WorkspaceTicket, phase: FirstPlanPhase): boolean {
    const current = workspaceController.capture();
    return workspaceController.state.status === "open" &&
      current.epoch === ticket.epoch &&
      current.generation === ticket.generation &&
      ctx.state.firstPlanState.generation === ticket.generation &&
      ctx.state.firstPlanState.phase === phase;
  }

  function onboardingResponseIsCurrent(
    ticket: WorkspaceTicket,
    phase: FirstPlanPhase,
    generation: number,
  ): boolean {
    return onboardingContextIsCurrent(ticket, phase) &&
      workspaceController.accepts(ticket, generation);
  }

  async function submitFirstPlan(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!(ctx.state.firstPlanState.phase === "plan" || ctx.state.firstPlanState.phase === "plan-failed")) return;
    const validation = validateOnboardingTitle(elements.onboardingPlanTitle.value, "plan");
    if (validation.error) {
      setFirstPlanState({
        type: "planInvalid",
        title: elements.onboardingPlanTitle.value,
        message: validation.error,
      }, true);
      return;
    }
    const ticket = workspaceController.capture();
    setFirstPlanState({ type: "createPlan", title: validation.value }, true);
    try {
      const result = await createFirstPlan(
        api(),
        ticket.generation,
        validation.value,
      );
      if (!onboardingResponseIsCurrent(
        ticket,
        "creating-plan",
        result.state.generation,
      )) return;
      setFirstPlanState({
        type: "planCreated",
        planId: result.plan.id,
        title: result.plan.title,
      }, true);
    } catch (error) {
      if (!onboardingContextIsCurrent(ticket, "creating-plan")) return;
      setFirstPlanState({
        type: "planFailed",
        message: `p-track could not confirm the first plan. Try Again to reconcile safely: ${messageFrom(error)}`,
      }, true);
    }
  }

  async function submitFirstTask(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!(ctx.state.firstPlanState.phase === "task" || ctx.state.firstPlanState.phase === "task-create-failed")) return;
    const validation = validateOnboardingTitle(elements.onboardingTaskTitle.value, "task");
    const startNow = elements.onboardingStartNow.checked;
    if (validation.error) {
      setFirstPlanState({
        type: "taskInvalid",
        title: elements.onboardingTaskTitle.value,
        startNow,
        message: validation.error,
      }, true);
      return;
    }
    const ticket = workspaceController.capture();
    const planId = ctx.state.firstPlanState.planId;
    setFirstPlanState({
      type: "createTask",
      title: validation.value,
      startNow,
    }, true);
    try {
      const result = await createFirstTask(
        api(),
        ticket.generation,
        planId,
        validation.value,
      );
      if (!onboardingResponseIsCurrent(
        ticket,
        "creating-task",
        result.state.generation,
      )) return;
      setFirstPlanState({
        type: "taskCreated",
        taskId: result.task.id,
        updatedAt: result.task.updatedAt,
        status: result.task.status,
      }, true);
      if (result.task.status === "doing" || !startNow) {
        await finishFirstPlanOnboarding(planId);
        return;
      }
      await startFirstTask();
    } catch (error) {
      if (!onboardingContextIsCurrent(ticket, "creating-task")) return;
      setFirstPlanState({
        type: "taskFailed",
        message: `p-track could not confirm the first task. Try Again to reconcile safely: ${messageFrom(error)}`,
      }, true);
    }
  }

  async function startFirstTask(): Promise<void> {
    if (ctx.state.firstPlanState.phase !== "starting-task") return;
    const ticket = workspaceController.capture();
    const planId = ctx.state.firstPlanState.planId;
    const taskId = ctx.state.firstPlanState.taskId;
    const taskTitle = ctx.state.firstPlanState.taskTitle;
    const expectedUpdatedAt = ctx.state.firstPlanState.taskUpdatedAt;
    try {
      const result = await runStartFirstTask(
        api(),
        ticket.generation,
        planId,
        taskId,
        taskTitle,
        expectedUpdatedAt,
      );
      if (!onboardingResponseIsCurrent(
        ticket,
        "starting-task",
        result.state.generation,
      )) return;
      setFirstPlanState({ type: "taskStarted" });
      await finishFirstPlanOnboarding(planId);
    } catch (error) {
      if (!onboardingContextIsCurrent(ticket, "starting-task")) return;
      setFirstPlanState({
        type: "taskStartFailed",
        message: `p-track could not confirm whether the task started. Try Starting Again to reconcile safely: ${messageFrom(error)}`,
      }, true);
    }
  }

  function retryFirstTaskStart(): void {
    if (ctx.state.firstPlanState.phase !== "task-start-failed") return;
    setFirstPlanState({ type: "retryStart" }, true);
    void startFirstTask();
  }

  function bind(): void {
    elements.onboardingPlanForm.addEventListener("submit", (event) => void submitFirstPlan(event));
    elements.onboardingSkipPlan.addEventListener("click", () => void finishFirstPlanOnboarding(0));
    elements.onboardingTaskForm.addEventListener("submit", (event) => void submitFirstTask(event));
    elements.onboardingFinishWithPlan.addEventListener("click", () =>
      void finishFirstPlanOnboarding(ctx.state.firstPlanState.planId),
    );
    elements.onboardingRetryStart.addEventListener("click", retryFirstTaskStart);
    elements.onboardingFinishSetup.addEventListener("click", () =>
      void finishFirstPlanOnboarding(ctx.state.firstPlanState.planId),
    );
  }

  return {
    bind,
    renderFirstPlanOnboarding,
    beginFirstPlanOnboarding,
  };
}

export type FirstPlanView = ReturnType<typeof createFirstPlanView>;
