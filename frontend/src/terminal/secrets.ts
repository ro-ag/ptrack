/**
 * Screen terminal copies before saving them to the project scratchpad.
 * Favor false positives over persisting credentials in the project database.
 */
const credentialPatterns: readonly RegExp[] = [
  // PEM private keys.
  /-----BEGIN [A-Z0-9 ]*PRIVATE KEY( BLOCK)?-----/,
  // AWS access keys.
  /\b(?:AKIA|ASIA|AGPA|AIDA|AROA|ANPA|ANVA|AIPA)[0-9A-Z]{16}\b/,
  /aws.{0,20}secret.{0,20}["']?\s*[:=]\s*["']?[A-Za-z0-9/+=]{40}\b/i,
  // GitHub tokens.
  /\bgh[pousr]_[A-Za-z0-9]{36,}\b/,
  /\bgithub_pat_[A-Za-z0-9_]{22,}/,
  // GitLab personal access tokens.
  /\bglpat-[A-Za-z0-9_-]{20,}/,
  // Slack tokens.
  /\bxox[abeprs]-[A-Za-z0-9-]{10,}/,
  // Provider and Stripe live keys.
  /\bsk-[A-Za-z0-9_-]{20,}/,
  /\b[rs]k_live_[A-Za-z0-9]{16,}/,
  // Google API keys.
  /\bAIza[0-9A-Za-z_-]{35}\b/,
  // JSON Web Tokens.
  /\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}/,
  // HTTP credentials.
  /\bauthorization\s*:\s*(?:bearer|basic|token)\s+\S{8,}/i,
  // A credential assigned by name: `password=…`, `"api_key": "…"`, `TOKEN: …`.
  /[A-Za-z0-9_.-]*(?:password|passwd|secret|token|api[_-]?key)[A-Za-z0-9_.-]*["']?\s*[:=]\s*["']?[^\s"',;]{4,}/i,
];

/** Whether captured text looks like it holds a credential. */
export function looksLikeSecret(text: string): boolean {
  return credentialPatterns.some((pattern) => pattern.test(text));
}

/** Said in place of the capture when a copy was not saved. */
export const secretCaptureNotice = "Not saved: looks like a secret";
