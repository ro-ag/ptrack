import type { ResetConfirmationCopy } from "../settings/sections";
import type { AppContext } from "./app-context";
import type { WorkspaceTicket } from "./controller";
import { element } from "./dom";
import { messageFrom } from "./format";
import { confirmationCopy } from "./presentation";
import {
  RECENT_RELOCATION_UNCONFIRMED,
  focusAfterForgottenProject,
  parseForgetRecentProjectResult,
  parseRecentProjectOpenResult,
  parseRecentProjectResolution,
  parseRecentProjects,
  recentProjectFocusKey,
  reduceRecentProjects,
  refreshedRecentProjectForOpen,
  type RecentProjectAvailability,
  type RecentProjectEntry,
  type RecentProjectIntent,
  type RecentProjectOpenResult,
  type RecentProjectResolution,
  type RecentProjectsEvent,
} from "./recent-projects";
import type { ActiveResources } from "./snapshot-types";

/** What a confirmation dialog says: an eyebrow, a question, its two answers. */
export type ConfirmationCopy = ResetConfirmationCopy;

/** One recent-project action, fenced to the list and workspace it began in. */
interface RecentOperationTicket {
  operationId: number;
  entryId: string;
  base: string;
  epoch: number;
  generation: number;
  intent: RecentProjectIntent;
}

export interface RecentProjectsReload {
  focusKey?: string;
  announcement?: string;
  errorMessage?: string;
}

export function recentProjectPrimaryLabel(availability: RecentProjectAvailability): string {
  if (availability === "available") return "Open";
  if (availability === "permission-required") return "Try Again";
  return "Locate…";
}

/** Why an open could not be confirmed, in the words the landing shows. */
export function recentOpenFailureMessage(entry: RecentProjectEntry, reason: string): string {
  return reason === "recent-project-entry-stale"
    ? `The Recent projects list for “${entry.name}” changed or expired. p-track refreshed it without replaying Open. Review the row and choose again.`
    : `p-track could not confirm that “${entry.name}” opened. The recent entry was not replayed: ${reason}`;
}

export function createRecentProjectsController(ctx: AppContext) {
  const { api, showError, workspaceController } = ctx;
  const elements = {
    confirmCancel: element("#workspace-confirm-cancel", HTMLButtonElement),
    confirmDetail: element("#workspace-confirm-detail", HTMLParagraphElement),
    confirmEyebrow: element("#workspace-confirm-eyebrow", HTMLParagraphElement),
    confirmHeading: element("#workspace-confirm-heading", HTMLHeadingElement),
    confirmModal: element("#workspace-confirm-modal", HTMLDivElement),
    confirmSubmit: element("#workspace-confirm-submit", HTMLButtonElement),
    recentHeading: element("#recent-project-heading", HTMLHeadingElement),
    welcomePanel: element("#welcome-panel", HTMLDivElement),
  };

  let confirmReturnFocus: HTMLElement | null = null;
  let confirmResolve: ((confirmed: boolean) => void) | null = null;
  let recentListRequest = 0;
  let recentWorkspaceEpoch = 0;
  let recentOperationSequence = 0;

  // -------------------------------------------------------- confirmations

  function showConfirmation(
    copy: ConfirmationCopy,
    returnFocus: Element | null = document.activeElement,
  ): Promise<boolean> {
    confirmReturnFocus = returnFocus instanceof HTMLElement ? returnFocus : null;
    elements.confirmEyebrow.textContent = copy.eyebrow;
    elements.confirmHeading.textContent = copy.heading;
    elements.confirmDetail.textContent = copy.detail;
    elements.confirmCancel.textContent = copy.cancel;
    elements.confirmSubmit.textContent = copy.submit;
    elements.confirmModal.hidden = false;
    requestAnimationFrame(() => elements.confirmCancel.focus());
    return new Promise((resolve) => {
      confirmResolve = resolve;
    });
  }

  function showWorkspaceConfirmation(
    action: "close" | "switch",
    resources: ActiveResources,
    returnFocus: Element | null = document.activeElement,
  ): Promise<boolean> {
    const copy = confirmationCopy(
      action,
      resources.terminals,
      resources.agentRuns,
      resources.pendingAdmissions || 0,
    );
    return showConfirmation({
      eyebrow: "Active project resources",
      heading: copy.heading,
      detail: copy.detail,
      cancel: "Stay here",
      submit: copy.submit,
    }, returnFocus);
  }

  function showRecentRelocationConfirmation(
    entry: RecentProjectEntry,
    resolution: RecentProjectResolution,
  ): Promise<boolean> {
    return showConfirmation({
      eyebrow: "Recent project location",
      heading: "Open a different project?",
      detail:
        `“${entry.name}” at ${entry.canonicalPath} now resolves to “${resolution.name}” at ${resolution.canonicalRoot}. Update this recent entry only after the different project opens?`,
      cancel: "Keep Current Entry",
      submit: "Update and Open",
    }, null);
  }

  function showForgetRecentProjectConfirmation(entry: RecentProjectEntry): Promise<boolean> {
    return showConfirmation({
      eyebrow: "Recent project",
      heading: "Forget this recent project?",
      detail:
        `Remove “${entry.name}” at ${entry.canonicalPath} from Recent projects only. Project files will not be changed.`,
      cancel: "Keep Recent Entry",
      submit: "Remove Recent Entry",
    }, null);
  }

  function finishWorkspaceConfirmation(confirmed: boolean): void {
    if (!confirmResolve) return;
    const resolve = confirmResolve;
    confirmResolve = null;
    ctx.snapshot.hideApplicationOverlay(elements.confirmModal);
    confirmReturnFocus?.focus();
    confirmReturnFocus = null;
    resolve(confirmed);
  }

  // ----------------------------------------------------- recent projects

  function cancelLandingReadsForEpoch(epoch: number): void {
    if (epoch === recentWorkspaceEpoch) return;
    recentWorkspaceEpoch = epoch;
    // A workspace transition invalidates pending landing reads immediately.
    // Clearing loading here lets Back to all projects start a fresh request
    // even while an abandoned startup read is still pending.
    recentListRequest += 1;
    ctx.landing.cancelGlobalOverviewRead();
    ctx.state.recentProjectsState = reduceRecentProjects(ctx.state.recentProjectsState, { type: "loadCancelled" });
  }

  function setRecentProjectsState(event: RecentProjectsEvent): void {
    ctx.state.recentProjectsState = reduceRecentProjects(ctx.state.recentProjectsState, event);
    ctx.landing.renderRecentProjects();
    ctx.landing.renderGlobalOverview();
  }

  function recentProjectActionElement(focusKey: string): HTMLElement {
    if (focusKey === "recent-project-heading") return elements.recentHeading;
    return [...elements.welcomePanel.querySelectorAll<HTMLElement>("[data-recent-focus-key]")]
      .find((candidate) => candidate.dataset.recentFocusKey === focusKey) ||
      elements.recentHeading;
  }

  function restoreRecentProjectFocus(focusKey: string, operationId = recentOperationSequence): void {
    requestAnimationFrame(() => {
      if (
        operationId !== recentOperationSequence ||
        ctx.state.firstRunState.phase !== "idle" ||
        workspaceController.state.status === "open" ||
        workspaceController.state.status === "loading"
      ) return;
      recentProjectActionElement(focusKey)?.focus();
    });
  }

  function recentProjectOperationActive(): boolean {
    return !["idle", "loading", "error"].includes(ctx.state.recentProjectsState.phase);
  }

  function recentProjectActionButton(
    entry: RecentProjectEntry,
    action: RecentProjectIntent,
    label: string,
    describedBy: string,
    handler: (event: MouseEvent) => void,
  ): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "button-secondary";
    button.textContent = label;
    button.dataset.recentFocusKey = recentProjectFocusKey(entry.entryId, action);
    button.setAttribute(
      "aria-label",
      action === "forget"
        ? `Forget ${entry.name} from Recent projects only`
        : `${label.replace("…", "")} ${entry.name}`,
    );
    button.setAttribute("aria-describedby", describedBy);
    button.disabled = ctx.state.recentProjectsState.listLoading ||
      !["idle", "error"].includes(ctx.state.recentProjectsState.phase);
    button.addEventListener("click", handler);
    return button;
  }

  function beginRecentProjectOperation(
    entry: RecentProjectEntry,
    intent: RecentProjectIntent,
  ): RecentOperationTicket | null {
    if (
      ctx.state.recentProjectsState.listLoading ||
      !["idle", "error"].includes(ctx.state.recentProjectsState.phase)
    ) return null;
    const operationId = ++recentOperationSequence;
    const workspace = workspaceController.capture();
    setRecentProjectsState({ type: "begin", operationId, entry, intent });
    return {
      operationId,
      entryId: entry.entryId,
      base: entry.base,
      epoch: workspace.epoch,
      generation: workspace.generation,
      intent,
    };
  }

  function recentProjectOperationMatches(ticket: RecentOperationTicket): boolean {
    return recentOperationSequence === ticket.operationId &&
      ctx.state.recentProjectsState.operationId === ticket.operationId &&
      ctx.state.recentProjectsState.activeEntryId === ticket.entryId &&
      ctx.state.recentProjectsState.activeBase === ticket.base;
  }

  function recentProjectOperationIsCurrent(ticket: RecentOperationTicket): boolean {
    const workspace = workspaceController.capture();
    return recentProjectOperationMatches(ticket) &&
      workspace.epoch === ticket.epoch &&
      workspace.generation === ticket.generation &&
      workspaceController.state.status !== "open" &&
      workspaceController.state.status !== "loading";
  }

  function settleRecentProjectOperation(
    ticket: RecentOperationTicket,
    announcement: string,
    focusKey: string,
  ): void {
    if (!recentProjectOperationIsCurrent(ticket)) return;
    setRecentProjectsState({ type: "settled", announcement });
    restoreRecentProjectFocus(focusKey, ticket.operationId);
  }

  function failRecentProjectOperation(
    ticket: RecentOperationTicket,
    error: unknown,
    focusKey: string,
  ): void {
    if (!recentProjectOperationIsCurrent(ticket)) return;
    const message =
      `p-track could not confirm the recent-project action: ${messageFrom(error)}`;
    setRecentProjectsState({ type: "settled" });
    void loadRecentProjects({
      focusKey,
      errorMessage: message,
    });
  }

  async function refreshRecentProjectsAfterOpen(): Promise<RecentProjectEntry[] | null> {
    const ticket = workspaceController.capture();
    try {
      const projects = parseRecentProjects(await api().GetRecentProjectsV1());
      if (!workspaceController.accepts(ticket, ticket.generation)) return null;
      ctx.state.recentProjectsState = reduceRecentProjects(ctx.state.recentProjectsState, {
        type: "loadStarted",
      });
      ctx.state.recentProjectsState = reduceRecentProjects(ctx.state.recentProjectsState, {
        type: "loaded",
        projects,
      });
      return projects;
    } catch {
      return null;
    }
  }

  async function reconcileRecentProjectOpenFailure(
    ticket: RecentOperationTicket,
    entry: RecentProjectEntry,
    resolution: RecentProjectResolution,
    error: unknown,
  ): Promise<void> {
    try {
      const state = await api().GetWorkspaceState();
      const exactOpen = state?.status === "open" &&
        state.project?.root === resolution.canonicalRoot;
      const publish = () => workspaceController.publish({
        status: state.status,
        generation: Number(state.generation || 0),
      });
      if (exactOpen) {
        ctx.state.recentProjectsState = reduceRecentProjects(ctx.state.recentProjectsState, { type: "settled" });
        publish();
        ctx.shell.renderWorkspaceState(state, true);
        await ctx.snapshot.loadSnapshot(ctx.state.board?.planId || 0);
        if (resolution.canonicalRoot !== entry.canonicalPath) {
          await refreshRecentProjectsAfterOpen();
          showError(new Error(RECENT_RELOCATION_UNCONFIRMED));
        }
        return;
      }
      publish();
      ctx.shell.renderWorkspaceState(state, false);
      if (state.status === "open") {
        showError(error);
        return;
      }
      if (!recentProjectOperationMatches(ticket)) return;
      const focusKey = recentProjectFocusKey(entry.entryId, ticket.intent);
      setRecentProjectsState({ type: "settled" });
      void loadRecentProjects({ focusKey, errorMessage: recentOpenFailureMessage(entry, messageFrom(error)) });
    } catch (stateError) {
      workspaceController.publish({ status: "error", generation: 0 });
      ctx.shell.renderWorkspaceState(
        { status: "error", generation: 0, error: messageFrom(stateError) },
        true,
      );
    }
  }

  function openRecentProjectRequest(
    entry: RecentProjectEntry,
    resolution: RecentProjectResolution,
    confirmationToken: string,
  ): Promise<RecentProjectOpenResult> {
    return api().OpenRecentProjectV1(
      entry.entryId,
      entry.base,
      resolution.canonicalRoot,
      resolution.confirmationToken,
      confirmationToken,
    ).then((value) => parseRecentProjectOpenResult(value, entry));
  }

  // Switching away from an open project with live terminals or agents asks
  // first. A cancel is a definitive "unchanged"; a confirm opens again with
  // the token the first answer carried.
  async function confirmRecentProjectSwitch(
    ticket: RecentOperationTicket,
    entry: RecentProjectEntry,
    result: RecentProjectOpenResult,
    transition: WorkspaceTicket,
  ): Promise<boolean> {
    if (!ctx.shell.publishBackendState(result.open.state, transition, false, true)) return false;
    const confirmed = await showWorkspaceConfirmation(
      "switch",
      result.open.activeResources ?? { terminals: 0, agentRuns: 0 },
      null,
    );
    if (!recentProjectOperationMatches(ticket)) return false;
    if (confirmed) return true;
    await api().CancelWorkspaceChange(result.open.confirmationToken ?? "");
    setRecentProjectsState({
      type: "settled",
      announcement: "Project unchanged.",
    });
    ctx.shell.renderWorkspaceState(result.open.state, false);
    restoreRecentProjectFocus(
      recentProjectFocusKey(entry.entryId, ticket.intent),
      ticket.operationId,
    );
    return false;
  }

  function reportOpenedRecentProject(result: RecentProjectOpenResult): void {
    const warnings: string[] = [];
    if (result.open.warning) warnings.push(result.open.warning);
    if (result.registryStatus === "stale") {
      warnings.push(
        "Project opened, but its recent entry changed elsewhere and was not updated.",
      );
      void refreshRecentProjectsAfterOpen();
    }
    if (warnings.length > 0) showError(new Error(warnings.join(" ")));
  }

  async function openResolvedRecentProject(
    ticket: RecentOperationTicket,
    entry: RecentProjectEntry,
    resolution: RecentProjectResolution,
  ): Promise<void> {
    if (!recentProjectOperationIsCurrent(ticket)) return;
    setRecentProjectsState({ type: "opening" });
    await ctx.state.terminalHandle?.flushPending();
    let transition = ctx.shell.beginWorkspaceTransition();
    try {
      let result = await openRecentProjectRequest(entry, resolution, "");
      if (!recentProjectOperationMatches(ticket)) return;
      if (result.open.requiresConfirmation) {
        if (!(await confirmRecentProjectSwitch(ticket, entry, result, transition))) return;
        transition = ctx.shell.beginWorkspaceTransition();
        result = await openRecentProjectRequest(entry, resolution, result.open.confirmationToken ?? "");
        if (!recentProjectOperationMatches(ticket)) return;
      }
      ctx.state.recentProjectsState = reduceRecentProjects(ctx.state.recentProjectsState, { type: "settled" });
      if (!ctx.shell.publishBackendState(result.open.state, transition, true)) return;
      reportOpenedRecentProject(result);
    } catch (error) {
      await reconcileRecentProjectOpenFailure(ticket, entry, resolution, error);
    }
  }

  async function openAvailableRecentProject(entry: RecentProjectEntry): Promise<void> {
    const ticket = beginRecentProjectOperation(entry, "open");
    if (!ticket) return;
    const focusKey = recentProjectFocusKey(entry.entryId, "open");
    try {
      // A rendered row can outlive the backend's bounded listing lease. Refresh
      // before Open so a screen left idle does not become permanently stale.
      const projects = parseRecentProjects(await api().GetRecentProjectsV1());
      if (!recentProjectOperationIsCurrent(ticket)) return;
      const refreshed = refreshedRecentProjectForOpen(projects, entry);
      if (!refreshed) {
        throw new Error(
          "The recent project changed while p-track refreshed it. Review the updated list and choose again.",
        );
      }
      await openResolvedRecentProject(ticket, refreshed, {
        entryId: refreshed.entryId,
        base: refreshed.base,
        canonicalRoot: refreshed.canonicalPath,
        name: refreshed.name,
        resolution: "ready",
        confirmationToken: "",
      });
    } catch (error) {
      failRecentProjectOperation(ticket, error, focusKey);
    }
  }

  async function retryRecentProject(entry: RecentProjectEntry): Promise<void> {
    const ticket = beginRecentProjectOperation(entry, "retry");
    if (!ticket) return;
    const focusKey = recentProjectFocusKey(entry.entryId, "retry");
    try {
      const resolution = parseRecentProjectResolution(
        await api().ResolveRecentProjectV1(entry.entryId, entry.base, entry.canonicalPath),
        entry,
      );
      if (!recentProjectOperationIsCurrent(ticket)) return;
      if (resolution.resolution === "confirmation-required") {
        setRecentProjectsState({ type: "settled" });
        void loadRecentProjects({
          focusKey,
          errorMessage:
            "This path now contains a different project. Review it with Locate…; Try Again never changes a recent entry.",
        });
        return;
      }
      await openResolvedRecentProject(ticket, entry, resolution);
    } catch (error) {
      failRecentProjectOperation(ticket, error, focusKey);
    }
  }

  async function locateRecentProject(entry: RecentProjectEntry): Promise<void> {
    const ticket = beginRecentProjectOperation(entry, "locate");
    if (!ticket) return;
    const focusKey = recentProjectFocusKey(entry.entryId, "locate");
    try {
      const candidatePath = await ctx.lifecycle.chooseProjectDirectory("locate-recent-project");
      if (!recentProjectOperationIsCurrent(ticket)) return;
      if (!candidatePath) {
        settleRecentProjectOperation(
          ticket,
          "Folder selection canceled. Recent entry unchanged.",
          focusKey,
        );
        return;
      }
      setRecentProjectsState({ type: "resolving" });
      const resolution = parseRecentProjectResolution(
        await api().ResolveRecentProjectV1(entry.entryId, entry.base, candidatePath),
        entry,
      );
      if (!recentProjectOperationIsCurrent(ticket)) return;
      if (resolution.resolution === "confirmation-required") {
        setRecentProjectsState({ type: "confirmRelocation" });
        const confirmed = await showRecentRelocationConfirmation(entry, resolution);
        if (!recentProjectOperationIsCurrent(ticket)) return;
        if (!confirmed) {
          settleRecentProjectOperation(ticket, "Recent entry unchanged.", focusKey);
          return;
        }
      }
      await openResolvedRecentProject(ticket, entry, resolution);
    } catch (error) {
      failRecentProjectOperation(ticket, error, focusKey);
    }
  }

  async function forgetRecentProject(entry: RecentProjectEntry): Promise<void> {
    const ticket = beginRecentProjectOperation(entry, "forget");
    if (!ticket) return;
    const focusKey = recentProjectFocusKey(entry.entryId, "forget");
    const confirmed = await showForgetRecentProjectConfirmation(entry);
    if (!recentProjectOperationIsCurrent(ticket)) return;
    if (!confirmed) {
      settleRecentProjectOperation(ticket, "Recent entry unchanged.", focusKey);
      return;
    }
    setRecentProjectsState({ type: "forgetting" });
    try {
      parseForgetRecentProjectResult(
        await api().ForgetRecentProjectV1(entry.entryId, entry.base),
        entry,
      );
      if (!recentProjectOperationIsCurrent(ticket)) return;
      const nextFocus = focusAfterForgottenProject(
        ctx.state.recentProjectsState.projects,
        entry.entryId,
      );
      setRecentProjectsState({ type: "settled" });
      await loadRecentProjects({
        focusKey: nextFocus,
        announcement:
          `Removed “${entry.name}” from Recent projects. Project files were not changed.`,
      });
    } catch (error) {
      failRecentProjectOperation(ticket, error, focusKey);
    }
  }

  // A list read answers only the Projects screen that asked: another read, a
  // recent-project action, or any workspace movement since makes it stale.
  function listReadIsCurrent(request: number, operationSequence: number, ticket: WorkspaceTicket): boolean {
    const current = workspaceController.capture();
    return request === recentListRequest &&
      operationSequence === recentOperationSequence &&
      current.epoch === ticket.epoch &&
      current.generation === ticket.generation &&
      workspaceController.state.status !== "open" &&
      workspaceController.state.status !== "loading";
  }

  async function loadRecentProjects({
    focusKey = "",
    announcement = "",
    errorMessage = "",
  }: RecentProjectsReload = {}): Promise<boolean> {
    if (
      workspaceController.state.status === "open" ||
      workspaceController.state.status === "loading" ||
      ctx.state.recentProjectsState.phase !== "idle" ||
      ctx.state.recentProjectsState.listLoading
    ) return false;
    void ctx.landing.loadGlobalOverview();
    const request = ++recentListRequest;
    const ticket = workspaceController.capture();
    const operationSequence = recentOperationSequence;
    setRecentProjectsState({ type: "loadStarted" });
    try {
      const projects = parseRecentProjects(await api().GetRecentProjectsV1());
      if (!listReadIsCurrent(request, operationSequence, ticket)) return false;
      setRecentProjectsState({ type: "loaded", projects, announcement });
      if (errorMessage) {
        setRecentProjectsState({ type: "alert", message: errorMessage });
      }
      if (focusKey) {
        requestAnimationFrame(() => {
          if (
            !listReadIsCurrent(request, operationSequence, ticket) ||
            ctx.state.recentProjectsState.phase !== "idle" ||
            ctx.state.recentProjectsState.listLoading ||
            ctx.state.firstRunState.phase !== "idle"
          ) return;
          recentProjectActionElement(focusKey)?.focus();
        });
      }
      return true;
    } catch (error) {
      if (!listReadIsCurrent(request, operationSequence, ticket)) return false;
      setRecentProjectsState({
        type: "loadFailed",
        message: errorMessage
          ? `${errorMessage} Registry reload also failed: ${messageFrom(error)}`
          : `Recent projects are unavailable: ${messageFrom(error)}`,
      });
      if (focusKey) restoreRecentProjectFocus(focusKey);
      return false;
    }
  }

  function bind(): void {
    elements.confirmCancel.addEventListener("click", () => finishWorkspaceConfirmation(false));
    elements.confirmSubmit.addEventListener("click", () => finishWorkspaceConfirmation(true));
  }

  return {
    bind,
    showConfirmation,
    showWorkspaceConfirmation,
    cancelLandingReadsForEpoch,
    finishWorkspaceConfirmation,
    recentProjectPrimaryLabel,
    recentProjectOperationActive,
    recentProjectActionButton,
    openAvailableRecentProject,
    retryRecentProject,
    locateRecentProject,
    forgetRecentProject,
    loadRecentProjects,
  };
}

export type RecentProjectsController = ReturnType<typeof createRecentProjectsController>;
