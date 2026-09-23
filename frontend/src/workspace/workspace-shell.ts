import type { AppContext, WorkspaceView } from "./app-context";
import type { WorkspaceTicket } from "./controller";
import { element, emptyMemory } from "./dom";
import { initialFirstPlanState } from "./first-plan";
import { initialFirstRunState } from "./first-run";
import {
  appVersionLabel,
  workspaceProjectChanged,
  workspaceStateCopy,
} from "./presentation";
import type { WorkspaceStateResponse } from "./snapshot-types";

export function workspaceView(value: unknown): WorkspaceView {
  return value === "overview" || value === "issues" ? value : "board";
}

export function createWorkspaceShell(ctx: AppContext) {
  const { setStatus, workspaceController } = ctx;
  const elements = {
    app: element("#app", HTMLDivElement),
    appVersion: element("#app-version", HTMLButtonElement),
    boardPanelToggle: element("#board-panel-toggle", HTMLButtonElement),
    boardShell: element("#board-shell", HTMLElement),
    closeProject: element("#close-project-button", HTMLButtonElement),
    issuesHeading: element("#issues-heading", HTMLHeadingElement),
    issuesPage: element("#issues-page", HTMLElement),
    navBoard: element("#nav-board", HTMLButtonElement),
    navIssues: element("#nav-issues", HTMLButtonElement),
    navOverview: element("#nav-overview", HTMLButtonElement),
    openProject: element("#open-project-button", HTMLButtonElement),
    overviewHeading: element("#overview-heading", HTMLHeadingElement),
    overviewPage: element("#overview-page", HTMLElement),
    planAdd: element("#plan-add", HTMLButtonElement),
    planFilterToggle: element("#plan-filter-toggle", HTMLButtonElement),
    planFilters: element("#plan-filters", HTMLDivElement),
    planList: element("#sidebar-plan-list", HTMLDivElement),
    planTitle: element("#plan-title", HTMLHeadingElement),
    planTotal: element("#plan-total", HTMLSpanElement),
    projectName: element("#project-name", HTMLHeadingElement),
    stateCard: element("#project-state-card", HTMLDivElement),
    stateDetail: element("#workspace-state-detail", HTMLParagraphElement),
    stateEyebrow: element("#workspace-state-eyebrow", HTMLParagraphElement),
    stateHeading: element("#workspace-state-heading", HTMLHeadingElement),
    stateInitialize: element("#state-initialize-project-button", HTMLButtonElement),
    stateOpen: element("#state-open-project-button", HTMLButtonElement),
    stateScreen: element("#workspace-state-screen", HTMLElement),
    switchProject: element("#switch-project-button", HTMLButtonElement),
    terminalPanelToggle: element("#terminal-panel-toggle", HTMLButtonElement),
    workArea: element("#workspace .work-area", HTMLElement),
    workspace: element("#workspace", HTMLElement),
  };

  function applyView(): void {
    const open = ctx.state.workspaceState.status === "open";
    // Board and terminal panel toggles mean nothing without a project.
    elements.boardPanelToggle.hidden = !open;
    elements.terminalPanelToggle.hidden = !open;
    // Never reveal a previous project's DOM under a new project's heading.
    for (const panel of [elements.workspace, elements.overviewPage, elements.issuesPage]) {
      panel.style.visibility = open && !ctx.state.snapshot ? "hidden" : "";
    }
    // The workspace (and the terminal dock inside it) stays up on every view;
    // only the page above the dock changes, so the dock never jumps.
    elements.workspace.hidden = !open;
    elements.boardShell.hidden = !open || ctx.state.view !== "board";
    elements.workArea.dataset.view = ctx.state.view;
    elements.overviewPage.hidden = !open || ctx.state.view !== "overview";
    elements.issuesPage.hidden = !open || ctx.state.view !== "issues";
    elements.navBoard.classList.toggle("active", ctx.state.view === "board");
    elements.navOverview.classList.toggle("active", ctx.state.view === "overview");
    elements.navIssues.classList.toggle("active", ctx.state.view === "issues");
    if (ctx.state.view === "board") elements.navBoard.setAttribute("aria-current", "page");
    else elements.navBoard.removeAttribute("aria-current");
    if (ctx.state.view === "overview") elements.navOverview.setAttribute("aria-current", "page");
    else elements.navOverview.removeAttribute("aria-current");
    if (ctx.state.view === "issues") elements.navIssues.setAttribute("aria-current", "page");
    else elements.navIssues.removeAttribute("aria-current");
    // The dock follows only the terminal-panel toggle, never the view.
    ctx.state.terminalHandle?.setVisible(open);
  }

  function setView(nextView: string, focusHeading = false): void {
    if (ctx.state.firstPlanState.phase !== "idle") return;
    ctx.state.view = workspaceView(nextView);
    applyView();
    if (ctx.state.view === "overview") {
      requestAnimationFrame(ctx.overview.fitRecentMemory);
      void ctx.overview.loadHeatmap();
      void ctx.overview.loadStackProfile();
      void ctx.overview.loadProjectHistory();
    }
    if (ctx.state.view === "issues") void ctx.issues.loadIssues();
    ctx.layout.recordProjectLayout();
    if (focusHeading) {
      const focusedView = ctx.state.view;
      requestAnimationFrame(() => {
        if (ctx.state.view !== focusedView || workspaceController.state.status !== "open") return;
        const heading: Record<WorkspaceView, HTMLElement> = {
          board: elements.planTitle,
          overview: elements.overviewHeading,
          issues: elements.issuesHeading,
        };
        heading[focusedView].focus();
      });
    }
  }

  // A project switch drops everything the previous project's pages read.
  function resetForProjectChange(): void {
    ctx.snapshot.discardSnapshotForProjectChange();
    ctx.overview.resetOverviewProjectData();
    ctx.issues.resetIssuesProjectData();
    ctx.issues.showIssuesLoading();
    ctx.overview.clearOverviewCharts();
    ctx.drawer.closeTaskDetail();
    ctx.issues.closeIssueDetail(false);
    ctx.palette.closePalette();
    elements.workspace.dataset.snapshotState = "loading";
  }

  function renderAppVersion(state: WorkspaceStateResponse): void {
    if (typeof state.version !== "string") return;
    const version = appVersionLabel(state.version);
    elements.appVersion.textContent = version;
    elements.appVersion.setAttribute(
      "aria-label",
      `About p-track version ${version} and check for updates`,
    );
  }

  function renderNavigation(open: boolean): void {
    elements.stateScreen.hidden = open;
    elements.navBoard.disabled = !open;
    elements.navOverview.disabled = !open;
    elements.navIssues.disabled = !open;
    elements.planAdd.disabled = !open;
    elements.planFilterToggle.disabled = !open;
    elements.switchProject.hidden = !open;
    elements.closeProject.hidden = !open;
    elements.openProject.hidden = true;
    setWorkspacePagesBusy(false);
    elements.switchProject.disabled = false;
    elements.closeProject.disabled = false;
  }

  function renderOpenWorkspace(
    state: WorkspaceStateResponse,
    projectChanged: boolean,
    focus: boolean,
  ): void {
    ctx.state.firstRunState = { ...initialFirstRunState };
    ctx.firstRun.renderFirstRunFlow(false);
    elements.projectName.textContent = state.project?.name || "Project workspace";
    if (projectChanged) {
      elements.planTotal.textContent = "0";
      elements.planList.replaceChildren(emptyMemory("Loading plans…"));
    }
    void ctx.recent.loadRecentProjects();
    void ctx.terminalBackend.ensureTerminalDock(state.generation, state.project?.root ?? "");
    // The restored view loads its own page the same way a click on it would.
    if (projectChanged) setView(ctx.state.view);
    if (ctx.state.firstPlanState.phase !== "idle") {
      ctx.firstPlan.renderFirstPlanOnboarding(focus);
      return;
    }
    if (focus) {
      requestAnimationFrame(() => {
        (ctx.layout.sidebarHeadingUnavailableForFocus()
          ? elements.planTitle
          : elements.projectName).focus();
      });
    }
  }

  // Closing (or never having) a project tears down every per-project surface
  // before the Projects screen renders.
  function teardownClosedWorkspace(): void {
    ctx.state.firstPlanState = { ...initialFirstPlanState };
    ctx.firstPlan.renderFirstPlanOnboarding(false);
    elements.stateCard.removeAttribute("aria-busy");
    ctx.snapshot.cancelSnapshotsForClose();
    ctx.agentActivity.resetAgentActivityAnnouncement();
    ctx.agentLaunch.closeAgentLaunchPicker(false, true);
    ctx.association.closeTerminalAssociationEditor(false, true);
    ctx.writeback.closeTerminalWriteback(false, true);
    ctx.snapshot.closeTaskTransition(false, false, true);
    ctx.terminalBackend.disposeTerminalDock();
    ctx.drawer.closeTaskDetail();
    ctx.issues.closeIssueDetail(false);
    ctx.palette.closePalette();
    ctx.overview.resetOverviewProjectData();
    ctx.state.board = null;
    ctx.board.clearPlanFilters();
    elements.planFilters.hidden = true;
    elements.planFilterToggle.setAttribute("aria-expanded", "false");
    ctx.state.snapshot = null;
    ctx.issues.resetIssuesProjectData();
    elements.projectName.textContent = "Project workspace";
    elements.planTotal.textContent = "0";
    elements.planList.replaceChildren(emptyMemory("No project open."));
  }

  function renderClosedWorkspace(state: WorkspaceStateResponse, focus: boolean): void {
    teardownClosedWorkspace();
    const copy = workspaceStateCopy(state.status, state.error);
    elements.stateEyebrow.textContent = copy.eyebrow;
    elements.stateHeading.textContent = state.status === "welcome" ? "Projects" : copy.heading;
    elements.stateDetail.textContent = state.status === "welcome" ? "Choose a project to preview its work." : copy.detail;
    elements.stateOpen.hidden = state.status === "loading";
    elements.stateInitialize.hidden = state.status === "loading" || state.status !== "welcome";
    if (ctx.state.firstRunState.phase === "idle") ctx.firstRun.renderFirstRunFlow(false);
    if (state.status !== "loading") void ctx.recent.loadRecentProjects();
    if (focus) {
      requestAnimationFrame(() => {
        if (state.status === "welcome" && focusLandingSearch()) return;
        if (!elements.stateOpen.hidden) elements.stateOpen.focus();
        else elements.stateHeading.focus();
      });
    }
  }

  function renderWorkspaceState(state: WorkspaceStateResponse, focus = false): void {
    ctx.recent.cancelLandingReadsForEpoch(workspaceController.capture().epoch);
    const projectChanged = workspaceProjectChanged(ctx.state.workspaceState, state);
    if (projectChanged || state.status !== "open") ctx.drawer.clearPendingTaskDetail();
    if (projectChanged) resetForProjectChange();
    ctx.state.workspaceState = state;
    elements.app.dataset.workspaceOpen = String(state.status === "open");
    renderAppVersion(state);
    const open = state.status === "open";
    // A plan dialog belongs to the project that opened it, even mid-submit.
    if (!open || projectChanged) {
      ctx.planDialogs.abandonPlanDialog();
      ctx.planDialogs.hidePlanCloseoutBanner();
    }
    if (!open) ctx.agentActivity.hideAgentActionForms();
    if (projectChanged) ctx.layout.restoreProjectLayout(state.project?.root || "");
    applyView();
    renderNavigation(open);
    if (open) renderOpenWorkspace(state, projectChanged, focus);
    else renderClosedWorkspace(state, focus);
  }

  function focusOpenProject(): void {
    elements.stateOpen.focus();
  }

  function focusLandingSearch(): boolean {
    const search = document.querySelector("#recent-project-search");
    if (!(search instanceof HTMLInputElement) || search.closest("[hidden]")) return false;
    search.focus();
    return true;
  }

  function publishBackendState(
    state: WorkspaceStateResponse,
    transition: WorkspaceTicket | undefined,
    focus = false,
    keepInert = false,
  ): boolean {
    const published = workspaceController.publish(
      { status: state.status, generation: Number(state.generation || 0) },
      transition,
    );
    if (!published) return false;
    renderWorkspaceState(state, focus);
    if (state.status === "open" && keepInert) setWorkspacePagesBusy(true);
    if (state.status === "open" && !keepInert) {
      void ctx.snapshot.loadSnapshot(ctx.layout.restoredPlanId(state.project?.root));
    }
    return true;
  }

  function beginWorkspaceTransition(): WorkspaceTicket {
    // A pending layout save still belongs to the project being left.
    ctx.layout.flushLayoutState();
    ctx.association.closeTerminalAssociationEditor(false, true);
    ctx.writeback.closeTerminalWriteback(false, true);
    ctx.snapshot.closeTaskTransition(false, false, true);
    ctx.agentActivity.hideAgentActionForms();
    const transition = workspaceController.beginTransition();
    if (ctx.state.workspaceState.status === "open") {
      setWorkspacePagesBusy(true);
      elements.switchProject.disabled = true;
      elements.closeProject.disabled = true;
      setStatus("Preparing project transition…");
    } else {
      renderWorkspaceState({
        status: "loading",
        generation: transition.generation,
      });
    }
    return transition;
  }

  // The board, Overview and Issues pages go inert together while a workspace
  // transition is pending, and come back together when it settles.
  function setWorkspacePagesBusy(busy: boolean): void {
    for (const page of [elements.workspace, elements.overviewPage, elements.issuesPage]) {
      page.inert = busy;
      if (busy) page.setAttribute("aria-busy", "true");
      else page.removeAttribute("aria-busy");
    }
  }

  function bind(): void {
    elements.navBoard.addEventListener("click", () => setView("board"));
    elements.navOverview.addEventListener("click", () => setView("overview"));
    elements.navIssues.addEventListener("click", () => setView("issues"));
  }

  return {
    bind,
    applyView,
    setView,
    renderWorkspaceState,
    focusLandingSearch,
    focusOpenProject,
    publishBackendState,
    beginWorkspaceTransition,
    setWorkspacePagesBusy,
  };
}

export type WorkspaceShell = ReturnType<typeof createWorkspaceShell>;
