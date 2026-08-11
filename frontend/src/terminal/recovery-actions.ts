export async function restartTerminalRecovery(options: {
  linked: boolean;
  close(): Promise<void>;
  accepted(): boolean;
  open(): Promise<void>;
}): Promise<"restarted" | "stale"> {
  if (options.linked) {
    throw new Error("Linked agent tabs must be launched again from their plan or task");
  }
  await options.close();
  if (!options.accepted()) return "stale";
  await options.open();
  return "restarted";
}

export async function forceStopTerminalRecovery<Ticket>(options: {
  capture(): Ticket | null;
  currentSessionId(): string | null;
  closing(): boolean;
  confirm(): Promise<boolean>;
  accepted(ticket: Ticket): boolean;
  close(): Promise<void>;
}): Promise<"stopped" | "cancelled" | "stale"> {
  const ticket = options.capture();
  const sessionId = options.currentSessionId();
  if (!ticket || !sessionId || options.closing()) return "stale";
  if (!(await options.confirm())) return "cancelled";
  if (
    !options.accepted(ticket) ||
    options.currentSessionId() !== sessionId ||
    options.closing()
  ) return "stale";
  await options.close();
  return "stopped";
}

export function retryTerminalRendererRecovery<Ticket>(options: {
  allowed: boolean;
  capture(): Ticket | null;
  accepted(ticket: Ticket): boolean;
  reset(ticket: Ticket): void;
  refresh(ticket: Ticket): void;
  attach(ticket: Ticket): void;
  fit(ticket: Ticket): void;
  render(ticket: Ticket): void;
}): boolean {
  if (!options.allowed) return false;
  const ticket = options.capture();
  if (!ticket || !options.accepted(ticket)) return false;
  options.reset(ticket);
  options.refresh(ticket);
  options.attach(ticket);
  options.fit(ticket);
  if (options.accepted(ticket)) options.render(ticket);
  return true;
}

export async function resetTerminalWorkspaceRecovery<Replacement>(options: {
  confirm(): Promise<boolean>;
  close(): Promise<void>;
  accepted(): boolean;
  replace(): Replacement | null;
  clear(replacement: Replacement): void;
}): Promise<"reset" | "cancelled" | "stale" | "unchanged"> {
  if (!(await options.confirm())) return "cancelled";
  await options.close();
  if (!options.accepted()) return "stale";
  const replacement = options.replace();
  if (replacement === null) return "unchanged";
  options.clear(replacement);
  return "reset";
}
