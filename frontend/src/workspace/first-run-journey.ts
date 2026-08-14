import {
  isProjectGuidePartiallyApplied,
  isProjectGuidePreviewStale,
  parseInitializationStatus,
  parseInitializeProjectResult,
  parseProjectTargetValidation,
  type InitializationStatus,
  type InitializeProjectResult,
  type ProjectTargetValidation,
} from "./first-run";

export interface InitializeProjectRequestV1 {
  operationId: string;
  root: string;
  goal: string;
  guideChoice: "skip" | "install";
  guidePreviewToken: string;
}

export interface FirstRunJourneyApi {
  ValidateProjectTargetV1(root: string): Promise<unknown>;
  InitializeProjectV1(request: InitializeProjectRequestV1): Promise<unknown>;
  GetInitializationStatusV1(operationId: string): Promise<unknown>;
  OpenProject(root: string, confirmationToken: string): Promise<unknown>;
  CancelWorkspaceChange(confirmationToken: string): Promise<unknown>;
}

export function initializeProjectRequest(
  operationId: string,
  root: string,
  goal: string,
  guide: { guideChoice: "skip" | "install"; guidePreviewToken: string },
): InitializeProjectRequestV1 {
  return {
    operationId,
    root,
    goal,
    guideChoice: guide.guideChoice,
    guidePreviewToken: guide.guidePreviewToken,
  };
}

export async function validateInitializationTarget(
  api: Pick<FirstRunJourneyApi, "ValidateProjectTargetV1">,
  root: string,
): Promise<ProjectTargetValidation> {
  return parseProjectTargetValidation(await api.ValidateProjectTargetV1(root));
}

export async function readInitializationStatus(
  api: Pick<FirstRunJourneyApi, "GetInitializationStatusV1">,
  operationId: string,
): Promise<InitializationStatus> {
  return parseInitializationStatus(
    await api.GetInitializationStatusV1(operationId),
  );
}

export type CommitInitializationOutcome =
  | { kind: "result"; result: InitializeProjectResult }
  | { kind: "status"; error: unknown; status: InitializationStatus }
  | { kind: "uncertain"; error: unknown; statusError: unknown };

export async function commitInitialization(
  api: Pick<
    FirstRunJourneyApi,
    "InitializeProjectV1" | "GetInitializationStatusV1"
  >,
  request: InitializeProjectRequestV1,
): Promise<CommitInitializationOutcome> {
  try {
    return {
      kind: "result",
      result: parseInitializeProjectResult(
        await api.InitializeProjectV1(request),
      ),
    };
  } catch (error) {
    try {
      return {
        kind: "status",
        error,
        status: await readInitializationStatus(api, request.operationId),
      };
    } catch (statusError) {
      return { kind: "uncertain", error, statusError };
    }
  }
}

export type ResumeInitializationOutcome =
  | { kind: "status"; status: InitializationStatus }
  | { kind: "validation"; validation: ProjectTargetValidation };

export async function resumeInitialization(
  api: Pick<
    FirstRunJourneyApi,
    "GetInitializationStatusV1" | "ValidateProjectTargetV1"
  >,
  operationId: string,
  root: string,
): Promise<ResumeInitializationOutcome> {
  if (operationId) {
    const status = await readInitializationStatus(api, operationId);
    if (
      status.outcome === "complete" ||
      isProjectGuidePartiallyApplied(status.errorKind) ||
      isProjectGuidePreviewStale(status.errorKind)
    ) {
      return { kind: "status", status };
    }
  }
  return {
    kind: "validation",
    validation: await validateInitializationTarget(api, root),
  };
}

interface WorkspaceChangeResult {
  state: Record<string, unknown>;
  requiresConfirmation: boolean;
  confirmationToken?: string;
  activeResources?: Record<string, unknown>;
  warning?: unknown;
}

export type ExactOpenDecision = "confirm" | "cancel" | "abort";

export type ExactOpenOutcome =
  | { kind: "opened"; result: WorkspaceChangeResult }
  | { kind: "cancelled"; result: WorkspaceChangeResult }
  | { kind: "aborted"; result: WorkspaceChangeResult };

const maximumWorkspaceConfirmationRounds = 3;

function workspaceConfirmationToken(result: WorkspaceChangeResult): string {
  if (
    typeof result.confirmationToken !== "string" ||
    result.confirmationToken.length === 0
  ) {
    throw new Error("workspace confirmation response is missing its token");
  }
  return result.confirmationToken;
}

async function abandonWorkspaceConfirmation(
  api: Pick<FirstRunJourneyApi, "CancelWorkspaceChange">,
  confirmationToken: string,
): Promise<void> {
  try {
    await api.CancelWorkspaceChange(confirmationToken);
  } catch {
    // A rejected UI transition no longer owns the confirmation. The backend
    // fence expires independently, so cancellation here is deliberately best effort.
  }
}

export async function openExactProject(
  api: Pick<FirstRunJourneyApi, "OpenProject" | "CancelWorkspaceChange">,
  root: string,
  decide: (result: WorkspaceChangeResult) => Promise<ExactOpenDecision>,
  beforeConfirmedOpen: () => void,
): Promise<ExactOpenOutcome> {
  let result = await api.OpenProject(root, "") as WorkspaceChangeResult;
  for (
    let confirmationRound = 0;
    result.requiresConfirmation;
    confirmationRound += 1
  ) {
    const confirmationToken = workspaceConfirmationToken(result);
    if (confirmationRound >= maximumWorkspaceConfirmationRounds) {
      await abandonWorkspaceConfirmation(api, confirmationToken);
      throw new Error("workspace confirmation changed too many times; try again");
    }
    const decision = await decide(result);
    if (decision === "abort") {
      await abandonWorkspaceConfirmation(api, confirmationToken);
      return { kind: "aborted", result };
    }
    if (decision === "cancel") {
      await api.CancelWorkspaceChange(confirmationToken);
      return { kind: "cancelled", result };
    }
    beforeConfirmedOpen();
    result = await api.OpenProject(
      root,
      confirmationToken,
    ) as WorkspaceChangeResult;
  }
  return { kind: "opened", result };
}
