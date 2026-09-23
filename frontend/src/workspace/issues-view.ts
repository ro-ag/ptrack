import type { AppContext } from "./app-context";
import { element, emptyMemory } from "./dom";
import { messageFrom, shortRelativeTime } from "./format";
import type {
  GenerationReply,
  IssueDetailResponse,
  IssueSummary,
  IssuesResponse,
} from "./snapshot-types";
import { severityColors } from "./task-status";

/** The drawer while a new report is being written: nothing saved yet. */
interface NewIssueDraft {
  newIssue: true;
  issue?: undefined;
}

type IssueDrawerDetail = NewIssueDraft | (IssueDetailResponse & { newIssue?: undefined });

type IssuesPage = Pick<IssuesResponse, "issues" | "bounds" | "offset">;

const ISSUES_PAGE_SIZE = 50;

function emptyIssuesPage(): IssuesPage {
  return { issues: [], bounds: { shown: 0, total: 0 } };
}

/** The inbox status line: a range while paging, a count otherwise. */
export function issuesStatusText(page: IssuesPage, filter: string): string {
  const total = Number(page.bounds?.total || page.issues.length);
  const shown = Number(page.bounds?.shown || page.issues.length);
  return total > shown
    ? `Showing ${Number(page.offset || 0) + 1}–${Number(page.offset || 0) + shown} of ${total} ${filter} issues.`
    : `${page.issues.length} ${filter === "all" ? "total" : filter} issue${page.issues.length === 1 ? "" : "s"}.`;
}

export function issuesEmptyText(filter: string): string {
  return filter === "unscheduled"
    ? "No unscheduled issues. New reports remain triage-only until you schedule them."
    : `No ${filter === "all" ? "" : `${filter} `}issues.`;
}

function issueInboxRow(issue: IssueSummary): HTMLButtonElement {
  const row = document.createElement("button");
  row.type = "button";
  row.className = "issue-inbox-row";
  row.style.setProperty("--issue-color", severityColors[issue.severity] || "var(--muted)");
  row.setAttribute(
    "aria-label",
    `Issue #${issue.id}: ${issue.title}. ${issue.severity} severity, ${issue.status}, ${issue.taskId ? `scheduled as task #${issue.taskId}` : "unscheduled"}.`,
  );
  const marker = document.createElement("span");
  marker.className = "issue-inbox-marker";
  marker.setAttribute("aria-hidden", "true");
  const content = document.createElement("div");
  const title = document.createElement("p");
  title.className = "issue-inbox-title";
  title.textContent = `#${issue.id} ${issue.title}`;
  const meta = document.createElement("p");
  meta.className = "issue-inbox-meta";
  meta.textContent = `${issue.severity} · ${issue.status} · ${shortRelativeTime(issue.updatedAt)}`;
  content.append(title, meta);
  const schedule = document.createElement("p");
  schedule.className = "issue-inbox-plan";
  schedule.textContent = issue.taskId
    ? `Plan #${issue.planId} ${issue.planTitle} · task #${issue.taskId}`
    : "Unscheduled · triage only";
  row.append(marker, content, schedule);
  return row;
}

function selectOption(value: string | number, label: string): HTMLOptionElement {
  const option = document.createElement("option");
  option.value = String(value);
  option.textContent = label;
  return option;
}

export function createIssuesView(ctx: AppContext) {
  const { api, showError, workspaceController } = ctx;
  const elements = {
    drawer: element("#task-drawer", HTMLDivElement),
    drawerClose: element("#drawer-close", HTMLButtonElement),
    issueBody: element("#issue-body", HTMLTextAreaElement),
    issueDrawer: element("#issue-drawer", HTMLDivElement),
    issueDrawerClose: element("#issue-drawer-close", HTMLButtonElement),
    issueDrawerEyebrow: element("#issue-drawer-eyebrow", HTMLParagraphElement),
    issueDrawerHeading: element("#issue-drawer-heading", HTMLHeadingElement),
    issueDrawerUpdated: element("#issue-drawer-updated", HTMLParagraphElement),
    issueForm: element("#issue-form", HTMLFormElement),
    issueFormMessage: element("#issue-form-message", HTMLParagraphElement),
    issueLink: element("#issue-link", HTMLButtonElement),
    issueLinkSummary: element("#issue-link-summary", HTMLParagraphElement),
    issueMove: element("#issue-move", HTMLButtonElement),
    issueMoveHelp: element("#issue-move-help", HTMLParagraphElement),
    issueOpenTask: element("#issue-open-task", HTMLButtonElement),
    issuePlanSelect: element("#issue-plan-select", HTMLSelectElement),
    issueSchedule: element("#issue-schedule", HTMLButtonElement),
    issueScheduling: element("#issue-scheduling", HTMLElement),
    issueSchedulingState: element("#issue-scheduling-state", HTMLSpanElement),
    issueSeverity: element("#issue-severity", HTMLSelectElement),
    issueStatusSelect: element("#issue-status-select", HTMLSelectElement),
    issueTargetBounds: element("#issue-target-bounds", HTMLParagraphElement),
    issueTargetFind: element("#issue-target-find", HTMLButtonElement),
    issueTargetSearch: element("#issue-target-search", HTMLInputElement),
    issueTaskSelect: element("#issue-task-select", HTMLSelectElement),
    issueTitle: element("#issue-title", HTMLInputElement),
    issueUnlink: element("#issue-unlink", HTMLButtonElement),
    issuesFilter: element("#issues-filter", HTMLSelectElement),
    issuesInbox: element("#issues-inbox", HTMLDivElement),
    issuesNew: element("#issues-new", HTMLButtonElement),
    issuesNext: element("#issues-next", HTMLButtonElement),
    issuesPagination: element("#issues-pagination", HTMLDivElement),
    issuesPrevious: element("#issues-previous", HTMLButtonElement),
    issuesStatus: element("#issues-status", HTMLParagraphElement),
    navIssues: element("#nav-issues", HTMLButtonElement),
  };

  let issuesState: IssuesPage = emptyIssuesPage();
  let issuesOffset = 0;
  let issuesRequest = 0;
  let issueDetail: IssueDrawerDetail | null = null;
  let issueRequest = 0;
  let issueReturnFocus: HTMLElement | null = null;
  let issueTargetRequest = 0;

  function renderIssuesInbox(): void {
    const filter = elements.issuesFilter.value;
    const issues = issuesState.issues;
    elements.issuesInbox.replaceChildren();
    elements.issuesInbox.setAttribute("aria-busy", "false");
    const total = Number(issuesState.bounds?.total || issuesState.issues.length);
    const shown = Number(issuesState.bounds?.shown || issuesState.issues.length);
    elements.issuesStatus.textContent = issuesStatusText(issuesState, filter);
    elements.issuesPrevious.disabled = issuesOffset === 0;
    elements.issuesNext.disabled = issuesOffset + shown >= total;
    // A single page needs no pager: hiding it beats two permanently disabled
    // buttons.
    elements.issuesPagination.hidden = total <= shown && issuesOffset === 0;
    if (issues.length === 0) {
      // One message, not a "0 open issues." count above "No open issues.".
      elements.issuesStatus.textContent = "";
      const empty = emptyMemory(issuesEmptyText(filter));
      elements.issuesInbox.append(empty);
      if (filter !== "closed" && filter !== "all") void offerClosedIssues(empty);
      return;
    }
    issues.forEach((issue) => {
      const row = issueInboxRow(issue);
      row.addEventListener("click", () => openIssueDetail(issue.id, row));
      const item = document.createElement("div");
      item.setAttribute("role", "listitem");
      item.append(row);
      elements.issuesInbox.append(item);
    });
  }

  // An empty inbox points at closed issues when there are any, so resolved
  // work is one click away instead of behind the filter menu.
  async function offerClosedIssues(empty: HTMLElement): Promise<void> {
    const ticket = workspaceController.capture();
    const request = issuesRequest;
    try {
      const response = await api().GetIssuesV1(ticket.generation, "closed", 0);
      const closed = Number(response.bounds?.total || response.issues?.length || 0);
      if (
        closed === 0 || request !== issuesRequest || !empty.isConnected ||
        !workspaceController.accepts(ticket, Number(response.generation))
      ) return;
      const show = document.createElement("button");
      show.type = "button";
      show.className = "button-secondary issues-show-closed";
      show.textContent = closed === 1 ? "Show 1 closed issue" : `Show ${closed} closed issues`;
      show.addEventListener("click", () => {
        elements.issuesFilter.value = "closed";
        issuesOffset = 0;
        void loadIssues();
      });
      empty.append(show);
    } catch {
      // The offer is a convenience; the empty message already stands on its own.
    }
  }

  async function loadIssues(quiet = false): Promise<boolean> {
    if (workspaceController.state.status !== "open") return false;
    const ticket = workspaceController.capture();
    const request = ++issuesRequest;
    if (!quiet) {
      elements.issuesInbox.setAttribute("aria-busy", "true");
      elements.issuesStatus.textContent = "Loading issues…";
    }
    try {
      const response = await api().GetIssuesV1(ticket.generation, elements.issuesFilter.value, issuesOffset);
      if (request !== issuesRequest || !workspaceController.accepts(ticket, Number(response.generation))) return false;
      if (issuesOffset > 0 && response.issues.length === 0) {
        issuesOffset = Math.max(0, issuesOffset - ISSUES_PAGE_SIZE);
        return loadIssues(quiet);
      }
      issuesState = response;
      renderIssuesInbox();
      return true;
    } catch (error) {
      if (request !== issuesRequest || ticket.epoch !== workspaceController.capture().epoch) return false;
      elements.issuesInbox.setAttribute("aria-busy", "false");
      elements.issuesStatus.textContent = "Issues are unavailable.";
      if (!quiet) showError(error);
      return false;
    }
  }

  function resetIssuesProjectData(): void {
    issuesState = emptyIssuesPage();
    issuesOffset = 0;
    issuesRequest += 1;
  }

  function showIssuesLoading(): void {
    elements.issuesInbox.replaceChildren(emptyMemory("Loading issues…"));
    elements.issuesStatus.textContent = "";
    elements.issuesPagination.hidden = true;
  }

  function syncMoveAndLinkControls(detail: IssueDetailResponse): void {
    elements.issueMove.disabled = !elements.issuePlanSelect.value ||
      Number(elements.issuePlanSelect.value) === Number(detail.issue.planId);
    elements.issueLink.disabled = !elements.issueTaskSelect.value ||
      Number(elements.issueTaskSelect.value) === Number(detail.issue.taskId);
  }

  function populateIssueAssociationOptions(detail: IssueDetailResponse): void {
    elements.issuePlanSelect.replaceChildren(
      ...(detail.plans || []).map((plan) =>
        selectOption(plan.id, `#${plan.id} ${plan.title}${plan.holdReason ? " · on hold" : ""}`)
      ),
    );
    elements.issueTaskSelect.replaceChildren(selectOption("", "Choose a task…"));
    (detail.tasks || []).forEach((task) => {
      const option = selectOption(task.id, `#${task.id} ${task.title} · plan #${task.planId} · ${task.status}`);
      option.selected = Number(task.id) === Number(detail.issue.taskId);
      elements.issueTaskSelect.append(option);
    });
    elements.issuePlanSelect.disabled = !detail.plans?.length;
    elements.issueSchedule.disabled = Boolean(detail.issue.taskId) || detail.issue.status !== "open" || !detail.plans?.length;
    syncMoveAndLinkControls(detail);
    elements.issueTargetBounds.textContent = `${detail.plans?.length || 0} plans and ${detail.tasks?.length || 0} tasks shown. Search by title or exact #ID to find any target.`;
  }

  function renderIssueDetail(detail: IssueDetailResponse): void {
    issueDetail = detail;
    const issue = detail.issue;
    elements.issueDrawerEyebrow.textContent = `Issue · #${issue.id}`;
    elements.issueDrawerHeading.textContent = issue.title;
    elements.issueDrawerUpdated.textContent = `updated ${shortRelativeTime(issue.updatedAt)}`;
    elements.issueTitle.value = issue.title;
    elements.issueBody.value = issue.body || "";
    elements.issueSeverity.value = issue.severity;
    elements.issueStatusSelect.value = issue.status;
    elements.issueFormMessage.textContent = "";
    elements.issueScheduling.hidden = false;
    elements.issueTargetSearch.value = "";
    populateIssueAssociationOptions(detail);
    const scheduled = Number(issue.taskId) !== 0;
    elements.issueSchedulingState.textContent = scheduled ? "Scheduled" : "Unscheduled";
    elements.issueLinkSummary.textContent = scheduled
      ? `Linked to task #${issue.taskId} ${issue.taskTitle} in plan #${issue.planId} ${issue.planTitle}.`
      : "This issue is triage context only until you schedule or link it.";
    elements.issueSchedule.disabled = scheduled || issue.status !== "open" || !detail.plans?.length;
    elements.issueSchedule.hidden = scheduled;
    elements.issueMove.hidden = !scheduled;
    elements.issueMoveHelp.hidden = !scheduled;
    elements.issuePlanSelect.disabled = !detail.plans?.length;
    if (scheduled) elements.issuePlanSelect.value = String(issue.planId);
    syncMoveAndLinkControls(detail);
    elements.issueUnlink.disabled = !scheduled;
    elements.issueOpenTask.disabled = !scheduled;
  }

  function openNewIssue(invoker: HTMLElement = elements.issuesNew): void {
    issueRequest += 1;
    issueReturnFocus = invoker;
    issueDetail = { newIssue: true };
    elements.issueForm.inert = false;
    elements.issueDrawerEyebrow.textContent = "New issue";
    elements.issueDrawerHeading.textContent = "Capture a detailed issue";
    elements.issueDrawerUpdated.textContent = "Unscheduled · triage only";
    elements.issueTitle.value = "";
    elements.issueBody.value = "";
    elements.issueSeverity.value = "medium";
    elements.issueStatusSelect.value = "open";
    elements.issueStatusSelect.disabled = true;
    elements.issueFormMessage.textContent = "Saving creates an unscheduled issue and does not change plan work.";
    elements.issueScheduling.hidden = true;
    elements.issueDrawer.hidden = false;
    requestAnimationFrame(() => elements.issueTitle.focus());
  }

  async function openIssueDetail(
    issueId: number,
    invoker: Element | null = document.activeElement,
  ): Promise<void> {
    if (workspaceController.state.status !== "open") return;
    const request = ++issueRequest;
    const ticket = workspaceController.capture();
    issueReturnFocus = invoker instanceof HTMLElement ? invoker : null;
    issueDetail = null;
    elements.issueScheduling.inert = false;
    elements.issueForm.inert = true;
    elements.issueDrawerEyebrow.textContent = `Issue · #${issueId}`;
    elements.issueDrawerHeading.textContent = "Loading issue…";
    elements.issueDrawerUpdated.textContent = "";
    elements.issueFormMessage.textContent = "Loading full report…";
    elements.issueScheduling.hidden = true;
    elements.issueDrawer.hidden = false;
    requestAnimationFrame(() => elements.issueDrawerClose.focus());
    try {
      const detail = await api().GetIssueDetailV1(ticket.generation, Number(issueId));
      if (request !== issueRequest || !workspaceController.accepts(ticket, Number(detail.generation))) return;
      elements.issueStatusSelect.disabled = false;
      renderIssueDetail(detail);
      elements.issueForm.inert = false;
    } catch (error) {
      if (request !== issueRequest || ticket.epoch !== workspaceController.capture().epoch) return;
      showError(error);
      closeIssueDetail();
    }
  }

  function closeIssueDetail(restoreFocus = true): void {
    if (elements.issueDrawer.hidden) return;
    ctx.snapshot.hideApplicationOverlay(elements.issueDrawer);
    issueRequest += 1;
    issueDetail = null;
    elements.issueStatusSelect.disabled = false;
    elements.issueForm.inert = false;
    const returnFocus = issueReturnFocus;
    if (restoreFocus) requestAnimationFrame(() => {
      if (returnFocus?.isConnected) returnFocus.focus();
      else if (!elements.drawer.hidden) elements.drawerClose.focus();
      else elements.navIssues.focus();
    });
    issueReturnFocus = null;
  }

  async function runIssueMutation<T extends GenerationReply>(
    operation: (generation: number) => Promise<T>,
    progress: string,
  ): Promise<T | null> {
    if (workspaceController.state.status !== "open" || !issueDetail) return null;
    const ticket = workspaceController.capture();
    const request = issueRequest;
    elements.issueForm.inert = true;
    elements.issueScheduling.inert = true;
    elements.issueDrawerClose.focus();
    elements.issueFormMessage.textContent = progress;
    try {
      const result = await operation(ticket.generation);
      if (!workspaceController.accepts(ticket, Number(result.generation))) return null;
      await ctx.snapshot.loadSnapshot(ctx.state.board?.planId || 0);
      await loadIssues(true);
      if (request !== issueRequest || elements.issueDrawer.hidden) return null;
      return result;
    } catch (error) {
      if (request === issueRequest && ticket.epoch === workspaceController.capture().epoch) {
        elements.issueFormMessage.textContent = messageFrom(error);
        showError(error);
      }
      return null;
    } finally {
      if (request === issueRequest) {
        elements.issueForm.inert = false;
        elements.issueScheduling.inert = issueReportDirty();
      }
    }
  }

  function issueReportDirty(): boolean {
    const issue = issueDetail?.issue;
    return Boolean(issue && (
      elements.issueTitle.value !== issue.title || elements.issueBody.value !== (issue.body || "") ||
      elements.issueSeverity.value !== issue.severity || elements.issueStatusSelect.value !== issue.status
    ));
  }

  async function findIssueTargets(): Promise<void> {
    const issueId = issueDetail?.issue?.id;
    if (!issueId) return;
    const request = ++issueTargetRequest;
    const detailRequest = issueRequest;
    const ticket = workspaceController.capture();
    elements.issueTargetBounds.textContent = "Finding targets…";
    try {
      const detail = await api().GetIssueDetailV1(ticket.generation, Number(issueId), elements.issueTargetSearch.value.trim());
      if (request !== issueTargetRequest || detailRequest !== issueRequest || ticket.epoch !== workspaceController.capture().epoch) return;
      populateIssueAssociationOptions(detail);
    } catch (error) {
      if (request !== issueTargetRequest || detailRequest !== issueRequest || ticket.epoch !== workspaceController.capture().epoch) return;
      elements.issueTargetBounds.textContent = "Targets are unavailable. Try again.";
      showError(error);
    }
  }

  // Every successful change reopens the issue it acted on, so the drawer shows
  // the report exactly as the runtime now holds it.
  async function reopenAfter<T extends GenerationReply>(
    issueId: number,
    operation: (generation: number) => Promise<T>,
    progress: string,
  ): Promise<void> {
    const result = await runIssueMutation(operation, progress);
    if (result) void openIssueDetail(issueId, issueReturnFocus);
  }

  async function saveIssueReport(): Promise<void> {
    if (!issueDetail) return;
    const title = elements.issueTitle.value.trim();
    if (!title) {
      elements.issueFormMessage.textContent = "Issue title cannot be empty.";
      elements.issueTitle.focus();
      return;
    }
    if (issueDetail.newIssue) {
      const result = await runIssueMutation(
        (generation) => api().AddIssueV1(
          generation,
          title,
          elements.issueBody.value,
          elements.issueSeverity.value,
        ),
        "Saving unscheduled issue…",
      );
      if (result) void openIssueDetail(result.issue.id, issueReturnFocus);
      return;
    }
    const id = issueDetail.issue.id;
    const expectedUpdatedAt = issueDetail.issue.updatedAt;
    await reopenAfter(
      id,
      (generation) => api().UpdateIssueV1(
        generation,
        Number(id),
        title,
        elements.issueBody.value,
        elements.issueSeverity.value,
        elements.issueStatusSelect.value,
        expectedUpdatedAt,
      ),
      `Saving issue #${id}…`,
    );
  }

  function moveIssueTask(): void {
    const issue = issueDetail?.issue;
    const planId = Number(elements.issuePlanSelect.value);
    if (!issue?.taskId || !planId) return;
    void reopenAfter(
      issue.id,
      (generation) => api().MoveIssueTaskV1(generation, Number(issue.id), Number(issue.taskId), Number(issue.planId), planId),
      `Moving task #${issue.taskId} and its linked issues…`,
    );
  }

  function scheduleIssue(): void {
    const issue = issueDetail?.issue;
    const planId = Number(elements.issuePlanSelect.value);
    if (!issue || !planId) return;
    void reopenAfter(
      issue.id,
      (generation) => api().ScheduleIssueV1(generation, Number(issue.id), planId, ""),
      `Scheduling issue #${issue.id}…`,
    );
  }

  function linkIssueTask(): void {
    const issue = issueDetail?.issue;
    const taskId = Number(elements.issueTaskSelect.value);
    if (!issue || !taskId) return;
    void reopenAfter(
      issue.id,
      (generation) => api().SetIssueTaskV1(
        generation,
        Number(issue.id),
        Number(issue.taskId || 0),
        taskId,
      ),
      `Linking issue #${issue.id}…`,
    );
  }

  function unlinkIssueTask(): void {
    const issue = issueDetail?.issue;
    if (!issue?.taskId) return;
    void reopenAfter(
      issue.id,
      (generation) => api().SetIssueTaskV1(
        generation,
        Number(issue.id),
        Number(issue.taskId),
        0,
      ),
      `Unlinking issue #${issue.id}…`,
    );
  }

  function openIssueTask(): void {
    const issue = issueDetail?.issue;
    if (!issue?.taskId || !issue.planId) return;
    const taskId = Number(issue.taskId);
    const planId = Number(issue.planId);
    closeIssueDetail(false);
    ctx.drawer.requestPendingTaskDetail(taskId);
    ctx.shell.setView("board");
    if (Number(ctx.state.board?.planId) === planId) ctx.drawer.openPendingTaskDetail();
    else ctx.board.selectPlan(planId);
  }

  function bindInbox(): void {
    elements.issuesFilter.addEventListener("change", () => { issuesOffset = 0; void loadIssues(); });
    elements.issuesPrevious.addEventListener("click", () => {
      issuesOffset = Math.max(0, issuesOffset - ISSUES_PAGE_SIZE);
      void loadIssues();
    });
    elements.issuesNext.addEventListener("click", () => { issuesOffset += ISSUES_PAGE_SIZE; void loadIssues(); });
  }

  function bindDrawer(): void {
    elements.issueTargetFind.addEventListener("click", () => void findIssueTargets());
    elements.issueTargetSearch.addEventListener("keydown", (event) => {
      if (event.key !== "Enter") return;
      event.preventDefault();
      void findIssueTargets();
    });
    elements.issuePlanSelect.addEventListener("change", () => {
      elements.issueMove.disabled = !elements.issuePlanSelect.value ||
        Number(elements.issuePlanSelect.value) === Number(issueDetail?.issue?.planId);
    });
    elements.issueMove.addEventListener("click", moveIssueTask);
    elements.issuesNew.addEventListener("click", () => openNewIssue(elements.issuesNew));
    elements.issueDrawerClose.addEventListener("click", () => closeIssueDetail());
    document.querySelectorAll("[data-close-issue-drawer]").forEach((closer) => {
      closer.addEventListener("click", () => closeIssueDetail());
    });
    elements.issueTaskSelect.addEventListener("change", () => {
      if (!issueDetail?.issue) return;
      elements.issueLink.disabled = !elements.issueTaskSelect.value ||
        Number(elements.issueTaskSelect.value) === Number(issueDetail.issue.taskId);
    });
    elements.issueForm.addEventListener("submit", (event) => {
      event.preventDefault();
      void saveIssueReport();
    });
    elements.issueForm.addEventListener("input", () => {
      const dirty = issueReportDirty();
      elements.issueScheduling.inert = dirty;
      elements.issueFormMessage.textContent = dirty ? "Unsaved report changes. Save issue before changing its scheduling or opening its task." : "";
    });
    elements.issueSchedule.addEventListener("click", scheduleIssue);
    elements.issueLink.addEventListener("click", linkIssueTask);
    elements.issueUnlink.addEventListener("click", unlinkIssueTask);
    elements.issueOpenTask.addEventListener("click", openIssueTask);
  }

  function bind(): void {
    bindInbox();
    bindDrawer();
  }

  return {
    bind,
    loadIssues,
    resetIssuesProjectData,
    showIssuesLoading,
    openIssueDetail,
    closeIssueDetail,
  };
}

export type IssuesView = ReturnType<typeof createIssuesView>;
