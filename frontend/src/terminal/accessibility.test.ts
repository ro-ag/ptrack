import { describe, expect, it } from "vitest";

import { terminalPaneInputLabel } from "./accessibility";

describe("terminal pane accessibility", () => {
  it("keeps pane identity concise and updates with tab title and split order", () => {
    expect(terminalPaneInputLabel("Terminal", 1)).toBe("Terminal pane 1");
    expect(terminalPaneInputLabel("Build", 2)).toBe("Build terminal pane 2");
    expect(terminalPaneInputLabel("Renamed", 1)).toBe("Renamed terminal pane 1");
    expect(terminalPaneInputLabel("  ", 0)).toBe("Terminal pane 1");
  });
});
