import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { build } from "vite";

const frontendRoot = resolve(import.meta.dirname, "..");
const distRoot = resolve(frontendRoot, "dist");

// Returns the matching closing tag index, or -1 for unclosed markup.
function closingIndex(html, start, tag) {
  const tags = new RegExp(`<${tag}\\b|</${tag}>`, "g");
  tags.lastIndex = start;
  let depth = 0;
  for (let match = tags.exec(html); match; match = tags.exec(html)) {
    depth += match[0].startsWith("</") ? -1 : 1;
    if (depth === 0) return match.index;
  }
  return -1;
}

describe("production asset layout", () => {
  beforeAll(async () => {
    await build({
      configFile: resolve(frontendRoot, "vite.config.ts"),
      root: frontendRoot,
      logLevel: "silent",
    });
  });

  it("keeps real project journeys separate from carousel selection and native settings", () => {
    const index = readFileSync(resolve(frontendRoot, "index.html"), "utf8");
    const hero = index.indexOf('<header class="orbit-toolbar"');
    const heroEnd = closingIndex(index, hero, "header");
    const projects = index.indexOf('<section class="projects-panel"');
    expect(hero).toBeGreaterThan(0);
    expect(index.indexOf('id="state-open-project-button"')).toBeGreaterThan(hero);
    expect(index.indexOf('id="state-initialize-project-button"')).toBeLessThan(heroEnd);
    expect(projects).toBeGreaterThan(heroEnd);
    expect(index.match(/id="orbit-selected-project"/g)).toHaveLength(1);
    expect(index).not.toContain('class="hero-panel"');
    expect(index).not.toContain('id="orbit-ribbon"');
    expect(index).toContain('id="orbit-previous" type="button" aria-label="Previous project"');
    expect(index).toContain('id="landing-settings-open"');
  });

  it("emits the embedded board assets with stable names", () => {
    const indexPath = resolve(distRoot, "index.html");

    expect(existsSync(indexPath)).toBe(true);
    expect(existsSync(resolve(distRoot, "app.js"))).toBe(true);
    expect(existsSync(resolve(distRoot, "style.css"))).toBe(true);
    const fontPath = "fonts/hack-nerd-font/HackNerdFontMono-Regular.ttf";
    expect(readFileSync(resolve(distRoot, fontPath)).equals(
      readFileSync(resolve(frontendRoot, "public", fontPath)),
    )).toBe(true);
    expect(existsSync(resolve(distRoot, "fonts/hack-nerd-font/LICENSE.md"))).toBe(true);

    const index = readFileSync(indexPath, "utf8");
    const app = readFileSync(resolve(distRoot, "app.js"), "utf8");
    const styles = readFileSync(resolve(distRoot, "style.css"), "utf8");
    expect(styles).toContain(fontPath);
    const paneSource = readFileSync(
      resolve(frontendRoot, "src/terminal/pane.ts"),
      "utf8",
    );
    const firstRunSource = readFileSync(
      resolve(frontendRoot, "src/workspace/first-run.ts"),
      "utf8",
    );
    const firstRunJourneySource = readFileSync(
      resolve(frontendRoot, "src/workspace/first-run-journey.ts"),
      "utf8",
    );
    const firstPlanSource = readFileSync(
      resolve(frontendRoot, "src/workspace/first-plan.ts"),
      "utf8",
    );
    const recentProjectsSource = readFileSync(
      resolve(frontendRoot, "src/workspace/recent-projects.ts"),
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
    const versionStyles = styles.match(/\.app-version\{([^}]*)\}/)?.[1];
    expect(versionStyles).toMatch(/(?:^|;)position:relative(?:;|$)/);
    expect(versionStyles).toMatch(/(?:^|;)z-index:1(?:;|$)/);
    // Tauri uses data-tauri-drag-region attributes.
    expect(styles).not.toContain("--wails-draggable");
    expect(styles).toMatch(/\.state-card\{[^}]*box-shadow:/);
    expect(styles).not.toMatch(
      /\.state-card\{[^}]*(?:animation|transform|opacity):/,
    );
    // A suspended background WebView may retain the first animation frame.
    const landingSource = readFileSync(resolve(frontendRoot, "src/landing.css"), "utf8");
    expect(landingSource).not.toMatch(/#welcome-panel\s*\{[^}]*opacity:\s*0(?:;|\s)/);
    expect(landingSource).toContain('prefers-reduced-motion:reduce');
    expect(landingSource).toMatch(/#welcome-panel \.stat-label \{[^}]*white-space:normal;[^}]*overflow:visible;/);
    expect(landingSource).toContain('data-reduced-motion="always"');
    expect(landingSource).toContain(':root:not([data-reduced-motion="never"]) #welcome-panel');
    expect(index).toMatch(/id="workspace-state-heading"[^>]*>Projects<\/h2>/);
    expect(index).toMatch(/id="state-open-project-button"[\s\S]*?Open folder…<\/button>/);
    expect(index).toMatch(/id="state-initialize-project-button"[\s\S]*?Initialize project<\/button>/);
    expect(index.match(/class="state-card"/g)).toHaveLength(1);
    expect(existsSync(resolve(distRoot, "bars.png"))).toBe(true);
    expect(index).toContain('src="/bars.png"');
    expect(index).toContain('id="landing-help-open"');
    expect(index).toMatch(
      /id="post-project-onboarding"[\s\S]*aria-labelledby="onboarding-heading"[\s\S]*id="onboarding-plan-form"[\s\S]*id="onboarding-create-plan"[^>]*>Create Plan<\/button>[\s\S]*id="onboarding-skip-plan"[^>]*>Skip for Now<\/button>[\s\S]*id="onboarding-task-form"[\s\S]*id="onboarding-start-now"[\s\S]*>Start this task now<[\s\S]*id="onboarding-create-task"[^>]*>Create Task<\/button>[\s\S]*id="onboarding-finish-with-plan"[^>]*>Finish with Plan<\/button>/,
    );
    expect(index).toMatch(
      /id="recent-project-heading"[\s\S]*tabindex="-1"[\s\S]*>Choose a project[\s\S]*?<\/h3>[\s\S]*id="recent-project-list"[\s\S]*role="list"[\s\S]*aria-busy="false"[\s\S]*id="recent-project-status"[\s\S]*role="status"[\s\S]*aria-live="polite"[\s\S]*id="recent-project-error"[\s\S]*role="alert"/,
    );
    expect(index).not.toMatch(/id="workspace-state-screen"[^>]*aria-live/);
    for (const id of [
      "recent-project-error",
      "setup-goal-error",
      "setup-error",
      "onboarding-plan-error",
      "onboarding-task-error",
      "onboarding-error",
    ]) {
      expect(index).toMatch(
        new RegExp(`id="${id}"[^>]*role="alert"[^>]*aria-atomic="true"`),
      );
    }
    expect(index).toMatch(
      /id="setup-operation"[^>]*role="group"[^>]*aria-labelledby="setup-heading"[^>]*aria-busy="false"/,
    );
    expect(index).toMatch(
      /id="onboarding-operation"[^>]*role="group"[^>]*aria-labelledby="onboarding-heading"[^>]*aria-busy="false"/,
    );
    expect(index).toMatch(
      /id="onboarding-start-failed-actions"[\s\S]*id="onboarding-retry-start"[^>]*>Try Starting Again<\/button>[\s\S]*id="onboarding-finish-setup"[^>]*>Finish Setup<\/button>/,
    );
    expect(index).toMatch(
      /id="setup-panel"[\s\S]*aria-labelledby="setup-heading"[\s\S]*id="setup-progress"[\s\S]*id="setup-heading"[^>]*tabindex="-1"[\s\S]*id="setup-goal"[\s\S]*aria-describedby="setup-goal-help setup-goal-error"[\s\S]*id="setup-status"[\s\S]*role="status"[\s\S]*aria-live="polite"[\s\S]*id="setup-error"[^>]*role="alert"/,
    );
    expect(index).toMatch(
      /id="setup-goal-back"[^>]*>Back<\/button>/,
    );
    expect(index).toMatch(
      /id="setup-new-target-actions"[\s\S]*id="setup-new-target-continue"[^>]*>Continue to Goal<\/button>[\s\S]*id="setup-new-target-choose"[^>]*>Choose Another Folder<\/button>[\s\S]*id="setup-new-target-cancel"[^>]*>Cancel Setup<\/button>/,
    );
    expect(index).toMatch(
      /id="setup-guide"[\s\S]*aria-label="Project guide choice"[\s\S]*Skip Guide[\s\S]*id="setup-guide-preview"[\s\S]*aria-label="Exact guide file preview"[\s\S]*id="setup-guide-preview-button"[^>]*>Preview Guide Changes<\/button>[\s\S]*id="setup-guide-install"[^>]*>Install These Guide Changes<\/button>/,
    );
    expect(index).toMatch(
      /id="setup-guide-stale-actions"[\s\S]*id="setup-guide-review-again"[^>]*>Review Again<\/button>[\s\S]*id="setup-guide-stale-skip"[^>]*>Skip Guide<\/button>[\s\S]*id="setup-guide-stale-back"[^>]*>Back<\/button>/,
    );
    expect(index).toMatch(
      /id="setup-review"[\s\S]*id="setup-review-goal"[\s\S]*id="setup-review-guide-choice"[\s\S]*id="setup-complete-changes"/,
    );
    expect(index).toContain("Private p-track project storage");
    expect(index).toMatch(
      /id="setup-recovery-actions"[\s\S]*id="setup-retry"[^>]*hidden[^>]*>Try Again<\/button>[\s\S]*id="setup-resume"[^>]*>Resume Setup<\/button>[\s\S]*id="setup-open-recovery"[^>]*>Open Project<\/button>[\s\S]*id="setup-recovery-help"[^>]*>Open Recovery Help<\/button>[\s\S]*id="setup-recovery-choose"[\s\S]*id="setup-return-welcome"/,
    );
    expect(index).toMatch(
      /id="setup-uncertain-actions"[\s\S]*id="setup-check-status"[^>]*>Check Status Again<\/button>/,
    );
    expect(firstRunJourneySource).toContain("api.ValidateProjectTargetV1(root)");
    expect(firstRunJourneySource).toContain("api.InitializeProjectV1(request)");
    expect(firstRunJourneySource).toContain("guideChoice: guide.guideChoice");
    expect(firstRunJourneySource).toContain(
      "guidePreviewToken: guide.guidePreviewToken",
    );
    expect(firstRunSource).toContain("project-guide-partially-applied");
    expect(firstRunSource).toContain(
      'const resumeFields = ["initialization", "goal", "guideChoice"]',
    );
    expect(firstRunSource).toContain('state.checkpoint === "guide-applied"');
    expect(firstRunSource).toContain('event.initialization.outcome === "in-progress"');
    expect(firstPlanSource).toContain("parseCreateFirstPlanResult");
    expect(firstPlanSource).toContain('task.status === "todo" || task.status === "doing"');
    expect(firstPlanSource).toContain("api.CreateFirstPlanV1(generation, title)");
    expect(firstPlanSource).toContain(
      "api.CreateFirstTaskV1(generation, planId, title)",
    );
    expect(firstPlanSource).toContain(
      "api.StartFirstTaskV1(generation, taskId, expectedUpdatedAt)",
    );
    expect(paneSource).toContain("setLayoutLocked(locked: boolean)");
    expect(paneSource).toContain(
      "this.#boardToggle.disabled = this.#layoutLocked || !dockInteractionEligible",
    );
    expect(recentProjectsSource).toMatch(
      /export type RecentProjectAvailability\s*=\s*\| "available"\s*\| "missing"\s*\| "permission-required"\s*\| "changed"/,
    );
    expect(recentProjectsSource).toContain("Recent projects exceeded the 20-entry limit.");
    expect(recentProjectsSource).toContain("Recent projects were not newest first.");
    expect(recentProjectsSource).toContain("bounded registry list");
    // A partial guide apply is never skippable; the status mapping lives in
    // first-run.ts and both the status and reconcile paths go through it.
    expect(firstRunSource).toContain("skipAllowed: false");
    const landingRenderer = readFileSync(resolve(frontendRoot, "src/workspace/landing.ts"), "utf8");
    expect(landingRenderer).toContain('element.dateTime = date.toISOString()');
    expect(landingRenderer).toContain('thumbnail.setAttribute("aria-current", String(project === selected))');
    expect(recentProjectsSource).toContain("export function preselectedRecentProject(");
    expect(styles).toMatch(
      /\.recent-project\[aria-current=(?:"true"|true)\]\{[^}]*border-color:var\(--accent\)/,
    );
    // Highlight is only legal under forced colors, so the pair pins both.
    expect(styles).toMatch(
      /\.recent-project\[aria-current=(?:"true"|true)\]\{[^}]*border-color:highlight/i,
    );
    expect(firstRunJourneySource).toContain(
      "api.GetInitializationStatusV1(operationId)",
    );
    expect(firstRunSource).toContain("parsePendingInitialization");
    expect(firstRunSource).toContain(
      '["project-committed", "guide-applied", "desktop-bound"]',
    );
    expect(firstRunSource).toContain(
      '["recovery", "guide", "guide-stale", "review"]',
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
      /class="terminal-actions"[\s\S]*aria-label="Session controls"[\s\S]*id="terminal-open"[\s\S]*aria-label="Open terminal"[\s\S]*<svg/,
    );
    expect(index).toMatch(
      /id="terminal-help"[\s\S]*aria-label="Open terminal guide"/,
    );
    expect(app).toContain("OpenHelpDestination");
    expect(index).toMatch(
      /id="terminal-close"[\s\S]*class="terminal-action-button terminal-action-stop"[\s\S]*aria-label="Stop terminal session"/,
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
      /id="terminal-diagnostics"[\s\S]*aria-live="polite"[\s\S]*Content-free state only\. Restart creates a fresh session; a stream lost\s+on its own is claimed back for the same session\./,
    );
    // Scratchpad: a toolbar toggle beside diagnostics, and a panel that ships
    // closed so the dock still opens on the terminal itself.
    expect(index).toMatch(
      /id="terminal-scratchpad-toggle"[^>]*class="terminal-action-button"[\s\S]*aria-pressed="false"[\s\S]*aria-controls="terminal-scratchpad"/,
    );
    expect(index.indexOf('id="terminal-scratchpad-toggle"')).toBeLessThan(
      index.indexOf('id="terminal-diagnostics-toggle"'),
    );
    expect(index).toMatch(
      /<aside[^>]*id="terminal-scratchpad"[^>]*class="terminal-scratchpad"[^>]*aria-label="Scratchpad"[^>]*hidden/,
    );
    expect(index).toMatch(
      /id="terminal-scratchpad-splitter"[^>]*role="separator"[^>]*tabindex="0"[\s\S]*aria-valuemin="240"[\s\S]*hidden/,
    );
    expect(index).toMatch(
      /<textarea[^>]*id="terminal-scratchpad-text"[^>]*aria-describedby="terminal-scratchpad-state"[^>]*spellcheck="false"/,
    );
    // The cap is 65 536 UTF-8 bytes, which a UTF-16 maxlength cannot express
    // and would enforce by silently dropping pasted text; the saver counts
    // bytes and says when the note is over.
    expect(index).not.toMatch(/<textarea[^>]*id="terminal-scratchpad-text"[^>]*maxlength=/);
    expect(index).toMatch(
      /<div[^>]*id="terminal-stage"[^>]*class="terminal-stage"[\s\S]*id="terminal-host"[\s\S]*id="terminal-message"[\s\S]*<\/div>/,
    );
    expect(index).toContain('id="terminal-scratchpad-add"');
    expect(index).toContain('id="terminal-scratchpad-snippets"');
    expect(index).toContain("Copy from a pane, or add a selection.");
    // The dock body becomes a row so the panel sits beside the panes.
    expect(styles).toMatch(/\.terminal-body\{[^}]*display:flex/);
    expect(styles).toMatch(/\.terminal-body\{[^}]*flex-direction:row/);
    expect(styles).toMatch(/\.terminal-stage\{[^}]*position:relative/);
    expect(styles).toContain(".terminal-scratchpad{");
    // The splitter's width is also `scratchpadSplitterWidth` in scratchpad.ts,
    // which the dock adds to the gutter it reserves for body-level overlays.
    expect(styles).toMatch(/\.terminal-scratchpad-splitter\{[^}]*width:5px/);
    expect(app).toContain("GetScratchpadV1");
    expect(app).toContain("SetScratchpadV1");
    expect(app).toContain("Selection is larger than 4 KB; not added to the scratchpad.");
    expect(app).toContain("All 50 snippets are pinned; unpin one to add more.");
    expect(app).toContain("Scratchpad changed elsewhere and was reloaded.");
    expect(app).toContain("Scratchpad is unavailable; the copy was not added.");
    expect(app).toContain("ptrack-terminal-scratchpad-open");
    expect(app).toContain("ptrack-terminal-scratchpad-width");
    // The popover carries its own close: it hangs below the dock header, so
    // the toggle that opened it is never underneath it, and a press anywhere
    // else dismisses it.
    expect(index).toMatch(
      /id="terminal-diagnostics-close"[^>]*aria-label="Hide terminal diagnostics"/,
    );
    expect(app).toContain("--terminal-diagnostics-top");
    // Pop out: a real labelled control, absent until a single pane can move.
    expect(index).toMatch(
      /id="terminal-pop-out"[^>]*class="terminal-action-button"[^>]*type="button"[^>]*aria-label="Pop out terminal into its own window"[^>]*title="Pop out terminal"[^>]*hidden/,
    );
    // Terminal window mode: one document, marked before first paint.
    expect(index).toMatch(/dataset\.windowMode\s*=\s*"terminal"/);
    expect(styles).toMatch(
      /html\[data-window-mode=["']?terminal["']?\]\s*#app\s*\{[^}]*display:\s*none/,
    );
    expect(index).toMatch(
      /id="terminal-window"[^>]*class="terminal-window"[^>]*aria-labelledby="terminal-window-heading"[^>]*hidden/,
    );
    expect(index).toMatch(
      /id="terminal-window-status"[^>]*role="status"[^>]*aria-live="polite"/,
    );
    // The way back is the window's own close: no in-page control can destroy
    // a window without a capability this feature deliberately does not take.
    expect(index).toMatch(
      /id="terminal-window-return"[^>]*>\s*Closing this window returns the original tab to p-track\. Tabs opened here close with this window\. Any tab but the last can be closed here\./,
    );
    // The gap notice states the fact in words, is not an alert, and keeps a
    // border under forced colors so it never reads by colour alone.
    expect(index).toMatch(
      /id="terminal-window-gap"[^>]*role="note"[^>]*hidden>\s*<strong>Scrollback gap\.<\/strong>/,
    );
    expect(styles).toMatch(
      /\.terminal-window-gap\s*\{[^}]*border-color:\s*canvastext/i,
    );
    expect(app).toContain("Earlier output was not carried over.");
    expect(app).toContain("This terminal is running in its own window.");
    expect(app).toContain("Reconnecting…");
    // Detached windows carry complete tab shapes and session controls.
    expect(app).toContain("GetTerminalWindowTab");
    expect(app).toContain("SetTerminalWindowTab");
    expect(index).toMatch(
      /id="terminal-window-search"[^>]*class="terminal-window-search"[^>]*role="search"/,
    );
    expect(index).toMatch(
      /id="terminal-window-search-input"[^>]*type="search"/,
    );
    expect(index).toMatch(
      /id="terminal-window-search-results"[^>]*role="status"[^>]*aria-live="polite"/,
    );
    // Splitting and closing panes stay acts of the window that owns the tab.
    expect(styles).toMatch(
      /\.terminal-window-host \.terminal-split-leaf-chrome button:not\(\.terminal-split-leaf-select\)\s*\{\s*display:\s*none/,
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
    expect(app).toContain("linked sessions, processes, and capabilities stay unchanged");
    expect(app).toContain("Finish the current task status change before starting another.");
    expect(app).toContain("Stale task transition response ignored");
    expect(app).toContain("Stale terminal association response ignored");
    expect(app).toContain("Linking changes context only and grants no capabilities.");
    expect(index).not.toMatch(/id="agent-activity"[^>]*aria-live/);
    expect(index).toMatch(
      /id="agent-activity-live"[^>]*role="status"[^>]*aria-live="polite"[^>]*aria-atomic="true"/,
    );
    expect(index).toMatch(/id="agent-activity-heading"[^>]*tabindex="-1"/);
    expect(index).toMatch(/id="agent-handoff-form"[^>]*hidden/);
    expect(index).toMatch(/id="agent-workflow-form"[^>]*hidden/);
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
    // The dock's 84px collapse steps aside while the scratchpad is open, so
    // the panel stays usable with no live terminal session.
    expect(styles).toMatch(
      /data-state=(?:"closed"|closed)\]\[data-layout-interactive=(?:"false"|false)\]:not\(\[data-scratchpad-open=(?:"true"|true)\]\)/,
    );
    expect(styles).toMatch(/data-board-hidden=(?:"true"|true)\] \.terminal-dock\{[^}]*height:100%/);
    expect(styles).toMatch(/data-terminal-hidden=(?:"true"|true)\] \.terminal-dock\{display:none/);
    expect(styles).toMatch(
      /\.board-heading\{[^}]*min-width:0[^}]*flex-wrap:wrap/,
    );
    expect(styles).toMatch(
      /\.title-row h2\{[^}]*(?=[^}]*min-width:0)(?=[^}]*flex:(?:1 1 auto|auto))(?=[^}]*text-overflow:ellipsis)/,
    );
    expect(styles).toMatch(
      /\.board-actions\{[^}]*(?=[^}]*min-width:0)(?=[^}]*flex-wrap:wrap)(?=[^}]*justify-content:flex-end)/,
    );
    expect(styles).toMatch(
      /\.add-form input\{[^}]*(?=[^}]*min-width:120px)(?=[^}]*flex:(?:1 1 170px|170px))/,
    );
    expect(styles).toMatch(
      /@media\s*\((?:max-width:960px|width<=960px)\)\{[^}]*#app[^}]*\}[^}]*\.board-heading[^}]*\}\.plan-context,\.board-actions\{(?=[^}]*width:100%)(?=[^}]*min-width:0)(?=[^}]*max-width:100%)(?=[^}]*flex:0 0 100%)[^}]*\}\.board-actions\{justify-content:flex-start\}\.add-form\{(?=[^}]*min-width:0)(?=[^}]*flex-basis:250px)[^}]*\}/,
    );
    expect(styles).toMatch(
      /\.panel-toggle:focus-visible,[^{]*\.terminal-context-menu button:focus-visible\{[^}]*outline:2px solid var\(--accent\)[^}]*outline-offset:-2px/,
    );
    expect(paneSource).toMatch(
      // Zoom reset lands on the profile's default size (terminalZoomFontSize).
      /this\.#setFontSize\(terminalZoomFontSize\(\s*action,\s*this\.#fontSize,\s*this\.#activeProfileDefaultFontSize\(\),?\s*\)\)/,
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
      /<main[^>]*id="main-content"[^>]*class="canvas-main"[^>]*aria-label="Workspace content"[\s\S]*id="overview-page"[^>]*aria-label="Project overview"/,
    );
    expect(index).toMatch(
      /class="section-label">Rolling project summary<\/p>[\s\S]*id="summary"[^>]*>No rolling summary yet\.<\/p>/,
    );
    expect(index).not.toContain("No rolling handoff yet.");
    expect(styles).toMatch(
      /\.canvas-main\s*\{[^}]*(?=[^}]*min-width:0)(?=[^}]*min-height:0)(?=[^}]*flex:(?:1 1 auto|auto))(?=[^}]*display:flex)(?=[^}]*flex-direction:column)/s,
    );
    // The deprecated Capabilities view is gone; capability brokering is
    // delegated to the companion project pam.
    expect(index).not.toContain('id="capabilities-page"');
    expect(index).not.toContain('id="nav-capabilities"');
    expect(landingRenderer).toContain('empty.setAttribute("role", "listitem")');
    expect(index).toMatch(
      /id="settings-open"[^>]*aria-label="Open Settings"[\s\S]*aria-haspopup="dialog"[\s\S]*aria-controls="settings-modal"/,
    );
    expect(index).toMatch(
      /id="settings-modal"[\s\S]*role="dialog"[\s\S]*aria-modal="true"[\s\S]*aria-labelledby="settings-dialog-heading"[\s\S]*id="settings-dialog-heading"[^>]*>Settings<\/h2>/,
    );
    expect(index).toMatch(
      /id="settings-section-list"[\s\S]*role="tablist"[\s\S]*aria-orientation="vertical"[\s\S]*aria-label="Settings sections"/,
    );
    for (const section of ["startup", "appearance", "terminal", "updates", "data"]) {
      expect(index).toMatch(
        new RegExp(
          `id="settings-tab-${section}"[\\s\\S]*?role="tab"[\\s\\S]*?aria-controls="settings-panel-${section}"`,
        ),
      );
      expect(index).toMatch(
        new RegExp(
          `id="settings-panel-${section}"[\\s\\S]*?role="tabpanel"[\\s\\S]*?aria-labelledby="settings-tab-${section}"[\\s\\S]*?tabindex="0"`,
        ),
      );
    }
    // The roving tabindex is an attribute of the tab itself, so the match must
    // not cross the tag boundary onto a panel or a later tab.
    expect(index).toMatch(/id="settings-tab-startup"[^>]*\stabindex="0"/);
    expect(index).toMatch(/id="settings-tab-appearance"[^>]*\stabindex="-1"/);
    expect(index).toMatch(/id="settings-tab-terminal"[^>]*\stabindex="-1"/);
    // Startup is an opt-in checkbox whose copy states the "still valid" rule.
    expect(index).toMatch(
      /id="settings-startup-restore"[^<>]*type="checkbox"[^<>]*\/>/,
    );
    expect(index).toContain("Reopen the last project when p-track starts");
    expect(index).toContain("The last project reopens only while it is still");
    // Both resets live in Data & Diagnostics, described by the copy that says
    // what they spare, and nowhere near the native menu.
    expect(index).toMatch(
      /id="settings-panel-data"[\s\S]*id="settings-reset-help"[\s\S]*class="settings-reset-actions"[\s\S]*id="settings-reset-window-layout"[^<>]*aria-describedby="settings-reset-help"[^<>]*>Reset Window Layout<\/button>[\s\S]*id="settings-reset-application-state"[^<>]*aria-describedby="settings-reset-help"[^<>]*>Reset Application State<\/button>/,
    );
    // Revoking a grant writes into the open project, so the copy names that
    // instead of promising the project database is untouched.
    expect(index).toContain(
      "Neither reset touches plans, tasks, notes, or Recent projects.",
    );
    expect(styles).toMatch(/\.settings-reset-actions\{[^}]*flex-wrap:wrap/);
    expect(styles).toMatch(
      /\.settings-reset-actions button\{border-color:canvastext\}/i,
    );
    // The save-status live region sits outside the aria-busy wrapper, and
    // shares one footer row with the reset instead of being pinned to the
    // opposite corner from it.
    expect(index).toMatch(
      /<div class="settings-dialog-footer">\s*<p\s*id="settings-save-status"[\s\S]*?role="status"[\s\S]*?aria-live="polite"[\s\S]*?aria-atomic="true"[\s\S]*?<div class="dialog-actions">\s*<button id="settings-reset"/,
    );
    // Source order proves nothing about nesting: `[\s\S]*` runs straight
    // through an unclosed element. Count the div depth instead, so the footer
    // has to start after the one that closes #settings-body.
    const settingsBodyEnd = closingIndex(
      index,
      index.lastIndexOf("<div", index.indexOf('id="settings-body"')),
      "div",
    );
    expect(settingsBodyEnd).toBeGreaterThan(0);
    expect(index.indexOf('class="settings-dialog-footer"')).toBeGreaterThan(
      settingsBodyEnd,
    );
    expect(styles).toMatch(
      /\.settings-dialog-footer\{[^}]*justify-content:space-between/,
    );

    expect(styles).toMatch(
      /\.settings-diagnostic-copy\{[^}]*(?=[^}]*width:26px)(?=[^}]*min-height:26px)(?=[^}]*height:26px)/,
    );
    expect(styles).toMatch(/\.settings-diagnostic-copy svg\{[^}]*stroke:currentColor/);
    expect(styles).toMatch(
      /\.settings-diagnostic-copy:focus-visible[^{]*\{[^}]*outline:2px solid var\(--accent\)/,
    );
    expect(styles).toMatch(
      /\.settings-diagnostic-copy,[^{]*\{border-color:canvastext\}/i,
    );
    expect(index).toMatch(
      /id="settings-terminal-font-size"[\s\S]*min="10"[\s\S]*max="24"[\s\S]*aria-describedby="settings-terminal-font-size-help"/,
    );
    expect(index).toMatch(
      /id="settings-terminal-scrollback"[\s\S]*min="1000"[\s\S]*max="200000"[\s\S]*aria-describedby="settings-terminal-scrollback-help"/,
    );
    expect(index).toContain("Checks are opt-in. Downloads and installations always stay manual.");
    expect(index).toMatch(
      /id="about-identity-heading"[\s\S]*id="about-version"[\s\S]*id="about-build"[\s\S]*id="about-license"[^>]*>Apache-2\.0<\/dd>/,
    );
    expect(index).toMatch(
      /id="about-project"[\s\S]*id="about-license-link"[\s\S]*id="about-help"[\s\S]*id="about-report"/,
    );
    expect(index.match(/id="updates-primary"/g)).toHaveLength(1);
    expect(app).toContain("GetPreferences");
    expect(app).toContain("SetPreferences");
    expect(app).toContain("ResetPreferences");
    expect(app).toContain("GetDiagnosticsReport");
    expect(app).toContain("GetLayoutState");
    expect(app).toContain("SetLayoutState");
    expect(app).toContain("ResetWindowLayout");
    expect(app).toContain("ResetApplicationState");
    // Settings owns the Unicode mode; the dock only follows it.
    expect(paneSource).toContain("setModernUnicode(enabled: boolean): void");
    expect(paneSource).not.toContain("saveUnicodeMode");
    expect(paneSource).not.toContain("writeModernUnicodeSetting");
    expect(paneSource).toContain("readTerminalPreferenceOverrides(localStorage)");
    expect(paneSource).toContain("webglPreferredByPreference(");
    expect(styles).toMatch(
      /:root\[data-density=(?:"compact"|compact)\]\{[^}]*--space-100:\s*6px/,
    );
    expect(styles).toMatch(
      /:root\[data-reduced-motion=(?:"always"|always)\][^{]*\{[^}]*animation-duration:\.01ms!important/,
    );
    expect(styles).toMatch(
      /prefers-reduced-motion:reduce\)\{:root:not\(\[data-reduced-motion=(?:"never"|never)\]\)/,
    );
    expect(styles).toMatch(
      /\.settings-section-tab\[aria-selected=(?:"true"|true)\]\{[^}]*border-color:var\(--control-border\)/,
    );
  });
});

describe("type scale", () => {
  const sheets = ["style.css", "landing.css", "cover-flow.css", "settings-kimi.css"];
  // SVG text sized in viewBox units, not CSS pixels on screen.
  const svgUnits = new Set([".heatmap-label", ".history-marker-label", ".plan-ring-caption"]);
  const labelSelector = /eyebrow|section-label|kicker|kbd|badge|project-code|summary|palette-kind|h4/;

  function fontRules() {
    const rules = [];
    for (const sheet of sheets) {
      const css = readFileSync(resolve(frontendRoot, "src", sheet), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
      for (const match of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
        const size = match[2].match(/font-size:\s*([\d.]+)(rem|px)\s*;/);
        if (!size) continue;
        const px = size[2] === "rem" ? Number(size[1]) * 16 : Number(size[1]);
        rules.push({
          sheet,
          selector: match[1].trim().replace(/\s+/g, " "),
          body: match[2],
          value: `${size[1]}${size[2]}`,
          px,
        });
      }
    }
    return rules;
  }

  it("has no 10px text and keeps 11px for a few uppercase labels only", () => {
    const rules = fontRules();
    expect(rules.filter((rule) => rule.value === "0.625rem" || rule.px === 10)).toEqual([]);
    const small = rules.filter((rule) => rule.px === 11);
    const selectors = small.flatMap((rule) => rule.selector.split(","));
    expect(selectors.length).toBeLessThanOrEqual(12);
    for (const rule of small) {
      const uppercase = /text-transform:\s*uppercase/.test(rule.body) || /eyebrow|section-label/.test(rule.selector);
      expect({ selector: rule.selector, label: uppercase || /kbd/.test(rule.selector) }).toEqual({
        selector: rule.selector,
        label: true,
      });
      expect(rule.selector).toMatch(labelSelector);
    }
  });

  it("keeps body, meta, and description text at 12px or larger", () => {
    const tooSmall = fontRules()
      .filter((rule) => rule.px < 12 && rule.px !== 11 && !svgUnits.has(rule.selector))
      .map((rule) => `${rule.sheet}: ${rule.selector} ${rule.value}`);
    expect(tooSmall).toEqual([]);
    const style = fontRules().filter((rule) => rule.sheet === "style.css");
    for (const selector of [".intelligence-detail", ".intelligence-path", ".card-meta", ".activity-detail"]) {
      const rule = style.find((candidate) => candidate.selector === selector);
      expect(rule?.px, selector).toBeGreaterThanOrEqual(12);
    }
  });
});

// Markup and stylesheet facts from UI review. The behavior each one backs is
// driven through the window in the controller tests: board-interactions,
// workspace-shell, shortcuts, and overview-view.
describe("UI review guards", () => {
  const index = readFileSync(resolve(frontendRoot, "index.html"), "utf8");
  const style = readFileSync(resolve(frontendRoot, "src/style.css"), "utf8");
  const rule = (selector) => {
    const start = style.indexOf(`${selector} {`);
    expect(start, selector).toBeGreaterThanOrEqual(0);
    return style.slice(start, style.indexOf("}", start));
  };

  it("keeps the board visible behind the task drawer", () => {
    const scrim = rule("#task-drawer > .modal-backdrop,\n#issue-drawer > .modal-backdrop");
    expect(scrim).toContain("backdrop-filter: none");
    for (const theme of [":root {", '[data-theme="light"] {']) {
      const block = style.slice(style.indexOf(theme), style.indexOf("\n}\n", style.indexOf(theme)));
      const alpha = Number(block.match(/--drawer-scrim: rgba\([^)]*,\s*([\d.]+)\)/)?.[1]);
      expect(alpha, theme).toBeLessThanOrEqual(0.45);
    }
  });

  it("styles no status control on the card and a full-height one in the drawer", () => {
    expect(style).not.toContain(".card-actions");
    expect(rule(".drawer-actions select")).toMatch(/min-height: (2[89]|3\d)px/);
  });

  it("places the terminal dock beside every page, with Settings as its only Unicode control", () => {
    const pages = index.indexOf('<div class="work-area">');
    expect(index.indexOf('id="overview-page"')).toBeGreaterThan(pages);
    expect(index.indexOf('id="issues-page"')).toBeLessThan(index.indexOf('id="terminal-dock"'));
    expect(index).not.toContain("terminal-unicode-setting");
    expect(index).not.toContain('id="terminal-modern-unicode"');
    expect(index).toMatch(/id="settings-terminal-unicode"/);
    expect(index).toMatch(/id="terminal-start-shell" type="button">Start shell<\/button>/);
    expect(style).not.toContain(".terminal-unicode-setting");
    expect(style).toContain('.terminal-dock[data-state="closed"] .terminal-status {\n  display: none;');
  });

  it("names every icon-only button with an aria-label and a tooltip", () => {
    for (const match of index.matchAll(/<button([\s\S]*?)>([\s\S]*?)<\/button>/g)) {
      const attributes = match[1];
      if (/modal-backdrop|terminal-paste-backdrop|terminal-termination-backdrop|id="(theme-toggle|terminal-window-theme-toggle)"/.test(attributes)) continue;
      const text = match[2].replace(/<svg[\s\S]*?<\/svg>/g, "").replace(/<!--[\s\S]*?-->/g, "").replace(/<[^>]+>/g, "").trim();
      // Short words such as "OK" are visible labels, not icons.
      if (text.length > 2 || /^[A-Za-z]{2}$/.test(text)) continue;
      expect({ button: attributes.trim().slice(0, 60), labelled: /aria-label=/.test(attributes) && /title=/.test(attributes) })
        .toEqual({ button: attributes.trim().slice(0, 60), labelled: true });
    }
  });

  it("starts with the panel toggles hidden and a search field labelled like its placeholder", () => {
    expect(index).toMatch(/id="board-panel-toggle"[\s\S]*?hidden\s*>/);
    expect(index).toMatch(/id="terminal-panel-toggle"[\s\S]*?hidden\s*>/);
    const search = index.match(/<span class="orbit-sr-only">([^<]+)<\/span><input[^>]*id="recent-project-search"[^>]*placeholder="([^"]+)"/);
    expect(search?.[2]).toBe(`${search?.[1]}…`);
    expect(index).not.toContain("summary-guidance");
    expect(index.match(/Refresh summaries/g)).toHaveLength(1);
  });

  it("says Projects, not Welcome, in the markup", () => {
    const visibleText = index.replace(/<!--[\s\S]*?-->/g, "").replace(/<[^>]+>/g, " ");
    expect(visibleText).not.toMatch(/\bWelcome\b/);
  });

  it("gives the version button a 24px hit target", () => {
    const version = rule(".app-version");
    expect(version).toContain("min-width: 24px");
    expect(version).toContain("min-height: 24px");
  });

  it("styles the closeout reminder as a neutral notice stacked with the toast", () => {
    expect(index).toMatch(/id="notice-stack"[\s\S]*?id="toast"/);
    const banner = rule(".plan-closeout-banner");
    expect(banner).toContain("border-left: 3px solid var(--info)");
    expect(banner).not.toContain("blocked");
  });
});
