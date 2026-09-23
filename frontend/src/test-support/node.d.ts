// The Node APIs the TypeScript tests use to read checked-in sources. The page
// never runs on Node, so its full typings stay out of the window's build.
declare module "node:fs" {
  export function readFileSync(path: string, encoding: "utf8"): string;
}

declare module "node:path" {
  export function resolve(...segments: string[]): string;
}

interface ImportMeta {
  /** The directory of the current module (Node 20.11+, Vitest). */
  readonly dirname: string;
}
