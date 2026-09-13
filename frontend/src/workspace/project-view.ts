/** Switch presentation only: keyed project buttons and selection remain intact. */
export function bindProjectView(root: ParentNode = document) {
  const panel = root.querySelector<HTMLElement>(".projects-panel");
  const stage = panel?.querySelector<HTMLElement>(".project-stage-wrap");
  const strip = panel?.querySelector<HTMLElement>("#orbit-project-strip");
  if (!panel || !stage || !strip) return () => {};
  const buttons = [...panel.querySelectorAll<HTMLButtonElement>("[data-project-view]")];
  const select = (view: string) => {
    const list = view === "list";
    panel.dataset.projectView = list ? "list" : "carousel";
    stage.hidden = list;
    strip.classList.toggle("is-list", list);
    for (const button of buttons) button.setAttribute("aria-pressed", String(button.dataset.projectView === panel.dataset.projectView));
    // Focus stays on the view control; reveal the current selection without opening it.
    strip.querySelector<HTMLElement>('[aria-current="true"]')?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  };
  const listeners = buttons.map((button) => {
    const listener = () => select(button.dataset.projectView ?? "carousel");
    button.addEventListener("click", listener);
    return () => button.removeEventListener("click", listener);
  });
  select(panel.dataset.projectView ?? "carousel");
  return () => listeners.forEach((remove) => remove());
}
