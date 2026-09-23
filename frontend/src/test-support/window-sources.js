// The window's own source modules — everything under src/ the page ships,
// minus tests and test support — for the few facts only source can show:
// what the code must never call or write, whichever module it lives in.
import { readdirSync, readFileSync } from "node:fs";
import { relative, resolve } from "node:path";

const srcRoot = resolve(import.meta.dirname, "..");

function walk(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) return entry.name === "test-support" ? [] : walk(path);
    if (!/\.(js|ts)$/.test(entry.name) || /\.test\.(js|ts)$/.test(entry.name)) return [];
    if (entry.name.endsWith(".d.ts")) return [];
    return [path];
  });
}

/** Source text of each window module, keyed by its path under src/. */
export const windowModules = new Map(
  walk(srcRoot)
    .sort()
    .map((path) => [relative(srcRoot, path), readFileSync(path, "utf8")]),
);

/** Every window module's source, joined. */
export const windowSource = [...windowModules.values()].join("\n");

/** One module's source; throws for a module that does not exist. */
export function moduleSource(path) {
  const source = windowModules.get(path);
  if (source === undefined) throw new Error(`no window module ${path}`);
  return source;
}
