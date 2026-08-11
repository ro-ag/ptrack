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
    expect(index).toContain('src="/app.js"');
    expect(index).toContain('href="/style.css"');
    expect(index).toMatch(/id="app-version"[^>]*>dev<\/p>/);
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
    expect(paneSource).toMatch(
      /action === "zoom-reset"\) \{\s*this\.#setFontSize\(this\.#activeProfileDefaultFontSize\(\)\)/,
    );
  });
});
