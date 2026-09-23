import { describe, expect, it } from "vitest";

import { looksLikeSecret } from "./secrets";

describe("looksLikeSecret", () => {
  it.each([
    ["a PEM private key", "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA"],
    ["an RSA private key", "-----BEGIN RSA PRIVATE KEY-----"],
    ["an AWS access key id", "export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE"],
    [
      "an AWS secret access key",
      "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    ],
    ["a GitHub token", "ghp_0123456789abcdefghijklmnopqrstuvwxyz"],
    ["a fine-grained GitHub token", "github_pat_11ABCDEFG0123456789_abcdefghijklmnop"],
    ["a GitLab token", "glpat-xxxxxxxxxxxxxxxxxxxx"],
    ["a Slack bot token", "xoxb-123456789012-abcdefghijkl"],
    ["an API key", "sk-proj-abcdefghijklmnopqrstuvwxyz0123"],
    ["a Stripe live key", "sk_live_abcdefghijklmnop1234"],
    ["a Google API key", "AIzaSyA-abcdefghijklmnopqrstuvwxyz01234"],
    [
      "a JWT",
      "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
    ],
    ["a bearer header", "Authorization: Bearer abcdefghijklmnop"],
    ["a password assignment", "DB_PASSWORD=hunter2hunter2"],
    ["a quoted api key", '{"api_key": "abcd1234efgh"}'],
    ["a token in yaml", "github_token: abcdef123456"],
  ])("refuses %s", (_label, text) => {
    expect(looksLikeSecret(text)).toBe(true);
  });

  it.each([
    ["a command", "cargo test -p ptrack-terminal"],
    ["a path", "/Users/me/dev/ptrack/src/token.rs"],
    ["prose about passwords", "reset the password from the settings page"],
    ["a git sha", "ddf178a release: v0.40.1"],
    ["an empty assignment", "TOKEN="],
  ])("keeps %s", (_label, text) => {
    expect(looksLikeSecret(text)).toBe(false);
  });
});
