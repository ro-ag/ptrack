import type { AssociationPointerV1 } from "../workspace/model";

export interface DiscoveredTerminalProfile {
  id: string;
  name: string;
  kind: "shell" | "agent";
  provider?: string;
  theme?: string;
  fontFamily?: string;
  fontSize?: number;
  scrollback?: number;
  cwdPolicy?: "requested" | "project" | "fixed";
  exitBehavior?: "keep" | "close-on-success" | "close";
}

export interface InstalledAgentProfile {
  id: string;
  name: string;
  kind: "agent";
}

export interface LinkedLaunchRequest {
  profileId: string;
  title: string;
  cwd?: string;
  association: AssociationPointerV1;
}

// The dock flushes its last committed workspace before entering a linked-tab
// stage. Persistence callbacks remain suppressed until that stage is released,
// so an unattached descriptor can never reach project storage.
export class LinkedLaunchPersistenceStage {
  #active = 0;

  get suppressed(): boolean {
    return this.#active > 0;
  }

  begin(flushCommitted: () => void): () => void {
    flushCommitted();
    this.#active += 1;
    let released = false;
    return () => {
      if (released) return;
      released = true;
      this.#active = Math.max(0, this.#active - 1);
    };
  }
}

export function persistUnlessLinkedLaunchStaged(
  stage: LinkedLaunchPersistenceStage,
  persist: () => void,
): boolean {
  if (stage.suppressed) return false;
  persist();
  return true;
}

export function installedAgentProfiles(
  profiles: readonly DiscoveredTerminalProfile[],
): InstalledAgentProfile[] {
  return profiles
    .filter((profile): profile is InstalledAgentProfile =>
      profile.kind === "agent" && profile.id.length > 0 && profile.name.length > 0
    )
    .map((profile) => ({ ...profile }));
}

export function selectedInstalledAgentProfile(
  profiles: readonly InstalledAgentProfile[],
  profileId: string,
): InstalledAgentProfile {
  const selected = profiles.find((profile) => profile.id === profileId);
  if (!selected) throw new Error("Select an installed agent profile");
  return selected;
}

export function linkedAssociationPointer(
  planId: number,
  taskId?: number,
): AssociationPointerV1 {
  if (!Number.isSafeInteger(planId) || planId <= 0) {
    throw new Error("A linked launch requires a selected plan");
  }
  if (taskId !== undefined && (!Number.isSafeInteger(taskId) || taskId <= 0)) {
    throw new Error("A linked task launch requires a valid task");
  }
  return {
    version: 1,
    planId,
    ...(taskId === undefined ? {} : { taskId }),
  };
}

interface LinkedLaunchTransactionOptions<TSession, TTab> {
  launch(): Promise<TSession>;
  createTab(session: TSession): TTab | null;
  attach(tab: TTab, session: TSession): Promise<void>;
  closeSession(session: TSession): Promise<void>;
  rollbackTab(tab: TTab): void;
}

// completeLinkedLaunchTransaction keeps live authority out of persistence:
// the backend session is created first, then the authority-free tab pointer is
// committed. Any tab or renderer failure force-closes the session and removes
// the staged descriptor.
export async function completeLinkedLaunchTransaction<TSession, TTab>(
  options: LinkedLaunchTransactionOptions<TSession, TTab>,
): Promise<{ session: TSession; tab: TTab }> {
  const session = await options.launch();
  let tab: TTab | null = null;
  try {
    tab = options.createTab(session);
    if (tab === null) throw new Error("Could not create a linked terminal tab");
    await options.attach(tab, session);
    return { session, tab };
  } catch (error) {
    try {
      await options.closeSession(session);
    } catch (closeError) {
      throw new Error(
        `${error instanceof Error ? error.message : "Linked launch failed"}; cleanup failed`,
        { cause: closeError },
      );
    }
    if (tab !== null) {
      try {
        options.rollbackTab(tab);
      } catch (rollbackError) {
        throw new Error(
          `${error instanceof Error ? error.message : "Linked launch failed"}; tab rollback failed`,
          { cause: rollbackError },
        );
      }
    }
    throw error;
  }
}
