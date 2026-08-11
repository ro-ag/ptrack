export function terminalPaneInputLabel(tabTitle: string, paneIndex: number): string {
  const title = tabTitle.trim() || "Terminal";
  const pane = Math.max(1, Math.trunc(paneIndex));
  const prefix = title.toLowerCase() === "terminal" ? "Terminal" : `${title} terminal`;
  return `${prefix} pane ${pane}`;
}
