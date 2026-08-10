export function splitCapabilityList(value) {
  return String(value || "")
    .split(/[\n,]+/)
    .map((item) => item.trim())
    .filter(Boolean);
}

export function capabilityStateLabel(state) {
  return {
    draft: "Draft",
    disabled: "Disabled",
    enabled: "Enabled",
    expired: "Expired",
    invalid: "Invalid",
  }[state] || "Unknown";
}

export function capabilityRiskGrants(capability) {
  if (capability?.kind === "http") {
    const methods = capability.http?.methods || [];
    return methods.filter((method) => !["GET", "HEAD", "OPTIONS"].includes(method));
  }
  if (capability?.kind === "git") {
    const scope = capability.git || {};
    return [
      ...(scope.operations || []).filter((operation) => ["pull", "push"].includes(operation)),
      ...(scope.allow_force_push ? ["force push"] : []),
      ...(scope.allow_delete_refs ? ["ref deletion"] : []),
      ...(scope.allow_tags ? ["tag writes"] : []),
    ];
  }
  if (capability?.kind === "ssh") {
    const scope = capability.ssh || {};
    return [
      ...(scope.remote_commands?.length ? ["remote commands"] : []),
      ...(scope.allow_upload ? ["uploads"] : []),
      ...(scope.allow_download ? ["downloads"] : []),
      ...(scope.allow_interactive_shell ? ["interactive shell"] : []),
      ...(scope.local_forward_targets?.length ? ["local forwarding"] : []),
      ...(scope.remote_forward_targets?.length ? ["remote forwarding"] : []),
    ];
  }
  return [];
}

export function canEnableCapability(view, confirmedDigest) {
  return Boolean(
    view?.capability?.id &&
      view?.capability?.scope_digest &&
      view.capability.scope_digest === confirmedDigest &&
      view.state !== "enabled",
  );
}

export function diagnosticLabel(diagnostic) {
  if (!diagnostic) return "Not tested";
  if (diagnostic.success) return "Connection test passed";
  return `${diagnostic.stage}: ${diagnostic.message}`;
}

export function capabilityResponseIsCurrent(
  request,
  latestRequest,
  formRevision,
  currentFormRevision,
) {
  return request === latestRequest && formRevision === currentFormRevision;
}

export function gitCapabilityNeedsSSH(capability) {
  if (capability?.kind !== "git") return false;
  const remote = String(capability.git?.remote_url || "").trim().toLowerCase();
  return remote.length > 0 && !remote.startsWith("https://");
}

export function canStartCapabilitySave(saveInFlight) {
  return !saveInFlight;
}
