const busyPhases = new Set([
  "recovering",
  "checking",
  "downloading",
  "applying",
  "canceling",
]);

export function updateStateIsNewer(current, incoming) {
  return Number(incoming?.revision || 0) >= Number(current?.revision || 0);
}

export function updatePresentation(state = {}) {
  const phase = String(state.phase || "idle");
  const release = state.release || null;
  const version = typeof release?.version === "string" ? release.version.trim() : "";
  const verified = Boolean(state.checksumVerified);
  const presentation = {
    busy: busyPhases.has(phase),
    cancel: ["checking", "downloading", "applying"].includes(phase),
    primaryAction: null,
    primaryLabel: "",
    title: "Updates are ready",
    detail: "Check GitHub Releases when you choose.",
    tone: "neutral",
  };

  switch (phase) {
    case "idle":
      presentation.primaryAction = "check";
      presentation.primaryLabel = "Check for updates";
      break;
    case "recovering":
      presentation.title = "Verifying saved updates";
      presentation.detail = "p-track is checking locally saved update files before allowing update actions.";
      break;
    case "recovery-required":
      presentation.title = "Update recovery required";
      presentation.detail = state.error || "A previous update needs manual recovery before another update can start.";
      presentation.tone = "error";
      break;
    case "checking":
      presentation.title = "Checking GitHub Releases";
      presentation.detail = "Looking for a newer stable release for this platform.";
      break;
    case "current":
      presentation.title = "p-track is up to date";
      presentation.detail = `Version ${state.currentVersion || "unknown"} is the latest stable release.`;
      presentation.primaryAction = "check";
      presentation.primaryLabel = "Check again";
      presentation.tone = "success";
      break;
    case "available":
      if (!version) return invalidUpdateState(presentation);
      presentation.title = `Version ${version} is available`;
      presentation.detail = "Download the packaged release asset and verify it before installation.";
      presentation.primaryAction = "download";
      presentation.primaryLabel = "Download and verify";
      presentation.tone = "info";
      break;
    case "downloading":
      presentation.title = `Downloading version ${version}`;
      presentation.detail = "The packaged release and its checksum are being verified locally.";
      break;
    case "ready":
      if (!version || !verified) return invalidUpdateState(presentation);
      presentation.title = `Version ${version} is verified`;
      presentation.detail = "The packaged release asset passed checksum and platform validation.";
      presentation.primaryAction = "apply";
      presentation.primaryLabel = "Install verified update…";
      presentation.tone = "success";
      break;
    case "applying":
      presentation.title = `Preparing version ${version}`;
      presentation.detail = "p-track is completing the safe installation handoff for this platform.";
      break;
    case "canceling":
      presentation.title = "Canceling update action";
      presentation.detail = "Waiting for the active update operation to stop safely.";
      break;
    case "action-required":
      presentation.title = "Complete installation manually";
      presentation.detail = manualActionDetail(state.applyAction);
      presentation.tone = "info";
      break;
    case "installed":
      presentation.title = `Version ${version || "update"} is installed`;
      presentation.detail = state.restartRequired
        ? "Restart p-track to use the installed version."
        : "The verified update was installed successfully.";
      presentation.tone = "success";
      break;
    case "unavailable":
      presentation.title = "Updates are unavailable for this build";
      presentation.detail = state.error || "Use an official packaged release to receive updates.";
      break;
    case "error":
      presentation.title = "The update could not continue safely";
      presentation.detail = state.error || "Try again. No unverified update was installed.";
      presentation.tone = "error";
      if (version && verified) {
        presentation.primaryAction = "apply";
        presentation.primaryLabel = "Retry installation…";
      } else if (version) {
        presentation.primaryAction = "download";
        presentation.primaryLabel = "Retry download";
      } else {
        presentation.primaryAction = "check";
        presentation.primaryLabel = "Check again";
      }
      break;
    default:
      return invalidUpdateState(presentation);
  }
  return presentation;
}

export function updateProgress(state = {}) {
  const downloaded = finiteUpdateNumber(state.downloadedBytes);
  const total = finiteUpdateNumber(state.totalBytes);
  return {
    downloaded,
    total,
    percent: total > 0 ? Math.min(100, Math.round((downloaded / total) * 100)) : 0,
  };
}

export function formatUpdateBytes(value) {
  const bytes = finiteUpdateNumber(value);
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function manualActionDetail(action) {
  if (action === "opened-native-installer") {
    return "The verified macOS installer is open. Complete installation there, then restart p-track.";
  }
  if (action === "revealed-verified-archive") {
    return "The verified Windows archive is selected. Close p-track before replacing the executable, then reopen it.";
  }
  return "Complete the platform installation step, then restart p-track.";
}

function invalidUpdateState(presentation) {
  return {
    ...presentation,
    busy: false,
    cancel: false,
    primaryAction: null,
    primaryLabel: "",
    title: "Update status is unavailable",
    detail: "Close and reopen this dialog before trying another update action.",
    tone: "error",
  };
}

function finiteUpdateNumber(value) {
  const number = Number(value || 0);
  return Number.isFinite(number) ? Math.max(0, number) : 0;
}
