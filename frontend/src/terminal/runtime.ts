import {
  findTerminalPane,
  type TerminalPane,
  type Workspace,
} from "../workspace/model";
import { resetPaneActivity, type PaneActivity } from "./activity";

export type PaneRuntimeState = "closed" | "opening" | "running" | "exited" | "failed";

export interface PaneSession {
  sessionId: string;
}

export interface PaneRuntime<
  Session extends PaneSession = PaneSession,
  Resources = unknown,
> {
  readonly paneId: string;
  state: PaneRuntimeState;
  detail: string;
  title: string;
  session: Session | null;
  resources: Resources | null;
  epoch: number;
  closing: boolean;
  busy: boolean;
  closePromise: Promise<void> | null;
  closingSessionId: string | null;
  activity: PaneActivity;
}

export interface PaneRuntimeTicket {
  readonly paneId: string;
  readonly epoch: number;
}

export interface ActiveTerminalDescriptor {
  tabId: string;
  pane: TerminalPane;
}

export type PaneRuntimeLifecycleEvent =
  | { kind: "stream-open" }
  | { kind: "stream-error" }
  | { kind: "stream-closed" }
  | { kind: "process-exit"; failed: boolean; detail: string };

export interface PaneRuntimeTransition {
  state: PaneRuntimeState;
  detail: string;
}

export function paneRuntimeEventAccepted(input: {
  ticketAccepted: boolean;
  closing: boolean;
  sessionId: string | null;
  eventSessionId?: string;
}): boolean {
  return input.ticketAccepted &&
    !input.closing &&
    (input.eventSessionId === undefined || input.sessionId === input.eventSessionId);
}

export function paneRuntimeTransition(
  state: PaneRuntimeState,
  event: PaneRuntimeLifecycleEvent,
): PaneRuntimeTransition | null {
  if (event.kind === "process-exit") {
    return { state: event.failed ? "failed" : "exited", detail: event.detail };
  }
  if (event.kind === "stream-open") {
    return state === "opening" ? { state: "running", detail: "" } : null;
  }
  if (event.kind === "stream-error") {
    return state === "opening" || state === "running"
      ? { state: "failed", detail: "Terminal stream failed" }
      : null;
  }
  return state === "opening" || state === "running"
    ? { state: "failed", detail: "Terminal stream disconnected" }
    : null;
}

export function ensureStoppedWorkspaceRuntimes<
  Session extends PaneSession,
  Resources,
>(
  registry: PaneRuntimeRegistry<Session, Resources>,
  workspace: Workspace,
): void {
  for (const tab of workspace.tabs) {
    const visit = (node: Workspace["tabs"][number]["root"]): void => {
      if (node.kind === "terminal") {
        registry.ensure(node.paneId);
        return;
      }
      visit(node.first);
      visit(node.second);
    };
    visit(tab.root);
  }
}

export function earlyExitCacheLimit(
  openingRuntimeCount: number,
  maximumPaneCount: number,
): number {
  if (!Number.isFinite(openingRuntimeCount) ||
    !Number.isFinite(maximumPaneCount)) return 0;
  return Math.max(0, Math.min(
    Math.trunc(openingRuntimeCount),
    Math.trunc(maximumPaneCount),
  ));
}

export function activeTerminalDescriptor(
  workspace: Workspace,
): ActiveTerminalDescriptor | null {
  const tab = workspace.tabs.find((candidate) => candidate.id === workspace.activeTabId);
  if (!tab) return null;
  const pane = findTerminalPane(tab.root, tab.activePaneId);
  return pane ? { tabId: tab.id, pane } : null;
}

export function runtimeBlocksDescriptorClose(
  runtime:
    | Pick<PaneRuntime, "state" | "session" | "busy" | "closing">
    | null
    | undefined,
): boolean {
  return Boolean(
    runtime &&
      (runtime.state === "opening" ||
        runtime.state === "running" ||
        runtime.busy ||
        runtime.closing ||
        runtime.session),
  );
}

export function runtimeDescriptorEditable(
  runtime:
    | Pick<PaneRuntime, "state" | "session" | "busy" | "closing">
    | null
    | undefined,
): boolean {
  if (!runtime || runtime.busy || runtime.closing) return false;
  if (runtime.state === "closed" || runtime.state === "exited") return true;
  return runtime.state === "failed" && runtime.session === null;
}

export class PaneRuntimeRegistry<
  Session extends PaneSession = PaneSession,
  Resources = unknown,
> {
  readonly #runtimes = new Map<string, PaneRuntime<Session, Resources>>();

  ensure(paneId: string): PaneRuntime<Session, Resources> {
    if (paneId.trim().length === 0) throw new Error("paneId must be nonempty");
    let runtime = this.#runtimes.get(paneId);
    if (!runtime) {
      runtime = {
        paneId,
        state: "closed",
        detail: "",
        title: "",
        session: null,
        resources: null,
        epoch: 0,
        closing: false,
        busy: false,
        closePromise: null,
        closingSessionId: null,
        activity: resetPaneActivity(),
      };
      this.#runtimes.set(paneId, runtime);
    }
    return runtime;
  }

  get(paneId: string): PaneRuntime<Session, Resources> | null {
    return this.#runtimes.get(paneId) ?? null;
  }

  begin(paneId: string): PaneRuntimeTicket {
    const runtime = this.ensure(paneId);
    runtime.epoch += 1;
    return { paneId, epoch: runtime.epoch };
  }

  capture(paneId: string): PaneRuntimeTicket | null {
    const runtime = this.#runtimes.get(paneId);
    return runtime ? { paneId, epoch: runtime.epoch } : null;
  }

  accepts(ticket: PaneRuntimeTicket): boolean {
    return this.#runtimes.get(ticket.paneId)?.epoch === ticket.epoch;
  }

  runtimeFor(ticket: PaneRuntimeTicket): PaneRuntime<Session, Resources> | null {
    return this.accepts(ticket) ? this.get(ticket.paneId) : null;
  }

  invalidate(paneId: string): PaneRuntimeTicket | null {
    const runtime = this.#runtimes.get(paneId);
    if (!runtime) return null;
    runtime.epoch += 1;
    return { paneId, epoch: runtime.epoch };
  }

  rollbackInvalidation(
    runtime: PaneRuntime<Session, Resources>,
    invalidated: PaneRuntimeTicket,
    previous: PaneRuntimeTicket,
  ): boolean {
    const current = this.#runtimes.get(invalidated.paneId);
    if (
      current !== runtime ||
      invalidated.paneId !== previous.paneId ||
      current.epoch !== invalidated.epoch ||
      previous.epoch >= invalidated.epoch
    ) return false;
    current.epoch = previous.epoch;
    return true;
  }

  findBySessionId(sessionId: string): PaneRuntime<Session, Resources> | null {
    for (const runtime of this.#runtimes.values()) {
      if (runtime.session?.sessionId === sessionId) return runtime;
    }
    return null;
  }

  remove(paneId: string): PaneRuntime<Session, Resources> | null {
    const runtime = this.#runtimes.get(paneId);
    if (!runtime) return null;
    runtime.epoch += 1;
    this.#runtimes.delete(paneId);
    return runtime;
  }

  values(): PaneRuntime<Session, Resources>[] {
    return [...this.#runtimes.values()];
  }
}
