// Windows and Linux draw their own title bars. macOS keeps its native inset
// traffic lights; ordinary browsers never invoke native window controls.
import { invoke } from "@tauri-apps/api/core";

const ACTIONS = new Set(["menu", "minimize", "toggle-maximize", "close", "state"]);

function installWindowChrome(target = globalThis, dependencies = {}) {
  const document = target.document;
  if (!["custom", "linux"].includes(document?.documentElement?.dataset.windowChrome)) return false;
  const invokeCommand = dependencies.invoke || invoke;
  const root = document.documentElement;

  const run = async (action, position = {}) => {
    if (!ACTIONS.has(action)) return;
    try {
      const maximized = await invokeCommand("window_chrome", { action, ...position });
      root.dataset.windowMaximized = String(maximized === true);
      for (const button of document.querySelectorAll('[data-window-control="toggle-maximize"]')) {
        const label = maximized === true ? "Restore" : "Maximize";
        button.setAttribute("aria-label", label);
        button.setAttribute("title", label);
      }
    } catch (error) {
      target.console?.error?.(`p-track window control failed: ${error}`);
    }
  };

  for (const button of document.querySelectorAll("[data-window-control]")) {
    button.addEventListener("click", () => void run(button.dataset.windowControl));
  }
  // The native menu opens just under the button that asked for it, in the
  // same logical pixels the webview lays out in.
  for (const button of document.querySelectorAll("[data-app-menu]")) {
    button.addEventListener("click", () => {
      const rect = button.getBoundingClientRect();
      const position = { x: Math.round(rect.left), y: Math.round(rect.bottom + 4) };
      if (button.dataset.appMenu) position.menuLabel = button.dataset.appMenu;
      void run("menu", position);
    });
  }
  // Maximizing by double-click, Win+Up, or a snap changes the window without
  // a click here, so the restore glyph follows every resize.
  let pending = 0;
  target.addEventListener("resize", () => {
    target.clearTimeout(pending);
    pending = target.setTimeout(() => void run("state"), 120);
  });
  void run("state");
  return true;
}

installWindowChrome();

export { installWindowChrome };
