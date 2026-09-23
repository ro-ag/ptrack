// Entry point: loads the stylesheets and the desktop bridge, builds the window
// controllers, then boots either the main window or a popped-out terminal
// window from the same document.
import "./settings-kimi.css";
import "./landing.css";
import "./cover-flow.css";
import "./tauri-bridge";

import { createApp } from "./app";
import { terminalWindowLabel } from "./terminal/pop-out";
import { resolveFirstRunStartupState } from "./workspace/first-run";
import { messageFrom } from "./workspace/format";

async function start(ctx) {
  const { api, workspaceController } = ctx;
  let startupError = null;
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      api();
      // The stored record is the authority, so it lands before the terminal
      // dock or the first paint-sensitive surface reads its cache.
      await ctx.settings.loadPreferences();
      await ctx.layout.loadLayoutState();
      const startup = await resolveFirstRunStartupState(
        () => api().GetWorkspaceState(),
        () => api().GetPendingInitializationV1(),
      );
      const state = startup.state;
      workspaceController.publish({
        status: state.status,
        generation: Number(state.generation || 0),
      });
      ctx.shell.renderWorkspaceState(state, false);
      const restored = state.status === "welcome" && startup.pending
        ? ctx.firstRun.hydratePendingInitialization(startup.pending)
        : false;
      if (state.status === "welcome" && !restored) {
        // Projects opens on its search field: typing filters right away, and
        // Enter can never initialize a folder by accident.
        requestAnimationFrame(() => {
          if (!ctx.shell.focusLandingSearch()) ctx.shell.focusOpenProject();
        });
      }
      ctx.nativeMenu.registerNativeProjectActions();
      ctx.snapshot.refreshLoop.start();
      if (state.status === "open") {
        await ctx.snapshot.loadSnapshot(ctx.layout.restoredPlanId(state.project?.root));
      }
      return;
    } catch (error) {
      startupError = error;
      await new Promise((resolve) => window.setTimeout(resolve, 100));
    }
  }
  workspaceController.publish({ status: "error", generation: 0 });
  ctx.shell.renderWorkspaceState(
    {
      status: "error",
      generation: 0,
      error: `Could not load the desktop startup state: ${messageFrom(startupError)}`,
    },
    true,
  );
}

/** The window's controllers; exported so tests can drive the booted window. */
export const app = createApp();
const terminalWindow = terminalWindowLabel(window.location.hash);
/** Resolves once startup has settled the first screen. */
export const started = terminalWindow
  ? app.terminalWindow.startTerminalWindow(terminalWindow)
  : start(app);
