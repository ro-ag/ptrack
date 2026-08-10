import type { AssociationPointerV1 } from "../workspace/model";

export interface ActiveTerminalAssociation {
  generation: number;
  tabId: string;
  paneId: string;
  sessionId: string;
  revision: number;
  pointer?: AssociationPointerV1;
}

export interface TerminalAssociationMutationResult {
  generation: number;
  sessionId: string;
  revision: number;
  detached: boolean;
  pointer?: AssociationPointerV1 | null;
}

export interface TerminalAssociationMutationOptions {
  expected: ActiveTerminalAssociation;
  pointer?: AssociationPointerV1;
  current(): ActiveTerminalAssociation | null;
  mutate(
    sessionId: string,
    expectedRevision: number,
    pointer?: AssociationPointerV1,
  ): Promise<TerminalAssociationMutationResult>;
  commit(next: ActiveTerminalAssociation): void;
}

export function cloneAssociationPointer(
  pointer: AssociationPointerV1 | undefined,
): AssociationPointerV1 | undefined {
  if (pointer === undefined) return undefined;
  if (
    pointer.version !== 1 ||
    !positiveSafeID(pointer.planId) ||
    (pointer.taskId !== undefined && !positiveSafeID(pointer.taskId))
  ) {
    throw new Error("A valid plan or task association is required");
  }
  return {
    version: 1,
    planId: pointer.planId,
    ...(pointer.taskId === undefined ? {} : { taskId: pointer.taskId }),
  };
}

export function associationPointersEqual(
  left: AssociationPointerV1 | undefined,
  right: AssociationPointerV1 | undefined,
): boolean {
  return left?.version === right?.version &&
    left?.planId === right?.planId &&
    left?.taskId === right?.taskId;
}

export function activeAssociationsEqual(
  left: ActiveTerminalAssociation | null,
  right: ActiveTerminalAssociation | null,
): boolean {
  if (left === null || right === null) return left === right;
  return left.generation === right.generation &&
    left.tabId === right.tabId &&
    left.paneId === right.paneId &&
    left.sessionId === right.sessionId &&
    left.revision === right.revision &&
    associationPointersEqual(left.pointer, right.pointer);
}

// A detached linked launch has no persisted pointer, but remains a linked
// runtime for ordinary open/restart/split/profile restrictions until the tab
// is removed. The final flag is transient and never serialized.
export function terminalHasLinkedOrigin(
  pointer: AssociationPointerV1 | undefined,
  sessionLinkedLaunch: boolean,
  transientLinkedLaunch: boolean,
): boolean {
  return pointer !== undefined || sessionLinkedLaunch || transientLinkedLaunch;
}

export async function commitTerminalAssociationMutation(
  options: TerminalAssociationMutationOptions,
): Promise<ActiveTerminalAssociation> {
  const expected: ActiveTerminalAssociation = {
    ...options.expected,
    pointer: cloneAssociationPointer(options.expected.pointer),
  };
  if (!activeAssociationsEqual(options.current(), expected)) {
    throw new Error("The active terminal association changed before relinking");
  }
  const requested = cloneAssociationPointer(options.pointer);
  const result = await options.mutate(
    expected.sessionId,
    expected.revision,
    requested,
  );
  if (!activeAssociationsEqual(options.current(), expected)) {
    throw new Error("Stale terminal association response ignored");
  }
  const returnedPointer = result.pointer == null
    ? undefined
    : cloneAssociationPointer(result.pointer);
  const detached = requested === undefined;
  if (
    result.generation !== expected.generation ||
    result.sessionId !== expected.sessionId ||
    !Number.isSafeInteger(result.revision) ||
    result.revision !== expected.revision + 1 ||
    result.detached !== detached ||
    !associationPointersEqual(returnedPointer, requested)
  ) {
    throw new Error("Stale or invalid terminal association response ignored");
  }
  const next: ActiveTerminalAssociation = {
    ...expected,
    revision: result.revision,
    pointer: returnedPointer,
  };
  options.commit(next);
  return next;
}

function positiveSafeID(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}
