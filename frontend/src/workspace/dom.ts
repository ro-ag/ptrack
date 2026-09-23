// DOM builders shared by the workspace views. Every one of them writes text
// through textContent; nothing here parses markup.

export interface ElementConstructor<T extends Element> {
  new (): T;
  prototype: T;
}

/**
 * The element `selector` names, checked against the type its controller
 * drives. index.html ships with the bundle, so a missing or retyped element
 * is a build defect: it fails loudly when the controller is created instead
 * of surfacing later as a null in some unrelated handler.
 */
export function element<T extends Element>(
  selector: string,
  type: ElementConstructor<T>,
  root: ParentNode = document,
): T {
  const node = root.querySelector(selector);
  if (!(node instanceof type)) {
    throw new Error(`The p-track window is missing ${selector}`);
  }
  return node;
}

export function statElement(value: string | number, label: string): HTMLDivElement {
  const stat = document.createElement("div");
  stat.className = "stat";
  const caption = document.createElement("span");
  caption.className = "stat-label";
  caption.textContent = label;
  const number = document.createElement("span");
  number.className = "stat-value";
  number.textContent = String(value);
  stat.append(caption, number);
  return stat;
}

export function emptyMemory(message: string): HTMLDivElement {
  const empty = document.createElement("div");
  empty.className = "memory-empty";
  empty.textContent = message;
  return empty;
}

export function intelligenceItem(titleText: string, detailText: string, state = ""): HTMLElement {
  const item = document.createElement("article");
  item.className = "intelligence-item";
  if (state) item.dataset.state = state;
  const title = document.createElement("p");
  title.className = "intelligence-title";
  title.textContent = titleText;
  const detail = document.createElement("p");
  detail.className = "intelligence-detail";
  detail.textContent = detailText;
  item.append(title, detail);
  return item;
}

export function pill(label: string, value: string | number, tone = ""): HTMLSpanElement {
  const item = document.createElement("span");
  item.className = "intelligence-pill";
  if (tone) item.dataset.tone = tone;
  item.textContent = `${label} ${value}`;
  return item;
}

export const SVG_NS = "http://www.w3.org/2000/svg";

export function svgElement(
  name: string,
  attributes: Record<string, string | number> = {},
): SVGElement {
  const node = document.createElementNS(SVG_NS, name);
  for (const [key, value] of Object.entries(attributes)) {
    node.setAttribute(key, String(value));
  }
  return node;
}

export function setAriaBoolean(element: Element, attribute: string, value: boolean): void {
  if (value) element.setAttribute(attribute, "true");
  else element.removeAttribute(attribute);
}

export function setFirstRunSectionVisible(element: HTMLElement, visible: boolean): void {
  element.hidden = !visible;
  element.inert = !visible;
}

export function appendTextItems(target: Element, items: readonly string[]): void {
  target.replaceChildren();
  items.forEach((text) => {
    const item = document.createElement("li");
    item.textContent = text;
    target.append(item);
  });
}
