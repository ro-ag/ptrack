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

export interface WebglPolicyRecoveryState {
  attempts: number;
  timerPending: boolean;
  paused: boolean;
}

export type WebglRecoveryPolicyAction = "attach" | "schedule" | "wait" | "none";

export function webglRecoveryAfterSuppression(
  state: WebglPolicyRecoveryState,
  applicationOverlayOpen: boolean,
): Pick<WebglPolicyRecoveryState, "attempts" | "paused"> {
  if (!applicationOverlayOpen) return { attempts: 0, paused: false };
  return {
    attempts: state.attempts,
    paused: state.paused || state.timerPending,
  };
}

export function webglRecoveryPolicyAction(
  state: WebglPolicyRecoveryState,
): WebglRecoveryPolicyAction {
  if (state.timerPending) return "wait";
  if (state.attempts < 0 || state.attempts >= maximumWebglRecoveryAttempts) {
    return "none";
  }
  if (state.paused || state.attempts > 0) return "schedule";
  return "attach";
}

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
