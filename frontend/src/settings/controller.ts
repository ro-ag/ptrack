import type { DiscoveredTerminalProfile } from "../terminal/linked-launch";
import { initTheme } from "../theme";
import type { AppContext } from "../workspace/app-context";
import { element, svgElement } from "../workspace/dom";
import { messageFrom } from "../workspace/format";
import { defaultLayoutState, normalizeLayoutState } from "../workspace/layout";
import { terminalWorkspaceStoragePrefix } from "../workspace/persistence";
import {
  applyPreferenceMirrors,
  densityPreferences,
  preferenceChoice,
  preferenceSaveMessage,
  preferencesFromMirrors,
  preferencesResponse,
  reducedMotionPreferences,
  rendererPreferences,
  storageStatusNotice,
  themePreferences,
  unicodeModePreferences,
  type PreferenceSavePhase,
  type Preferences,
  type PreferencesPatch,
  type PreferencesStorageStatus,
} from "./preferences";
import {
  diagnosticsRows,
  nextSettingsSectionIndex,
  resetApplicationStateConfirmation,
  resetApplicationStateMessage,
  resetSettingsConfirmation,
  resetWindowLayoutConfirmation,
  settingsPanelId,
  settingsSectionIndex,
  settingsSections,
  settingsTabId,
  type DiagnosticsRow,
  type SettingsSectionId,
} from "./sections";

// Long enough to read the longest status the dialog writes, short enough that
// it never becomes part of the furniture.
const settingsStatusClearDelay = 6000;

function settingsSectionFromTab(tabId: string): SettingsSectionId | null {
  const id = tabId.replace("settings-tab-", "");
  return settingsSections.find((section) => section.id === id)?.id ?? null;
}

function copyIcon(): SVGElement {
  const icon = svgElement("svg", { viewBox: "0 0 16 16", "aria-hidden": "true" });
  icon.append(
    svgElement("rect", { x: "5.5", y: "5.5", width: "8.5", height: "8.5", rx: "2" }),
    svgElement("path", { d: "M10.5 5.5V4a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2v4.5a2 2 0 0 0 2 2h1.5" }),
  );
  return icon;
}

export function createSettingsController(ctx: AppContext) {
  const { api, openHelpDestination, showError } = ctx;
  const elements = {
    landingSettingsOpen: element("#landing-settings-open", HTMLButtonElement),
    settingsBody: element("#settings-body", HTMLDivElement),
    settingsClose: element("#settings-close", HTMLButtonElement),
    settingsDensity: element("#settings-density", HTMLSelectElement),
    settingsDiagnostics: element("#settings-diagnostics", HTMLDivElement),
    settingsModal: element("#settings-modal", HTMLDivElement),
    settingsNotificationsCompletion: element("#settings-notifications-completion", HTMLInputElement),
    settingsNotificationsFailureDrift: element("#settings-notifications-failure-drift", HTMLInputElement),
    settingsNotificationsHandoff: element("#settings-notifications-handoff", HTMLInputElement),
    settingsOpen: element("#settings-open", HTMLButtonElement),
    settingsOpenUpdates: element("#settings-open-updates", HTMLButtonElement),
    settingsReducedMotion: element("#settings-reduced-motion", HTMLSelectElement),
    settingsReset: element("#settings-reset", HTMLButtonElement),
    settingsResetApplicationState: element("#settings-reset-application-state", HTMLButtonElement),
    settingsResetWindowLayout: element("#settings-reset-window-layout", HTMLButtonElement),
    settingsSaveStatus: element("#settings-save-status", HTMLParagraphElement),
    settingsSectionList: element("#settings-section-list", HTMLDivElement),
    settingsStartupRestore: element("#settings-startup-restore", HTMLInputElement),
    settingsStorageNotice: element("#settings-storage-notice", HTMLParagraphElement),
    settingsTerminalFontFamily: element("#settings-terminal-font-family", HTMLInputElement),
    settingsTerminalFontSize: element("#settings-terminal-font-size", HTMLInputElement),
    settingsTerminalProfile: element("#settings-terminal-profile", HTMLSelectElement),
    settingsTerminalRenderer: element("#settings-terminal-renderer", HTMLSelectElement),
    settingsTerminalScrollback: element("#settings-terminal-scrollback", HTMLInputElement),
    settingsTerminalUnicode: element("#settings-terminal-unicode", HTMLSelectElement),
    settingsTheme: element("#settings-theme", HTMLSelectElement),
    settingsUpdatesAutomatic: element("#settings-updates-automatic", HTMLInputElement),
    themeToggle: element("#theme-toggle", HTMLButtonElement),
  };

  let settingsModalReturnFocus: HTMLElement | null = null;
  let settingsSection: SettingsSectionId = settingsSections[0].id;
  let settingsSaveSequence = 0;
  let settingsDiagnosticsRequest = 0;
  let settingsStatusTimer = 0;

  // ------------------------------------------------------------ settings

  function applyPreferences(next: Preferences): void {
    // The startup opt-in is what decides the Welcome preselect, so turning it
    // off clears the highlight now rather than at the next list load. Every
    // other preference leaves the list alone: rebuilding it would drop focus.
    const startupChanged =
      ctx.state.preferences.startup.restoreLastProject !== next.startup.restoreLastProject ||
      ctx.state.preferences.startup.lastProjectRoot !== next.startup.lastProjectRoot;
    ctx.state.preferences = next;
    themeController.setTheme(next.appearance.theme);
    const root = document.documentElement;
    root.dataset.density = next.appearance.density;
    if (next.appearance.reducedMotion === "system") {
      delete root.dataset.reducedMotion;
    } else {
      root.dataset.reducedMotion = next.appearance.reducedMotion;
    }
    applyPreferenceMirrors(localStorage, next);
    renderPreferences();
    if (startupChanged) ctx.landing.renderRecentProjects();
  }

  function renderPreferences(): void {
    const preferences = ctx.state.preferences;
    elements.settingsStartupRestore.checked = preferences.startup.restoreLastProject;
    elements.settingsTheme.value = preferences.appearance.theme;
    elements.settingsDensity.value = preferences.appearance.density;
    elements.settingsReducedMotion.value = preferences.appearance.reducedMotion;
    renderTerminalProfilePreference();
    elements.settingsTerminalFontFamily.value = preferences.terminal.fontFamily;
    elements.settingsTerminalFontSize.value = String(preferences.terminal.fontSize);
    elements.settingsTerminalUnicode.value = preferences.terminal.unicodeMode;
    elements.settingsTerminalScrollback.value = String(preferences.terminal.scrollback);
    elements.settingsTerminalRenderer.value = preferences.terminal.renderer;
    elements.settingsNotificationsHandoff.checked = preferences.notifications.handoffArrival;
    elements.settingsNotificationsFailureDrift.checked =
      preferences.notifications.runFailureOrDrift;
    elements.settingsNotificationsCompletion.checked = preferences.notifications.runCompletion;
  }

  // A stored default profile that no longer resolves is reported as
  // unavailable instead of being coerced onto an installed profile.
  function renderTerminalProfilePreference(): void {
    const stored = ctx.state.preferences.terminal.defaultProfileId || "";
    const select = elements.settingsTerminalProfile;
    const missing = select.querySelector("[data-unavailable-profile]");
    if (missing) missing.remove();
    if (stored && !select.querySelector(`option[value="${CSS.escape(stored)}"]`)) {
      const option = document.createElement("option");
      option.value = stored;
      option.textContent = `${stored} · unavailable`;
      option.dataset.unavailableProfile = "true";
      select.append(option);
    }
    select.value = stored;
  }

  async function loadTerminalProfileOptions(): Promise<void> {
    let profiles: DiscoveredTerminalProfile[];
    try {
      profiles = await api().GetTerminalProfiles();
    } catch {
      profiles = [];
    }
    const select = elements.settingsTerminalProfile;
    const installed = select.querySelectorAll("option:not([value=''])");
    installed.forEach((option) => option.remove());
    for (const profile of profiles) {
      const option = document.createElement("option");
      option.value = profile.id;
      option.textContent = `${profile.name}${profile.kind === "agent" ? " · agent" : ""}`;
      select.append(option);
    }
    renderTerminalProfilePreference();
  }

  function renderSettingsStorageNotice(status: PreferencesStorageStatus): void {
    const notice = storageStatusNotice(status);
    elements.settingsStorageNotice.textContent = notice;
    elements.settingsStorageNotice.hidden = notice === "";
  }

  // The dialog's single live region. It sits outside the aria-busy wrapper, so
  // a long reset is still announced.
  //
  // The element never leaves the DOM — removing it is what breaks announcements —
  // but its text is transient: a confirmation that stays on screen stops reading
  // as "that just happened" and starts reading as a permanent label. Clearing the
  // text does not retract what was already announced. Nothing moves or fades, so
  // there is no motion for a reduced-motion preference to have an opinion about.
  // A failure stays until the next action: it is the one thing left to act on.
  function setSettingsStatus(message: string, failed = false, sticky = false): void {
    clearTimeout(settingsStatusTimer);
    elements.settingsSaveStatus.textContent = message;
    elements.settingsSaveStatus.dataset.tone = failed ? "error" : "";
    if (message === "" || failed || sticky) return;
    settingsStatusTimer = window.setTimeout(() => {
      elements.settingsSaveStatus.textContent = "";
    }, settingsStatusClearDelay);
  }

  function setSettingsSaveStatus(phase: PreferenceSavePhase | ""): void {
    setSettingsStatus(
      phase ? preferenceSaveMessage(phase) : "",
      phase === "failed",
      // "Saving…" is superseded by its own outcome, so it must not time out and
      // leave a slow save looking like nothing was ever asked for.
      phase === "saving",
    );
  }

  async function loadPreferences(): Promise<void> {
    try {
      const response = preferencesResponse(await api().GetPreferences());
      applyPreferences(response.preferences);
      renderSettingsStorageNotice(response.storage);
    } catch {
      // The cached values are what this window is already using, so they are
      // shown as-is rather than being replaced with defaults.
      ctx.state.preferences = preferencesFromMirrors(localStorage);
      renderPreferences();
      renderSettingsStorageNotice("unavailable");
    }
  }

  async function savePreferences(patch: PreferencesPatch): Promise<void> {
    const sequence = ++settingsSaveSequence;
    setSettingsSaveStatus("saving");
    elements.settingsBody.setAttribute("aria-busy", "true");
    try {
      const response = preferencesResponse(await api().SetPreferences(patch));
      if (sequence !== settingsSaveSequence) return;
      applyPreferences(response.preferences);
      renderSettingsStorageNotice(response.storage);
      setSettingsSaveStatus("saved");
    } catch {
      if (sequence !== settingsSaveSequence) return;
      renderPreferences();
      setSettingsSaveStatus("failed");
    } finally {
      if (sequence === settingsSaveSequence) {
        elements.settingsBody.removeAttribute("aria-busy");
      }
    }
  }

  async function resetPreferences(invoker: HTMLElement = elements.settingsReset): Promise<void> {
    if (!(await ctx.recent.showConfirmation(resetSettingsConfirmation, invoker))) return;
    const sequence = ++settingsSaveSequence;
    setSettingsSaveStatus("saving");
    elements.settingsReset.disabled = true;
    try {
      const response = preferencesResponse(await api().ResetPreferences());
      if (sequence !== settingsSaveSequence) return;
      applyPreferences(response.preferences);
      renderSettingsStorageNotice(response.storage);
      setSettingsSaveStatus("reset");
    } catch {
      if (sequence === settingsSaveSequence) setSettingsSaveStatus("failed");
    } finally {
      elements.settingsReset.disabled = false;
    }
  }

  async function resetWindowLayout(invoker: HTMLElement): Promise<void> {
    if (!(await ctx.recent.showConfirmation(resetWindowLayoutConfirmation, invoker))) return;
    elements.settingsResetWindowLayout.disabled = true;
    try {
      ctx.layout.applyLayoutState(normalizeLayoutState(await api().ResetWindowLayout()));
      // Sticky: a reset outcome is the result of an explicit destructive action
      // and the one thing left to read. Clearing it also collapses several
      // wrapped lines out of the footer, which moves the button underneath it
      // six seconds after anyone last touched anything.
      setSettingsStatus("Window layout reset to defaults.", false, true);
    } catch (error) {
      setSettingsStatus(messageFrom(error), true);
    } finally {
      elements.settingsResetWindowLayout.disabled = false;
    }
  }

  // The runtime cannot reach WebView storage, so the saved terminal workspaces
  // are cleared here. A dock that is still open keeps its live tabs and saves
  // them again on the next change.
  function clearTerminalWorkspaceDescriptors(): void {
    try {
      for (const key of Object.keys(localStorage)) {
        if (key.startsWith(terminalWorkspaceStoragePrefix)) localStorage.removeItem(key);
      }
    } catch {
      // Persistence is optional.
    }
  }

  async function resetApplicationState(invoker: HTMLElement): Promise<void> {
    if (!(await ctx.recent.showConfirmation(resetApplicationStateConfirmation, invoker))) return;
    elements.settingsResetApplicationState.disabled = true;
    try {
      const result = await api().ResetApplicationState();
      clearTerminalWorkspaceDescriptors();
      ctx.layout.applyLayoutState(defaultLayoutState());
      await loadPreferences();
      void ctx.updates.refreshUpdateState();
      // Sticky for the same reason as the layout reset, and more so: this
      // message is three clauses long and wraps to about four lines.
      setSettingsStatus(resetApplicationStateMessage(result), false, true);
    } catch (error) {
      setSettingsStatus(messageFrom(error), true);
    } finally {
      elements.settingsResetApplicationState.disabled = false;
    }
  }

  async function loadDiagnosticsReport(): Promise<void> {
    const request = ++settingsDiagnosticsRequest;
    try {
      const report = await api().GetDiagnosticsReport();
      if (request === settingsDiagnosticsRequest) renderDiagnosticsReport(report);
    } catch {
      if (request === settingsDiagnosticsRequest) renderDiagnosticsReport(null);
    }
  }

  // A word-wrapping "Copy" label is what broke this column, so the control is
  // an icon that cannot break. Its accessible name says what it copies, and
  // the title repeats it for pointer users who get no label at all.
  function diagnosticCopyButton(row: DiagnosticsRow, label: string): HTMLButtonElement {
    const copy = document.createElement("button");
    copy.type = "button";
    copy.className = "settings-diagnostic-copy";
    copy.setAttribute("aria-label", label);
    copy.title = label;
    copy.append(copyIcon());
    copy.addEventListener("click", () => void copyDiagnosticValue(row));
    return copy;
  }

  function diagnosticEntry(row: DiagnosticsRow): HTMLDivElement {
    const group = document.createElement("div");
    group.className = "settings-diagnostic";
    const term = document.createElement("dt");
    term.textContent = row.label;
    const description = document.createElement("dd");
    const value = document.createElement("span");
    value.className = "settings-diagnostic-value";
    value.textContent = row.value;
    description.append(value);
    if (row.detail) {
      const detail = document.createElement("span");
      detail.className = "settings-diagnostic-detail";
      detail.textContent = row.detail;
      description.append(detail);
    }
    if (row.copy) description.append(diagnosticCopyButton(row, row.copy));
    group.append(term, description);
    return group;
  }

  function renderDiagnosticsReport(report: unknown): void {
    const rows = diagnosticsRows(report);
    elements.settingsDiagnostics.replaceChildren();
    if (rows.length === 0) {
      const empty = document.createElement("p");
      empty.className = "dialog-help";
      empty.textContent = "No diagnostics are available yet.";
      elements.settingsDiagnostics.append(empty);
      return;
    }
    // One headed list per group: Global storage first, then This project.
    let list: HTMLDListElement | null = null;
    let currentGroup = "";
    for (const row of rows) {
      if (row.group !== currentGroup || !list) {
        currentGroup = row.group;
        const heading = document.createElement("h4");
        heading.className = "settings-diagnostics-group";
        heading.textContent = row.group;
        list = document.createElement("dl");
        list.className = "settings-diagnostics-list";
        elements.settingsDiagnostics.append(heading, list);
      }
      list.append(diagnosticEntry(row));
    }
  }

  // Copy confirmations go through the one live region the dialog has, so they are
  // announced and then clear on the same terms as every other status.
  async function copyDiagnosticValue(row: DiagnosticsRow): Promise<void> {
    try {
      if ((await window.runtime?.ClipboardSetText?.(row.value)) !== true) {
        throw new Error("clipboard unavailable");
      }
      setSettingsStatus(`${row.label} copied.`);
    } catch {
      showError(new Error(`Could not copy ${row.label}.`));
    }
  }

  function settingsAvailable(): boolean {
    return ctx.state.firstRunState.phase === "idle" &&
      ctx.state.firstPlanState.phase === "idle" &&
      !ctx.recent.recentProjectOperationActive();
  }

  function openSettings(invoker: Element | null = document.activeElement): boolean {
    if (!settingsAvailable()) return false;
    const competingOverlayOpen = ctx.nativeMenu.nativeMenuOpenOverlayIDs().some(
      (overlayID) => overlayID !== "settings-modal",
    );
    if (competingOverlayOpen) return false;
    const wasHidden = elements.settingsModal.hidden;
    if (wasHidden) {
      settingsModalReturnFocus = invoker instanceof HTMLElement ? invoker : null;
      elements.settingsModal.hidden = false;
      setSettingsSaveStatus("");
    }
    void loadPreferences();
    void loadTerminalProfileOptions();
    void loadDiagnosticsReport();
    void ctx.updates.refreshUpdateState();
    if (wasHidden) {
      requestAnimationFrame(() => selectSettingsSection(settingsSection, true));
    }
    return true;
  }

  function closeSettings(): void {
    if (elements.settingsModal.hidden) return;
    ctx.snapshot.hideApplicationOverlay(elements.settingsModal);
    settingsModalReturnFocus?.focus?.();
    settingsModalReturnFocus = null;
  }

  function selectSettingsSection(section: SettingsSectionId, focus = false): void {
    settingsSection = section;
    for (const entry of settingsSections) {
      const tab = element(`#${settingsTabId(entry.id)}`, HTMLElement);
      const panel = element(`#${settingsPanelId(entry.id)}`, HTMLElement);
      const active = entry.id === section;
      tab.setAttribute("aria-selected", String(active));
      tab.tabIndex = active ? 0 : -1;
      panel.hidden = !active;
    }
    if (focus) element(`#${settingsTabId(section)}`, HTMLElement).focus();
    if (section === "data") void loadDiagnosticsReport();
  }

  const themeButtons = [
    elements.themeToggle,
    element("#terminal-window-theme-toggle", HTMLButtonElement),
  ];

  const themeController = initTheme({
    root: document.documentElement,
    storage: localStorage,
    media: matchMedia("(prefers-color-scheme: light)"),
    onChange: (theme: string) => {
      // Show the theme a click switches to: sun in dark mode, moon in light.
      for (const button of themeButtons) {
        button.textContent = theme === "dark" ? "☀" : "☾";
        button.title = theme === "dark" ? "Switch to light theme" : "Switch to dark theme";
      }
    },
  });

  // A select only offers its own choices, so an unknown value is never saved.
  function saveChoice<T extends string>(
    select: HTMLSelectElement,
    choices: readonly T[],
    patch: (value: T) => PreferencesPatch,
  ): void {
    const value = preferenceChoice(select.value, choices);
    if (value !== null) void savePreferences(patch(value));
  }

  function saveOnChange(control: HTMLInputElement | HTMLSelectElement, patch: () => PreferencesPatch): void {
    control.addEventListener("change", () => void savePreferences(patch()));
  }

  function bindAppearanceAndTerminal(): void {
    elements.settingsTheme.addEventListener("change", () =>
      saveChoice(elements.settingsTheme, themePreferences, (theme) => ({ appearance: { theme } })),
    );
    elements.settingsDensity.addEventListener("change", () =>
      saveChoice(elements.settingsDensity, densityPreferences, (density) => ({ appearance: { density } })),
    );
    elements.settingsReducedMotion.addEventListener("change", () =>
      saveChoice(elements.settingsReducedMotion, reducedMotionPreferences, (reducedMotion) => ({ appearance: { reducedMotion } })),
    );
    saveOnChange(elements.settingsTerminalProfile, () => ({
      terminal: { defaultProfileId: elements.settingsTerminalProfile.value || null },
    }));
    saveOnChange(elements.settingsTerminalFontFamily, () => ({
      terminal: { fontFamily: elements.settingsTerminalFontFamily.value },
    }));
    saveOnChange(elements.settingsTerminalFontSize, () => ({
      terminal: { fontSize: Number(elements.settingsTerminalFontSize.value) },
    }));
    elements.settingsTerminalUnicode.addEventListener("change", () => {
      const unicodeMode = preferenceChoice(elements.settingsTerminalUnicode.value, unicodeModePreferences);
      if (unicodeMode === null) return;
      void savePreferences({ terminal: { unicodeMode } });
      // Settings is the only Unicode control; an open dock follows it into its
      // live panes.
      ctx.state.terminalHandle?.setModernUnicode(unicodeMode === "modern");
    });
    saveOnChange(elements.settingsTerminalScrollback, () => ({
      terminal: { scrollback: Number(elements.settingsTerminalScrollback.value) },
    }));
    elements.settingsTerminalRenderer.addEventListener("change", () =>
      saveChoice(elements.settingsTerminalRenderer, rendererPreferences, (renderer) => ({ terminal: { renderer } })),
    );
  }

  function bindNotificationsAndData(): void {
    saveOnChange(elements.settingsStartupRestore, () => ({
      startup: { restoreLastProject: elements.settingsStartupRestore.checked },
    }));
    saveOnChange(elements.settingsNotificationsHandoff, () => ({
      notifications: { handoffArrival: elements.settingsNotificationsHandoff.checked },
    }));
    saveOnChange(elements.settingsNotificationsFailureDrift, () => ({
      notifications: { runFailureOrDrift: elements.settingsNotificationsFailureDrift.checked },
    }));
    saveOnChange(elements.settingsNotificationsCompletion, () => ({
      notifications: { runCompletion: elements.settingsNotificationsCompletion.checked },
    }));
    elements.settingsUpdatesAutomatic.addEventListener("change", () => {
      void ctx.updates.setAutomaticUpdateChecks(elements.settingsUpdatesAutomatic.checked);
    });
    elements.settingsResetWindowLayout.addEventListener("click", () => {
      void resetWindowLayout(elements.settingsResetWindowLayout);
    });
    elements.settingsResetApplicationState.addEventListener("click", () => {
      void resetApplicationState(elements.settingsResetApplicationState);
    });
    elements.settingsOpenUpdates.addEventListener("click", () => {
      const invoker = elements.settingsOpen;
      closeSettings();
      ctx.updates.openAboutUpdates(invoker);
    });
    elements.settingsReset.addEventListener("click", () =>
      void resetPreferences(elements.settingsReset),
    );
  }

  function bindDialog(): void {
    themeButtons.forEach((button) => button.addEventListener("click", () => {
      // The topbar toggle is the same setting as Appearance ▸ Color theme, so it
      // writes through to the stored record instead of only the cache.
      const theme = preferenceChoice(themeController.toggle(), themePreferences);
      if (theme) void savePreferences({ appearance: { theme } });
    }));
    element("#landing-help-open", HTMLButtonElement).addEventListener("click", () => openHelpDestination("help-center"));
    elements.landingSettingsOpen.addEventListener("click", () => openSettings(elements.landingSettingsOpen));
    elements.settingsOpen.addEventListener("click", () => {
      openSettings(elements.settingsOpen);
    });
    elements.settingsClose.addEventListener("click", closeSettings);
    document.querySelectorAll("[data-close-settings]").forEach((closer) => {
      closer.addEventListener("click", closeSettings);
    });
    elements.settingsSectionList.addEventListener("click", (event) => {
      const tab = event.target instanceof Element ? event.target.closest('[role="tab"]') : null;
      const section = tab ? settingsSectionFromTab(tab.id) : null;
      if (section) selectSettingsSection(section);
    });
    elements.settingsSectionList.addEventListener("keydown", (event) => {
      const next = nextSettingsSectionIndex(
        event.key,
        settingsSectionIndex(settingsSection),
        settingsSections.length,
      );
      if (next < 0) return;
      event.preventDefault();
      selectSettingsSection(settingsSections[next].id, true);
    });
  }

  function bind(): void {
    bindDialog();
    bindNotificationsAndData();
    bindAppearanceAndTerminal();
  }

  return {
    bind,
    loadPreferences,
    savePreferences,
    openSettings,
    closeSettings,
    themeController,
  };
}

export type SettingsController = ReturnType<typeof createSettingsController>;
