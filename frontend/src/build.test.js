import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { build } from "vite";

const frontendRoot = resolve(import.meta.dirname, "..");
const distRoot = resolve(frontendRoot, "dist");

describe("production asset layout", () => {
  beforeAll(async () => {
    await build({
      configFile: resolve(frontendRoot, "vite.config.ts"),
      root: frontendRoot,
      logLevel: "silent",
    });
  });

  it("emits the embedded board assets with stable names", () => {
    const indexPath = resolve(distRoot, "index.html");

    expect(existsSync(indexPath)).toBe(true);
    expect(existsSync(resolve(distRoot, "app.js"))).toBe(true);
    expect(existsSync(resolve(distRoot, "style.css"))).toBe(true);

    const index = readFileSync(indexPath, "utf8");
    const app = readFileSync(resolve(distRoot, "app.js"), "utf8");
    const styles = readFileSync(resolve(distRoot, "style.css"), "utf8");
    const paneSource = readFileSync(
      resolve(frontendRoot, "src/terminal/pane.ts"),
      "utf8",
    );
    const appSource = readFileSync(
      resolve(frontendRoot, "src/app.js"),
      "utf8",
    );
    const firstRunSource = readFileSync(
      resolve(frontendRoot, "src/workspace/first-run.ts"),
      "utf8",
    );
    const firstRunJourneySource = readFileSync(
      resolve(frontendRoot, "src/workspace/first-run-journey.ts"),
      "utf8",
    );
    const firstPlanSource = readFileSync(
      resolve(frontendRoot, "src/workspace/first-plan.ts"),
      "utf8",
    );
    const recentProjectsSource = readFileSync(
      resolve(frontendRoot, "src/workspace/recent-projects.ts"),
      "utf8",
    );
    const applicationOverlaySource = readFileSync(
      resolve(frontendRoot, "src/workspace/application-overlay.ts"),
      "utf8",
    );
    expect(index).toContain('src="/app.js"');
    expect(index).toContain('href="/style.css"');
    expect(index).toMatch(
      /id="app-version"[^>]*tabindex="0"[^>]*aria-haspopup="dialog"[^>]*>dev<\/button>/,
    );
    expect(styles).toMatch(
      /\.app-version\{[^}]*position:relative[^}]*z-index:1[^}]*--wails-draggable:\s*no-drag/,
    );
    expect(styles).toMatch(/\.state-card\{[^}]*box-shadow:/);
    expect(styles).not.toMatch(
      /\.state-card\{[^}]*(?:animation|transform|opacity):/,
    );
    expect(index).toMatch(
      /id="project-state-card"[\s\S]*id="workspace-state-heading"[^>]*>Start with a project<\/h2>[\s\S]*Initialize p-track in a folder, or open a project you already use\.[\s\S]*id="state-initialize-project-button"[^>]*>Initialize Project<\/button>[\s\S]*id="state-open-project-button"[\s\S]*>Open Project…<\/button>[\s\S]*>Recent projects<\/p>/,
    );
    expect(index.match(/class="state-card"/g)).toHaveLength(1);
    expect(index).toMatch(
      /id="post-project-onboarding"[\s\S]*aria-labelledby="onboarding-heading"[\s\S]*id="onboarding-plan-form"[\s\S]*id="onboarding-create-plan"[^>]*>Create Plan<\/button>[\s\S]*id="onboarding-skip-plan"[^>]*>Skip for Now<\/button>[\s\S]*id="onboarding-task-form"[\s\S]*id="onboarding-start-now"[\s\S]*>Start this task now<[\s\S]*id="onboarding-create-task"[^>]*>Create Task<\/button>[\s\S]*id="onboarding-finish-with-plan"[^>]*>Finish with Plan<\/button>/,
    );
    expect(index).toMatch(
      /id="recent-project-heading"[\s\S]*tabindex="-1"[\s\S]*>Recent projects<\/p>[\s\S]*id="recent-project-list"[\s\S]*role="list"[\s\S]*aria-busy="false"[\s\S]*id="recent-project-status"[\s\S]*role="status"[\s\S]*aria-live="polite"[\s\S]*id="recent-project-error"[\s\S]*role="alert"/,
    );
    expect(index).not.toMatch(/id="workspace-state-screen"[^>]*aria-live/);
    for (const id of [
      "recent-project-error",
      "setup-goal-error",
      "setup-error",
      "onboarding-plan-error",
      "onboarding-task-error",
      "onboarding-error",
    ]) {
      expect(index).toMatch(
        new RegExp(`id="${id}"[^>]*role="alert"[^>]*aria-atomic="true"`),
      );
    }
    expect(index).toMatch(
      /id="setup-operation"[^>]*role="group"[^>]*aria-labelledby="setup-heading"[^>]*aria-busy="false"/,
    );
    expect(index).toMatch(
      /id="onboarding-operation"[^>]*role="group"[^>]*aria-labelledby="onboarding-heading"[^>]*aria-busy="false"/,
    );
    expect(index).toMatch(
      /id="onboarding-start-failed-actions"[\s\S]*id="onboarding-retry-start"[^>]*>Try Starting Again<\/button>[\s\S]*id="onboarding-finish-setup"[^>]*>Finish Setup<\/button>/,
    );
    expect(index).toMatch(
      /id="setup-panel"[\s\S]*aria-labelledby="setup-heading"[\s\S]*id="setup-progress"[\s\S]*id="setup-heading"[^>]*tabindex="-1"[\s\S]*id="setup-goal"[\s\S]*aria-describedby="setup-goal-help setup-goal-error"[\s\S]*id="setup-status"[\s\S]*role="status"[\s\S]*aria-live="polite"[\s\S]*id="setup-error"[^>]*role="alert"/,
    );
    expect(index).toMatch(
      /id="setup-goal-back"[^>]*>Back<\/button>/,
    );
    expect(index).toMatch(
      /id="setup-new-target-actions"[\s\S]*id="setup-new-target-continue"[^>]*>Continue to Goal<\/button>[\s\S]*id="setup-new-target-choose"[^>]*>Choose Another Folder<\/button>[\s\S]*id="setup-new-target-cancel"[^>]*>Cancel Setup<\/button>/,
    );
    expect(index).toMatch(
      /id="setup-guide"[\s\S]*aria-label="Project guide choice"[\s\S]*Skip Guide[\s\S]*id="setup-guide-preview"[\s\S]*aria-label="Exact guide file preview"[\s\S]*id="setup-guide-preview-button"[^>]*>Preview Guide Changes<\/button>[\s\S]*id="setup-guide-install"[^>]*>Install These Guide Changes<\/button>/,
    );
    expect(index).toMatch(
      /id="setup-guide-stale-actions"[\s\S]*id="setup-guide-review-again"[^>]*>Review Again<\/button>[\s\S]*id="setup-guide-stale-skip"[^>]*>Skip Guide<\/button>[\s\S]*id="setup-guide-stale-back"[^>]*>Back<\/button>/,
    );
    expect(index).toMatch(
      /id="setup-review"[\s\S]*id="setup-review-goal"[\s\S]*id="setup-review-guide-choice"[\s\S]*id="setup-complete-changes"/,
    );
    expect(index).toContain("Private p-track project storage");
    expect(index).toMatch(
      /id="setup-recovery-actions"[\s\S]*id="setup-retry"[^>]*hidden[^>]*>Try Again<\/button>[\s\S]*id="setup-resume"[^>]*>Resume Setup<\/button>[\s\S]*id="setup-open-recovery"[^>]*>Open Project<\/button>[\s\S]*id="setup-recovery-help"[^>]*>Open Recovery Help<\/button>[\s\S]*id="setup-recovery-choose"[\s\S]*id="setup-return-welcome"/,
    );
    expect(index).toMatch(
      /id="setup-uncertain-actions"[\s\S]*id="setup-check-status"[^>]*>Check Status Again<\/button>/,
    );
    expect(appSource).not.toContain(".ptrack/ptrack.redb");
    expect(firstRunJourneySource).toContain("api.ValidateProjectTargetV1(root)");
    expect(firstRunJourneySource).toContain("api.InitializeProjectV1(request)");
    expect(appSource).toContain("validateInitializationTarget(api(), path)");
    expect(appSource).toContain("commitInitialization(api(), request)");
    expect(appSource).toContain("runExactProjectOpen(");
    expect(appSource).toMatch(
      /resumeInitialization\(\s*api\(\),\s*operationId,\s*canonicalRoot,?\s*\)/,
    );
    expect(appSource).toContain("PreviewProjectGuideV1({ operationId, root: canonicalRoot })");
    expect(firstRunJourneySource).toContain("guideChoice: guide.guideChoice");
    expect(firstRunJourneySource).toContain(
      "guidePreviewToken: guide.guidePreviewToken",
    );
    expect(firstRunSource).toContain("project-guide-partially-applied");
    expect(firstRunSource).toContain(
      'const resumeFields = ["initialization", "goal", "guideChoice"]',
    );
    expect(firstRunSource).toContain('state.checkpoint === "guide-applied"');
    expect(appSource).toContain("if (validation.resume)");
    expect(appSource).toContain("goal: validation.resume.goal");
    expect(appSource).toContain("firstRunState.storageAlreadyCreated");
    expect(appSource).toContain("firstRunState.resumeLocked");
    expect(appSource).toContain(
      "No project files were written. You can try again safely.",
    );
    expect(firstRunSource).toContain('event.initialization.outcome === "in-progress"');
    expect(firstPlanSource).toContain("parseCreateFirstPlanResult");
    expect(firstPlanSource).toContain('task.status === "todo" || task.status === "doing"');
    expect(firstPlanSource).toContain("api.CreateFirstPlanV1(generation, title)");
    expect(firstPlanSource).toContain(
      "api.CreateFirstTaskV1(generation, planId, title)",
    );
    expect(firstPlanSource).toContain(
      "api.StartFirstTaskV1(generation, taskId, expectedUpdatedAt)",
    );
    expect(appSource).toContain("await createFirstPlan(");
    expect(appSource).toContain("await createFirstTask(");
    expect(appSource).toContain("await runStartFirstTask(");
    expect(appSource).toContain("workspaceController.accepts(ticket, generation)");
    expect(appSource).toContain('firstPlanState.phase !== "idle"');
    expect(appSource).toMatch(
      /function selectPlan\(planId\) \{\s*if \(firstPlanState\.phase !== "idle"\) return;/,
    );
    expect(appSource).toMatch(
      /function openPalette\(\) \{\s*if \([\s\S]*workspaceController\.state\.status !== "open" \|\|[\s\S]*firstPlanState\.phase !== "idle"[\s\S]*\) return;/,
    );
    expect(appSource).toContain("elements.planList.inert = active");
    expect(appSource).toContain("elements.sidebarToggle.disabled = active");
    expect(appSource).toContain("elements.sidebarResize.inert = active");
    expect(appSource).toContain("terminalHandle.setLayoutLocked(active)");
    expect(appSource).toContain(
      'handle.setLayoutLocked(firstPlanState.phase !== "idle")',
    );
    expect(appSource).toContain(
      "firstPlanState = { ...initialFirstPlanState };\n  renderFirstPlanOnboarding(false);",
    );
    expect(paneSource).toContain("setLayoutLocked(locked: boolean)");
    expect(paneSource).toContain(
      "this.#boardToggle.disabled = this.#layoutLocked || !dockInteractionEligible",
    );
    expect(appSource).toContain(
      "firstPlanExitFocusTarget(planId, sidebarHeadingUnavailableForFocus())",
    );
    expect(appSource).toContain("await loadSnapshot(planId > 0 ? planId : 0)");
    expect(appSource).toMatch(
      /applyView\(\);\s*document\.getElementById\(\s*firstPlanExitFocusTarget\(planId, sidebarHeadingUnavailableForFocus\(\)\),\s*\)\?\.focus\(\);\s*await loadSnapshot\(planId > 0 \? planId : 0\);/,
    );
    expect(appSource).toContain("Saving or reconciling the first plan…");
    expect(appSource).toContain("Saving or reconciling the first task…");
    expect(appSource).toContain("p-track is reconciling the requested start.");
    expect(appSource).not.toContain("Creating the first task in Todo…");
    expect(appSource).not.toContain("safely stored in Todo while p-track starts it");
    expect(recentProjectsSource).toMatch(
      /export type RecentProjectAvailability\s*=\s*\| "available"\s*\| "missing"\s*\| "permission-required"\s*\| "changed"/,
    );
    expect(recentProjectsSource).toContain("Recent projects exceeded the 20-entry limit.");
    expect(recentProjectsSource).toContain("Recent projects were not newest first.");
    expect(appSource).toContain("GetRecentProjectsV1");
    expect(appSource).toContain("ResolveRecentProjectV1");
    expect(appSource).toContain("OpenRecentProjectV1");
    expect(appSource).toContain("ForgetRecentProjectV1");
    expect(appSource).toContain("recentProjectOperationIsCurrent(ticket)");
    expect(appSource).toContain("function recentProjectOperationActive()");
    expect(appSource).toContain("elements.stateInitialize.disabled = operationActive");
    expect(appSource).toContain("elements.stateOpen.disabled = operationActive");
    expect(appSource).toMatch(
      /async function requestOpenProject\([^)]*\) \{\s*if \(recentProjectOperationActive\(\)\) return;/,
    );
    expect(appSource).toMatch(
      /async function requestInitializeProject\([^)]*\) \{\s*if \(recentProjectOperationActive\(\)\) return;/,
    );
    expect(appSource).toMatch(
      /function nativeCommandAllowed\(command\) \{[\s\S]*recentProjectOperationActive\(\)/,
    );
    expect(appSource).toContain(
      "button.disabled = recentProjectsState.listLoading ||",
    );
    expect(appSource).toMatch(
      /function beginRecentProjectOperation\(entry, intent\) \{[\s\S]*recentProjectsState\.listLoading \|\|[\s\S]*!\["idle", "error"\]\.includes\(recentProjectsState\.phase\)[\s\S]*return null;/,
    );
    expect(appSource).toContain(
      "operationSequence !== recentOperationSequence",
    );
    expect(appSource).toContain('warnings.join(" ")');
    expect(appSource).toContain("Folder not found");
    expect(appSource).toContain("Permission required");
    expect(appSource).toContain("Project changed");
    expect(appSource).toContain("Project files will not be changed.");
    expect(appSource).toContain("registryStatus === \"stale\"");
    expect(appSource).toContain("await refreshRecentProjectsAfterOpen()");
    expect(appSource).toContain("RECENT_RELOCATION_UNCONFIRMED");
    expect(recentProjectsSource).toContain("bounded registry list");
    expect(appSource).toMatch(
      /CancelWorkspaceChange\(result\.open\.confirmationToken\)[\s\S]*setRecentProjectsState\(\{[\s\S]*type: "settled"[\s\S]*renderWorkspaceState\(result\.open\.state, false\)[\s\S]*restoreRecentProjectFocus/,
    );
    expect(appSource).not.toContain("projects.filter((project) => project.available)");
    expect(appSource).not.toContain("The first plan was not created");
    expect(appSource).not.toContain("The first task was not created");
    expect(appSource).not.toContain("The task remains in Todo");
    expect(appSource).toContain(
      "elements.setupGuideStaleSkip.hidden = !firstRunState.guideSkipAllowed",
    );
    expect(appSource).toContain("skipAllowed: false");
    expect(appSource).toContain("code.textContent = file.diff");
    expect(appSource).not.toContain("code.innerHTML = file.diff");
    expect(appSource).toContain("element.inert = !visible");
    expect(appSource).not.toMatch(/toggleAttribute\(\s*["']aria-/);
    expect(appSource).toMatch(
      /setAriaBoolean\(\s*elements\.setupOperation,\s*"aria-busy"/,
    );
    expect(appSource).toMatch(
      /setAriaBoolean\(\s*elements\.onboardingOperation,\s*"aria-busy"/,
    );
    expect(appSource).toContain('nativeCommandAllowed("checkForUpdates")');
    expect(appSource).toMatch(
      /function openAboutUpdates[\s\S]*?firstRunState\.phase !== "idle"[\s\S]*?firstPlanState\.phase !== "idle"[\s\S]*?recentProjectOperationActive\(\)/,
    );
    expect(appSource).toMatch(
      /function updateAboutUpdatesAvailability\(\) \{[\s\S]*?elements\.appVersion\.disabled = firstRunState\.phase !== "idle" \|\|[\s\S]*?firstPlanState\.phase !== "idle" \|\|[\s\S]*?recentProjectOperationActive\(\)/,
    );
    expect(appSource.match(/updateAboutUpdatesAvailability\(\);/g)?.length)
      .toBeGreaterThanOrEqual(3);
    expect(appSource).toContain('lastOpened.dateTime = project.lastOpenedAt');
    expect(appSource).toContain('item.setAttribute("aria-labelledby", name.id)');
    // The relocated last project is pointed at, never opened: the row is
    // marked, says so in text, and no focus moves to it.
    expect(recentProjectsSource).toContain(
      "export function preselectedRecentProject(",
    );
    expect(appSource).toContain(
      "preselectedRecentProject(projects, preferences.startup)",
    );
    expect(appSource).toMatch(
      /if \(project\.entryId === preselectedEntryId\) \{\n      item\.setAttribute\("aria-current", "true"\);/,
    );
    expect(appSource).toContain(
      'preselect.textContent = "Preselected — last project p-track recorded"',
    );
    // The row's own text carries the state, so the description names it too,
    // and the existing polite status line announces it without taking focus.
    expect(appSource).toContain("descriptionIDs.push(preselect.id)");
    expect(appSource).toContain("if (startupChanged) renderRecentProjects();");
    expect(appSource).toMatch(
      /elements\.recentStatus\.textContent = recentProjectsState\.announcement \|\|\n {4}\(preselected\n {6}\? `“\$\{preselected\.name\}” is preselected as the last project/,
    );
    expect(appSource).not.toMatch(
      /preselect[^\n]*\.focus\(\)/,
    );
    expect(styles).toMatch(
      /\.recent-project\[aria-current=(?:"true"|true)\]\{[^}]*border-color:var\(--accent\)/,
    );
    // Highlight is only legal under forced colors, so the pair pins both.
    expect(styles).toMatch(
      /\.recent-project\[aria-current=(?:"true"|true)\]\{[^}]*border-color:Highlight/,
    );
    expect(appSource).toContain('button.setAttribute("aria-describedby", describedBy)');
    expect(appSource).toContain('status.checkpoint !== "desktop-bound"');
    expect(firstRunJourneySource).toContain(
      "api.GetInitializationStatusV1(operationId)",
    );
    expect(appSource).toContain("GetPendingInitializationV1()");
    expect(firstRunSource).toContain("parsePendingInitialization");
    expect(appSource).toContain("resolveFirstRunStartupState");
    expect(appSource).toContain("hydratePendingInitialization(pending)");
    expect(appSource).toContain("elements.openProject.disabled = !idle");
    expect(appSource).toContain("elements.closeProject.disabled = !idle");
    expect(appSource).toContain('firstRunState.phase === "idle"');
    expect(appSource).toContain('elements.setupRetry.addEventListener("click", retryFirstRunValidation)');
    expect(appSource).toContain("retryInitializationStatus");
    expect(appSource).toContain('openHelpDestination("project-recovery")');
    expect(appSource).toContain("resumeFirstRunSetup");
    expect(appSource).toContain("openProjectFromRecovery");
    expect(appSource).toContain("rebindCompletedInitializationWorkspace");
    expect(appSource).toContain(
      "completedInitializationWorkspaceMatches(workspace, canonicalRoot)",
    );
    expect(appSource).toContain(
      "Initialization is complete, but this window could not open the project:",
    );
    expect(appSource).toContain('firstRunState.recoveryMode === "durable"');
    expect(firstRunSource).toContain(
      '["project-committed", "guide-applied", "desktop-bound"]',
    );
    expect(appSource).toContain("showCommittedGuideRecoveryActions");
    expect(appSource).toMatch(
      /case "review":[\s\S]*setFirstRunSectionVisible\(elements\.setupReview, true\);\s*showCommittedGuideRecoveryActions\(\);/,
    );
    expect(appSource).toMatch(
      /async function openProjectFromRecovery\(\) \{[\s\S]*canOpenPreservedFirstRunProject\(firstRunState\)/,
    );
    expect(firstRunSource).toContain(
      '["recovery", "guide", "guide-stale", "review"]',
    );
    expect(appSource).toContain("resumable: false");
    expect(appSource).toContain(
      "Could not load the desktop startup state:",
    );
    expect(appSource).toMatch(
      /function cancelFirstRunSetup\(\) \{[\s\S]*firstRunState\.resumeLocked[\s\S]*firstRunState\.recoveryMode === "durable"[\s\S]*"committing", "reconciling", "uncertain"/,
    );
    expect(appSource).toContain(
      'elements.setupGoalBack.addEventListener("click", returnToSelectedFirstRunFolder)',
    );
    expect(appSource).toContain(
      'elements.setupGoal.addEventListener("input", preserveFirstRunGoalDraft)',
    );
    expect(appSource).toContain('type: "goalDrafted"');
    expect(appSource).toContain('type: "continueToGoal"');
    expect(appSource).toContain("pickerCancelState = { ...firstRunState }");
    expect(appSource).toContain('type: pickerCancelState ? "repick" : "pick"');
    expect(appSource).toContain("elements.setupNewTargetChoose");
    expect(appSource).toMatch(
      /type: "pickerCancelled", restore: pickerCancelState[\s\S]*requestAnimationFrame\(\(\) => returnFocus\?\.focus\(\)\)/,
    );
    expect(index).toMatch(
      /id="updates-modal"[\s\S]*role="dialog"[\s\S]*aria-modal="true"[\s\S]*id="updates-automatic"[\s\S]*aria-label="Update download progress"[\s\S]*id="updates-primary"/,
    );
    expect(index).toContain("Automatic checks never download or install anything");
    expect(app).toContain("GetUpdateState");
    expect(app).toContain("CheckForUpdates");
    expect(app).toContain("DownloadUpdate");
    expect(app).toContain("ApplyUpdate");
    expect(app).toContain("CancelUpdateOperation");
    expect(app).toContain("update:state-changed");
    expect(app).not.toContain("releases/download");
    expect(app).not.toContain("checksums.txt");
    expect(index).toMatch(/id="terminal-tabs"[\s\S]*role="tablist"/);
    expect(index).not.toMatch(/id="terminal-tabs"[^>]*aria-live/);
    expect(index).toMatch(
      /<div[^>]*id="terminal-tabs"[^>]*role="tablist"[^>]*>\s*<\/div>/,
    );
    expect(index).toMatch(
      /<div[^>]*id="terminal-tab-actions"[^>]*role="toolbar"[^>]*aria-label="Active terminal tab actions"[^>]*>\s*<\/div>/,
    );
    expect(index).not.toMatch(/id="terminal-body"[^>]*role=/);
    expect(app).toContain("terminal-tab-panel-");
    expect(app).toContain("tabpanel");
    expect(app).toContain("aria-controls");
    expect(app).toContain("aria-labelledby");
    expect(app).toContain("setVisible");
    expect(app).toContain("setApplicationOverlayOpen");
    expect(app).toContain("body > .modal, body > [data-terminal-overlay]");
    expect(app).toContain("MutationObserver");
    expect(app).toContain("subtree:!0");
    expect(index).toContain('id="terminal-cwd"');
    expect(index).toContain('id="terminal-reset-workspace"');
    expect(index).toMatch(
      /id="terminal-termination-modal"[\s\S]*aria-modal="true"[\s\S]*>Terminate<\/button>/,
    );
    expect(index).toMatch(
      /id="terminal-paste-form"[\s\S]*aria-modal="true"[\s\S]*aria-describedby="terminal-paste-detail"/,
    );
    expect(index).toMatch(
      /id="terminal-link-context"[\s\S]*aria-label="Link terminal context"[\s\S]*disabled/,
    );
    expect(index).toMatch(
      /class="terminal-actions"[\s\S]*aria-label="Terminal actions"[\s\S]*id="terminal-open"[\s\S]*aria-label="Open terminal"[\s\S]*<svg/,
    );
    expect(index).toMatch(
      /id="terminal-help"[\s\S]*aria-label="Open terminal guide"/,
    );
    expect(index).toMatch(
      /class="capabilities-heading-actions"[\s\S]*id="capability-help"[\s\S]*>Capability guide<\/button>/,
    );
    expect(app).toContain("OpenHelpDestination");
    expect(appSource).toContain('openHelpDestination("terminals")');
    expect(appSource).toContain('openHelpDestination("capabilities")');
    expect(appSource).not.toContain("ro-ag.github.io/ptrack/help");
    expect(index).toMatch(
      /id="terminal-close"[\s\S]*class="terminal-action-button terminal-action-stop"[\s\S]*aria-label="Close terminal"/,
    );
    expect(index).toMatch(
      /id="terminal-diagnostics-toggle"[\s\S]*aria-controls="terminal-diagnostics"[\s\S]*aria-expanded="false"[\s\S]*<svg[^>]*aria-hidden="true"/,
    );
    expect(index).toMatch(
      /id="terminal-renderer-retry"[\s\S]*aria-label="Retry terminal renderer"[\s\S]*<svg[^>]*aria-hidden="true"/,
    );
    expect(index).toMatch(
      /id="terminal-force-stop"[\s\S]*class="terminal-action-button terminal-action-stop"[\s\S]*aria-label="Force stop terminal"[\s\S]*<svg[^>]*aria-hidden="true"/,
    );
    expect(index).toMatch(
      /id="terminal-diagnostics"[\s\S]*aria-live="polite"[\s\S]*Content-free state only\. Restart creates a fresh session; streams are never reconnected\./,
    );
    expect(styles).toMatch(
      /\.terminal-diagnostics\s*\{[\s\S]*max-height:[^;]+;[\s\S]*overflow-y:\s*auto/,
    );
    expect(index).toMatch(
      /id="terminal-association-modal"[\s\S]*role="dialog"[\s\S]*aria-modal="true"[\s\S]*id="terminal-association-target"[\s\S]*>Detach<\/button>/,
    );
    expect(index).toMatch(
      /id="terminal-writeback"[\s\S]*id="terminal-writeback-modal"[\s\S]*role="dialog"[\s\S]*aria-modal="true"[\s\S]*id="terminal-writeback-kind"[\s\S]*id="terminal-writeback-content"[\s\S]*id="terminal-writeback-preview"[\s\S]*id="terminal-writeback-summary-confirm"/,
    );
    expect(index).toMatch(
      /id="task-transition-modal"[\s\S]*role="alertdialog"[\s\S]*aria-modal="true"[\s\S]*id="task-transition-detail"[\s\S]*id="task-transition-cancel"[\s\S]*id="task-transition-submit"/,
    );
    expect(app).toContain("MoveTaskV3");
    expect(appSource).not.toContain("api().MoveTaskV2");
    expect(app).toContain("linked sessions, processes, and capabilities stay unchanged");
    expect(app).toContain("Finish the current task status change before starting another.");
    expect(app).toContain("Stale task transition response ignored");
    expect(app).toContain("Stale terminal association response ignored");
    expect(app).toContain("Linking changes context only and grants no capabilities.");
    expect(index).not.toMatch(/id="agent-activity"[^>]*aria-live/);
    expect(index).toMatch(
      /id="agent-activity-live"[^>]*role="status"[^>]*aria-live="polite"[^>]*aria-atomic="true"/,
    );
    expect(app).toContain("mutationFocusKey");
    expect(styles).toContain(".terminal-tab-indicator");
    expect(styles).toMatch(/\[data-indicator=(?:"failed"|failed)\]/);
    expect(styles).toMatch(/\[data-unread=(?:"true"|true)\]/);
    expect(styles).toContain(".terminal-split-node");
    expect(styles).toMatch(
      /\.terminal-split-leaf\{[^}]*width:100%[^}]*height:100%/,
    );
    expect(styles).toContain("touch-action:none");
    expect(styles).toMatch(
      /data-state=(?:"closed"|closed)\]\[data-layout-interactive=(?:"false"|false)\]/,
    );
    expect(styles).toMatch(/data-board-hidden=(?:"true"|true)\] \.terminal-dock\{height:100%/);
    expect(styles).toMatch(/data-terminal-hidden=(?:"true"|true)\] \.terminal-dock\{display:none/);
    expect(styles).toMatch(
      /\.board-heading\{[^}]*min-width:0[^}]*flex-wrap:wrap/,
    );
    expect(styles).toMatch(
      /\.title-row h2\{[^}]*min-width:0[^}]*flex:1 1 auto[^}]*text-overflow:ellipsis/,
    );
    expect(styles).toMatch(
      /\.board-actions\{[^}]*min-width:0[^}]*flex-wrap:wrap[^}]*justify-content:flex-end/,
    );
    expect(styles).toMatch(
      /\.add-form input\{[^}]*min-width:120px[^}]*flex:1 1 170px/,
    );
    expect(styles).toMatch(
      /@media\(max-width:960px\)\{[^}]*#app[^}]*\}[^}]*\.board-heading[^}]*\}\.plan-context,\.board-actions\{width:100%;min-width:0;max-width:100%;flex:0 0 100%\}\.board-actions\{justify-content:flex-start\}\.add-form\{min-width:0;flex-basis:250px\}/,
    );
    expect(styles).toMatch(
      /\.panel-toggle:focus-visible,[^{]*\.terminal-context-menu button:focus-visible\{[^}]*outline:2px solid var\(--accent\)[^}]*outline-offset:-2px/,
    );
    expect(paneSource).toMatch(
      /action === "zoom-reset"\) \{\s*this\.#setFontSize\(this\.#activeProfileDefaultFontSize\(\)\)/,
    );
    expect(paneSource).toMatch(
      /setApplicationOverlayOpen\(open: boolean, focusTerminal: false\): void \{[\s\S]*?#renderPanelVisibility\(focusTerminal\)/,
    );
    expect(paneSource).toContain("revision !== this.#panelVisibilityRevision");
    expect(paneSource).toContain("webglRecoveryPaused");
    expect(paneSource).toContain("webglRecoveryAfterSuppression");
    expect(paneSource).toContain("webglRecoveryPolicyAction");
    expect(applicationOverlaySource).toContain("class ApplicationOverlayCoordinator");
    expect(applicationOverlaySource).toContain("this.#lastOpen !== open");
    expect(applicationOverlaySource).toContain("setApplicationOverlayOpen(open, false)");
    expect(applicationOverlaySource).toContain('setAttribute("aria-hidden", "true")');
    expect(applicationOverlaySource).toContain("this.#background.inert = true");
    expect(applicationOverlaySource).toContain("get activeOverlay()");
    expect(applicationOverlaySource).toContain('"data-application-overlay-layer", "active"');
    expect(applicationOverlaySource).toContain('"data-application-overlay-layer", "underlay"');
    expect(appSource).toContain("attributeOldValue: true");
    expect(appSource).toContain(
      "const modal = applicationOverlayCoordinator.activeOverlay",
    );
    expect(appSource).toContain("applicationOverlayKeyboardPolicy(");
    expect(appSource).toContain("if (!policy.trapTab) return");
    expect(appSource).toContain("closeActiveApplicationOverlay(event)");
    expect(appSource).toContain("event.stopImmediatePropagation()");
    expect(appSource).not.toMatch(
      /event\.key === "Escape" && !elements\.[A-Za-z]+\.hidden/,
    );
    expect(paneSource).toMatch(
      /!this\.#pasteModal\.hidden && event\.key === "Tab"[\s\S]*this\.#trapPasteFocus\(event\)/,
    );
    expect(paneSource).toMatch(
      /const dismissOnKey = \(event: KeyboardEvent\) => \{[\s\S]*if \(event\.defaultPrevented\) return;[\s\S]*!this\.#pasteModal\.hidden && event\.key === "Tab"[\s\S]*!this\.#pasteModal\.hidden[\s\S]*this\.#finishPasteConfirmation\(false\)/,
    );
    expect(paneSource).toMatch(
      /this\.#terminationModal, "keydown"[\s\S]*keyEvent\.key === "Tab"[\s\S]*this\.#trapTerminationFocus\(keyEvent\)/,
    );
    expect(paneSource).toMatch(
      /#trapPasteFocus[\s\S]*focusCycleIndex\(focusable\.length, current, event\.shiftKey\)/,
    );
    expect(paneSource).toMatch(
      /#trapTerminationFocus[\s\S]*focusCycleIndex\(focusable\.length, current, event\.shiftKey\)/,
    );
    expect(styles).toMatch(/data-application-overlay-layer=(?:active|"active")/);
    expect(styles).toMatch(/data-application-overlay-layer=(?:underlay|"underlay")/);
    expect(index).toMatch(
      /<main[^>]*id="main-content"[^>]*class="canvas-main"[^>]*aria-label="Workspace content"[\s\S]*id="capabilities-page"[^>]*aria-labelledby="capabilities-heading"[\s\S]*id="capabilities-heading"[^>]*tabindex="-1"/,
    );
    expect(index).toMatch(
      /<main[^>]*id="main-content"[^>]*class="canvas-main"[\s\S]*id="overview-page"[^>]*aria-label="Project overview"/,
    );
    expect(index).toMatch(
      /class="section-label">Rolling project summary<\/p>[\s\S]*id="summary"[^>]*>No rolling summary yet\.<\/p>/,
    );
    expect(appSource).toContain(
      "No rolling summary yet. Agents can update it with ptrack summary set.",
    );
    expect(index).not.toContain("No rolling handoff yet.");
    expect(styles).toMatch(
      /\.canvas-main\s*\{[^}]*min-width:\s*0[^}]*min-height:\s*0[^}]*flex:\s*1 1 auto[^}]*display:\s*flex[^}]*flex-direction:\s*column/s,
    );
    expect(index).toMatch(
      /id="capability-status"[^>]*role="status"[^>]*aria-live="polite"[^>]*aria-atomic="true"/,
    );
    expect(index).toMatch(
      /class="memory-section capability-editor"[^>]*aria-labelledby="capability-editor-title"[\s\S]*id="capability-preview-result"[^>]*aria-labelledby="capability-preview-heading"/,
    );
    expect(index).toMatch(
      /class="memory-section capability-list-section"[^>]*aria-labelledby="capability-list-heading"[\s\S]*id="capability-audit-list"[^>]*aria-labelledby="capability-audit-heading"/,
    );
    expect(index).toMatch(
      /id="capability-git-fields"[^>]*hidden[^>]*disabled[\s\S]*id="capability-ssh-fields"[^>]*hidden[^>]*disabled/,
    );
    expect(appSource).toContain("setCapabilityStatus(action, phase)");
    expect(appSource).toContain("showCapabilityError(action)");
    expect(appSource).toContain(
      'showError(new Error(capabilityAnnouncement(action, "failure")))',
    );
    for (const action of ["audit", "test", "preview"]) {
      expect(appSource).toContain(`showCapabilityError("${action}")`);
    }
    expect(appSource).toContain("capabilityFocusRestoreKey(");
    expect(appSource).toContain('empty.setAttribute("role", "listitem")');
    expect(appSource).toContain("fieldset.disabled = !active");
    expect(appSource).toContain('EnableCapabilityV2(generation, Number(view.capability.id), digest)');
    expect(appSource).toContain("canEnableCapability(view, digest)");
    expect(appSource).toContain('setView("capabilities", true)');
    expect(appSource).not.toContain('setStatus("Testing connection');
    expect(appSource).not.toContain('setView("settings"');
    expect(index).toMatch(
      /id="settings-open"[^>]*aria-label="Open Settings"[\s\S]*aria-haspopup="dialog"[\s\S]*aria-controls="settings-modal"/,
    );
    expect(index).toMatch(
      /id="settings-modal"[\s\S]*role="dialog"[\s\S]*aria-modal="true"[\s\S]*aria-labelledby="settings-dialog-heading"[\s\S]*id="settings-dialog-heading"[^>]*>Settings<\/h2>/,
    );
    expect(index).toMatch(
      /id="settings-section-list"[\s\S]*role="tablist"[\s\S]*aria-orientation="vertical"[\s\S]*aria-label="Settings sections"/,
    );
    for (const section of ["startup", "appearance", "terminal", "updates", "data"]) {
      expect(index).toMatch(
        new RegExp(
          `id="settings-tab-${section}"[\\s\\S]*?role="tab"[\\s\\S]*?aria-controls="settings-panel-${section}"`,
        ),
      );
      expect(index).toMatch(
        new RegExp(
          `id="settings-panel-${section}"[\\s\\S]*?role="tabpanel"[\\s\\S]*?aria-labelledby="settings-tab-${section}"[\\s\\S]*?tabindex="0"`,
        ),
      );
    }
    // The roving tabindex is an attribute of the tab itself, so the match must
    // not cross the tag boundary onto a panel or a later tab.
    expect(index).toMatch(/id="settings-tab-startup"[^>]*\stabindex="0"/);
    expect(index).toMatch(/id="settings-tab-appearance"[^>]*\stabindex="-1"/);
    expect(index).toMatch(/id="settings-tab-terminal"[^>]*\stabindex="-1"/);
    // Startup is an opt-in checkbox whose copy states the "still valid" rule.
    expect(index).toMatch(
      /id="settings-startup-restore"[^<>]*type="checkbox"[^<>]*\/>/,
    );
    expect(index).toContain("Reopen the last project when p-track starts");
    expect(index).toContain("The last project reopens only while it is still");
    // Both resets live in Data & Diagnostics, described by the copy that says
    // what they spare, and nowhere near the native menu.
    expect(index).toMatch(
      /id="settings-panel-data"[\s\S]*id="settings-reset-help"[\s\S]*class="settings-reset-actions"[\s\S]*id="settings-reset-window-layout"[^<>]*aria-describedby="settings-reset-help"[^<>]*>Reset Window Layout<\/button>[\s\S]*id="settings-reset-application-state"[^<>]*aria-describedby="settings-reset-help"[^<>]*>Reset Application State<\/button>/,
    );
    // Revoking a grant writes into the open project, so the copy names that
    // instead of promising the project database is untouched.
    expect(index).toContain(
      "Neither reset touches plans, tasks, notes, or Recent projects.",
    );
    expect(index).toMatch(
      /id="settings-reset-help"[^<>]*>[^<]*revokes every network capability grant in the\s+open project/,
    );
    expect(styles).toMatch(/\.settings-reset-actions\{[^}]*flex-wrap:wrap/);
    expect(styles).toMatch(
      /\.settings-reset-actions button\{border-color:CanvasText\}/,
    );
    // The save-status live region sits outside the aria-busy wrapper.
    expect(index).toMatch(
      /id="settings-body"[\s\S]*<\/div>\s*<p\s*id="settings-save-status"[\s\S]*role="status"[\s\S]*aria-live="polite"[\s\S]*aria-atomic="true"/,
    );
    expect(index).toMatch(
      /id="settings-terminal-font-size"[\s\S]*min="10"[\s\S]*max="24"[\s\S]*aria-describedby="settings-terminal-font-size-help"/,
    );
    expect(index).toMatch(
      /id="settings-terminal-scrollback"[\s\S]*min="1000"[\s\S]*max="200000"[\s\S]*aria-describedby="settings-terminal-scrollback-help"/,
    );
    expect(index).toContain("Checks are opt-in. Downloads and installations always stay manual.");
    expect(index).toMatch(
      /id="about-identity-heading"[\s\S]*id="about-version"[\s\S]*id="about-build"[\s\S]*id="about-license"[^>]*>Apache-2\.0<\/dd>/,
    );
    expect(index).toMatch(
      /id="about-project"[\s\S]*id="about-license-link"[\s\S]*id="about-help"[\s\S]*id="about-report"/,
    );
    expect(index.match(/id="updates-primary"/g)).toHaveLength(1);
    expect(app).toContain("GetPreferences");
    expect(app).toContain("SetPreferences");
    expect(app).toContain("ResetPreferences");
    expect(app).toContain("GetDiagnosticsReport");
    expect(appSource).toContain("preferencesResponse(await api().GetPreferences())");
    expect(appSource).toContain("preferencesResponse(await api().SetPreferences(patch))");
    expect(appSource).toContain("preferencesResponse(await api().ResetPreferences())");
    expect(appSource).toContain("await api().GetDiagnosticsReport()");
    expect(appSource).toContain("applyPreferenceMirrors(localStorage, next)");
    expect(app).toContain("GetLayoutState");
    expect(app).toContain("SetLayoutState");
    expect(app).toContain("ResetWindowLayout");
    expect(app).toContain("ResetApplicationState");
    expect(appSource).toContain("normalizeLayoutState(await api().GetLayoutState())");
    expect(appSource).toContain(
      'layoutStatePatch(layoutState, workspaceState.project?.root || "")',
    );
    expect(appSource).toContain(
      "applyLayoutState(normalizeLayoutState(await api().ResetWindowLayout()))",
    );
    expect(appSource).toContain("await api().ResetApplicationState()");
    expect(appSource).toContain("resetApplicationStateMessage(result)");
    // Panel changes are recorded from the user's click, never from the
    // attribute the dock also writes on its own.
    expect(appSource).toContain(
      'elements.panelControls.addEventListener("click", recordPanelLayout)',
    );
    expect(appSource).not.toMatch(/MutationObserver\(recordPanelLayout\)/);
    // The guard covers the synthetic restore clicks and nothing else: it is
    // cleared once the restore attempt ends, so a dock that refused the
    // restore still records the user's own later gestures.
    expect(appSource).toMatch(
      /if \(\(toggle\.getAttribute\("aria-pressed"\) === "true"\) !== hidden\) toggle\.click\(\);\n  \}\n(?:  \/\/[^\n]*\n)*  panelLayoutRestored = true;/,
    );
    expect(appSource).not.toMatch(/panelLayoutRestored = Boolean\(/);
    // The eviction counter is backend-owned, so no patch can carry it.
    expect(appSource).not.toContain("usedAt");
    // Layout writes share the existing scheduler rather than adding a second.
    expect(appSource).toContain("new WorkspacePersistenceScheduler(");
    expect(appSource).toContain("layoutStateScheduler.markDirty()");
    expect(appSource).toContain("layoutStateScheduler.flush()");
    expect(appSource).toMatch(
      /savePreferences\(\{\s*startup: \{ restoreLastProject: event\.currentTarget\.checked \}/,
    );
    // The window is Rust-owned: the frontend never asks for its geometry.
    expect(appSource).not.toContain("WindowState");
    // A runtime that never answered is stated plainly instead of looking healthy.
    expect(appSource).toContain('renderSettingsStorageNotice("unavailable")');
    // The terminal dock toggle writes through the stored record, never the mirror.
    expect(appSource).toContain(
      "saveUnicodeMode: (unicodeMode) => void savePreferences({ terminal: { unicodeMode } })",
    );
    expect(paneSource).toContain(
      'this.#saveUnicodeMode(enabled ? "modern" : "legacy")',
    );
    expect(paneSource).not.toContain("writeModernUnicodeSetting");
    expect(appSource).toContain("themeController.setTheme(next.appearance.theme)");
    expect(appSource).toContain("root.dataset.density = next.appearance.density");
    expect(appSource).toContain(
      "void savePreferences({ appearance: { theme: themeController.toggle() } })",
    );
    // Updates stay a single source of truth on the existing command.
    expect(appSource.match(/api\(\)\.SetAutomaticUpdateChecks\(/g)).toHaveLength(1);
    expect(appSource).toContain("await loadPreferences();");
    expect(appSource).toContain('escapeAction === "settings"');
    expect(appSource).toContain("nextSettingsSectionIndex(");
    expect(appSource).toContain('openSettings(elements.settingsOpen)');
    expect(appSource).toContain('openHelpDestination("help-center")');
    expect(appSource).toContain('openHelpDestination("report-issue")');
    expect(paneSource).toContain("readTerminalPreferenceOverrides(localStorage)");
    expect(paneSource).toContain("webglPreferredByPreference(");
    expect(styles).toMatch(
      /:root\[data-density=(?:"compact"|compact)\]\{[^}]*--space-100:\s*6px/,
    );
    expect(styles).toMatch(
      /:root\[data-reduced-motion=(?:"always"|always)\][^{]*\{[^}]*animation-duration:\.01ms!important/,
    );
    expect(styles).toMatch(
      /prefers-reduced-motion:reduce\)\{:root:not\(\[data-reduced-motion=(?:"never"|never)\]\)/,
    );
    expect(styles).toMatch(
      /\.settings-section-tab\[aria-selected=(?:"true"|true)\]\{[^}]*border-color:var\(--control-border\)/,
    );
  });
});
