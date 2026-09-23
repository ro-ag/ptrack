import type { AppContext } from "./app-context";
import { GenerationSlot } from "./controller";
import type { AgentTaskDetail } from "./copy-context";
import { element, intelligenceItem } from "./dom";
import { compactAriaText, shortRelativeTime } from "./format";
import {
  agentIntelligenceLabel,
  handoffPreviewResponseIsCurrent,
  linkedTaskRuntimePresentation,
  type LinkedTaskRuntimeSummary,
} from "./presentation";
import type {
  AgentIntelligenceEntry,
  AgentRuntimeRow,
  BoardTask,
  LinkedRuntimeDetail,
  TaskCommit,
  TaskDetailResponse,
  TaskIssue,
  TaskNote,
  TerminalRuntimeRow,
} from "./snapshot-types";
import { severityColors, statusTitles, statuses } from "./task-status";

const NO_LINKED_RUNTIME = "No current terminal or agent is linked to this task.";

export function drawerEmptyState(message: string): HTMLDivElement {
  const empty = document.createElement("div");
  empty.className = "drawer-empty";
  empty.textContent = message;
  return empty;
}

export function drawerNoteElement(note: TaskNote): HTMLElement {
  const item = document.createElement("article");
  item.className = "drawer-note";
  const body = document.createElement("p");
  body.className = "drawer-note-body";
  body.textContent = note.body;
  const meta = document.createElement("span");
  meta.className = "drawer-item-meta";
  meta.textContent = `${note.kind || "note"} · ${shortRelativeTime(note.occurredAt)}`;
  item.append(body, meta);
  return item;
}

export function drawerCommitElement(commit: TaskCommit): HTMLElement {
  const item = document.createElement("article");
  item.className = "drawer-commit";
  const row = document.createElement("p");
  row.className = "drawer-commit-title";
  const sha = document.createElement("span");
  sha.className = "drawer-sha";
  sha.textContent = commit.sha.slice(0, 8);
  row.append(sha, document.createTextNode(commit.subject));
  const meta = document.createElement("span");
  meta.className = "drawer-item-meta";
  meta.textContent = shortRelativeTime(commit.occurredAt);
  item.append(row, meta);
  return item;
}

function terminalRuntimeItem(session: TerminalRuntimeRow): HTMLElement {
  return intelligenceItem(
    `Terminal · ${session.profileKind}`,
    `${session.live ? "live" : "historical"} · ${session.state} · ${session.profileKind}`,
    session.state === "failed" ? "error" : "",
  );
}

/** How an agent run relates to the terminal the task links. */
export function agentRuntimeOrigin(run: AgentRuntimeRow): string {
  return run.terminalBacked
    ? run.correspondingTerminal
      ? "paired with linked terminal"
      : run.terminalPresent
        ? "terminal present · association does not correspond"
        : "terminal unavailable"
    : "external";
}

function agentRuntimeItem(run: AgentRuntimeRow): HTMLElement {
  const intelligence = agentIntelligenceLabel(run.intelligence);
  return intelligenceItem(
    `${run.terminalBacked ? "Terminal-backed" : "External"} agent`,
    `${run.live ? "live" : "historical"} · lifecycle ${run.state} · process ${run.processState} · lease ${run.leaseState} · ${agentRuntimeOrigin(run)}` +
      `${intelligence ? ` · ${intelligence}` : ""}`,
    run.state === "stale" ? "stale" : "",
  );
}

function intelligenceTone(state: string): string {
  return state === "failed" ? "error" : state === "potentiallyDrifting" ? "stale" : "";
}

export function createTaskDrawer(ctx: AppContext) {
  const { api, showError, workspaceController } = ctx;
  const elements = {
    drawer: element("#task-drawer", HTMLDivElement),
    drawerClose: element("#drawer-close", HTMLButtonElement),
    drawerCommits: element("#drawer-commits", HTMLDivElement),
    drawerCommitsCount: element("#drawer-commits-count", HTMLSpanElement),
    drawerCopyContext: element("#drawer-copy-context", HTMLButtonElement),
    drawerEyebrow: element("#drawer-eyebrow", HTMLParagraphElement),
    drawerIssues: element("#drawer-issues", HTMLDivElement),
    drawerIssuesCount: element("#drawer-issues-count", HTMLSpanElement),
    drawerLaunchAgent: element("#drawer-launch-agent", HTMLButtonElement),
    drawerMemory: element("#drawer-memory", HTMLButtonElement),
    drawerNotes: element("#drawer-notes", HTMLDivElement),
    drawerNotesCount: element("#drawer-notes-count", HTMLSpanElement),
    drawerRename: element("#drawer-rename", HTMLButtonElement),
    drawerRuntime: element("#drawer-runtime", HTMLDivElement),
    drawerRuntimeCount: element("#drawer-runtime-count", HTMLSpanElement),
    drawerStatus: element("#drawer-status", HTMLSpanElement),
    drawerStatusSelect: element("#drawer-status-select", HTMLSelectElement),
    drawerTitle: element("#drawer-title", HTMLHeadingElement),
    drawerUpdated: element("#drawer-updated", HTMLSpanElement),
  };

  let detailRequest = 0;
  let drawerReturnFocus: HTMLElement | null = null;
  // Notes/issues/commits from the loaded task detail, so "Copy context" can
  // paste a richer brief than the board's summary rows carry.
  let taskDetailExtras: AgentTaskDetail | null = null;
  // A task the palette or an issue asked to open once its plan's board loads.
  // It belongs to the workspace generation that asked, so a project switch or
  // close can never open another project's task with the same number.
  const pendingDetailTask = new GenerationSlot<number>();

  // Opens the drawer for a task chosen in the palette once the board for its
  // plan has loaded. Called from the snapshot success path and directly when
  // the task's plan is already selected.
  function requestPendingTaskDetail(taskId: number): void {
    pendingDetailTask.set(Number(taskId), workspaceController.state.generation);
  }

  function clearPendingTaskDetail(): void {
    pendingDetailTask.clear();
  }

  function openPendingTaskDetail(): void {
    if (!pendingDetailTask.pending || !ctx.state.board) return;
    const taskId = pendingDetailTask.take(workspaceController.state);
    if (!taskId) return;
    const task = ctx.snapshot.boardTask(taskId);
    if (task) { openTaskDetail(task); return; }
    const request = ++detailRequest;
    const ticket = workspaceController.capture();
    void api().GetTaskDetailV2(ticket.generation, taskId).then((detail) => {
      if (request !== detailRequest || !workspaceController.accepts(ticket, Number(detail.generation)) || ctx.state.view !== "board") return;
      openTaskDetail(detail.task);
    }).catch((error: unknown) => {
      if (request === detailRequest && workspaceController.accepts(ticket, ticket.generation)) showError(error);
    });
  }

  function renderDrawerTask(task: BoardTask): void {
    elements.drawerEyebrow.textContent = `Task · #${task.id}`;
    elements.drawerTitle.textContent = task.title;
    elements.drawerStatus.dataset.status = task.status;
    elements.drawerStatus.textContent = statusTitles[task.status] || task.status;
    elements.drawerUpdated.textContent = task.updatedAt
      ? `updated ${shortRelativeTime(task.updatedAt)}`
      : "";
    elements.drawerStatusSelect.replaceChildren();
    statuses.forEach((status) => {
      const option = document.createElement("option");
      option.value = status;
      option.textContent = statusTitles[status];
      option.selected = status === task.status;
      elements.drawerStatusSelect.append(option);
    });
    renderDrawerRuntimeSummary(task.linkedRuntime);
  }

  function renderDrawerRuntimeSummary(summary: LinkedTaskRuntimeSummary | undefined): void {
    const presentation = linkedTaskRuntimePresentation(summary);
    elements.drawerRuntimeCount.textContent = presentation
      ? presentation.compact
      : "0";
    elements.drawerRuntime.replaceChildren(
      drawerEmptyState(presentation ? presentation.detail : NO_LINKED_RUNTIME),
    );
  }

  // The on-demand handoff preview for one agent run. It belongs to the task
  // and association the drawer showed when it was asked for.
  function handoffPreviewControls(
    run: AgentRuntimeRow,
    intelligence: AgentIntelligenceEntry,
  ): [HTMLButtonElement, HTMLPreElement] {
    const handoffButton = document.createElement("button");
    handoffButton.type = "button";
    handoffButton.className = "button-secondary";
    handoffButton.textContent = "Preview handoff";
    const handoffPreview = document.createElement("pre");
    handoffPreview.className = "intelligence-detail";
    handoffPreview.hidden = true;
    handoffPreview.style.whiteSpace = "pre-wrap";
    const handoffTaskId = Number(ctx.state.detailTask?.id || 0);
    const handoffAssociation = intelligence.association;
    handoffButton.addEventListener("click", async () => {
      const ticket = workspaceController.capture();
      handoffButton.disabled = true;
      handoffButton.textContent = "Generating preview…";
      try {
        const result = await api().PreviewAgentHandoffV2(ticket.generation, run.runId);
        if (!workspaceController.accepts(ticket, Number(result.generation))) return;
        if (!handoffPreviewResponseIsCurrent(
          handoffTaskId,
          handoffAssociation,
          result.association,
          Number(ctx.state.detailTask?.id || 0),
        )) return;
        handoffPreview.textContent = `${result.preview.text}\n\nPreview only · project memory was not changed.`;
        handoffPreview.hidden = false;
      } catch (error) {
        showError(error);
      } finally {
        handoffButton.disabled = false;
        handoffButton.textContent = "Refresh handoff preview";
      }
    });
    return [handoffButton, handoffPreview];
  }

  function appendAgentIntelligence(run: AgentRuntimeRow, intelligence: AgentIntelligenceEntry): void {
    const entry = intelligenceItem(
      `Agent intelligence · ${intelligence.intelligence.state}`,
      `${intelligence.intelligence.confidence || "low"} confidence · ${intelligence.eventBounds?.total || 0} retained structured events`,
      intelligenceTone(intelligence.intelligence.state),
    );
    entry.append(...handoffPreviewControls(run, intelligence));
    elements.drawerRuntime.append(entry);
    (intelligence.suggestions || []).forEach((suggestion) => {
      elements.drawerRuntime.append(
        intelligenceItem(
          `Suggestion · ${suggestion.kind}`,
          `${suggestion.label} · ${suggestion.reason}`,
        ),
      );
    });
  }

  function renderDrawerRuntimeDetail(
    linkedRuntime: LinkedRuntimeDetail | undefined,
    agentIntelligence: readonly AgentIntelligenceEntry[] = [],
  ): void {
    const presentation = linkedTaskRuntimePresentation(linkedRuntime?.summary);
    const intelligenceByRun = new Map(
      (agentIntelligence || []).map((entry) => [entry.runId, entry]),
    );
    elements.drawerRuntimeCount.textContent = presentation
      ? presentation.compact
      : "0";
    elements.drawerRuntime.replaceChildren();
    if (!presentation) {
      elements.drawerRuntime.append(drawerEmptyState(NO_LINKED_RUNTIME));
      return;
    }
    (linkedRuntime?.terminals || []).forEach((session) => {
      elements.drawerRuntime.append(terminalRuntimeItem(session));
    });
    (linkedRuntime?.agents || []).forEach((run) => {
      elements.drawerRuntime.append(agentRuntimeItem(run));
      const intelligence = intelligenceByRun.get(run.runId);
      if (intelligence) appendAgentIntelligence(run, intelligence);
    });
    const terminalRowsMore = Number(linkedRuntime?.terminalRowsMore || 0);
    const agentRowsMore = Number(linkedRuntime?.agentRowsMore || 0);
    if (terminalRowsMore || agentRowsMore) {
      elements.drawerRuntime.append(
        drawerEmptyState(
          `${terminalRowsMore} more terminal${terminalRowsMore === 1 ? "" : "s"} · ` +
          `${agentRowsMore} more agent${agentRowsMore === 1 ? "" : "s"}`,
        ),
      );
    }
  }

  function renderDrawerLoading(): void {
    elements.drawerRuntimeCount.textContent = "…";
    elements.drawerRuntime.replaceChildren(drawerEmptyState("Loading linked runtime…"));
    elements.drawerNotesCount.textContent = "…";
    elements.drawerCommitsCount.textContent = "…";
    elements.drawerIssuesCount.textContent = "…";
    elements.drawerNotes.replaceChildren(drawerEmptyState("Loading notes…"));
    elements.drawerCommits.replaceChildren(drawerEmptyState("Loading commits…"));
    elements.drawerIssues.replaceChildren(drawerEmptyState("Loading issues…"));
  }

  function drawerIssueElement(issue: TaskIssue): HTMLButtonElement {
    const item = document.createElement("button");
    item.type = "button";
    item.className = "drawer-issue";
    item.style.setProperty(
      "--issue-color",
      severityColors[issue.severity] || "var(--muted)",
    );
    const title = document.createElement("p");
    title.className = "drawer-issue-title";
    title.textContent = issue.title;
    const meta = document.createElement("span");
    meta.className = "drawer-item-meta";
    meta.textContent = `${issue.severity} · ${issue.status || "open"} · issue #${issue.id}`;
    item.append(title, meta);
    item.setAttribute(
      "aria-label",
      `Open issue #${issue.id}, ${issue.severity}: ${compactAriaText(issue.title)}`,
    );
    item.addEventListener("click", () => {
      void ctx.issues.openIssueDetail(issue.id, item);
    });
    return item;
  }

  function renderDrawerList<T>(
    host: HTMLElement,
    count: HTMLElement,
    entries: readonly T[],
    empty: string,
    render: (entry: T) => Node,
  ): void {
    count.textContent = String(entries.length);
    host.replaceChildren();
    if (entries.length === 0) host.append(drawerEmptyState(empty));
    else entries.forEach((entry) => host.append(render(entry)));
  }

  function renderDrawerSections(detail: TaskDetailResponse): void {
    renderDrawerRuntimeDetail(detail.linkedRuntime, detail.agentIntelligence);
    renderDrawerList(
      elements.drawerNotes,
      elements.drawerNotesCount,
      detail.notes,
      "No notes yet. Use “Add note” to capture a decision.",
      drawerNoteElement,
    );
    renderDrawerList(
      elements.drawerCommits,
      elements.drawerCommitsCount,
      detail.commits,
      "No commits linked to this task yet.",
      drawerCommitElement,
    );
    renderDrawerList(
      elements.drawerIssues,
      elements.drawerIssuesCount,
      detail.issues,
      "No issues linked to this task.",
      drawerIssueElement,
    );
  }

  async function loadTaskDetail(task: BoardTask): Promise<void> {
    const request = ++detailRequest;
    const ticket = workspaceController.capture();
    try {
      const detail = await api().GetTaskDetailV2(ticket.generation, Number(task.id));
      if (
        request !== detailRequest ||
        !ctx.state.detailTask ||
        Number(ctx.state.detailTask.id) !== Number(task.id) ||
        !workspaceController.accepts(ticket, Number(detail.generation))
      ) {
        return;
      }
      ctx.state.detailTask = detail.task;
      taskDetailExtras = {
        notes: detail.notes,
        issues: detail.issues,
        commits: detail.commits,
      };
      renderDrawerTask(detail.task);
      renderDrawerSections(detail);
    } catch (error) {
      if (request !== detailRequest) return;
      if (ticket.epoch !== workspaceController.capture().epoch) return;
      showError(error);
      closeTaskDetail();
    }
  }

  function openTaskDetail(task: BoardTask): void {
    if (workspaceController.state.status !== "open") return;
    ctx.state.detailTask = task;
    const active = document.activeElement;
    drawerReturnFocus = active instanceof HTMLElement ? active : null;
    renderDrawerTask(task);
    renderDrawerLoading();
    elements.drawer.hidden = false;
    requestAnimationFrame(() => elements.drawerClose.focus());
    void loadTaskDetail(task);
  }

  function closeTaskDetail(restoreFocus = true): void {
    if (elements.drawer.hidden) return;
    ctx.snapshot.hideApplicationOverlay(elements.drawer);
    detailRequest += 1;
    const taskId = ctx.state.detailTask?.id;
    ctx.state.detailTask = null;
    taskDetailExtras = null;
    const card = taskId
      ? document.querySelector<HTMLElement>(`.card[data-task-id="${taskId}"] .card-drag-zone`)
      : null;
    if (restoreFocus) (card || drawerReturnFocus)?.focus?.();
    drawerReturnFocus = null;
  }

  function bind(): void {
    elements.drawerCopyContext.addEventListener("click", () => {
      if (ctx.state.detailTask) {
        void ctx.board.copyAgentContext(
          "task",
          { ...ctx.state.detailTask, detail: taskDetailExtras ?? undefined },
          elements.drawerCopyContext,
        );
      }
    });
    document.querySelectorAll("[data-close-drawer]").forEach((closer) => {
      closer.addEventListener("click", () => closeTaskDetail());
    });
    elements.drawerClose.addEventListener("click", () => closeTaskDetail());
    elements.drawerStatusSelect.addEventListener("change", () => {
      if (!ctx.state.detailTask) return;
      void ctx.snapshot.moveTask(
        ctx.state.detailTask.id,
        elements.drawerStatusSelect.value,
        elements.drawerStatusSelect,
      );
    });
    elements.drawerRename.addEventListener("click", () => {
      if (ctx.state.detailTask) ctx.planDialogs.openRename(ctx.state.detailTask);
    });
    elements.drawerMemory.addEventListener("click", () => {
      if (ctx.state.detailTask) ctx.planDialogs.openMemory(ctx.state.detailTask);
    });
    elements.drawerLaunchAgent.addEventListener("click", () => {
      if (!ctx.state.detailTask || !ctx.state.board?.planId) return;
      void ctx.agentLaunch.openAgentLaunchPicker(
        { planId: Number(ctx.state.board.planId), task: ctx.state.detailTask },
        elements.drawerLaunchAgent,
      );
    });
  }

  return {
    bind,
    requestPendingTaskDetail,
    clearPendingTaskDetail,
    openPendingTaskDetail,
    renderDrawerTask,
    loadTaskDetail,
    openTaskDetail,
    closeTaskDetail,
  };
}

export type TaskDrawer = ReturnType<typeof createTaskDrawer>;
