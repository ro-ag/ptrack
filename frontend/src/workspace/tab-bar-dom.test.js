// The rendered tab bar: an indicator refresh keeps each tab's position in its
// accessible name, the same name a full render gives it.
import { afterEach, describe, expect, it } from "vitest";

import { installFakeDom } from "../test-support/fake-dom";
import { WorkspaceTabBar } from "./tab-bar";
import { WorkspaceTabController } from "./tab-controller";

function sequentialIds() {
  let sequence = 0;
  return { next: (kind) => `${kind}-${++sequence}` };
}

describe("WorkspaceTabBar", () => {
  let dom;
  afterEach(() => dom?.restore());

  it("keeps the position suffix when only the indicators refresh", () => {
    dom = installFakeDom('<div id="tabs"></div><div id="actions"></div><button id="new"></button>');
    const controller = new WorkspaceTabController(sequentialIds());
    controller.dispatch({ type: "create-tab", profileId: "shell", cwd: "" });
    let kind = "closed";
    const bar = new WorkspaceTabBar({
      tabList: dom.document.querySelector("#tabs"),
      actionToolbar: dom.document.querySelector("#actions"),
      newTabButton: dom.document.querySelector("#new"),
      controller,
      indicatorForTab: () => ({ kind, unread: false }),
    });
    const labels = () => dom.document.querySelectorAll('#tabs [role="tab"]').map((tab) => tab.getAttribute("aria-label"));
    const rendered = labels();
    expect(rendered).toHaveLength(2);
    expect(rendered[1]).toContain(", 2 of 2");
    kind = "running";
    bar.refresh();
    const refreshed = labels();
    expect(refreshed[0]).toContain(", 1 of 2");
    expect(refreshed[1]).toContain(", 2 of 2");
    expect(refreshed).not.toEqual(rendered);
    bar.dispose();
  });
});
