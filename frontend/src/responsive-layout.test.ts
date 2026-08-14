import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const styles = readFileSync(resolve(import.meta.dirname, "style.css"), "utf8");

function relativeLuminance(hex: string): number {
  const channels = hex.match(/[0-9a-f]{2}/gi)?.map((part) =>
    Number.parseInt(part, 16) / 255
  ) ?? [];
  const linear = channels.map((channel) =>
    channel <= 0.04045
      ? channel / 12.92
      : ((channel + 0.055) / 1.055) ** 2.4
  );
  return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
}

function contrastRatio(foreground: string, background: string): number {
  const first = relativeLuminance(foreground);
  const second = relativeLuminance(background);
  return (Math.max(first, second) + 0.05) /
    (Math.min(first, second) + 0.05);
}

describe("responsive desktop layout contracts", () => {
  it("paints the cold-start state card without compositor entry effects", () => {
    const stateCardRule = styles.match(/\.state-card\s*\{([^}]*)\}/);

    expect(stateCardRule?.[1]).not.toMatch(/\b(?:animation|transform|opacity)\s*:/);
  });

  it("allows long plan titles to shrink through every header flex layer", () => {
    expect(styles).toMatch(
      /\.board-heading\s*\{[^}]*min-width:\s*0;[^}]*flex-wrap:\s*wrap;/,
    );
    expect(styles).toMatch(
      /\.plan-context\s*\{[^}]*min-width:\s*0;[^}]*flex:\s*1 1 260px;/,
    );
    expect(styles).toMatch(
      /\.title-row h2\s*\{[^}]*min-width:\s*0;[^}]*flex:\s*1 1 auto;[^}]*text-overflow:\s*ellipsis;/,
    );
  });

  it("wraps board actions and bounds the add-task input at compact widths", () => {
    expect(styles).toMatch(
      /\.board-actions\s*\{[^}]*min-width:\s*0;[^}]*flex-wrap:\s*wrap;[^}]*justify-content:\s*flex-end;/,
    );
    expect(styles).toMatch(
      /\.add-form\s*\{[^}]*min-width:\s*min\(250px, 100%\);[^}]*flex:\s*1 1 300px;/,
    );
    expect(styles).toMatch(
      /\.add-form input\s*\{[^}]*min-width:\s*120px;[^}]*flex:\s*1 1 170px;/,
    );
    expect(styles).toMatch(
      /@media \(max-width:\s*960px\)[\s\S]*?\.plan-context,\s*\.board-actions\s*\{[^}]*width:\s*100%;[^}]*min-width:\s*0;[^}]*max-width:\s*100%;[^}]*flex:\s*0 0 100%;[^}]*\}[\s\S]*?\.board-actions\s*\{[^}]*justify-content:\s*flex-start;[^}]*\}[\s\S]*?\.add-form\s*\{[^}]*min-width:\s*0;[^}]*flex-basis:\s*250px;/,
    );
  });

  it("keeps a visible inward focus ring on clipped compact controls", () => {
    const focusRule = styles.match(
      /\.panel-toggle:focus-visible,[\s\S]*?\.terminal-context-menu button:focus-visible\s*\{([^}]*)\}/,
    );

    expect(focusRule?.[0]).toContain(".column-fold:focus-visible");
    expect(focusRule?.[0]).toContain("#palette-input:focus-visible");
    expect(focusRule?.[0]).toContain(".terminal-split-separator:focus-visible");
    expect(focusRule?.[0]).toContain(".terminal-split-leaf-chrome button:focus-visible");
    expect(focusRule?.[1]).toMatch(/outline:\s*2px solid var\(--accent\);/);
    expect(focusRule?.[1]).toMatch(/outline-offset:\s*-2px;/);
  });

  it("retains the overview and settings single-column compact layout", () => {
    expect(styles).toMatch(
      /@media \(max-width:\s*960px\)[\s\S]*?\.overview-grid\s*\{[^}]*grid-template-columns:\s*minmax\(0, 1fr\);[^}]*\}[\s\S]*?\.settings-grid\s*\{[^}]*grid-template-columns:\s*minmax\(0, 1fr\);/,
    );
  });

  it("keeps the automatic-update checkbox from consuming the dialog row", () => {
    expect(styles).toMatch(
      /\.updates-automatic-option input\s*\{[^}]*flex:\s*0 0 auto;[^}]*width:\s*16px;[^}]*min-height:\s*16px;[^}]*height:\s*16px;[^}]*padding:\s*0;/,
    );
  });

  it("wraps first-run actions and keeps long reviewed paths readable", () => {
    expect(styles).toMatch(
      /\.welcome-actions,\s*\.setup-actions\s*\{[^}]*display:\s*flex;[^}]*flex-wrap:\s*wrap;/,
    );
    expect(styles).toMatch(
      /\.setup-actions > button\s*\{[^}]*min-width:\s*0;[^}]*max-width:\s*100%;[^}]*white-space:\s*normal;/,
    );
    expect(styles).toMatch(
      /\.setup-target,\s*\.setup-review-goal,\s*\.setup-change-summary > :last-child\s*\{[^}]*overflow-wrap:\s*anywhere;/,
    );
    expect(styles).toMatch(
      /\.setup-guide-diff\s*\{[^}]*max-width:\s*100%;[^}]*max-height:\s*280px;[^}]*overflow:\s*auto;/,
    );
    expect(styles).toMatch(
      /@media \(max-width:\s*960px\)[\s\S]*?\.setup-guide-file-header\s*\{[^}]*flex-direction:\s*column;[^}]*\}[\s\S]*?\.setup-guide-diff\s*\{[^}]*max-height:\s*220px;/,
    );
  });

  it("retains reduced-motion and high-contrast guide contracts", () => {
    expect(styles).toMatch(
      /@media \(prefers-reduced-motion:\s*reduce\)[\s\S]*?animation-duration:\s*0\.01ms\s*!important;/,
    );
    expect(styles).toMatch(
      /@media \(forced-colors:\s*active\)[\s\S]*?\.setup-guide-file,[\s\S]*?\.setup-guide-diff,[\s\S]*?\.recent-project\s*\{[^}]*border-color:\s*CanvasText;/,
    );
    expect(styles).toMatch(
      /@media \(forced-colors:\s*active\)[\s\S]*?button:focus-visible,[\s\S]*?\[tabindex\]:focus-visible\s*\{[^}]*outline:\s*2px solid Highlight;[^}]*box-shadow:\s*none;/,
    );
    expect(styles).toMatch(
      /@media \(forced-colors:\s*active\)[\s\S]*?button:disabled\s*\{[^}]*color:\s*GrayText;[^}]*opacity:\s*1;/,
    );
  });

  it("keeps small recovery text and form boundaries above contrast thresholds", () => {
    expect(styles).toContain("--control-border: #687386");
    expect(styles).toContain("--control-border: #7a8494");
    expect(contrastRatio("#687386", "#10151e")).toBeGreaterThanOrEqual(3);
    expect(contrastRatio("#7a8494", "#ffffff")).toBeGreaterThanOrEqual(3);
    expect(contrastRatio("#8a93a8", "#10151e")).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio("#617089", "#ffffff")).toBeGreaterThanOrEqual(4.5);
    expect(styles).toMatch(
      /input,\s*textarea\s*\{[^}]*border:\s*1px solid var\(--control-border\);/,
    );
    expect(styles).toMatch(
      /\.recent-project-path\s*\{[^}]*color:\s*var\(--muted\);/,
    );
    expect(styles).toMatch(/\.dialog-help\s*\{[^}]*color:\s*var\(--muted\);/);
  });

  it("keeps post-project onboarding controls bounded in the single state card", () => {
    expect(styles).toMatch(
      /\.post-project-onboarding\s*\{[^}]*min-width:\s*0;[^}]*display:\s*grid;/,
    );
    expect(styles).toMatch(
      /\.post-project-onboarding \.setup-form input\[type="text"\]\s*\{[^}]*width:\s*100%;[^}]*min-width:\s*0;/,
    );
    expect(styles).toMatch(
      /\.onboarding-start-option\s*\{[^}]*min-width:\s*0;[^}]*display:\s*flex;/,
    );
  });

  it("stacks recent-project recovery actions and preserves forced-color borders", () => {
    expect(styles).toMatch(
      /\.recent-project-actions\s*\{[^}]*min-width:\s*0;[^}]*display:\s*flex;[^}]*flex-wrap:\s*wrap;/,
    );
    expect(styles).toMatch(
      /@media \(max-width:\s*600px\)[\s\S]*?\.recent-project\s*\{[^}]*grid-template-columns:\s*minmax\(0, 1fr\);[^}]*align-items:\s*start;/,
    );
    expect(styles).toMatch(
      /@media \(max-width:\s*600px\)[\s\S]*?\.recent-project-name,\s*\.recent-project-path\s*\{[^}]*overflow-wrap:\s*anywhere;[^}]*white-space:\s*normal;/,
    );
    expect(styles).toMatch(
      /@media \(forced-colors:\s*active\)[\s\S]*?\.recent-project\s*\{[^}]*border-color:\s*CanvasText;/,
    );
  });

  it("reflows the setup card and confirmations at 320px and 400% zoom widths", () => {
    expect(styles).not.toMatch(
      /@media \(max-width:\s*600px\)[\s\S]*?#sidebar-toggle\s*\{[^}]*display:\s*none;/,
    );
    expect(styles).toMatch(
      /@media \(max-width:\s*600px\)[\s\S]*?#app,\s*#app\[data-sidebar-hidden="true"\]\s*\{[^}]*grid-template-columns:\s*minmax\(0, 1fr\);[^}]*grid-template-rows:\s*auto minmax\(0, 1fr\);[^}]*overflow:\s*visible;/,
    );
    expect(styles).toMatch(
      /@media \(max-width:\s*600px\)[\s\S]*?\.sidebar\s*\{[^}]*max-height:\s*min\(320px, 70dvh\);[^}]*overflow:\s*auto;[^}]*\}[\s\S]*?\.sidebar-resize\s*\{[^}]*display:\s*none;/,
    );
    expect(styles).toMatch(
      /@media \(max-width:\s*600px\)[\s\S]*?\.canvas\s*\{[^}]*grid-column:\s*1;[^}]*width:\s*calc\(100% - var\(--space-100\)\);[^}]*height:\s*100dvh;[^}]*margin:\s*var\(--space-050\);/,
    );
    expect(styles).toMatch(
      /@media \(max-width:\s*600px\)[\s\S]*?\.workspace-state-screen\s*\{[^}]*place-items:\s*start center;[^}]*padding:\s*var\(--space-200\);/,
    );
    expect(styles).toMatch(
      /\.modal\s*\{[^}]*overflow:\s*auto;[^}]*padding:\s*var\(--space-250\);/,
    );
    expect(styles).toMatch(
      /\.dialog\s*\{[^}]*max-height:\s*calc\(100dvh - 40px\);[^}]*overflow:\s*auto;/,
    );
    expect(styles).toMatch(
      /@media \(max-width:\s*600px\)[\s\S]*?\.modal\s*\{[^}]*place-items:\s*start center;[^}]*padding:\s*var\(--space-100\);[^}]*\}[\s\S]*?\.dialog-actions\s*\{[^}]*flex-wrap:\s*wrap;/,
    );
  });
});
