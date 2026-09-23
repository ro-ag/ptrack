import type { Backend } from "../backend";
import type { AppContext } from "../workspace/app-context";
import { element } from "../workspace/dom";
import { mountTerminalDock, type TerminalBackend } from "./pane";

/**
 * The dock's backend: the desktop bridge fenced to one workspace generation.
 * A reply from any other generation is refused rather than applied.
 */
export function generationTerminalBackend(api: () => Backend, generation: number): TerminalBackend {
  function assertGeneration<T extends { generation: unknown }>(response: T): T {
    if (Number(response.generation) !== generation) {
      throw new Error("Stale terminal response ignored");
    }
    return response;
  }
  return {
    async GetTerminalProfiles() {
      return assertGeneration(await api().GetTerminalProfilesV2(generation)).profiles;
    },
    async CreateTerminal(profileID, cwd, rows, columns) {
      return assertGeneration(
        await api().CreateTerminalV2(generation, profileID, cwd, rows, columns),
      );
    },
    async LaunchLinkedAgent(profileID, cwd, rows, columns, association) {
      return assertGeneration(
        await api().LaunchLinkedAgentV2(
          generation,
          profileID,
          cwd,
          rows,
          columns,
          association,
        ),
      );
    },
    RollbackLinkedAgent(sessionID) {
      return api().RollbackLinkedAgentLaunchV2(generation, sessionID);
    },
    // Both are fenced by the workspace generation inside the runtime, so their
    // responses carry no generation of their own to assert here.
    OpenTerminalWindow(sessions, shape) {
      return api().OpenTerminalWindow(sessions, shape);
    },
    ClaimTerminalStream(sessionID, fromSequence) {
      return api().ClaimTerminalStream(sessionID, fromSequence);
    },
    async MutateTerminalAssociation(sessionID, expectedRevision, association) {
      return assertGeneration(
        await api().MutateTerminalAssociationV2(
          generation,
          sessionID,
          expectedRevision,
          association === undefined,
          association ?? { version: 1 },
        ),
      );
    },
    async PreviewTerminalWriteback(sessionID, expectedRevision, kind, content) {
      return assertGeneration(
        await api().PreviewTerminalWritebackV2(
          generation,
          sessionID,
          expectedRevision,
          kind,
          content,
        ),
      );
    },
    async WriteTerminalMemory(
      sessionID,
      expectedRevision,
      requestID,
      kind,
      content,
      confirmSummary,
    ) {
      return assertGeneration(
        await api().WriteTerminalMemoryV2(
          generation,
          sessionID,
          expectedRevision,
          requestID,
          kind,
          content,
          confirmSummary,
        ),
      );
    },
    async ValidateTerminalCWDs(cwds) {
      return assertGeneration(
        await api().ValidateTerminalCWDsV2(generation, cwds),
      ).results;
    },
    ResizeTerminal(sessionID, rows, columns) {
      return api().ResizeTerminalV2(generation, sessionID, rows, columns);
    },
    CloseTerminal(sessionID, force) {
      return api().CloseTerminalV2(generation, sessionID, force);
    },
    async GetScratchpadV1() {
      return assertGeneration(await api().GetScratchpadV1(generation));
    },
    async SetScratchpadV1(_generation, revision, scratchpad) {
      return assertGeneration(
        await api().SetScratchpadV1(generation, revision, scratchpad),
      );
    },
  };
}

/**
 * Glue between the window and the terminal dock: the dock's backend is the
 * desktop bridge fenced to one workspace generation, and the dock is mounted
 * for one project root at a time.
 */
export function createTerminalBackendController(ctx: AppContext) {
  const { api, openHelpDestination, showError, workspaceController } = ctx;
  const elements = {
    terminalHelp: element("#terminal-help", HTMLButtonElement),
  };

  let terminalGeneration = 0;
  let terminalProjectRoot = "";



  async function ensureTerminalDock(generation: number, projectRoot: string): Promise<void> {
    if (
      ctx.state.terminalHandle &&
      terminalGeneration === generation &&
      terminalProjectRoot === projectRoot
    ) return;
    if (ctx.state.terminalHandle) {
      ctx.agentLaunch.closeAgentLaunchPicker(false, true);
      ctx.association.closeTerminalAssociationEditor(false, true);
      ctx.writeback.closeTerminalWriteback(false, true);
      ctx.snapshot.closeTaskTransition(false, false, true);
    }
    disposeTerminalDock();
    terminalGeneration = generation;
    terminalProjectRoot = projectRoot;
    try {
      const handle = mountTerminalDock({
        backend: generationTerminalBackend(api, generation),
        workspaceGeneration: generation,
        projectRoot,
        showError,
      });
      ctx.state.terminalHandle = handle;
      handle.setLayoutLocked(ctx.state.firstPlanState.phase !== "idle");
      handle.setVisible(ctx.state.workspaceState.status === "open");
      ctx.snapshot.applicationOverlayCoordinator.setDock(handle);
      await handle.ready;
      const current = workspaceController.state;
      if (
        ctx.state.terminalHandle !== handle ||
        current.generation !== generation ||
        !["open", "loading"].includes(current.status)
      ) {
        handle.dispose();
        if (ctx.state.terminalHandle === handle) {
          ctx.state.terminalHandle = null;
          ctx.snapshot.applicationOverlayCoordinator.setDock(null);
        }
        return;
      }
      ctx.layout.restorePanelLayout();
    } catch (error) {
      const current = workspaceController.state;
      if (current.status === "open" && current.generation === generation) {
        showError(error);
      }
    }
  }

  function terminalDockProjectRoot(): string {
    return terminalProjectRoot;
  }

  function disposeTerminalDock(): void {
    ctx.association.closeTerminalAssociationEditor(false, true);
    ctx.writeback.closeTerminalWriteback(false, true);
    ctx.snapshot.closeTaskTransition(false, false, true);
    ctx.snapshot.applicationOverlayCoordinator.setDock(null);
    ctx.state.terminalHandle?.dispose();
    ctx.state.terminalHandle = null;
    ctx.layout.forgetPanelLayoutRestore();
    terminalGeneration = 0;
    terminalProjectRoot = "";
  }

  // The working-directory field is narrower than some paths: its tooltip
  // carries the full path, and when it is not being edited it scrolls to the
  // end so the project folder name stays visible.
  function bindWorkingDirectoryTail(): void {
    const cwd = document.querySelector("#terminal-cwd");
    if (!(cwd instanceof HTMLInputElement)) return;
    const showTail = () => {
      cwd.title = cwd.value || "Project root";
      if (document.activeElement !== cwd) cwd.scrollLeft = cwd.scrollWidth;
    };
    cwd.addEventListener("pointerenter", showTail);
    cwd.addEventListener("blur", showTail);
    cwd.addEventListener("change", showTail);
    new MutationObserver(() => requestAnimationFrame(showTail)).observe(
      document.querySelector("#terminal-dock") ?? document.body,
      { attributes: true, attributeFilter: ["data-state"] },
    );
  }

  function bind(): void {
    bindWorkingDirectoryTail();
    elements.terminalHelp.addEventListener("click", () => {
      openHelpDestination("terminals");
    });
  }

  return {
    bind,
    ensureTerminalDock,
    terminalDockProjectRoot,
    disposeTerminalDock,
  };
}

export type TerminalBackendController = ReturnType<typeof createTerminalBackendController>;
