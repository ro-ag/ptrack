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
    expect(index).toContain('src="/app.js"');
    expect(index).toContain('href="/style.css"');
  });
});
