// Boots the real window controllers against index.html in the fake DOM with a
// scripted desktop backend, so tests drive the UI the way the WebView does:
// clicks, typing, and backend replies, then read the DOM back.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { vi } from "vitest";

import { fire, installFakeDom } from "./fake-dom";

const html = readFileSync(resolve(import.meta.dirname, "../../index.html"), "utf8");

export const projectRoot = "/projects/alpha";

/** A workspace-state reply for an open project. */
export function openState(generation = 3, root = projectRoot) {
  return { status: "open", generation, version: "1.2.3", project: { root, name: "Alpha" } };
}

export function task(id, status = "todo", fields = {}) {
  return {
    id,
    title: `Task ${id}`,
    status,
    updatedAt: "2026-09-20T10:00:00Z",
    noteCount: 0,
    commitCount: 0,
    issueCount: 0,
    latestNote: "",
    ...fields,
  };
}

export function plan(id, fields = {}) {
  return {
    id,
    title: `Plan ${id}`,
    status: "active",
    isActive: false,
    tasksTotal: 0,
    tasksDone: 0,
    ...fields,
  };
}

/** A board with the given plans and the tasks of the shown plan. */
export function board({ planId = 1, plans = [plan(1, { isActive: true })], tasks = [], ...fields } = {}) {
  const lanes = [["todo", "Todo"], ["doing", "Doing"], ["blocked", "Blocked"], ["done", "Done"]];
  const shown = plans.find((candidate) => candidate.id === planId);
  return {
    projectName: "Alpha",
    goal: "Ship the tracker",
    summary: "",
    summaryUpdatedAt: null,
    plans,
    planId,
    planTitle: shown?.title ?? "",
    columns: lanes.map(([status, title]) => ({
      status,
      title,
      tasks: tasks.filter((entry) => entry.status === status),
    })),
    stats: {
      planTasks: tasks.length,
      planTasksDone: tasks.filter((entry) => entry.status === "done").length,
      tasksOpen: tasks.filter((entry) => entry.status !== "done").length,
      tasksBlocked: tasks.filter((entry) => entry.status === "blocked").length,
      notes: 0,
      commits: 0,
      openIssues: 0,
      tasks: tasks.length,
      tasksDone: tasks.filter((entry) => entry.status === "done").length,
      plans: plans.length,
      plansDone: plans.filter((entry) => entry.status === "done").length,
      milestones: 0,
      milestonesDone: 0,
    },
    activity: [],
    openIssues: [],
    ...fields,
  };
}

/** A GetWorkspaceSnapshot reply around `boardValue`. */
export function snapshot(boardValue = board(), generation = 3) {
  return {
    generation,
    capturedAt: "2026-09-22T12:00:00Z",
    project: {
      name: "Alpha",
      root: projectRoot,
      storage: { exists: true, formatVersion: 7, sizeBytes: 2048, lastWriteVersion: "1.2.3" },
    },
    tracking: { state: "ready", board: boardValue, blockers: [], notes: [], bounds: {} },
    git: { state: "ready", snapshot: { state: "ready", status: { branch: "main", upstream: "origin/main" } } },
    agentActivity: { items: [], bounds: { shown: 0, total: 0 } },
    drift: { findings: [] },
  };
}

function clone(value) {
  return value === undefined ? undefined : structuredClone(value);
}

/**
 * A scripted backend. `responses[method]` is a value (cloned per call), a
 * function of the call's arguments, or an Error to reject with. Unscripted
 * methods reject, so a test notices a call it did not expect.
 */
export function fakeBackend(responses) {
  const calls = [];
  const api = new Proxy({}, {
    get(_, method) {
      // Not a thenable: code that awaits the backend object must get it back.
      if (typeof method !== "string" || method === "then") return undefined;
      return async (...args) => {
        calls.push([method, args]);
        const response = responses[method];
        if (response === undefined) throw new Error(`unexpected ${method}`);
        if (response instanceof Error) throw response;
        return typeof response === "function" ? response(...args) : clone(response);
      };
    },
  });
  return {
    api,
    calls,
    responses,
    names: () => calls.map(([method]) => method),
    callsTo: (method) => calls.filter(([name]) => name === method).map(([, args]) => args),
  };
}

/** Replies every window boot needs; tests override what they exercise. */
export function baseResponses() {
  return {
    GetPreferences: { preferences: {}, storage: "ok" },
    GetLayoutState: { storage: "defaults" },
    GetWorkspaceState: { status: "welcome", generation: 0, version: "1.2.3" },
    GetPendingInitializationV1: { pending: false },
    GetRecentProjectsV1: { projects: [] },
    GetGlobalOverviewV1: { trackedProjects: 0, summarizedProjects: 0, counts: {}, projects: [] },
    GetStackProfileV1: { state: "unavailable" },
    GetTerminalProfilesV2: (generation) => ({
      generation,
      profiles: [{ id: "zsh", name: "zsh", kind: "shell" }],
    }),
    GetScratchpadV1: (generation) => ({ generation, scratchpad: { revision: 0, text: "", snippets: [] } }),
    GetUpdateState: { revision: 1, phase: "idle", currentVersion: "1.2.3" },
    OpenHelpDestination: () => undefined,
  };
}

/**
 * Holds the window's delayed timers (debounces, status clears) so a test runs
 * them on demand; zero-delay turns still run, so the harness keeps settling.
 */
export function holdTimers(harness) {
  const held = [];
  const { window } = harness.dom;
  const run = window.setTimeout;
  const cancel = window.clearTimeout;
  let nextHandle = 1_000_000;
  window.setTimeout = (callback, delay) => {
    if (!delay) return run(callback, delay);
    // A cleared timer leaves the list, as it would never fire.
    nextHandle += 1;
    const timer = { callback, delay, handle: nextHandle };
    held.push(timer);
    return timer.handle;
  };
  window.clearTimeout = (handle) => {
    const index = held.findIndex((timer) => timer.handle === handle);
    if (index >= 0) held.splice(index, 1);
    else cancel(handle);
  };
  return held;
}

/** Lets pending promises, timers at 0 ms, and animation frames run. */
export async function settle(dom, rounds = 12) {
  for (let round = 0; round < rounds; round += 1) {
    await new Promise((done) => setTimeout(done, 0));
    dom.flushFrames();
  }
}

/**
 * Boots the window. `responses` extend the base replies; `open` boots straight
 * into an open project whose snapshots are `responses.GetWorkspaceSnapshot`,
 * or else `open.snapshot`.
 */
export async function bootApp({ responses = {}, open = null, hash = "", beforeStart = null } = {}) {
  vi.resetModules();
  const dom = installFakeDom(html);
  dom.window.location.hash = hash;
  const scripted = { ...baseResponses(), ...responses };
  if (open) {
    scripted.GetWorkspaceState = openState(open.generation ?? 3);
    scripted.GetWorkspaceSnapshot = responses.GetWorkspaceSnapshot ??
      open.snapshot ?? snapshot(board(), open.generation ?? 3);
  }
  const backend = fakeBackend(scripted);
  dom.window.go = { gui: { App: backend.api } };
  // The desktop runtime beside the commands: native events the test emits,
  // and a clipboard that records what the window copied.
  const listeners = new Map();
  const copied = [];
  dom.window.runtime = {
    EventsOnMultiple(name, callback) {
      const entries = listeners.get(name) ?? new Set();
      entries.add(callback);
      listeners.set(name, entries);
      return () => entries.delete(callback);
    },
    ClipboardSetText: async (text) => {
      copied.push(text);
      return true;
    },
    ClipboardGetText: async () => copied.at(-1) ?? "",
    BrowserOpenURL: async () => {},
  };
  beforeStart?.(dom);
  const main = await import("../main.js");
  await main.started;
  await settle(dom);
  const { document } = dom;
  const $ = (selector) => document.querySelector(selector);
  return {
    dom,
    document,
    backend,
    app: main.app,
    $,
    $$: (selector) => document.querySelectorAll(selector),
    settle: (rounds) => settle(dom, rounds),
    click: async (target) => {
      const node = typeof target === "string" ? $(target) : target;
      node.click();
      await settle(dom);
    },
    type: async (target, value, eventType = "input") => {
      const node = typeof target === "string" ? $(target) : target;
      node.value = value;
      fire(node, eventType);
      await settle(dom);
    },
    submit: async (target) => {
      const node = typeof target === "string" ? $(target) : target;
      fire(node, "submit");
      await settle(dom);
    },
    key: async (target, key, init = {}) => {
      const node = typeof target === "string" ? $(target) : target;
      const event = fire(node, "keydown", { key, ...init });
      await settle(dom);
      return event;
    },
    toast: () => ($("#toast").hidden ? "" : $("#toast").textContent),
    copied,
    /** Delivers a native runtime event, as the desktop shell would. */
    emit: async (name, payload) => {
      for (const callback of listeners.get(name) ?? []) callback(payload);
      await settle(dom);
    },
  };
}
