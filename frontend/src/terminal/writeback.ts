import type { ActiveTerminalAssociation } from "./association-editor";

export const terminalWritebackMaximumBytes = 8 * 1024;
export const terminalWritebackMaximumCharacters = 4_000;
export const terminalWritebackMaximumLines = 128;

export type TerminalWritebackKind = "summary" | "decision" | "blocker" | "handoff";

export interface TerminalWritebackPreview {
  generation: number;
  sessionId: string;
  revision: number;
  kind: TerminalWritebackKind;
  content: string;
  contentBytes: number;
  associationTarget: string;
  destination: string;
  replacesSummary: boolean;
}

export interface TerminalWritebackResult {
  generation: number;
  sessionId: string;
  revision: number;
  requestId: string;
  kind: TerminalWritebackKind;
  destination: string;
  noteId?: number;
  replayed: boolean;
}

export interface TerminalWritebackContentPolicy {
  valid: boolean;
  normalized: string;
  bytes: number;
  message: string;
}

// This client policy is UX only. The backend repeats every bound and performs
// credential screening before it opens a write transaction.
export function terminalWritebackContentPolicy(
  value: string,
): TerminalWritebackContentPolicy {
  const normalized = value.replace(/\r\n?/g, "\n").trim();
  const bytes = new TextEncoder().encode(normalized).byteLength;
  const characters = Array.from(normalized).length;
  const lines = normalized === "" ? 0 : normalized.split("\n").length;
  if (normalized === "") {
    return { valid: false, normalized, bytes, message: "Enter memory to preview." };
  }
  if (bytes > terminalWritebackMaximumBytes ||
    characters > terminalWritebackMaximumCharacters ||
    lines > terminalWritebackMaximumLines) {
    return {
      valid: false,
      normalized,
      bytes,
      message: "Memory exceeds the hard write-back limit.",
    };
  }
  return { valid: true, normalized, bytes, message: `${bytes} / ${terminalWritebackMaximumBytes} bytes` };
}

export function terminalWritebackStateMatches(
  expected: ActiveTerminalAssociation,
  current: ActiveTerminalAssociation | null,
): boolean {
  return current !== null &&
    current.generation === expected.generation &&
    current.tabId === expected.tabId &&
    current.paneId === expected.paneId &&
    current.sessionId === expected.sessionId &&
    current.revision === expected.revision &&
    current.pointer?.version === expected.pointer?.version &&
    current.pointer?.planId === expected.pointer?.planId &&
    current.pointer?.taskId === expected.pointer?.taskId;
}

// Re-previewing unchanged content after an ambiguous transport error must keep
// the original idempotency key. Form changes explicitly clear the existing ID.
export function stableTerminalWritebackRequestID(
  existing: string | null,
  create: () => string,
): string {
  return existing ?? create();
}
