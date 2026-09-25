import { createSettingsController } from "./settings/controller";
import { createAgentLaunchController } from "./terminal/agent-launch-controller";
import { createAssociationController } from "./terminal/association-controller";
import { createTerminalBackendController } from "./terminal/backend";
import { createTerminalWindow } from "./terminal/window";
import { createWritebackController } from "./terminal/writeback-controller";
import { createUpdatesController } from "./updates/controller";
import { createAgentActivityView } from "./workspace/agent-activity-view";
import { createAppContext, createAppCore } from "./workspace/app-context";
import { createBoardView } from "./workspace/board-view";
import { createFirstPlanView } from "./workspace/first-plan-view";
import { createFirstRunView } from "./workspace/first-run-view";
import { createIssuesView } from "./workspace/issues-view";
import { createLandingController } from "./workspace/landing";
import { createLayoutController } from "./workspace/layout-controller";
import { createNativeMenuController } from "./workspace/native-menu";
import { createOverviewView } from "./workspace/overview-view";
import { createPalette } from "./workspace/palette";
import { createPlanDialogs } from "./workspace/plan-dialogs";
import { createProjectLifecycle } from "./workspace/project-lifecycle";
import { createRecentProjectsController } from "./workspace/recent-projects-controller";
import { createShortcuts } from "./workspace/shortcuts";
import { createSnapshotController } from "./workspace/snapshot-controller";
import { createTaskDrawer } from "./workspace/task-drawer";
import { createWorkspaceShell } from "./workspace/workspace-shell";

const controllers = [
  ["snapshot", createSnapshotController],
  ["layout", createLayoutController],
  ["shell", createWorkspaceShell],
  ["overview", createOverviewView],
  ["issues", createIssuesView],
  ["agentActivity", createAgentActivityView],
  ["palette", createPalette],
  ["settings", createSettingsController],
  ["lifecycle", createProjectLifecycle],
  ["firstRun", createFirstRunView],
  ["firstPlan", createFirstPlanView],
  ["updates", createUpdatesController],
  ["terminalBackend", createTerminalBackendController],
  ["board", createBoardView],
  ["recent", createRecentProjectsController],
  ["drawer", createTaskDrawer],
  ["planDialogs", createPlanDialogs],
  ["agentLaunch", createAgentLaunchController],
  ["association", createAssociationController],
  ["writeback", createWritebackController],
  ["shortcuts", createShortcuts],
  ["landing", createLandingController],
  ["nativeMenu", createNativeMenuController],
  ["terminalWindow", createTerminalWindow],
];

export function createApp() {
  const { ctx, install } = createAppContext(createAppCore());
  for (const [key, create] of controllers) install(key, create(ctx));
  for (const [key] of controllers) ctx[key].bind();
  return ctx;
}
