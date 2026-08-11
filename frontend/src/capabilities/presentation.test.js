import { describe, expect, it } from "vitest";

import {
  capabilityActionAccessibleName,
  capabilityAnnouncement,
  capabilityFocusKey,
  capabilityFocusRestoreKey,
  canEnableCapability,
  canStartCapabilitySave,
  capabilityResponseIsCurrent,
  capabilityRiskGrants,
  capabilityScopeFieldState,
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

  it("gives repeated actions bounded capability-specific names", () => {
    expect(capabilityActionAccessibleName("remove", {
      id: 17,
      kind: "ssh",
      name: "Production deploy",
    })).toBe('Remove “Production deploy” (SSH capability #17)');
    const unsafe = capabilityActionAccessibleName("edit", {
      id: "not-an-id",
      kind: "other",
      name: `line\n${"x".repeat(100)}`,
    });
    expect(unsafe).not.toContain("\n");
    expect(unsafe).toContain("Unknown capability draft");
    expect(unsafe.length).toBeLessThan(100);
  });

  it("uses fixed bounded live announcements that cannot echo sensitive values", () => {
    for (const action of [
      "preview",
      "test",
      "save",
      "enable",
      "disable",
      "expire",
      "audit",
      "remove",
    ]) {
      for (const phase of ["progress", "success", "failure"]) {
        const announcement = capabilityAnnouncement(action, phase);
        expect(announcement.length).toBeLessThan(80);
        expect(announcement).not.toMatch(/digest|authorization|credential|header|response body/i);
      }
    }
    expect(capabilityAnnouncement("test", "blocked")).toBe(
      "Choose an approved SSH capability before testing.",
    );
    expect(capabilityAnnouncement("secret-token", "raw-args")).toBe(
      "Capability status changed.",
    );
  });

  it("activates only the selected kind fieldset", () => {
    expect(capabilityScopeFieldState("git")).toEqual({
      http: false,
      git: true,
      ssh: false,
    });
    expect(capabilityScopeFieldState("unknown")).toEqual({
      http: false,
      git: false,
      ssh: false,
    });
  });

  it("restores stable action focus and falls forward after removal", () => {
    const edit7 = capabilityFocusKey(7, "edit");
    expect(capabilityFocusRestoreKey(edit7, 0, [7, 8], [
      "capability:list",
      edit7,
    ])).toBe(edit7);

    const remove7 = capabilityFocusKey(7, "remove");
    const remove8 = capabilityFocusKey(8, "remove");
    expect(capabilityFocusRestoreKey(remove7, 0, [8, 9], [
      "capability:list",
      remove8,
    ])).toBe(remove8);
    expect(capabilityFocusRestoreKey(remove7, 0, [], ["capability:list"]))
      .toBe("capability:list");
    expect(capabilityFocusRestoreKey(remove7, 0, [8], [
      "capability:list",
      capabilityFocusKey(8),
    ])).toBe(capabilityFocusKey(8));
    expect(capabilityFocusRestoreKey(null, 0, [8], [remove8])).toBeNull();
  });
});
