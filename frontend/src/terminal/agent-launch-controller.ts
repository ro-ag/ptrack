import type { AppContext } from "../workspace/app-context";
import { element } from "../workspace/dom";
import { messageFrom } from "../workspace/format";
import type { AssociationPointerV1 } from "../workspace/model";
import type { BoardTask } from "../workspace/snapshot-types";
import {
  linkedAssociationPointer,
  selectedInstalledAgentProfile,
  type InstalledAgentProfile,
} from "./linked-launch";
import type { TerminalDockHandle } from "./pane";

/** What an agent is launched for: a plan, or one of its tasks. */
export interface AgentLaunchTarget {
  planId: number;
  task?: BoardTask;
}

interface AgentLaunchRequest {
  association: AssociationPointerV1;
  generation: number;
  handle: TerminalDockHandle;
  title: string;
}

export function createAgentLaunchController(ctx: AppContext) {
  const { setStatus, showError, workspaceController } = ctx;
  const elements = {
    agentLaunchCancel: element("#agent-launch-cancel", HTMLButtonElement),
    agentLaunchDetail: element("#agent-launch-detail", HTMLParagraphElement),
    agentLaunchForm: element("#agent-launch-form", HTMLFormElement),
    agentLaunchHeading: element("#agent-launch-heading", HTMLHeadingElement),
    agentLaunchMessage: element("#agent-launch-message", HTMLParagraphElement),
    agentLaunchModal: element("#agent-launch-modal", HTMLDivElement),
    agentLaunchSelect: element("#agent-launch-profile", HTMLSelectElement),
    agentLaunchSubmit: element("#agent-launch-submit", HTMLButtonElement),
    drawer: element("#task-drawer", HTMLDivElement),
  };

  let agentLaunchRequest: AgentLaunchRequest | null = null;
  let agentLaunchProfiles: InstalledAgentProfile[] = [];
  let agentLaunchReturnFocus: HTMLElement | null = null;
  let agentLaunchSequence = 0;
  let agentLaunchBusy = false;

  async function openAgentLaunchPicker(
    target: AgentLaunchTarget,
    invoker: Element | null = document.activeElement,
  ): Promise<void> {
    if (workspaceController.state.status !== "open") return;
    let association: AssociationPointerV1;
    try {
      association = linkedAssociationPointer(
        Number(target.planId),
        target.task ? Number(target.task.id) : undefined,
      );
    } catch (error) {
      showError(error);
      return;
    }
    closeAgentLaunchPicker(false, true);
    const sequence = ++agentLaunchSequence;
    const generation = workspaceController.state.generation;
    agentLaunchReturnFocus = invoker instanceof HTMLElement ? invoker : null;
    agentLaunchProfiles = [];
    elements.agentLaunchHeading.textContent = target.task
      ? `Launch agent for task #${target.task.id}`
      : `Launch agent for plan #${target.planId}`;
    elements.agentLaunchDetail.textContent = target.task
      ? target.task.title
      : ctx.state.board?.planTitle || `Plan #${target.planId}`;
    elements.agentLaunchMessage.textContent = "Discovering installed agent profiles…";
    elements.agentLaunchSelect.replaceChildren();
    elements.agentLaunchSelect.disabled = true;
    elements.agentLaunchCancel.disabled = false;
    elements.agentLaunchSubmit.disabled = true;
    elements.agentLaunchModal.hidden = false;
    requestAnimationFrame(() => elements.agentLaunchCancel.focus());

    try {
      await ctx.terminalBackend.ensureTerminalDock(
        generation,
        ctx.state.workspaceState.project?.root || ctx.terminalBackend.terminalDockProjectRoot(),
      );
      const handle = ctx.state.terminalHandle;
      if (!handle) throw new Error("Terminal workspace is unavailable");
      const profiles = await handle.agentProfiles();
      if (
        sequence !== agentLaunchSequence ||
        elements.agentLaunchModal.hidden ||
        workspaceController.state.status !== "open" ||
        workspaceController.state.generation !== generation ||
        ctx.state.terminalHandle !== handle
      ) return;
      agentLaunchProfiles = profiles;
      agentLaunchRequest = {
        association,
        generation,
        handle,
        title: target.task
          ? `Task #${target.task.id} · agent`
          : `Plan #${target.planId} · agent`,
      };
      if (profiles.length === 0) {
        elements.agentLaunchMessage.textContent =
          "No installed agent profiles were discovered. Install a supported agent to launch it here.";
        return;
      }
      for (const profile of profiles) {
        const option = document.createElement("option");
        option.value = profile.id;
        option.textContent = profile.name;
        elements.agentLaunchSelect.append(option);
      }
      elements.agentLaunchMessage.textContent =
        "Only installed agent profiles are available; this link grants no capabilities.";
      elements.agentLaunchSelect.disabled = false;
      elements.agentLaunchSubmit.disabled = false;
      elements.agentLaunchSelect.focus();
    } catch (error) {
      if (sequence !== agentLaunchSequence || elements.agentLaunchModal.hidden) return;
      elements.agentLaunchMessage.textContent = messageFrom(error);
      showError(error);
    }
  }

  function closeAgentLaunchPicker(restoreFocus = true, force = false): void {
    if (agentLaunchBusy && !force) return;
    agentLaunchSequence += 1;
    agentLaunchBusy = false;
    agentLaunchRequest = null;
    agentLaunchProfiles = [];
    ctx.snapshot.hideApplicationOverlay(elements.agentLaunchModal);
    elements.agentLaunchSelect.disabled = true;
    elements.agentLaunchCancel.disabled = false;
    elements.agentLaunchSubmit.disabled = true;
    if (restoreFocus) agentLaunchReturnFocus?.focus?.();
    agentLaunchReturnFocus = null;
  }

  async function submitAgentLaunch(): Promise<void> {
    const request = agentLaunchRequest;
    if (!request || elements.agentLaunchSubmit.disabled) return;
    let profile: InstalledAgentProfile;
    try {
      profile = selectedInstalledAgentProfile(
        agentLaunchProfiles,
        elements.agentLaunchSelect.value,
      );
    } catch (error) {
      showError(error);
      return;
    }
    const sequence = agentLaunchSequence;
    agentLaunchBusy = true;
    elements.agentLaunchSelect.disabled = true;
    elements.agentLaunchCancel.disabled = true;
    elements.agentLaunchSubmit.disabled = true;
    elements.agentLaunchMessage.textContent = `Launching ${profile.name}…`;
    try {
      await request.handle.launchLinked({
        profileId: profile.id,
        title: request.title.replace("agent", profile.name),
        association: request.association,
      });
      if (
        sequence !== agentLaunchSequence ||
        workspaceController.state.status !== "open" ||
        workspaceController.state.generation !== request.generation ||
        ctx.state.terminalHandle !== request.handle
      ) return;
      agentLaunchBusy = false;
      closeAgentLaunchPicker(false);
      if (!elements.drawer.hidden) ctx.drawer.closeTaskDetail();
      setStatus(`${profile.name} launched in a linked terminal tab.`);
      await ctx.snapshot.loadSnapshot(ctx.state.board?.planId || 0, false);
    } catch (error) {
      if (sequence !== agentLaunchSequence || elements.agentLaunchModal.hidden) return;
      agentLaunchBusy = false;
      elements.agentLaunchMessage.textContent = messageFrom(error);
      elements.agentLaunchSelect.disabled = agentLaunchProfiles.length === 0;
      elements.agentLaunchCancel.disabled = false;
      elements.agentLaunchSubmit.disabled = agentLaunchProfiles.length === 0;
      showError(error);
    }
  }

  function bind(): void {
    elements.agentLaunchForm.addEventListener("submit", (event) => {
      event.preventDefault();
      void submitAgentLaunch();
    });
    elements.agentLaunchCancel.addEventListener("click", () => closeAgentLaunchPicker());
    document.querySelectorAll("[data-close-agent-launch]").forEach((closer) => {
      closer.addEventListener("click", () => closeAgentLaunchPicker());
    });
  }

  return {
    bind,
    openAgentLaunchPicker,
    closeAgentLaunchPicker,
  };
}

export type AgentLaunchController = ReturnType<typeof createAgentLaunchController>;
