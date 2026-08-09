import { describe, expect, it } from "vitest";

import { terminalSearchResultLabel } from "./search";

describe("terminal search result label", () => {
  it("formats empty, missing, active, and capped result states", () => {
    expect(terminalSearchResultLabel({ resultIndex: 0, resultCount: 0 }, false)).toBe("");
    expect(terminalSearchResultLabel({ resultIndex: 0, resultCount: 0 }, true)).toBe(
      "No results",
    );
    expect(terminalSearchResultLabel({ resultIndex: 1, resultCount: 7 }, true)).toBe(
      "2 of 7",
    );
    expect(terminalSearchResultLabel({ resultIndex: -1, resultCount: 1_000 }, true)).toBe(
      "1000+ results",
    );
  });
});
