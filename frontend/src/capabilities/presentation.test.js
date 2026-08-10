import { describe, expect, it } from "vitest";

import {
  canEnableCapability,
  canStartCapabilitySave,
  capabilityResponseIsCurrent,
  capabilityRiskGrants,
  capabilityStateLabel,
  diagnosticLabel,
  gitCapabilityNeedsSSH,
  splitCapabilityList,
} from "./presentation";

describe("capability settings presentation", () => {
  it("normalizes comma and newline lists", () => {
    expect(splitCapabilityList(" GET, POST\n/api ")).toEqual(["GET", "POST", "/api"]);
  });

  it("surfaces high-risk grants separately", () => {
    expect(capabilityRiskGrants({
      kind: "ssh",
      ssh: { allow_upload: true, allow_interactive_shell: true, remote_commands: ["uptime"] },
    })).toEqual(["remote commands", "uploads", "interactive shell"]);
    expect(capabilityRiskGrants({ kind: "http", http: { methods: ["GET", "DELETE"] } })).toEqual(["DELETE"]);
  });

  it("requires the exact displayed digest before enable", () => {
    const view = { state: "disabled", capability: { id: 7, scope_digest: "abc" } };
    expect(canEnableCapability(view, "abc")).toBe(true);
    expect(canEnableCapability(view, "stale")).toBe(false);
    expect(canEnableCapability({ ...view, state: "enabled" }, "abc")).toBe(false);
  });

  it("renders bounded state and diagnostic labels", () => {
    expect(capabilityStateLabel("expired")).toBe("Expired");
    expect(diagnosticLabel({ success: false, stage: "tls", message: "TLS failed." })).toBe("tls: TLS failed.");
  });

  it("rejects responses for superseded requests or edited forms", () => {
    expect(capabilityResponseIsCurrent(4, 4, 9, 9)).toBe(true);
    expect(capabilityResponseIsCurrent(3, 4, 9, 9)).toBe(false);
    expect(capabilityResponseIsCurrent(4, 4, 8, 9)).toBe(false);
  });

  it("requires a separately selected SSH capability for SSH Git remotes", () => {
    expect(gitCapabilityNeedsSSH({ kind: "git", git: { remote_url: "https://example.com/repo.git" } })).toBe(false);
    expect(gitCapabilityNeedsSSH({ kind: "git", git: { remote_url: "ssh://git@example.com/repo.git" } })).toBe(true);
    expect(gitCapabilityNeedsSSH({ kind: "git", git: { remote_url: "git@example.com:repo.git" } })).toBe(true);
  });

  it("serializes capability saves", () => {
    expect(canStartCapabilitySave(false)).toBe(true);
    expect(canStartCapabilitySave(true)).toBe(false);
  });
});
