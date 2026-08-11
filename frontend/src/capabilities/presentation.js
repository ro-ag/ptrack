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

const capabilityActionLabels = {
  edit: "Edit",
  test: "Test",
  enable: "Review and enable",
  disable: "Disable",
  expire: "Expire",
  audit: "Audit",
  remove: "Remove",
};

function boundedCapabilityLabel(value) {
  const label = String(value || "")
    .replace(/[\p{Cc}\p{Cf}]+/gu, " ")
    .replace(/\s+/g, " ")
    .trim();
  if (!label) return "Unnamed";
  return label.length > 48 ? `${label.slice(0, 47)}…` : label;
}

function capabilityKindLabel(kind) {
  return {
    http: "HTTP",
    git: "Git",
    ssh: "SSH",
  }[kind] || "Unknown";
}

function capabilityIDLabel(id) {
  const number = Number(id);
  return Number.isSafeInteger(number) && number > 0 ? `#${number}` : "draft";
}

export function capabilityActionAccessibleName(action, capability) {
  const actionLabel = capabilityActionLabels[action] || "Use";
  return `${actionLabel} “${boundedCapabilityLabel(capability?.name)}” (${capabilityKindLabel(capability?.kind)} capability ${capabilityIDLabel(capability?.id)})`;
}

const capabilityAnnouncements = {
  preview: {
    progress: "Preparing capability scope preview…",
    success: "Capability scope preview ready.",
    failure: "Capability scope preview failed.",
  },
  test: {
    progress: "Testing capability connection…",
    success: "Capability connection test passed.",
    failure: "Capability connection test failed.",
    blocked: "Choose an approved SSH capability before testing.",
  },
  save: {
    progress: "Saving disabled capability draft…",
    success: "Capability draft saved and remains disabled.",
    failure: "Capability draft could not be saved.",
  },
  enable: {
    progress: "Enabling the confirmed capability scope…",
    success: "Confirmed capability scope enabled.",
    failure: "Capability scope could not be enabled.",
  },
  disable: {
    progress: "Disabling capability…",
    success: "Capability disabled.",
    failure: "Capability could not be disabled.",
  },
  expire: {
    progress: "Expiring capability…",
    success: "Capability expired.",
    failure: "Capability could not be expired.",
  },
  audit: {
    progress: "Loading capability audit metadata…",
    success: "Capability audit metadata loaded.",
    failure: "Capability audit metadata could not be loaded.",
  },
  remove: {
    progress: "Removing capability…",
    success: "Capability removed.",
    failure: "Capability could not be removed.",
  },
};

export function capabilityAnnouncement(action, phase) {
  return capabilityAnnouncements[action]?.[phase] || "Capability status changed.";
}

export function capabilityScopeFieldState(kind) {
  return {
    http: kind === "http",
    git: kind === "git",
    ssh: kind === "ssh",
  };
}

const capabilityFocusActions = [
  "edit",
  "test",
  "enable",
  "disable",
  "expire",
  "audit",
  "remove",
];

export function capabilityFocusKey(capabilityID, action = "card") {
  const id = Number(capabilityID);
  if (!Number.isSafeInteger(id) || id <= 0) return "capability:list";
  const safeAction = capabilityFocusActions.includes(action) ? action : "card";
  return `capability:${id}:${safeAction}`;
}

export function capabilityFocusRestoreKey(
  previousKey,
  previousIndex,
  capabilityIDs,
  availableKeys,
) {
  if (!previousKey) return null;
  const available = new Set(availableKeys || []);
  if (available.has(previousKey)) return previousKey;
  const match = /^capability:\d+:(card|edit|test|enable|disable|expire|audit|remove)$/.exec(
    previousKey,
  );
  const ids = (capabilityIDs || []).filter((id) => Number(id) > 0);
  if (match && ids.length > 0) {
    const index = Math.min(Math.max(Number(previousIndex) || 0, 0), ids.length - 1);
    const candidateID = ids[index];
    const previousAction = match[1];
    const candidates = [
      capabilityFocusKey(candidateID, previousAction),
      ...capabilityFocusActions.map((action) => capabilityFocusKey(candidateID, action)),
      capabilityFocusKey(candidateID),
    ];
    const candidate = candidates.find((key) => available.has(key));
    if (candidate) return candidate;
  }
  return available.has("capability:list") ? "capability:list" : null;
}
