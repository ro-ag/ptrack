import type { AppContext } from "./app-context";
import { element, emptyMemory, intelligenceItem, pill } from "./dom";
import { shortRelativeTime } from "./format";
import {
  agentActivityAnnouncement,
  agentActivityPresentation,
  mutationFocusFallback,
  runtimeAssociationLabel,
  workflowMutationFocusKey,
  worktreeSelectionForRerender,
  type AgentActivityItemView,
  type AgentActivitySectionPresentation,
} from "./presentation";

type AgentActivity = ReturnType<typeof agentActivityPresentation>;
type AgentNotification = AgentActivity["notifications"][number];
type WorkflowProposal = AgentActivity["workflows"]["items"][number];
type HandoffProposal = AgentActivity["handoffs"]["items"][number];

interface FocusedWorktreeSelection {
  runId: string;
  value: string;
}

const notificationLabels: Record<AgentNotification["kind"], readonly [string, string]> = {
  approvalRequested: ["Approval requested", "Attention required; no permission has been granted."],
  question: ["Agent question", "The agent is waiting for user attention."],
  failure: ["Agent failure", "The provider reported an explicit failure."],
  completion: ["Agent completed", "The provider reported explicit lifecycle completion."],
};

function stateTone(state: string): string {
  return ["failed", "blocked"].includes(state)
    ? "error"
    : state === "waiting"
      ? "warning"
      : "";
}

function notificationTone(kind: AgentNotification["kind"]): string {
  return ["approvalRequested", "question"].includes(kind)
    ? "waiting"
    : kind === "failure"
      ? "failed"
      : "completed";
}

/** Where an agent run came from, relative to the terminal it may belong to. */
export function agentOrigin(item: AgentActivityItemView): string {
  return item.terminalBacked
    ? item.correspondingTerminal
      ? "terminal-backed"
      : item.terminalPresent
        ? "terminal-backed · association does not correspond"
        : "terminal-backed · terminal unavailable"
    : "external";
}

export function agentRowDetail(item: AgentActivityItemView): string {
  const evidence = Number(item.evidenceCount || 0);
  const events = Number(item.eventCount || 0);
  const observed = item.lastEventAt ? ` · last event ${shortRelativeTime(item.lastEventAt)}` : "";
  return `${agentOrigin(item)} · ${runtimeAssociationLabel(item.association)} · ${evidence} evidence signal${evidence === 1 ? "" : "s"} · ${events} structured event${events === 1 ? "" : "s"}${observed}`;
}

function actionButton(label: string, focusKey: string): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "button-secondary agent-ownership-action";
  button.dataset.mutationFocusKey = focusKey;
  button.textContent = label;
  return button;
}

function liveAgents(items: readonly AgentActivityItemView[]): AgentActivityItemView[] {
  return items.filter((item) => item.live && item.runId);
}

function focusedMutationKey(): string {
  const active = document.activeElement;
  return active instanceof HTMLElement ? active.dataset.mutationFocusKey || "" : "";
}

function needsWorkflowTarget(kind: string): boolean {
  return ["pullRequest", "merge"].includes(kind);
}

export function createAgentActivityView(ctx: AppContext) {
  const { api, showError } = ctx;
  const elements = {
    agentActivity: element("#agent-activity", HTMLDivElement),
    agentActivityHeading: element("#agent-activity-heading", HTMLParagraphElement),
    agentActivityLive: element("#agent-activity-live", HTMLParagraphElement),
    agentActivitySummary: element("#agent-activity-summary", HTMLDivElement),
    agentActivityTotal: element("#agent-activity-total", HTMLSpanElement),
    agentHandoffForm: element("#agent-handoff-form", HTMLFormElement),
    agentHandoffHelp: element("#agent-handoff-help", HTMLParagraphElement),
    agentHandoffInbox: element("#agent-handoff-inbox", HTMLDivElement),
    agentHandoffSend: element("#agent-handoff-send", HTMLButtonElement),
    agentHandoffSource: element("#agent-handoff-source", HTMLSelectElement),
    agentHandoffTarget: element("#agent-handoff-target", HTMLSelectElement),
    agentWorkflowForm: element("#agent-workflow-form", HTMLFormElement),
    agentWorkflowHelp: element("#agent-workflow-help", HTMLParagraphElement),
    agentWorkflowInbox: element("#agent-workflow-inbox", HTMLDivElement),
    agentWorkflowKind: element("#agent-workflow-kind", HTMLSelectElement),
    agentWorkflowPrepare: element("#agent-workflow-prepare", HTMLButtonElement),
    agentWorkflowRun: element("#agent-workflow-run", HTMLSelectElement),
    agentWorkflowTarget: element("#agent-workflow-target", HTMLSelectElement),
  };

  let agentActivityAnnouncementKey = "";

  function appendNotice(title: string, detail: string, state: string): void {
    elements.agentActivity.append(intelligenceItem(title, detail, state));
  }

  function renderActivityNotices(activity: AgentActivity): void {
    if (activity.analysisIncomplete) {
      appendNotice(
        "Overlap analysis incomplete",
        "The bounded runtime snapshot omitted agents or conflict groups. Absence of another warning does not prove exclusive task ownership.",
        "stale",
      );
    }
    if (activity.worktreesIncomplete) {
      appendNotice(
        "Worktree discovery incomplete",
        "Only the bounded set of existing host-observed worktrees shown here may be selected.",
        "stale",
      );
    }
    activity.conflicts.forEach((conflict) => {
      const ownership = conflict.ownerCount
        ? ` · ${conflict.ownerCount} explicit owner${conflict.ownerCount === 1 ? "" : "s"}`
        : "";
      appendNotice(
        `Overlap warning · plan #${conflict.planId} · task #${conflict.taskId}`,
        `${conflict.agentCount} active agents share this task${ownership}. Advisory only; no agent, association, or task was changed.`,
        "blocked",
      );
    });
    if (activity.notificationsIncomplete) {
      appendNotice(
        "Notifications incomplete",
        "Older structured events or agent rows were omitted by the workspace bounds.",
        "stale",
      );
    }
    activity.notifications.forEach((notification) => {
      const [title, meaning] = notificationLabels[notification.kind];
      appendNotice(
        `${title} · agent ${notification.runId.slice(0, 8)}`,
        `${meaning} · ${runtimeAssociationLabel(notification.association)} · ${shortRelativeTime(notification.observedAt)}`,
        notificationTone(notification.kind),
      );
    });
  }

  function ownershipButton(item: AgentActivityItemView, taskId: number, revision: number): HTMLButtonElement {
    const owned = Boolean(item.ownership);
    const button = actionButton(owned ? "Release ownership" : "Claim task", `ownership:${item.runId}`);
    button.setAttribute(
      "aria-label",
      `${owned ? "Release ownership of" : "Claim"} task #${taskId} for agent ${item.runId.slice(0, 8)}`,
    );
    button.addEventListener("click", () => {
      void ctx.snapshot.runMutation(
        (generation) => api().SetAgentTaskOwnershipV2(
          generation,
          item.runId,
          revision,
          !owned,
        ),
        `${owned ? "Releasing" : "Claiming"} task #${taskId}…`,
        `Could not ${owned ? "release" : "claim"} task ownership`,
      );
    });
    return button;
  }

  function worktreeControls(
    item: AgentActivityItemView,
    activity: AgentActivity,
    revision: number,
    focusedWorktreeSelection: FocusedWorktreeSelection | null,
  ): HTMLDivElement {
    const controls = document.createElement("div");
    controls.className = "agent-worktree-controls";
    const select = document.createElement("select");
    select.dataset.mutationFocusKey = `worktree-select:${item.runId}`;
    select.dataset.worktreeRunId = item.runId;
    select.setAttribute(
      "aria-label",
      `Existing worktree for agent ${item.runId.slice(0, 8)}`,
    );
    activity.worktrees.forEach((worktree) => {
      const option = document.createElement("option");
      option.value = worktree.root;
      option.textContent = `${worktree.branch || "detached"} · ${worktree.head.slice(0, 8)} · ${worktree.root}`;
      select.append(option);
    });
    select.value = worktreeSelectionForRerender(
      activity.worktrees.map((entry) => entry.root),
      item.worktree?.identity?.root,
      focusedWorktreeSelection,
      item.runId,
    );
    const associate = actionButton("Associate worktree", `worktree-associate:${item.runId}`);
    associate.addEventListener("click", () => {
      void ctx.snapshot.runMutation(
        (generation) => api().SetAgentWorktreeV2(
          generation,
          item.runId,
          revision,
          select.value,
          true,
        ),
        "Verifying existing worktree…",
        "Could not associate worktree",
        `worktree:${item.runId}`,
      );
    });
    controls.append(select, associate);
    if (item.worktree) {
      const detach = actionButton("Detach worktree", `worktree-detach:${item.runId}`);
      detach.addEventListener("click", () => {
        void ctx.snapshot.runMutation(
          (generation) => api().SetAgentWorktreeV2(
            generation,
            item.runId,
            revision,
            "",
            false,
          ),
          "Detaching worktree metadata…",
          "Could not detach worktree",
          `worktree:${item.runId}`,
        );
      });
      controls.append(detach);
    }
    return controls;
  }

  function agentRow(
    item: AgentActivityItemView,
    activity: AgentActivity,
    focusedWorktreeSelection: FocusedWorktreeSelection | null,
  ): HTMLElement {
    const row = intelligenceItem(
      `${item.state[0].toUpperCase()}${item.state.slice(1)} agent · ${item.runId.slice(0, 8)}`,
      agentRowDetail(item),
      item.state,
    );
    if (item.ownership) {
      row.querySelector(".intelligence-detail")?.append(
        document.createTextNode(" · explicit task owner"),
      );
    }
    if (item.worktree?.verified) {
      row.querySelector(".intelligence-detail")?.append(
        document.createTextNode(
          ` · worktree verified · ${item.worktree.isolated ? "isolated checkout" : "project checkout"} · CWD ${item.worktree.cwdMatches ? "matches" : "does not match"}`,
        ),
      );
    }
    const taskId = Number(item.association?.taskId || 0);
    const revision = Number(item.association?.revision || 0);
    if (item.live && taskId > 0 && revision > 0) row.append(ownershipButton(item, taskId, revision));
    if (item.live && activity.worktrees.length > 0) {
      row.append(worktreeControls(item, activity, revision, focusedWorktreeSelection));
    }
    return row;
  }

  function renderAgentActivity(section: AgentActivitySectionPresentation | null | undefined): void {
    const focusKey = focusedMutationKey();
    const focusedWorktreeSelection = captureFocusedWorktreeSelection();
    elements.agentActivity.replaceChildren();
    elements.agentActivitySummary.replaceChildren();
    const activity = agentActivityPresentation(section);
    const announcement = agentActivityAnnouncement(activity, agentActivityAnnouncementKey);
    if (announcement) {
      agentActivityAnnouncementKey = announcement.key;
      elements.agentActivityLive.textContent = announcement.text;
    }
    renderAgentHandoffs(activity.items, activity.handoffs, activity);
    renderAgentWorkflows(
      activity.items,
      activity.workflows,
      activity.workflowTargets,
      activity.workflowTargetsIncomplete,
      activity,
    );
    elements.agentActivityTotal.textContent = activity.compact;
    elements.agentActivityTotal.title = activity.detail;
    activity.counts.forEach(({ state, count }) => {
      elements.agentActivitySummary.append(pill(state, count, stateTone(state)));
    });
    renderActivityNotices(activity);
    if (activity.items.length === 0) {
      elements.agentActivity.append(emptyMemory(
        activity.registeredTotal === 0
          ? "No registered agent activity."
          : "No agent activity is available in this bounded snapshot.",
      ));
    } else {
      activity.items.forEach((item) => {
        elements.agentActivity.append(agentRow(item, activity, focusedWorktreeSelection));
      });
    }
    restoreMutationFocus(focusKey);
  }

  function resetAgentActivityAnnouncement(): void {
    agentActivityAnnouncementKey = "";
    elements.agentActivityLive.textContent = "";
  }

  function captureFocusedWorktreeSelection(): FocusedWorktreeSelection | null {
    const active = document.activeElement;
    if (!(active instanceof HTMLElement)) return null;
    const controls = active.closest(".agent-worktree-controls");
    const select = controls?.querySelector("select[data-worktree-run-id]");
    if (!(select instanceof HTMLSelectElement)) return null;
    return { runId: select.dataset.worktreeRunId || "", value: select.value };
  }

  function replaceOptions(select: HTMLSelectElement, entries: ReadonlyArray<readonly [string, string]>): void {
    select.replaceChildren();
    for (const [value, label] of entries) {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = label;
      select.append(option);
    }
  }

  function workflowProposalRow(proposal: WorkflowProposal): HTMLElement {
    const target = proposal.targetBranch
      ? ` → ${proposal.targetBranch} ${proposal.targetHead.slice(0, 8)}`
      : "";
    const status = proposal.status;
    const row = intelligenceItem(
      `${proposal.kind} · ${proposal.state} · agent ${proposal.runId.slice(0, 8)}`,
      `${proposal.branch}${target} · ${proposal.head.slice(0, 8)} · staged ${status.staged} · unstaged ${status.unstaged} · untracked ${status.untracked} · conflicts ${status.conflicted} · proposal only; no execution`,
      proposal.state === "approved" ? "completed" : "waiting",
    );
    if (proposal.state === "proposed") {
      const approve = actionButton("Approve proposal", workflowMutationFocusKey("approve", proposal.id));
      approve.addEventListener("click", () => {
        void ctx.snapshot.runMutation(
          (generation) => api().ApproveAgentWorkflowV2(generation, proposal.id),
          "Revalidating workflow proposal…",
          "Could not approve workflow proposal",
        );
      });
      row.append(approve);
    }
    const dismiss = actionButton("Dismiss", workflowMutationFocusKey("dismiss", proposal.id));
    dismiss.addEventListener("click", () => {
      void ctx.snapshot.runMutation(
        (generation) => api().DismissAgentWorkflowV2(generation, proposal.id),
        "Dismissing workflow proposal…",
        "Could not dismiss workflow proposal",
      );
    });
    row.append(dismiss);
    return row;
  }

  function renderAgentWorkflows(
    items: readonly AgentActivityItemView[],
    inbox: AgentActivity["workflows"],
    targets: readonly string[],
    targetsIncomplete: boolean,
    availability: AgentActivity,
  ): void {
    const previousRun = elements.agentWorkflowRun.value;
    const previousTarget = elements.agentWorkflowTarget.value;
    const live = liveAgents(items);
    const focusWasInForm = !availability.canPrepareWorkflow &&
      elements.agentWorkflowForm.contains(document.activeElement);
    elements.agentWorkflowForm.hidden = !availability.canPrepareWorkflow;
    elements.agentWorkflowHelp.hidden = !availability.canPrepareWorkflow;
    if (focusWasInForm) elements.agentActivityHeading.focus();
    replaceOptions(elements.agentWorkflowRun, live.map((item) => [
      item.runId,
      `Agent ${item.runId.slice(0, 8)} · ${runtimeAssociationLabel(item.association)}`,
    ]));
    if (live.some((item) => item.runId === previousRun)) elements.agentWorkflowRun.value = previousRun;
    replaceOptions(elements.agentWorkflowTarget, targets.map((branch) => [branch, branch]));
    if (targets.includes(previousTarget)) elements.agentWorkflowTarget.value = previousTarget;
    const needsTarget = needsWorkflowTarget(elements.agentWorkflowKind.value);
    elements.agentWorkflowTarget.disabled = !needsTarget;
    elements.agentWorkflowPrepare.disabled = live.length === 0 || (needsTarget && targets.length === 0);
    elements.agentWorkflowInbox.replaceChildren();
    if (targetsIncomplete) {
      elements.agentWorkflowInbox.append(intelligenceItem(
        "Target branches incomplete",
        "Only branches present in the bounded read-only Git snapshot can be selected.",
        "stale",
      ));
    }
    if (inbox.incomplete) {
      elements.agentWorkflowInbox.append(intelligenceItem(
        "Workflow inbox incomplete",
        "Some bounded runtime rows were omitted; absence of a proposal is not conclusive.",
        "stale",
      ));
    }
    if (inbox.items.length === 0) {
      elements.agentWorkflowInbox.append(emptyMemory(
        availability.registeredTotal === 0
          ? "No registered agents. Register or launch an agent to prepare a workflow proposal."
          : !availability.canPrepareWorkflow
            ? "No live agents. Start an agent to prepare a workflow proposal."
            : "No workflow proposals. Nothing has been approved or executed.",
      ));
      return;
    }
    inbox.items.forEach((proposal) => {
      elements.agentWorkflowInbox.append(workflowProposalRow(proposal));
    });
  }

  function handoffRow(handoff: HandoffProposal): HTMLElement {
    const row = intelligenceItem(
      `Handoff proposal · ${handoff.sourceRunId.slice(0, 8)} → ${handoff.targetRunId.slice(0, 8)}`,
      `Created ${shortRelativeTime(handoff.createdAt)} · expires ${shortRelativeTime(handoff.expiresAt)} · proposal only; no authority granted.`,
      "waiting",
    );
    const preview = document.createElement("pre");
    preview.className = "intelligence-detail agent-handoff-preview";
    preview.textContent = handoff.preview.text;
    const acknowledge = actionButton("Acknowledge / dismiss", `handoff:${handoff.id}`);
    acknowledge.addEventListener("click", () => {
      void ctx.snapshot.runMutation(
        (generation) => api().AcknowledgeAgentHandoffV2(
          generation,
          handoff.id,
          handoff.targetRunId,
        ),
        "Acknowledging handoff proposal…",
        "Could not acknowledge handoff proposal",
      );
    });
    row.append(preview, acknowledge);
    return row;
  }

  function renderAgentHandoffs(
    items: readonly AgentActivityItemView[],
    inbox: AgentActivity["handoffs"],
    availability: AgentActivity,
  ): void {
    const previousSource = elements.agentHandoffSource.value;
    const previousTarget = elements.agentHandoffTarget.value;
    elements.agentHandoffSource.replaceChildren();
    elements.agentHandoffTarget.replaceChildren();
    const live = liveAgents(items);
    const focusWasInForm = !availability.canHandoff &&
      elements.agentHandoffForm.contains(document.activeElement);
    elements.agentHandoffForm.hidden = !availability.canHandoff;
    elements.agentHandoffHelp.hidden = !availability.canHandoff;
    if (focusWasInForm) elements.agentActivityHeading.focus();
    live.forEach((item) => {
      const label = `Agent ${item.runId.slice(0, 8)} · ${runtimeAssociationLabel(item.association)}`;
      for (const select of [elements.agentHandoffSource, elements.agentHandoffTarget]) {
        const option = document.createElement("option");
        option.value = item.runId;
        option.textContent = label;
        select.append(option);
      }
    });
    if (live.some((item) => item.runId === previousSource)) elements.agentHandoffSource.value = previousSource;
    if (live.some((item) => item.runId === previousTarget)) elements.agentHandoffTarget.value = previousTarget;
    if (elements.agentHandoffTarget.value === elements.agentHandoffSource.value && live.length > 1) {
      elements.agentHandoffTarget.value = live[1].runId;
    }
    elements.agentHandoffSend.disabled = live.length < 2;
    elements.agentHandoffInbox.replaceChildren();
    if (inbox.incomplete) {
      elements.agentHandoffInbox.append(
        intelligenceItem("Handoff inbox incomplete", "Some runtime rows were omitted; proposals may be unavailable.", "stale"),
      );
    }
    if (inbox.items.length === 0) {
      elements.agentHandoffInbox.append(emptyMemory(
        availability.registeredTotal === 0
          ? "Sending a handoff needs two registered agents."
          : !availability.canHandoff
            ? "Two live agents are required to send a handoff."
            : "No pending handoff proposals.",
      ));
      return;
    }
    inbox.items.forEach((handoff) => elements.agentHandoffInbox.append(handoffRow(handoff)));
  }

  function restoreMutationFocus(focusKey: string): void {
    if (!focusKey) return;
    const exact = Array.from(document.querySelectorAll<HTMLElement>("[data-mutation-focus-key]"))
      .find((candidate) => candidate.dataset.mutationFocusKey === focusKey);
    if (exact instanceof HTMLElement) {
      if (exact.offsetParent !== null) exact.focus();
      return;
    }
    const fallback = mutationFocusFallback(focusKey);
    if (fallback === "handoffSend" &&
      !elements.agentHandoffForm.hidden && !elements.agentHandoffSend.disabled) {
      elements.agentHandoffSend.focus();
    } else if (fallback === "workflowPrepare" &&
      !elements.agentWorkflowForm.hidden && !elements.agentWorkflowPrepare.disabled) {
      elements.agentWorkflowPrepare.focus();
    }
  }

  function hideAgentActionForms(): void {
    elements.agentHandoffForm.hidden = true;
    elements.agentHandoffHelp.hidden = true;
    elements.agentWorkflowForm.hidden = true;
    elements.agentWorkflowHelp.hidden = true;
  }

  function snapshotRun(runId: string) {
    return ctx.state.snapshot?.agentActivity?.items?.find((item) => item.runId === runId);
  }

  function sendHandoff(): void {
    const sourceRunId = elements.agentHandoffSource.value;
    const targetRunId = elements.agentHandoffTarget.value;
    if (!sourceRunId || !targetRunId || sourceRunId === targetRunId) {
      showError(new Error("Choose two distinct live agents for the handoff."));
      return;
    }
    const sourceRevision = Number(snapshotRun(sourceRunId)?.association?.revision || 0);
    const targetRevision = Number(snapshotRun(targetRunId)?.association?.revision || 0);
    void ctx.snapshot.runMutation(
      (generation) => api().SendAgentHandoffV2(
        generation,
        sourceRunId,
        targetRunId,
        sourceRevision,
        targetRevision,
      ),
      "Sending bounded handoff proposal…",
      "Could not send handoff proposal",
      "handoff-send",
    );
  }

  function prepareWorkflow(): void {
    const runId = elements.agentWorkflowRun.value;
    const kind = elements.agentWorkflowKind.value;
    const needsTarget = needsWorkflowTarget(kind);
    const target = needsTarget ? elements.agentWorkflowTarget.value : "";
    const run = snapshotRun(runId);
    if (!runId || !run?.live || (needsTarget && !target)) {
      showError(new Error("Choose a live agent and an eligible target branch."));
      return;
    }
    void ctx.snapshot.runMutation(
      (generation) => api().PrepareAgentWorkflowV2(
        generation,
        runId,
        Number(run.association?.revision || 0),
        kind,
        target,
      ),
      "Preparing exact workflow proposal…",
      "Could not prepare workflow proposal",
      "workflow-prepare",
    );
  }

  function bind(): void {
    elements.agentHandoffForm.addEventListener("submit", (event) => {
      event.preventDefault();
      sendHandoff();
    });
    elements.agentWorkflowKind.addEventListener("change", () => {
      const needsTarget = needsWorkflowTarget(elements.agentWorkflowKind.value);
      elements.agentWorkflowTarget.disabled = !needsTarget;
      elements.agentWorkflowPrepare.disabled = !elements.agentWorkflowRun.value ||
        (needsTarget && !elements.agentWorkflowTarget.value);
    });
    elements.agentWorkflowForm.addEventListener("submit", (event) => {
      event.preventDefault();
      prepareWorkflow();
    });
  }

  return {
    bind,
    renderAgentActivity,
    resetAgentActivityAnnouncement,
    restoreMutationFocus,
    hideAgentActionForms,
  };
}

export type AgentActivityView = ReturnType<typeof createAgentActivityView>;
