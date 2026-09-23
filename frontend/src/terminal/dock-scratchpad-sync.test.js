// The main window's dock follows scratchpad writes made elsewhere (a detached
// terminal window): the runtime announces each landed write, and the dock
// re-reads unless it holds an unsaved edit of its own.
import { afterEach, describe, expect, it } from "vitest";

import { bootApp } from "../test-support/app-harness";

function record(text, revision) {
  return { text, snippets: [], revision, updatedAt: 1 };
}

describe("dock scratchpad sync", () => {
  let harness;
  afterEach(() => harness?.dom.restore());

  async function openDockScratchpad() {
    let stored = record("dock note", 1);
    harness = await bootApp({
      open: {},
      responses: {
        GetScratchpadV1: (generation) => ({ generation, scratchpad: structuredClone(stored) }),
      },
    });
    await harness.click("#terminal-scratchpad-toggle");
    expect(harness.$("#terminal-scratchpad").hidden).toBe(false);
    expect(harness.$("#terminal-scratchpad-text").value).toBe("dock note");
    return {
      store: (next) => {
        stored = next;
      },
      reads: () => harness.backend.callsTo("GetScratchpadV1").length,
    };
  }

  it("shows a note written in a terminal window", async () => {
    const { store, reads } = await openDockScratchpad();
    store(record("typed in the detached window", 2));
    await harness.emit("scratchpad:changed", { generation: 3, revision: 2 });
    expect(harness.$("#terminal-scratchpad-text").value).toBe("typed in the detached window");
    expect(harness.$("#terminal-scratchpad-state").textContent).toBe("Saved");
    // Its own revision, or one it already holds, is not re-read.
    const before = reads();
    await harness.emit("scratchpad:changed", { generation: 3, revision: 2 });
    expect(reads()).toBe(before);
  });

  it("ignores another project's writes", async () => {
    const { store, reads } = await openDockScratchpad();
    const before = reads();
    store(record("another project's note", 5));
    await harness.emit("scratchpad:changed", { generation: 4, revision: 5 });
    expect(reads()).toBe(before);
    expect(harness.$("#terminal-scratchpad-text").value).toBe("dock note");
  });

  it("keeps a note that is still being typed", async () => {
    const { store } = await openDockScratchpad();
    harness.$("#terminal-scratchpad-text").focus();
    await harness.type("#terminal-scratchpad-text", "half-typed here");
    store(record("typed in the detached window", 2));
    await harness.emit("scratchpad:changed", { generation: 3, revision: 2 });
    expect(harness.$("#terminal-scratchpad-text").value).toBe("half-typed here");
  });
});
