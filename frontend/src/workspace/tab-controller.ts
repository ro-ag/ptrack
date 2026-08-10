import {
  createWorkspace,
  isWorkspace,
  normalizeWorkspace,
  type IdFactory,
  type Workspace,
  type WorkspaceIdKind,
} from "./model";
import { reduceWorkspace, type WorkspaceAction } from "./reducer";

export type WorkspaceTabListener = (
  workspace: Workspace,
  previous: Workspace,
) => void;

export interface WorkspaceTabControllerOptions {
  interceptAction?: (
    action: WorkspaceAction,
    workspace: Workspace,
  ) => WorkspaceAction | null;
  allowAction?: (action: WorkspaceAction, workspace: Workspace) => boolean;
}

export function createCryptoIdFactory(
  source: Pick<Crypto, "randomUUID"> = globalThis.crypto,
): IdFactory {
  if (!source || typeof source.randomUUID !== "function") {
    throw new Error("Secure workspace id generation is unavailable");
  }
  return {
    next(kind: WorkspaceIdKind): string {
      return `${kind}-${source.randomUUID()}`;
    },
  };
}

export class WorkspaceTabController {
  readonly #ids: IdFactory;
  readonly #listeners = new Set<WorkspaceTabListener>();
  readonly #options: WorkspaceTabControllerOptions;
  #workspace: Workspace;
  #disposed = false;

  constructor(
    ids: IdFactory,
    initialWorkspace?: Workspace,
    options: WorkspaceTabControllerOptions = {},
  ) {
    this.#ids = ids;
    this.#options = options;
    if (initialWorkspace === undefined) {
      this.#workspace = createWorkspace(ids);
    } else {
      this.#workspace = isWorkspace(initialWorkspace)
        ? initialWorkspace
        : normalizeWorkspace(initialWorkspace, ids);
    }
  }

  get workspace(): Workspace {
    return this.#workspace;
  }

  get state(): Workspace {
    return this.#workspace;
  }

  #intentFor(action: WorkspaceAction): WorkspaceAction | null {
    const intent = this.#options.interceptAction
      ? this.#options.interceptAction(action, this.#workspace)
      : action;
    if (!intent || this.#options.allowAction?.(intent, this.#workspace) === false) {
      return null;
    }
    return intent;
  }

  canDispatch(action: WorkspaceAction): boolean {
    return !this.#disposed && this.#intentFor(action) !== null;
  }

  dispatch(action: WorkspaceAction): Workspace | null {
    if (this.#disposed) return null;
    const previous = this.#workspace;
    const intent = this.#intentFor(action);
    if (!intent) return null;
    const next = reduceWorkspace(previous, intent, this.#ids);
    if (next === previous) return null;
    this.#workspace = next;
    for (const listener of [...this.#listeners]) listener(next, previous);
    return next;
  }

  replace(workspace: Workspace): Workspace | null {
    if (this.#disposed || workspace === this.#workspace || !isWorkspace(workspace)) {
      return null;
    }
    const previous = this.#workspace;
    this.#workspace = workspace;
    for (const listener of [...this.#listeners]) listener(workspace, previous);
    return workspace;
  }

  subscribe(listener: WorkspaceTabListener): () => void {
    if (this.#disposed) return () => {};
    this.#listeners.add(listener);
    let subscribed = true;
    return () => {
      if (!subscribed) return;
      subscribed = false;
      this.#listeners.delete(listener);
    };
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#listeners.clear();
  }
}
