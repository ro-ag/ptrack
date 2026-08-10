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
    expect(index).toContain('src="/app.js"');
    expect(index).toContain('href="/style.css"');
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
    expect(styles).toContain(".terminal-tab-indicator");
    expect(styles).toMatch(/\[data-indicator=(?:"failed"|failed)\]/);
    expect(styles).toMatch(/\[data-unread=(?:"true"|true)\]/);
    expect(styles).toContain(".terminal-split-node");
    expect(styles).toContain("touch-action:none");
    expect(styles).toMatch(
      /data-state=(?:"closed"|closed)\]\[data-layout-interactive=(?:"false"|false)\]/,
    );
  });
});
