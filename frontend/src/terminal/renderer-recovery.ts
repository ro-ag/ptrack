export const maximumWebglRecoveryAttempts = 3;

export interface WebglRecoveryState {
  disposed: boolean;
  attached: boolean;
  timerPending: boolean;
  attempts: number;
  accepted: boolean;
  preferred: boolean;
  terminalHidden: boolean;
  documentHidden: boolean;
}

export type WebglAttachSource = "policy" | "retry";

export function webglAttachAllowed(
  state: WebglRecoveryState,
  source: WebglAttachSource,
): boolean {
  if (
    state.disposed ||
    state.attached ||
    state.timerPending ||
    state.attempts < 0 ||
    state.attempts > maximumWebglRecoveryAttempts ||
    !state.accepted ||
    !state.preferred ||
    state.terminalHidden ||
    state.documentHidden
  ) return false;
  return source === "retry"
    ? state.attempts > 0 && state.attempts <= maximumWebglRecoveryAttempts
    : state.attempts === 0;
}

export function webglRecoveryDelay(state: WebglRecoveryState): number | null {
  if (
    state.disposed ||
    state.attached ||
    state.timerPending ||
    state.attempts < 0 ||
    state.attempts >= maximumWebglRecoveryAttempts ||
    !state.accepted ||
    !state.preferred ||
    state.terminalHidden ||
    state.documentHidden
  ) return null;
  return 250 * 2 ** state.attempts;
}
