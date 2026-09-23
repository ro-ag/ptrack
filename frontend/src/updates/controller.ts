import { element, setAriaBoolean } from "../workspace/dom";
import { formatBytes, messageFrom } from "../workspace/format";
import { appVersionLabel } from "../workspace/presentation";
import {
  updateActionFailureMessage,
  updateModalOpenTransition,
  updatePresentation,
  updateProgress,
  updateStateIsNewer,
} from "./presentation";
import type { AppContext } from "../workspace/app-context";

export interface UpdateRelease {
  version: string;
  notes?: string;
  pageUrl?: string;
  publishedAt?: string;
  sizeBytes?: number;
}

/** The updater's state as the desktop runtime reports it. */
export interface UpdateState {
  revision: number;
  phase: string;
  currentVersion: string;
  automaticChecks?: boolean;
  checksumVerified?: boolean;
  release?: UpdateRelease | null;
  error?: string;
  downloadedBytes?: number;
  totalBytes?: number;
  restartRequired?: boolean;
  applyAction?: string;
}

type UpdateAction = "check" | "download" | "apply";

function optionalString(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function optionalNumber(value: unknown): number | undefined {
  return typeof value === "number" ? value : undefined;
}

function optionalBoolean(value: unknown): boolean | undefined {
  return typeof value === "boolean" ? value : undefined;
}

function releaseFrom(value: unknown): UpdateRelease | null {
  if (!value || typeof value !== "object") return null;
  const release = value as Record<string, unknown>;
  if (typeof release.version !== "string") return null;
  return {
    version: release.version,
    notes: optionalString(release.notes),
    pageUrl: optionalString(release.pageUrl),
    publishedAt: optionalString(release.publishedAt),
    sizeBytes: optionalNumber(release.sizeBytes),
  };
}

/** An update-state event payload, when it is one. */
export function updateStateFrom(value: unknown): UpdateState | null {
  if (!value || typeof value !== "object") return null;
  const state = value as Record<string, unknown>;
  if (typeof state.phase !== "string") return null;
  return {
    revision: Number(state.revision) || 0,
    phase: state.phase,
    currentVersion: optionalString(state.currentVersion) ?? "",
    automaticChecks: optionalBoolean(state.automaticChecks),
    checksumVerified: optionalBoolean(state.checksumVerified),
    release: releaseFrom(state.release),
    error: optionalString(state.error),
    downloadedBytes: optionalNumber(state.downloadedBytes),
    totalBytes: optionalNumber(state.totalBytes),
    restartRequired: optionalBoolean(state.restartRequired),
    applyAction: optionalString(state.applyAction),
  };
}

function isUpdateAction(value: unknown): value is UpdateAction {
  return value === "check" || value === "download" || value === "apply";
}

const projectRepositoryURL = "https://github.com/ro-ag/ptrack";
const projectLicenseURL = `${projectRepositoryURL}/blob/main/LICENSE`;

// The build line states the platform this window runs on and whether the
// build can receive packaged updates at all.
export function aboutBuildLabel(
  phase: string,
  platform = navigatorPlatform(),
): string {
  return `${platform} · ${phase === "unavailable" ? "unpackaged build" : "packaged release"}`;
}

function navigatorPlatform(): string {
  const agent = navigator as Navigator & { userAgentData?: { platform?: string } };
  return agent.userAgentData?.platform || navigator.platform || "desktop";
}

export function updateReleaseMeta(release: UpdateRelease): string {
  const parts: string[] = [];
  if (release.publishedAt) {
    const published = new Date(release.publishedAt);
    if (Number.isFinite(published.getTime())) {
      parts.push(published.toLocaleDateString(undefined, {
        year: "numeric",
        month: "short",
        day: "numeric",
      }));
    }
  }
  if (Number(release.sizeBytes) > 0) parts.push(formatBytes(release.sizeBytes));
  return parts.join(" · ");
}

/** Only this project's own repository, and its release pages, open from About. */
export function projectLinkAllowed(url: string): boolean {
  return url.startsWith(projectRepositoryURL);
}

export function releasePageAllowed(url: string): boolean {
  return url.startsWith(`${projectRepositoryURL}/releases/`);
}

export function createUpdatesController(ctx: AppContext) {
  const { api, openHelpDestination, showError } = ctx;
  const elements = {
    aboutBuild: element("#about-build", HTMLElement),
    aboutHelp: element("#about-help", HTMLButtonElement),
    aboutLicenseLink: element("#about-license-link", HTMLButtonElement),
    aboutProject: element("#about-project", HTMLButtonElement),
    aboutReport: element("#about-report", HTMLButtonElement),
    aboutVersion: element("#about-version", HTMLElement),
    appVersion: element("#app-version", HTMLButtonElement),
    landingSettingsOpen: element("#landing-settings-open", HTMLButtonElement),
    settingsOpen: element("#settings-open", HTMLButtonElement),
    settingsUpdatesAutomatic: element("#settings-updates-automatic", HTMLInputElement),
    updatesAutomatic: element("#updates-automatic", HTMLInputElement),
    updatesCancel: element("#updates-cancel", HTMLButtonElement),
    updatesClose: element("#updates-close", HTMLButtonElement),
    updatesCurrentVersion: element("#updates-current-version", HTMLParagraphElement),
    updatesModal: element("#updates-modal", HTMLDivElement),
    updatesPrimary: element("#updates-primary", HTMLButtonElement),
    updatesProgress: element("#updates-progress", HTMLProgressElement),
    updatesProgressLabel: element("#updates-progress-label", HTMLSpanElement),
    updatesProgressWrap: element("#updates-progress-wrap", HTMLDivElement),
    updatesRelease: element("#updates-release", HTMLElement),
    updatesReleaseMeta: element("#updates-release-meta", HTMLSpanElement),
    updatesReleaseNotes: element("#updates-release-notes", HTMLParagraphElement),
    updatesReleasePage: element("#updates-release-page", HTMLButtonElement),
    updatesReleaseVersion: element("#updates-release-version", HTMLElement),
    updatesStatus: element("#updates-status", HTMLElement),
    updatesStatusDetail: element("#updates-status-detail", HTMLParagraphElement),
    updatesStatusTitle: element("#updates-status-title", HTMLHeadingElement),
    updatesVerified: element("#updates-verified", HTMLParagraphElement),
  };

  let updatesModalReturnFocus: HTMLElement | null = null;
  let updateState: UpdateState = { revision: 0, phase: "idle", currentVersion: "dev" };
  let updateActionBusy = false;
  let updateCancelRequested = false;

  function renderUpdateState(nextState: UpdateState | null | undefined): void {
    if (!nextState || !updateStateIsNewer(updateState, nextState)) return;
    updateState = nextState;
    const presentation = updatePresentation(nextState);
    const release = nextState.release || null;
    const progress = updateProgress(nextState);
    const currentVersion = appVersionLabel(nextState.currentVersion || "dev");

    setUpdateText(elements.updatesCurrentVersion, `Current version: ${currentVersion}`);
    setUpdateText(elements.aboutVersion, currentVersion);
    setUpdateText(elements.aboutBuild, aboutBuildLabel(nextState.phase));
    elements.updatesAutomatic.checked = Boolean(nextState.automaticChecks);
    elements.settingsUpdatesAutomatic.checked = Boolean(nextState.automaticChecks);
    elements.updatesStatus.dataset.tone = presentation.tone;
    setAriaBoolean(elements.updatesStatus, "aria-busy", presentation.busy);
    setUpdateText(elements.updatesStatusTitle, presentation.title);
    setUpdateText(elements.updatesStatusDetail, presentation.detail);
    elements.updatesProgressWrap.hidden = nextState.phase !== "downloading";
    elements.updatesProgress.value = progress.percent;
    elements.updatesProgressLabel.textContent = progress.total > 0
      ? `${progress.percent}% · ${formatBytes(progress.downloaded)} of ${formatBytes(progress.total)}`
      : `${formatBytes(progress.downloaded)} downloaded`;

    elements.updatesRelease.hidden = !release;
    elements.updatesReleaseVersion.textContent = release ? `Version ${release.version}` : "";
    elements.updatesReleaseMeta.textContent = release ? updateReleaseMeta(release) : "";
    elements.updatesReleaseNotes.textContent = release?.notes || "No release notes were provided.";
    elements.updatesReleasePage.hidden = !release?.pageUrl;
    elements.updatesVerified.hidden = !nextState.checksumVerified;
    elements.updatesCancel.hidden = !presentation.cancel;
    elements.updatesCancel.disabled = !presentation.cancel;
    elements.updatesPrimary.hidden = !presentation.primaryAction;
    elements.updatesPrimary.disabled = updateActionBusy || !presentation.primaryAction;
    elements.updatesPrimary.dataset.action = presentation.primaryAction || "";
    elements.updatesPrimary.textContent = presentation.primaryLabel;
  }

  function setUpdateText(element: Element, value: string): void {
    if (element.textContent !== value) element.textContent = value;
  }

  async function refreshUpdateState(): Promise<void> {
    try {
      renderUpdateState(await api().GetUpdateState());
    } catch (error) {
      showError(new Error(`Could not load update status: ${messageFrom(error)}`));
    }
  }

  function openAboutUpdates(invoker: Element | null = document.activeElement): boolean {
    if (
      ctx.state.firstRunState.phase !== "idle" ||
      ctx.state.firstPlanState.phase !== "idle" ||
      ctx.recent.recentProjectOperationActive()
    ) return false;
    const competingOverlayOpen = ctx.nativeMenu.nativeMenuOpenOverlayIDs().some(
      (overlayID) => overlayID !== "updates-modal",
    );
    if (competingOverlayOpen) return false;
    const transition = updateModalOpenTransition(
      elements.updatesModal.hidden,
      updatesModalReturnFocus,
      invoker instanceof HTMLElement ? invoker : null,
    );
    updatesModalReturnFocus = transition.returnFocus;
    if (transition.makeVisible) elements.updatesModal.hidden = false;
    renderUpdateState(updateState);
    void refreshUpdateState();
    if (transition.scheduleOpeningFocus) {
      requestAnimationFrame(() => elements.updatesClose.focus());
    }
    return true;
  }

  function closeAboutUpdates(): void {
    if (elements.updatesModal.hidden) return;
    ctx.snapshot.hideApplicationOverlay(elements.updatesModal);
    updatesModalReturnFocus?.focus?.();
    updatesModalReturnFocus = null;
  }

  async function runUpdateAction(action: string | undefined): Promise<void> {
    if (updateActionBusy) return;
    if (!isUpdateAction(action)) return;
    updateCancelRequested = false;
    const version = updateState.release?.version || "";
    if ((action === "download" || action === "apply") && !version) {
      await refreshUpdateState();
      return;
    }
    updateActionBusy = true;
    renderUpdateState(updateState);
    try {
      let state: UpdateState | undefined;
      if (action === "check") state = await api().CheckForUpdates();
      if (action === "download") state = await api().DownloadUpdate(version);
      if (action === "apply") state = await api().ApplyUpdate(version);
      if (state) renderUpdateState(state);
    } catch (error) {
      await refreshUpdateState();
      if (!updateCancelRequested) {
        showError(new Error(updateActionFailureMessage(action, error)));
      }
    } finally {
      updateActionBusy = false;
      updateCancelRequested = false;
      renderUpdateState(updateState);
    }
  }

  async function setAutomaticUpdateChecks(enabled: boolean): Promise<void> {
    elements.updatesAutomatic.disabled = true;
    elements.settingsUpdatesAutomatic.disabled = true;
    try {
      renderUpdateState(await api().SetAutomaticUpdateChecks(Boolean(enabled)));
    } catch (error) {
      await refreshUpdateState();
      showError(new Error(`Could not save the automatic update preference: ${messageFrom(error)}`));
    } finally {
      elements.updatesAutomatic.disabled = false;
      elements.settingsUpdatesAutomatic.disabled = false;
    }
  }

  // External links always travel through the validated native opener; the
  // browser fallback only matters in a plain dev server.
  function openProjectURL(url: string): void {
    if (!projectLinkAllowed(url)) return;
    const openURL = window.runtime?.BrowserOpenURL;
    if (typeof openURL === "function") {
      void Promise.resolve().then(() => openURL(url)).catch((error: unknown) => {
        showError(new Error(`Could not open the link: ${messageFrom(error)}`));
      });
      return;
    }
    window.open(url, "_blank", "noopener,noreferrer");
  }

  function openUpdateReleasePage(): void {
    const page = updateState.release?.pageUrl || "";
    if (!releasePageAllowed(page)) return;
    openProjectURL(page);
  }

  function updateAboutUpdatesAvailability(): void {
    elements.appVersion.disabled = ctx.state.firstRunState.phase !== "idle" ||
      ctx.state.firstPlanState.phase !== "idle" ||
      ctx.recent.recentProjectOperationActive();
    elements.settingsOpen.disabled = elements.appVersion.disabled;
    elements.landingSettingsOpen.disabled = elements.appVersion.disabled;
  }

  function bind(): void {
    elements.appVersion.addEventListener("click", () => {
      openAboutUpdates(elements.appVersion);
    });
    elements.updatesClose.addEventListener("click", closeAboutUpdates);
    document.querySelectorAll("[data-close-updates]").forEach((element) => {
      element.addEventListener("click", closeAboutUpdates);
    });
    elements.updatesAutomatic.addEventListener("change", () => {
      void setAutomaticUpdateChecks(elements.updatesAutomatic.checked);
    });
    elements.updatesPrimary.addEventListener("click", () => {
      void runUpdateAction(elements.updatesPrimary.dataset.action);
    });
    elements.updatesCancel.addEventListener("click", async () => {
      updateCancelRequested = true;
      try {
        renderUpdateState(await api().CancelUpdateOperation());
      } catch {
        updateCancelRequested = false;
        await refreshUpdateState();
        showError(new Error("The update operation could not be canceled."));
      }
      if (!updateActionBusy) updateCancelRequested = false;
    });
    elements.updatesReleasePage.addEventListener("click", openUpdateReleasePage);
    elements.aboutProject.addEventListener("click", () => {
      openProjectURL(projectRepositoryURL);
    });
    elements.aboutLicenseLink.addEventListener("click", () => {
      openProjectURL(projectLicenseURL);
    });
    elements.aboutHelp.addEventListener("click", () => openHelpDestination("help-center"));
    elements.aboutReport.addEventListener("click", () => openHelpDestination("report-issue"));
  }

  return {
    bind,
    renderUpdateState,
    refreshUpdateState,
    openAboutUpdates,
    closeAboutUpdates,
    runUpdateAction,
    setAutomaticUpdateChecks,
    updateAboutUpdatesAvailability,
  };
}

export type UpdatesController = ReturnType<typeof createUpdatesController>;
