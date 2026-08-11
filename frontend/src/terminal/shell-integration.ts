export type ShellIntegrationQuality = "none" | "basic" | "rich";
export type ShellPhase = "unknown" | "prompt" | "editing" | "executing" | "completed";

export interface ShellIntegrationDescriptor {
  quality?: ShellIntegrationQuality;
  nonce?: string;
}

export type ShellSignal =
  | { kind: "prompt-start"; authenticated: boolean }
  | { kind: "prompt-end"; authenticated: boolean }
  | { kind: "command-start"; authenticated: boolean }
  | { kind: "command-finish"; authenticated: boolean; exitCode?: number }
  | { kind: "cwd"; authenticated: boolean; cwd: string };

export interface ShellState {
  quality: ShellIntegrationQuality;
  phase: ShellPhase;
  startedAt: number | null;
  lastExitCode: number | null;
  lastDurationMs: number | null;
  sequence: number;
}

export interface ShellCWDValidationDecision {
  request: number;
  validate: boolean;
}

export const initialShellState: ShellState = {
  quality: "none",
  phase: "unknown",
  startedAt: null,
  lastExitCode: null,
  lastDurationMs: null,
  sequence: 0,
};

const maximumOSCPayload = 4096;
const maximumCWDLength = 4096;
const signedDecimal = /^-?(?:0|[1-9][0-9]*)$/;
const windowsAbsolutePath = /^[A-Za-z]:[\\/]/;

function nonceMatches(received: string, expected: string): boolean {
  if (received.length !== expected.length || expected.length === 0) return false;
  let difference = 0;
  for (let index = 0; index < received.length; index += 1) {
    difference |= received.charCodeAt(index) ^ expected.charCodeAt(index);
  }
  return difference === 0;
}

function parseExitCode(value: string | undefined): number | undefined | null {
  if (value === undefined || value === "") return undefined;
  if (!signedDecimal.test(value)) return null;
  const exitCode = Number(value);
  if (!Number.isSafeInteger(exitCode) || exitCode < -2_147_483_648 || exitCode > 2_147_483_647) {
    return null;
  }
  return exitCode;
}

function decodedFileURI(value: string): string | null {
  if (!value.startsWith("file://") || value.includes("?") || value.includes("#")) return null;
  const remainder = value.slice("file://".length);
  let encodedPath = remainder;
  if (!remainder.startsWith("/")) {
    const slash = remainder.indexOf("/");
    if (slash < 0) return null;
    const host = remainder.slice(0, slash).toLowerCase();
    if (host !== "localhost") return null;
    encodedPath = remainder.slice(slash);
  }
  try {
    let path = decodeURIComponent(encodedPath);
    if (/^\/[A-Za-z]:[\\/]/.test(path)) path = path.slice(1);
    return path;
  } catch {
    return null;
  }
}

export function safeShellCWD(value: string): string | null {
  if (value.length === 0 || value.length > maximumCWDLength || /[\x00-\x1f\x7f]/.test(value)) {
    return null;
  }
  const path = value.startsWith("file://") ? decodedFileURI(value) : value;
  if (!path || path.length > maximumCWDLength || /[\x00-\x1f\x7f]/.test(path)) return null;
  if (!path.startsWith("/") && !windowsAbsolutePath.test(path) && !path.startsWith("\\\\")) {
    return null;
  }
  return path;
}

function standardSignal(parts: string[]): ShellSignal | null {
  const [opcode, value, ...extra] = parts;
  if (extra.length !== 0) return null;
  switch (opcode) {
  case "A":
    return value === undefined ? { kind: "prompt-start", authenticated: false } : null;
  case "B":
    return value === undefined ? { kind: "prompt-end", authenticated: false } : null;
  case "C":
    return value === undefined ? { kind: "command-start", authenticated: false } : null;
  case "D": {
    const exitCode = parseExitCode(value);
    return exitCode === null
      ? null
      : { kind: "command-finish", authenticated: false, ...(exitCode === undefined ? {} : { exitCode }) };
  }
  default:
    return null;
  }
}

function signedSignal(parts: string[], expectedNonce: string): ShellSignal | null {
  const opcode = parts[0];
  if (opcode === "P") {
    if (parts.length !== 3 || !parts[1].startsWith("Cwd=") ||
      !nonceMatches(parts[2], expectedNonce)) return null;
    const cwd = safeShellCWD(parts[1].slice("Cwd=".length));
    return cwd ? { kind: "cwd", authenticated: true, cwd } : null;
  }
  if (opcode === "D") {
    if (parts.length !== 3 || !nonceMatches(parts[2], expectedNonce)) return null;
    const exitCode = parseExitCode(parts[1]);
    return exitCode === null
      ? null
      : { kind: "command-finish", authenticated: true, ...(exitCode === undefined ? {} : { exitCode }) };
  }
  if ((opcode === "A" || opcode === "B" || opcode === "C") && parts.length === 2 &&
    nonceMatches(parts[1], expectedNonce)) {
    return {
      kind: opcode === "A" ? "prompt-start" : opcode === "B" ? "prompt-end" : "command-start",
      authenticated: true,
    };
  }
  return null;
}

export function parseShellOSC(
  identifier: 7 | 133 | 633,
  payload: string,
  expectedNonce = "",
): ShellSignal | null {
  if (payload.length === 0 || payload.length > maximumOSCPayload || /[\x00-\x1f\x7f]/.test(payload)) {
    return null;
  }
  if (identifier === 7) {
    const cwd = safeShellCWD(payload);
    return cwd ? { kind: "cwd", authenticated: false, cwd } : null;
  }
  const parts = payload.split(";");
  if (identifier === 633 && expectedNonce !== "") {
    return signedSignal(parts, expectedNonce);
  }
  if (identifier === 133 && expectedNonce !== "") return null;
  // Standard 133/633 markers remain advisory. Command-line-bearing E markers
  // and unknown properties are intentionally ignored without retaining data.
  return standardSignal(parts);
}

export function applyShellSignal(
  state: ShellState,
  signal: ShellSignal,
  now: number,
): ShellState {
  const quality: ShellIntegrationQuality = signal.authenticated ? "rich" :
    state.quality === "rich" ? "rich" : "basic";
  if (signal.kind === "cwd") return { ...state, quality };
  const sequence = state.sequence + 1;
  switch (signal.kind) {
  case "prompt-start":
    return { ...state, quality, phase: "prompt", startedAt: null, sequence };
  case "prompt-end":
    return state.phase === "prompt"
      ? { ...state, quality, phase: "editing", sequence }
      : { ...state, quality, phase: "unknown", startedAt: null, sequence };
  case "command-start":
    return state.phase === "editing"
      ? { ...state, quality, phase: "executing", startedAt: now, sequence }
      : { ...state, quality, phase: "unknown", startedAt: null, sequence };
  case "command-finish":
    if (state.phase !== "executing" || state.startedAt === null) {
      return { ...state, quality, phase: "unknown", startedAt: null, sequence };
    }
    return {
      ...state,
      quality,
      phase: "completed",
      startedAt: null,
      lastExitCode: signal.exitCode ?? null,
      lastDurationMs: Math.max(0, now - state.startedAt),
      sequence,
    };
  }
}

export function shellStatusLabel(state: ShellState): string | null {
  switch (state.phase) {
  case "prompt":
  case "editing":
    return state.lastExitCode === null ? "Prompt" : `Prompt · last ${state.lastExitCode}`;
  case "executing":
    return "Command running";
  case "completed":
    return state.lastExitCode === null ? "Command finished" :
      `Command finished · ${state.lastExitCode}`;
  default:
    return null;
  }
}

export function nextShellCWDValidation(
  currentRequest: number,
  lastValidatedCWD: string,
  candidate: string,
): ShellCWDValidationDecision {
  return {
    request: currentRequest + 1,
    validate: candidate !== lastValidatedCWD,
  };
}
