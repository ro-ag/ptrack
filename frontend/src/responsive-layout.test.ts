import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const styles = readFileSync(resolve(import.meta.dirname, "style.css"), "utf8");

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
});
