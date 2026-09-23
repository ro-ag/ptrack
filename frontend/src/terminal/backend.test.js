// The terminal dock as the main window mounts it: the Start shell control
// and the Settings-owned Unicode mode reach the dock through its handle.
// xterm itself cannot run in the harness, so the renderer is a stand-in that
// accepts every call; the dock's own logic around it is real.
import { afterEach, describe, expect, it, vi } from "vitest";

import { bootApp, fakeBackend } from "../test-support/app-harness";
import { generationTerminalBackend } from "./backend";

vi.mock("./renderer", () => {
  // Any property is a callable stand-in; numbers read as zero.
  const stub = new Proxy(function stand() {}, {
    get: (_, key) => {
      if (key === Symbol.toPrimitive) return () => 0;
      if (key === "then") return undefined;
      return stub;
    },
    apply: () => stub,
    construct: () => stub,
    set: () => true,
  });
  return {
    applyTerminalTheme: () => {},
    paintTerminalBackground: () => {},
    createTerminalRenderer: () => ({ terminal: stub, fit: stub, search: stub, unicode: stub }),
    openExternalURL: async () => {},
    terminalLinkActivation: () => false,
  };
});

describe("terminal dock controls", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  it("starts the default shell from Start shell, like the Open control", async () => {
    harness = await bootApp({ open: {}, responses: { CreateTerminalV2: new Error("no pty in tests") } });
    const start = harness.$("#terminal-start-shell");
    expect(start.disabled).toBe(false);
    await harness.click(start);
    const created = harness.backend.callsTo("CreateTerminalV2");
    expect(created).toHaveLength(1);
    // An empty working directory lets the backend start in the project root.
    expect(created[0].slice(0, 3)).toEqual([3, "zsh", ""]);
  });

  it("keeps Start shell reachable inside the stopped pane while the scratchpad is open", async () => {
    harness = await bootApp({ open: {} });
    const empty = harness.$("#terminal-empty");
    // A fresh dock is the compact bar; its Open control starts the shell.
    expect(empty.hidden).toBe(true);
    await harness.click("#terminal-scratchpad-toggle");
    // The open scratchpad keeps the terminal body up, so the notice moves
    // into the stopped pane instead of staying hidden above the body.
    expect(harness.$("#terminal-body").hidden).toBe(false);
    expect(empty.hidden).toBe(false);
    expect(empty.parentElement.className).toBe("terminal-split-leaf-mount");
  });

  it("offers no hidden Unicode checkbox; Settings drives the dock's mode", async () => {
    harness = await bootApp({ open: {}, responses: { SetPreferences: (patch) => ({ storage: "ok", preferences: patch }) } });
    expect(harness.$("#terminal-modern-unicode")).toBeNull();
    const handle = harness.app.state.terminalHandle;
    const setModernUnicode = vi.spyOn(handle, "setModernUnicode");
    await harness.click("#settings-open");
    await harness.type("#settings-terminal-unicode", "modern", "change");
    expect(setModernUnicode).toHaveBeenCalledWith(true);
    expect(harness.backend.callsTo("SetPreferences")).toEqual([[{ terminal: { unicodeMode: "modern" } }]]);
  });
});

describe("generation-fenced terminal backend", () => {
  function adapter(responses, generation = 5) {
    const backend = fakeBackend(responses);
    return { backend, terminal: generationTerminalBackend(() => backend.api, generation) };
  }

  it("forwards both pop-out arguments, sessions and the tab shape", async () => {
    const { backend, terminal } = adapter({ OpenTerminalWindow: { label: "terminal-1" } });
    const shape = { id: "tab-1", title: "zsh" };
    await expect(terminal.OpenTerminalWindow(["s1", "s2"], shape)).resolves.toEqual({ label: "terminal-1" });
    expect(backend.callsTo("OpenTerminalWindow")).toEqual([[["s1", "s2"], shape]]);
  });

  it("stamps every fenced call with its generation and refuses another generation's reply", async () => {
    const { backend, terminal } = adapter({
      CreateTerminalV2: (generation) => ({ generation: generation + 1, sessionId: "s1" }),
      GetTerminalProfilesV2: (generation) => ({ generation, profiles: [{ id: "zsh" }] }),
    });
    await expect(terminal.GetTerminalProfiles()).resolves.toEqual([{ id: "zsh" }]);
    await expect(terminal.CreateTerminal("zsh", "", 24, 80)).rejects.toThrow("Stale terminal response ignored");
    expect(backend.callsTo("CreateTerminalV2")).toEqual([[5, "zsh", "", 24, 80]]);
  });

  it("detaches an association by sending an empty pointer", async () => {
    const { backend, terminal } = adapter({ MutateTerminalAssociationV2: (generation) => ({ generation }) });
    await terminal.MutateTerminalAssociation("s1", 3, undefined);
    expect(backend.callsTo("MutateTerminalAssociationV2")).toEqual([[5, "s1", 3, true, { version: 1 }]]);
  });
});

