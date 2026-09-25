import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// Parse source tokens so palette edits retain WCAG AA contrast.
const css = readFileSync(resolve(import.meta.dirname, "style.css"), "utf8");

function tokenBlock(selector) {
  const start = css.indexOf(`${selector} {`);
  if (start < 0) throw new Error(`missing token block ${selector}`);
  const end = css.indexOf("\n}\n", start);
  const tokens = {};
  for (const match of css.slice(start, end).matchAll(/--([\w-]+):\s*([^;]+);/g)) {
    tokens[match[1]] = match[2].trim();
  }
  return tokens;
}

const dark = tokenBlock(":root");
const light = { ...dark, ...tokenBlock('[data-theme="light"]') };

function resolveToken(theme, name) {
  let value = theme[name];
  for (let guard = 0; value && value.startsWith("var(") && guard < 8; guard += 1) {
    value = theme[value.slice(6, -1)];
  }
  if (!value) throw new Error(`unresolved token --${name}`);
  return value;
}

// Every opaque colour stop in a token (a plain hex or each hex in a gradient).
function stops(theme, name) {
  const found = resolveToken(theme, name).match(/#[0-9a-f]{6}\b/gi);
  if (!found) throw new Error(`--${name} has no opaque colour`);
  return found;
}

function luminance(hex) {
  const channels = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
  const [r, g, b] = channels.map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrastRatio(foreground, background) {
  const [high, low] = [luminance(foreground), luminance(background)].sort((a, b) => b - a);
  return (high + 0.05) / (low + 0.05);
}

function worstRatio(theme, foreground, background) {
  let worst = Infinity;
  for (const fg of stops(theme, foreground)) {
    for (const bg of stops(theme, background)) worst = Math.min(worst, contrastRatio(fg, bg));
  }
  return worst;
}

const textTokens = ["text", "text-soft", "muted", "faint", "accent-text", "placeholder"];
const surfaces = ["surface", "surface-raised", "surface-soft", "canvas-bg", "dialog-bg"];

describe("design token contrast (WCAG AA 4.5:1)", () => {
  it("computes the WCAG reference ratios", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 5);
    expect(contrastRatio("#777777", "#ffffff")).toBeCloseTo(4.48, 2);
  });

  for (const [themeName, theme] of [
    ["dark", dark],
    ["light", light],
  ]) {
    for (const token of textTokens) {
      for (const surface of surfaces) {
        it(`${themeName}: --${token} on --${surface}`, () => {
          expect(worstRatio(theme, token, surface)).toBeGreaterThanOrEqual(4.5);
        });
      }
    }

    it(`${themeName}: button text on every stop of the accent fill`, () => {
      expect(worstRatio(theme, "ink", "button-bg")).toBeGreaterThanOrEqual(4.5);
      expect(worstRatio(theme, "ink", "button-bg-hover")).toBeGreaterThanOrEqual(4.5);
    });
  }

  it("light: --faint stays on white and on --surface-soft", () => {
    expect(contrastRatio(stops(light, "faint")[0], "#ffffff")).toBeGreaterThanOrEqual(4.5);
    expect(worstRatio(light, "faint", "surface-soft")).toBeGreaterThanOrEqual(4.5);
  });

  it("light: accent used as text stays readable on white", () => {
    expect(contrastRatio(stops(light, "accent-text")[0], "#ffffff")).toBeGreaterThanOrEqual(4.5);
  });
});
