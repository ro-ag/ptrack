import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { build } from "vite";

const frontendRoot = resolve(import.meta.dirname, "..");
const distRoot = resolve(frontendRoot, "dist");

describe("production asset layout", () => {
  beforeAll(async () => {
    await build({
      configFile: resolve(frontendRoot, "vite.config.ts"),
      root: frontendRoot,
      logLevel: "silent",
    });
  });

  it("emits the embedded board assets with stable names", () => {
    const indexPath = resolve(distRoot, "index.html");

    expect(existsSync(indexPath)).toBe(true);
    expect(existsSync(resolve(distRoot, "app.js"))).toBe(true);
    expect(existsSync(resolve(distRoot, "style.css"))).toBe(true);

    const index = readFileSync(indexPath, "utf8");
    const app = readFileSync(resolve(distRoot, "app.js"), "utf8");
    const styles = readFileSync(resolve(distRoot, "style.css"), "utf8");
    const paneSource = readFileSync(
      resolve(frontendRoot, "src/terminal/pane.ts"),
      "utf8",
    );
    const appSource = readFileSync(
      resolve(frontendRoot, "src/app.js"),
      "utf8",
    );
    const applicationOverlaySource = readFileSync(
      resolve(frontendRoot, "src/workspace/application-overlay.ts"),
      "utf8",
    );
    expect(index).toContain('src="/app.js"');
    expect(index).toContain('href="/style.css"');
    expect(index).toMatch(
      /id="app-version"[^>]*tabindex="0"[^>]*aria-haspopup="dialog"[^>]*>dev<\/button>/,
    );
    expect(styles).toMatch(
      /\.app-version\{[^}]*position:relative[^}]*z-index:1[^}]*--wails-draggable:\s*no-drag/,
    );
    expect(styles).toMatch(/\.state-card\{[^}]*box-shadow:/);
    expect(styles).not.toMatch(
      /\.state-card\{[^}]*(?:animation|transform|opacity):/,
    );
    expect(index).toMatch(
      /id="updates-modal"[\s\S]*role="dialog"[\s\S]*aria-modal="true"[\s\S]*id="updates-automatic"[\s\S]*aria-label="Update download progress"[\s\S]*id="updates-primary"/,
    );
    expect(index).toContain("Automatic checks never download or install anything");
    expect(app).toContain("GetUpdateState");
    expect(app).toContain("CheckForUpdates");
    expect(app).toContain("DownloadUpdate");
    expect(app).toContain("ApplyUpdate");
    expect(app).toContain("CancelUpdateOperation");
    expect(app).toContain("update:state-changed");
    expect(app).not.toContain("releases/download");
    expect(app).not.toContain("checksums.txt");
    expect(index).toMatch(/id="terminal-tabs"[\s\S]*role="tablist"/);
    expect(index).not.toMatch(/id="terminal-tabs"[^>]*aria-live/);
    expect(index).toMatch(
      /<div[^>]*id="terminal-tabs"[^>]*role="tablist"[^>]*>\s*<\/div>/,
    );
    expect(index).toMatch(
      /<div[^>]*id="terminal-tab-actions"[^>]*role="toolbar"[^>]*aria-label="Active terminal tab actions"[^>]*>\s*<\/div>/,
    );
    expect(index).not.toMatch(/id="terminal-body"[^>]*role=/);
    expect(app).toContain("terminal-tab-panel-");
    expect(app).toContain("tabpanel");
    expect(app).toContain("aria-controls");
    expect(app).toContain("aria-labelledby");
    expect(app).toContain("setVisible");
    expect(app).toContain("setApplicationOverlayOpen");
    expect(app).toContain("body > .modal, body > [data-terminal-overlay]");
    expect(app).toContain("MutationObserver");
    expect(app).toContain("subtree:!0");
    expect(index).toContain('id="terminal-cwd"');
    expect(index).toContain('id="terminal-reset-workspace"');
    expect(index).toMatch(
      /id="terminal-termination-modal"[\s\S]*aria-modal="true"[\s\S]*>Terminate<\/button>/,
    );
    expect(index).toMatch(
      /id="terminal-paste-form"[\s\S]*aria-modal="true"[\s\S]*aria-describedby="terminal-paste-detail"/,
    );
    expect(index).toMatch(
      /id="terminal-link-context"[\s\S]*aria-label="Link terminal context"[\s\S]*disabled/,
    );
    expect(index).toMatch(
      /class="terminal-actions"[\s\S]*aria-label="Terminal actions"[\s\S]*id="terminal-open"[\s\S]*aria-label="Open terminal"[\s\S]*<svg/,
    );
    expect(index).toMatch(
      /id="terminal-help"[\s\S]*aria-label="Open terminal guide"/,
    );
    expect(index).toMatch(
      /class="settings-heading-actions"[\s\S]*id="capability-help"[\s\S]*>Capability guide<\/button>/,
    );
    expect(app).toContain("OpenHelpDestination");
    expect(appSource).toContain('openHelpDestination("terminals")');
    expect(appSource).toContain('openHelpDestination("capabilities")');
    expect(appSource).not.toContain("ro-ag.github.io/ptrack/help");
    expect(index).toMatch(
      /id="terminal-close"[\s\S]*class="terminal-action-button terminal-action-stop"[\s\S]*aria-label="Close terminal"/,
    );
    expect(index).toMatch(
      /id="terminal-diagnostics-toggle"[\s\S]*aria-controls="terminal-diagnostics"[\s\S]*aria-expanded="false"[\s\S]*<svg[^>]*aria-hidden="true"/,
    );
    expect(index).toMatch(
      /id="terminal-renderer-retry"[\s\S]*aria-label="Retry terminal renderer"[\s\S]*<svg[^>]*aria-hidden="true"/,
    );
    expect(index).toMatch(
      /id="terminal-force-stop"[\s\S]*class="terminal-action-button terminal-action-stop"[\s\S]*aria-label="Force stop terminal"[\s\S]*<svg[^>]*aria-hidden="true"/,
    );
    expect(index).toMatch(
      /id="terminal-diagnostics"[\s\S]*aria-live="polite"[\s\S]*Content-free state only\. Restart creates a fresh session; streams are never reconnected\./,
    );
    expect(styles).toMatch(
      /\.terminal-diagnostics\s*\{[\s\S]*max-height:[^;]+;[\s\S]*overflow-y:\s*auto/,
    );
    expect(index).toMatch(
      /id="terminal-association-modal"[\s\S]*role="dialog"[\s\S]*aria-modal="true"[\s\S]*id="terminal-association-target"[\s\S]*>Detach<\/button>/,
    );
    expect(index).toMatch(
      /id="terminal-writeback"[\s\S]*id="terminal-writeback-modal"[\s\S]*role="dialog"[\s\S]*aria-modal="true"[\s\S]*id="terminal-writeback-kind"[\s\S]*id="terminal-writeback-content"[\s\S]*id="terminal-writeback-preview"[\s\S]*id="terminal-writeback-summary-confirm"/,
    );
    expect(index).toMatch(
      /id="task-transition-modal"[\s\S]*role="alertdialog"[\s\S]*aria-modal="true"[\s\S]*id="task-transition-detail"[\s\S]*id="task-transition-cancel"[\s\S]*id="task-transition-submit"/,
    );
    expect(app).toContain("MoveTaskV3");
    expect(app).not.toContain("MoveTaskV2");
    expect(app).toContain("linked sessions, processes, and capabilities stay unchanged");
    expect(app).toContain("Finish the current task status change before starting another.");
    expect(app).toContain("Stale task transition response ignored");
    expect(app).toContain("Stale terminal association response ignored");
    expect(app).toContain("Linking changes context only and grants no capabilities.");
    expect(index).not.toMatch(/id="agent-activity"[^>]*aria-live/);
    expect(index).toMatch(
      /id="agent-activity-live"[^>]*role="status"[^>]*aria-live="polite"[^>]*aria-atomic="true"/,
    );
    expect(app).toContain("mutationFocusKey");
    expect(styles).toContain(".terminal-tab-indicator");
    expect(styles).toMatch(/\[data-indicator=(?:"failed"|failed)\]/);
    expect(styles).toMatch(/\[data-unread=(?:"true"|true)\]/);
    expect(styles).toContain(".terminal-split-node");
    expect(styles).toMatch(
      /\.terminal-split-leaf\{[^}]*width:100%[^}]*height:100%/,
    );
    expect(styles).toContain("touch-action:none");
    expect(styles).toMatch(
      /data-state=(?:"closed"|closed)\]\[data-layout-interactive=(?:"false"|false)\]/,
    );
    expect(styles).toMatch(/data-board-hidden=(?:"true"|true)\] \.terminal-dock\{height:100%/);
    expect(styles).toMatch(/data-terminal-hidden=(?:"true"|true)\] \.terminal-dock\{display:none/);
    expect(styles).toMatch(
      /\.board-heading\{[^}]*min-width:0[^}]*flex-wrap:wrap/,
    );
    expect(styles).toMatch(
      /\.title-row h2\{[^}]*min-width:0[^}]*flex:1 1 auto[^}]*text-overflow:ellipsis/,
    );
    expect(styles).toMatch(
      /\.board-actions\{[^}]*min-width:0[^}]*flex-wrap:wrap[^}]*justify-content:flex-end/,
    );
    expect(styles).toMatch(
      /\.add-form input\{[^}]*min-width:120px[^}]*flex:1 1 170px/,
    );
    expect(styles).toMatch(
      /@media\(max-width:960px\)\{[^}]*#app[^}]*\}[^}]*\.board-heading[^}]*\}\.plan-context,\.board-actions\{width:100%;min-width:0;max-width:100%;flex:0 0 100%\}\.board-actions\{justify-content:flex-start\}\.add-form\{min-width:0;flex-basis:250px\}/,
    );
    expect(styles).toMatch(
      /\.panel-toggle:focus-visible,[^{]*\.terminal-context-menu button:focus-visible\{[^}]*outline:2px solid var\(--accent\)[^}]*outline-offset:-2px/,
    );
    expect(paneSource).toMatch(
      /action === "zoom-reset"\) \{\s*this\.#setFontSize\(this\.#activeProfileDefaultFontSize\(\)\)/,
    );
    expect(paneSource).toMatch(
      /setApplicationOverlayOpen\(open: boolean, focusTerminal: false\): void \{[\s\S]*?#renderPanelVisibility\(focusTerminal\)/,
    );
    expect(paneSource).toContain("revision !== this.#panelVisibilityRevision");
    expect(paneSource).toContain("webglRecoveryPaused");
    expect(paneSource).toContain("webglRecoveryAfterSuppression");
    expect(paneSource).toContain("webglRecoveryPolicyAction");
    expect(applicationOverlaySource).toContain("class ApplicationOverlayCoordinator");
    expect(applicationOverlaySource).toContain("this.#lastOpen !== open");
    expect(applicationOverlaySource).toContain("setApplicationOverlayOpen(open, false)");
    expect(applicationOverlaySource).toContain('setAttribute("aria-hidden", "true")');
    expect(applicationOverlaySource).toContain("this.#background.inert = true");
    expect(applicationOverlaySource).toContain("get activeOverlay()");
    expect(applicationOverlaySource).toContain('"data-application-overlay-layer", "active"');
    expect(applicationOverlaySource).toContain('"data-application-overlay-layer", "underlay"');
    expect(appSource).toContain("attributeOldValue: true");
    expect(appSource).toContain(
      "const modal = applicationOverlayCoordinator.activeOverlay",
    );
    expect(appSource).toContain("applicationOverlayKeyboardPolicy(");
    expect(appSource).toContain("if (!policy.trapTab) return");
    expect(appSource).toContain("closeActiveApplicationOverlay(event)");
    expect(appSource).toContain("event.stopImmediatePropagation()");
    expect(appSource).not.toMatch(
      /event\.key === "Escape" && !elements\.[A-Za-z]+\.hidden/,
    );
    expect(paneSource).toMatch(
      /!this\.#pasteModal\.hidden && event\.key === "Tab"[\s\S]*this\.#trapPasteFocus\(event\)/,
    );
    expect(paneSource).toMatch(
      /const dismissOnKey = \(event: KeyboardEvent\) => \{[\s\S]*if \(event\.defaultPrevented\) return;[\s\S]*!this\.#pasteModal\.hidden && event\.key === "Tab"[\s\S]*!this\.#pasteModal\.hidden[\s\S]*this\.#finishPasteConfirmation\(false\)/,
    );
    expect(paneSource).toMatch(
      /this\.#terminationModal, "keydown"[\s\S]*keyEvent\.key === "Tab"[\s\S]*this\.#trapTerminationFocus\(keyEvent\)/,
    );
    expect(paneSource).toMatch(
      /#trapPasteFocus[\s\S]*focusCycleIndex\(focusable\.length, current, event\.shiftKey\)/,
    );
    expect(paneSource).toMatch(
      /#trapTerminationFocus[\s\S]*focusCycleIndex\(focusable\.length, current, event\.shiftKey\)/,
    );
    expect(styles).toMatch(/data-application-overlay-layer=(?:active|"active")/);
    expect(styles).toMatch(/data-application-overlay-layer=(?:underlay|"underlay")/);
    expect(index).toMatch(
      /<main[^>]*id="main-content"[^>]*class="canvas-main"[^>]*aria-label="Workspace content"[\s\S]*id="settings-page"[^>]*aria-labelledby="capabilities-heading"[\s\S]*id="capabilities-heading"[^>]*tabindex="-1"/,
    );
    expect(index).toMatch(
      /<main[^>]*id="main-content"[^>]*class="canvas-main"[\s\S]*id="overview-page"[^>]*aria-label="Project overview"/,
    );
    expect(index).toMatch(
      /class="section-label">Rolling project summary<\/p>[\s\S]*id="summary"[^>]*>No rolling summary yet\.<\/p>/,
    );
    expect(appSource).toContain(
      "No rolling summary yet. Agents can update it with ptrack summary set.",
    );
    expect(index).not.toContain("No rolling handoff yet.");
    expect(styles).toMatch(
      /\.canvas-main\s*\{[^}]*min-width:\s*0[^}]*min-height:\s*0[^}]*flex:\s*1 1 auto[^}]*display:\s*flex[^}]*flex-direction:\s*column/s,
    );
    expect(index).toMatch(
      /id="capability-status"[^>]*role="status"[^>]*aria-live="polite"[^>]*aria-atomic="true"/,
    );
    expect(index).toMatch(
      /class="memory-section capability-editor"[^>]*aria-labelledby="capability-editor-title"[\s\S]*id="capability-preview-result"[^>]*aria-labelledby="capability-preview-heading"/,
    );
    expect(index).toMatch(
      /class="memory-section capability-list-section"[^>]*aria-labelledby="capability-list-heading"[\s\S]*id="capability-audit-list"[^>]*aria-labelledby="capability-audit-heading"/,
    );
    expect(index).toMatch(
      /id="capability-git-fields"[^>]*hidden[^>]*disabled[\s\S]*id="capability-ssh-fields"[^>]*hidden[^>]*disabled/,
    );
    expect(appSource).toContain("setCapabilityStatus(action, phase)");
    expect(appSource).toContain("showCapabilityError(action)");
    expect(appSource).toContain(
      'showError(new Error(capabilityAnnouncement(action, "failure")))',
    );
    for (const action of ["audit", "test", "preview"]) {
      expect(appSource).toContain(`showCapabilityError("${action}")`);
    }
    expect(appSource).toContain("capabilityFocusRestoreKey(");
    expect(appSource).toContain('empty.setAttribute("role", "listitem")');
    expect(appSource).toContain("fieldset.disabled = !active");
    expect(appSource).toContain('EnableCapabilityV2(generation, Number(view.capability.id), digest)');
    expect(appSource).toContain("canEnableCapability(view, digest)");
    expect(appSource).toContain('setView("settings", true)');
    expect(appSource).not.toContain('setStatus("Testing connection');
  });
});
