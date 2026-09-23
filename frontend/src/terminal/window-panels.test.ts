// A detached terminal window's info row, ⓘ diagnostics, and scratchpad panel,
// mounted over the window's own markup in index.html. The dock mounts the same
// diagnostics popover and scratchpad panel over its markup, which is also in
// the document, so the two surfaces can be driven side by side against one
// store the way the runtime serves them.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { fire, installFakeDom } from "../test-support/fake-dom";
import { terminalDiagnosticView } from "./diagnostics";
import { TerminalDiagnosticsPopover, type TerminalDiagnosticsElements } from "./diagnostics-popover";
import {
  detachedScratchpadStorage,
  scratchpadNotices,
  scratchpadOpenStorageKey,
  scratchpadStatus,
  type Scratchpad,
} from "./scratchpad";
import { ScratchpadPanel, type ScratchpadPanelElements } from "./scratchpad-panel";
import { secretCaptureNotice } from "./secrets";
import { initialShellState, type ShellState } from "./shell-integration";
import {
  detachedDiagnosticInput,
  TerminalWindowInfo,
  type DetachedPaneFacts,
} from "./window-info";

const html = readFileSync(resolve(import.meta.dirname, "../../index.html"), "utf8");

interface FakeDom {
  document: Document;
  window: Window & { localStorage: Storage };
  restore(): void;
}

let dom: FakeDom;
const $ = <T extends HTMLElement = HTMLElement>(selector: string): T => {
  const found = dom.document.querySelector<T>(selector);
  if (!found) throw new Error(`missing ${selector}`);
  return found;
};

beforeEach(() => {
  dom = installFakeDom(html) as unknown as FakeDom;
});
afterEach(() => dom.restore());

function windowScratchpadElements(): ScratchpadPanelElements {
  return {
    toggle: $("#terminal-window-scratchpad-toggle"),
    panel: $("#terminal-window-scratchpad"),
    splitter: $("#terminal-window-scratchpad-splitter"),
    state: $("#terminal-window-scratchpad-state"),
    close: $("#terminal-window-scratchpad-close"),
    text: $("#terminal-window-scratchpad-text"),
    add: $("#terminal-window-scratchpad-add"),
    list: $("#terminal-window-scratchpad-snippets"),
    empty: $("#terminal-window-scratchpad-empty"),
    body: $("#terminal-window-body"),
  };
}

function dockScratchpadElements(): ScratchpadPanelElements {
  return {
    toggle: $("#terminal-scratchpad-toggle"),
    panel: $("#terminal-scratchpad"),
    splitter: $("#terminal-scratchpad-splitter"),
    state: $("#terminal-scratchpad-state"),
    close: $("#terminal-scratchpad-close"),
    text: $("#terminal-scratchpad-text"),
    add: $("#terminal-scratchpad-add"),
    list: $("#terminal-scratchpad-snippets"),
    empty: $("#terminal-scratchpad-empty"),
    body: $("#terminal-body"),
  };
}

function windowDiagnosticsElements(): TerminalDiagnosticsElements {
  return {
    toggle: $("#terminal-window-diagnostics-toggle"),
    popover: $("#terminal-window-diagnostics"),
    close: $("#terminal-window-diagnostics-close"),
    process: $("#terminal-window-diagnostic-process"),
    stream: $("#terminal-window-diagnostic-stream"),
    renderer: $("#terminal-window-diagnostic-renderer"),
    layout: $("#terminal-window-diagnostic-layout"),
    updated: $("#terminal-window-diagnostic-updated"),
    header: $("#terminal-window-info"),
    surface: $("#terminal-window"),
  };
}

/** Debounces that run only when the test says so. */
class ManualClock {
  #next = 1;
  readonly pending = new Map<number, () => void>();
  setTimeout = (callback: () => void): unknown => {
    const handle = this.#next++;
    this.pending.set(handle, callback);
    return handle;
  };
  clearTimeout = (handle: unknown): void => {
    this.pending.delete(handle as number);
  };
  runAll(): void {
    const callbacks = [...this.pending.values()];
    this.pending.clear();
    for (const callback of callbacks) callback();
  }
}

const settle = async () => {
  for (let round = 0; round < 6; round += 1) {
    await new Promise<void>((done) => setTimeout(done, 0));
  }
};

/**
 * The project store the runtime fronts: one record, a revision fence that
 * refuses a stale write with the stored record, and the `scratchpad:changed`
 * announcement after every write that landed.
 */
function projectStore(generation = 7) {
  let record: Scratchpad = { text: "", snippets: [], revision: 0, updatedAt: 0 };
  const listeners = new Set<(revision: number) => void>();
  const writes: Array<{ revision: number; text: string }> = [];
  return {
    writes,
    get record() {
      return record;
    },
    backend: {
      get: async (requested: number) => ({
        generation: requested,
        scratchpad: structuredClone(record),
      }),
      set: async (requested: number, revision: number, scratchpad: Scratchpad) => {
        if (requested !== generation) throw new Error("stale workspace generation");
        if (revision !== record.revision) {
          throw Object.assign(new Error("scratchpad revision conflict"), {
            stored: structuredClone(record),
          });
        }
        record = { ...structuredClone(scratchpad), revision: revision + 1, updatedAt: 1 };
        writes.push({ revision: record.revision, text: record.text });
        for (const listener of listeners) listener(record.revision);
        return { generation: requested, revision: record.revision };
      },
    },
    /** A surface subscribing to the desktop event. */
    subscribe(listener: (revision: number) => void) {
      listeners.add(listener);
    },
  };
}

function mountPanel(
  elements: ScratchpadPanelElements,
  store: ReturnType<typeof projectStore>,
  options: {
    clock?: ManualClock;
    storage?: Pick<Storage, "getItem" | "setItem">;
    selection?: string;
  } = {},
) {
  const clock = options.clock ?? new ManualClock();
  const copied: string[] = [];
  const pasted: string[] = [];
  const errors: unknown[] = [];
  const panel = new ScratchpadPanel(elements, {
    generation: 7,
    backend: store.backend,
    storage: options.storage ?? dom.window.localStorage,
    bodyWidth: () => 1200,
    hasSelection: () => Boolean(options.selection),
    selection: () => options.selection ?? null,
    pasteReady: () => true,
    paste: async (text) => {
      pasted.push(text);
    },
    clipboardAvailable: () => true,
    setClipboardText: async (text) => {
      copied.push(text);
    },
    openChanged: () => {},
    resized: () => {},
    reportError: (error) => errors.push(error),
    setTimeout: clock.setTimeout,
    clearTimeout: clock.clearTimeout,
  });
  panel.mount();
  store.subscribe((revision) => void panel.refresh(revision));
  return { panel, clock, copied, pasted, errors };
}

async function type(text: HTMLTextAreaElement, value: string) {
  text.focus();
  text.value = value;
  fire(text, "input");
}

describe("detached window info row", () => {
  const running = (shell: ShellState = initialShellState): DetachedPaneFacts => ({
    stream: "open",
    ended: false,
    failed: false,
    shell,
    changedAt: Date.UTC(2026, 8, 23, 8, 0, 0),
  });

  function info() {
    return new TerminalWindowInfo({
      state: $("#terminal-window-state"),
      profile: $("#terminal-window-profile"),
      cwd: $("#terminal-window-cwd"),
      association: $("#terminal-window-association"),
      associationLabel: $("#terminal-window-association-label"),
    });
  }

  it("states the shell, profile, and working directory the dock would", () => {
    info().render({
      pane: running({ ...initialShellState, phase: "prompt", lastExitCode: 0 }),
      profileName: "zsh",
      cwd: "/Users/rodox/dev/rs/ptrack/crates/ptrack-app",
      association: undefined,
    });
    expect($("#terminal-window-state").textContent).toBe("Prompt · last 0");
    expect($("#terminal-window-profile").textContent).toBe("zsh");
    // The value truncates from its start; the title keeps the whole path.
    expect($("#terminal-window-cwd").textContent).toBe(
      "‎/Users/rodox/dev/rs/ptrack/crates/ptrack-app‎",
    );
    expect($("#terminal-window-cwd").title).toBe("/Users/rodox/dev/rs/ptrack/crates/ptrack-app");
    expect($("#terminal-window-association").hidden).toBe(true);
  });

  it("follows the shell through a command and its exit", () => {
    const row = info();
    const base = { profileName: "zsh", cwd: "/p", association: undefined };
    row.render({ ...base, pane: running({ ...initialShellState, phase: "executing" }) });
    expect($("#terminal-window-state").textContent).toBe("Command running");
    row.render({ ...base, pane: running() });
    expect($("#terminal-window-state").textContent).toBe("Running");
    row.render({ ...base, pane: { ...running(), ended: true } });
    expect($("#terminal-window-state").textContent).toBe("Exited");
    row.render({ ...base, pane: { ...running(), ended: true, failed: true } });
    expect($("#terminal-window-state").textContent).toBe("Failed");
  });

  it("shows the linked plan and task read-only", () => {
    info().render({
      pane: running(),
      profileName: "Claude",
      cwd: "",
      association: { version: 1, planId: 3, taskId: 9 },
    });
    expect($("#terminal-window-association").hidden).toBe(false);
    expect($("#terminal-window-association-label").textContent).toBe(
      "Linked · plan #3 · task #9",
    );
    expect($("#terminal-window-association").title).toContain("main window");
    expect($("#terminal-window-cwd").textContent).toBe("Project root");
  });
});

describe("detached window diagnostics", () => {
  function popover(pane: DetachedPaneFacts | null) {
    const diagnostics = new TerminalDiagnosticsPopover(
      windowDiagnosticsElements(),
      () => terminalDiagnosticView(detachedDiagnosticInput({ pane, linked: false, visible: true })),
    );
    diagnostics.mount();
    return diagnostics;
  }
  const live: DetachedPaneFacts = {
    stream: "open",
    ended: false,
    failed: false,
    shell: null,
    changedAt: Date.UTC(2026, 8, 23, 8, 0, 0),
  };

  it("opens with the dock's content-free rows for the active pane", () => {
    popover(live);
    $("#terminal-window-diagnostics-toggle").click();
    expect($("#terminal-window-diagnostics").hidden).toBe(false);
    expect($("#terminal-window-diagnostics-toggle").getAttribute("aria-expanded")).toBe("true");
    expect($("#terminal-window-diagnostic-process").textContent).toBe("Running");
    expect($("#terminal-window-diagnostic-stream").textContent).toBe("Connected");
    expect($("#terminal-window-diagnostic-renderer").textContent).toBe("DOM");
    expect($("#terminal-window-diagnostic-layout").textContent).toBe("Default");
    expect($("#terminal-window-diagnostic-updated").textContent).toBe("2026-09-23T08:00:00.000Z");
    expect(dom.document.activeElement).toBe($("#terminal-window-diagnostics"));
  });

  it("reads a lost stream and an ended shell the way the dock does", () => {
    const view = (pane: DetachedPaneFacts) =>
      terminalDiagnosticView(detachedDiagnosticInput({ pane, linked: false, visible: true }));
    const rows = (pane: DetachedPaneFacts) =>
      Object.fromEntries(view(pane).rows.map((row) => [row.key, row.value]));
    expect(rows({ ...live, stream: "error" }).stream).toBe("Failed");
    expect(rows({ ...live, stream: "closed", ended: true })).toMatchObject({
      process: "Exited",
      stream: "Disconnected",
    });
    // Restart and force stop belong to the main window.
    expect(view({ ...live, ended: true }).canForceStop).toBe(false);
  });

  it("closes from its button, Escape, and a press anywhere else", () => {
    popover(live);
    const toggle = $("#terminal-window-diagnostics-toggle");
    const panel = $("#terminal-window-diagnostics");
    toggle.click();
    $("#terminal-window-diagnostics-close").click();
    expect(panel.hidden).toBe(true);
    expect(dom.document.activeElement).toBe(toggle);

    toggle.click();
    fire(panel, "keydown", { key: "Escape" });
    expect(panel.hidden).toBe(true);
    expect(toggle.getAttribute("aria-label")).toBe("Show terminal diagnostics");

    toggle.click();
    fire($("#terminal-window-diagnostic-stream"), "pointerdown");
    expect(panel.hidden).toBe(false);
    fire($("#terminal-window-host"), "pointerdown");
    expect(panel.hidden).toBe(true);
  });
});

describe("detached window scratchpad", () => {
  it("opens the project's note with its save state and keeps its own open flag", async () => {
    const store = projectStore();
    await store.backend.set(7, 0, { text: "release checklist", snippets: [], revision: 0, updatedAt: 0 });
    const storage = detachedScratchpadStorage(dom.window.localStorage);
    mountPanel(windowScratchpadElements(), store, { storage });
    expect($("#terminal-window-scratchpad").hidden).toBe(true);
    $("#terminal-window-scratchpad-toggle").click();
    await settle();
    expect($("#terminal-window-scratchpad").hidden).toBe(false);
    expect($("#terminal-window-scratchpad-splitter").hidden).toBe(false);
    expect($("#terminal-window-scratchpad-toggle").getAttribute("aria-pressed")).toBe("true");
    expect($<HTMLTextAreaElement>("#terminal-window-scratchpad-text").value).toBe("release checklist");
    expect($("#terminal-window-scratchpad-state").textContent).toBe(scratchpadStatus.saved);
    // Opening it here does not open the dock's panel on its next launch.
    expect(dom.window.localStorage.getItem(`${scratchpadOpenStorageKey}-window`)).toBe("true");
    expect(dom.window.localStorage.getItem(scratchpadOpenStorageKey)).toBeNull();
  });

  it("saves a typed note and counts down the byte budget near the cap", async () => {
    const store = projectStore();
    const { clock } = mountPanel(windowScratchpadElements(), store);
    $("#terminal-window-scratchpad-toggle").click();
    await settle();
    const text = $<HTMLTextAreaElement>("#terminal-window-scratchpad-text");
    await type(text, "typed in the detached window");
    expect($("#terminal-window-scratchpad-state").textContent).toBe(scratchpadStatus.saving);
    clock.runAll();
    await settle();
    expect(store.record.text).toBe("typed in the detached window");
    expect($("#terminal-window-scratchpad-state").textContent).toBe(scratchpadStatus.saved);

    await type(text, "x".repeat(65_000));
    expect($("#terminal-window-scratchpad-state").textContent).toBe("Saving… · 536 bytes left");
    await type(text, "x".repeat(65_600));
    expect($("#terminal-window-scratchpad-state").textContent).toBe("Too long by 64 bytes; not saved");
  });

  it("captures copies into the clipboard strip, never one that looks like a secret", async () => {
    const store = projectStore();
    const { panel, clock } = mountPanel(windowScratchpadElements(), store);
    $("#terminal-window-scratchpad-toggle").click();
    await settle();
    panel.capture("cargo test -p ptrack-app");
    expect($("#terminal-window-scratchpad-snippets").querySelectorAll("li")).toHaveLength(1);
    expect($("#terminal-window-scratchpad-empty").hidden).toBe(true);
    panel.capture("export GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789");
    expect($("#terminal-window-scratchpad-state").textContent).toBe(secretCaptureNotice);
    expect($("#terminal-window-scratchpad-snippets").querySelectorAll("li")).toHaveLength(1);
    clock.runAll();
    await settle();
    expect(store.record.snippets.map((snippet) => snippet.text)).toEqual(["cargo test -p ptrack-app"]);
  });

  it("adds the active pane's selection and pastes a snippet through the window's paste path", async () => {
    const store = projectStore();
    const { pasted } = mountPanel(windowScratchpadElements(), store, { selection: "git status" });
    $("#terminal-window-scratchpad-toggle").click();
    await settle();
    expect($<HTMLButtonElement>("#terminal-window-scratchpad-add").disabled).toBe(false);
    $("#terminal-window-scratchpad-add").click();
    const paste = $("#terminal-window-scratchpad-snippets").querySelector<HTMLButtonElement>(
      '[data-scratchpad-action="paste"]',
    );
    paste?.click();
    await settle();
    expect(pasted).toEqual(["git status"]);
  });
});

describe("dock and detached window stay consistent", () => {
  async function bothOpen() {
    const store = projectStore();
    const dock = mountPanel(dockScratchpadElements(), store);
    const detached = mountPanel(windowScratchpadElements(), store, {
      storage: detachedScratchpadStorage(dom.window.localStorage),
    });
    $("#terminal-scratchpad-toggle").click();
    $("#terminal-window-scratchpad-toggle").click();
    await settle();
    return { store, dock, detached };
  }

  it("shows a note typed in the window in the dock, and the other way round", async () => {
    const { detached, dock } = await bothOpen();
    const windowText = $<HTMLTextAreaElement>("#terminal-window-scratchpad-text");
    const dockText = $<HTMLTextAreaElement>("#terminal-scratchpad-text");

    await type(windowText, "from the window");
    // Leaving the note writes it at once, without waiting for the debounce.
    windowText.blur();
    fire(windowText, "blur");
    await settle();
    expect(dockText.value).toBe("from the window");
    expect(dock.panel.saver.dirty).toBe(false);
    expect(detached.clock.pending.size).toBe(0);

    await type(dockText, "from the dock");
    dock.clock.runAll();
    await settle();
    expect(windowText.value).toBe("from the dock");
    expect($("#terminal-window-scratchpad-state").textContent).toBe(scratchpadStatus.saved);
  });

  it("keeps an unsaved edit and reports the conflict instead of dropping text", async () => {
    const { store, dock, detached } = await bothOpen();
    const windowText = $<HTMLTextAreaElement>("#terminal-window-scratchpad-text");
    const dockText = $<HTMLTextAreaElement>("#terminal-scratchpad-text");

    // Mid-edit in the dock: the window's write does not replace what is typed.
    await type(dockText, "half-typed in the dock");
    await type(windowText, "saved from the window");
    detached.clock.runAll();
    await settle();
    expect(dockText.value).toBe("half-typed in the dock");

    // The dock's write meets the revision check: its text reaches the
    // clipboard, the stored note replaces the field, and the panel says so.
    dock.clock.runAll();
    await settle();
    expect(dock.copied).toEqual(["half-typed in the dock"]);
    expect(dockText.value).toBe("saved from the window");
    expect(store.record.text).toBe("saved from the window");
    expect($("#terminal-scratchpad-state").textContent).toBe(scratchpadNotices.reloadedCopied);
  });
});
