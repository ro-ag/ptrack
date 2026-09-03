export type TerminalControlIcon = "left" | "right" | "rename" | "duplicate" | "close" | "split-right" | "split-down";

const paths: Record<TerminalControlIcon, string> = {
  left: "M12 4 8 8l4 4M8 8h8",
  right: "m8 4 4 4-4 4M4 8h8",
  rename: "m4 11 7-7 3 3-7 7H4zM10 5l3 3",
  duplicate: "M6 6h8v8H6zM6 10H2V2h8v4",
  close: "m4 4 8 8M12 4l-8 8",
  "split-right": "M2 2h12v12H2zM8 2v12",
  "split-down": "M2 2h12v12H2zM2 8h12",
};

export function terminalControlIcon(kind: TerminalControlIcon): SVGSVGElement {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.4");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", paths[kind]);
  svg.append(path);
  return svg;
}
