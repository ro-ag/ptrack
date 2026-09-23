// A small in-memory DOM for controller tests. It parses index.html into a
// real tree so the window controllers can be created, bound, and driven the
// way the WebView drives them, without a browser dependency. It implements
// only what the controllers use; anything else throws or stays inert.

const VOID_TAGS = new Set([
  "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr",
]);

const ELEMENT_NODE = 1;
const TEXT_NODE = 3;

export class FakeEvent {
  constructor(type, init = {}) {
    this.type = type;
    this.bubbles = Boolean(init.bubbles);
    this.cancelable = init.cancelable !== false;
    this.defaultPrevented = false;
    this.target = null;
    this.currentTarget = null;
    this.propagationStopped = false;
    this.immediateStopped = false;
    Object.assign(this, init);
  }

  preventDefault() {
    if (this.cancelable) this.defaultPrevented = true;
  }

  stopPropagation() {
    this.propagationStopped = true;
  }

  stopImmediatePropagation() {
    this.propagationStopped = true;
    this.immediateStopped = true;
  }

  composedPath() {
    const path = [];
    for (let node = this.target; node; node = node.parentNode) path.push(node);
    return path;
  }
}

class EventTargetMixin {
  constructor() {
    this.listeners = new Map();
  }

  addEventListener(type, listener, options) {
    if (!listener) return;
    const capture = typeof options === "boolean" ? options : Boolean(options?.capture);
    const once = typeof options === "object" && Boolean(options?.once);
    if (!this.listeners.has(type)) this.listeners.set(type, []);
    const list = this.listeners.get(type);
    if (list.some((entry) => entry.listener === listener && entry.capture === capture)) return;
    list.push({ listener, capture, once });
  }

  removeEventListener(type, listener, options) {
    const capture = typeof options === "boolean" ? options : Boolean(options?.capture);
    const list = this.listeners.get(type);
    if (!list) return;
    const index = list.findIndex((entry) => entry.listener === listener && entry.capture === capture);
    if (index >= 0) list.splice(index, 1);
  }

  invokeListeners(event, phase) {
    const list = [...(this.listeners.get(event.type) ?? [])];
    for (const entry of list) {
      if (phase === "capture" && !entry.capture) continue;
      if (phase === "bubble" && entry.capture) continue;
      if (entry.once) this.removeEventListener(event.type, entry.listener, entry.capture);
      event.currentTarget = this;
      if (typeof entry.listener === "function") entry.listener.call(this, event);
      else entry.listener.handleEvent(event);
      if (event.immediateStopped) break;
    }
  }

  dispatchEvent(event) {
    event.target = this;
    const path = [];
    for (let node = this.parentNode; node; node = node.parentNode) path.push(node);
    const window = this.ownerDocument?.defaultView;
    if (window && this !== window) path.push(window);
    for (const node of [...path].reverse()) {
      node.invokeListeners(event, "capture");
      if (event.propagationStopped) return !event.defaultPrevented;
    }
    this.invokeListeners(event, "target");
    if (event.bubbles && !event.propagationStopped) {
      for (const node of path) {
        node.invokeListeners(event, "bubble");
        if (event.propagationStopped) break;
      }
    }
    return !event.defaultPrevented;
  }
}

export class FakeNode extends EventTargetMixin {
  constructor(document) {
    super();
    this.ownerDocument = document;
    this.parentNode = null;
    this.childNodes = [];
  }

  get parentElement() {
    return this.parentNode?.nodeType === ELEMENT_NODE ? this.parentNode : null;
  }

  get isConnected() {
    let node = this;
    while (node.parentNode) node = node.parentNode;
    return node === this.ownerDocument;
  }

  get firstChild() {
    return this.childNodes[0] ?? null;
  }

  get textContent() {
    return this.childNodes.map((node) => node.textContent).join("");
  }

  set textContent(value) {
    this.replaceChildren();
    const text = value === null || value === undefined ? "" : String(value);
    if (text) this.appendChild(this.ownerDocument.createTextNode(text));
  }

  appendChild(node) {
    if (node.nodeType === 11) {
      for (const child of [...node.childNodes]) this.appendChild(child);
      return node;
    }
    node.remove?.();
    node.parentNode = this;
    this.childNodes.push(node);
    return node;
  }

  insertBefore(node, reference) {
    if (!reference) return this.appendChild(node);
    node.remove?.();
    node.parentNode = this;
    const index = this.childNodes.indexOf(reference);
    this.childNodes.splice(index < 0 ? this.childNodes.length : index, 0, node);
    return node;
  }

  removeChild(node) {
    const index = this.childNodes.indexOf(node);
    if (index >= 0) this.childNodes.splice(index, 1);
    node.parentNode = null;
    return node;
  }

  remove() {
    this.parentNode?.removeChild(this);
  }

  contains(node) {
    for (let current = node; current; current = current.parentNode) {
      if (current === this) return true;
    }
    return false;
  }

  toNode(value) {
    return value instanceof FakeNode ? value : this.ownerDocument.createTextNode(String(value));
  }

  append(...nodes) {
    for (const node of nodes) this.appendChild(this.toNode(node));
  }

  prepend(...nodes) {
    const first = this.childNodes[0] ?? null;
    for (const node of nodes) this.insertBefore(this.toNode(node), first);
  }

  replaceChildren(...nodes) {
    for (const child of [...this.childNodes]) this.removeChild(child);
    this.append(...nodes);
  }

  cloneNode(deep = false) {
    return this.cloneInto(this.ownerDocument, deep);
  }
}

export class FakeText extends FakeNode {
  constructor(document, data) {
    super(document);
    this.nodeType = TEXT_NODE;
    this.data = data;
  }

  get textContent() {
    return this.data;
  }

  set textContent(value) {
    this.data = String(value);
  }

  cloneInto(document) {
    return new FakeText(document, this.data);
  }
}

class FakeFragment extends FakeNode {
  constructor(document) {
    super(document);
    this.nodeType = 11;
  }
}

function camelToDataAttribute(name) {
  return `data-${name.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`;
}

function dataAttributeToCamel(name) {
  return name.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
}

function styleDeclaration() {
  const properties = new Map();
  return new Proxy({
    setProperty(name, value) { properties.set(name, String(value)); },
    removeProperty(name) { properties.delete(name); },
    getPropertyValue(name) { return properties.get(name) ?? ""; },
  }, {
    get(target, key) {
      if (key in target) return target[key];
      return properties.get(String(key)) ?? "";
    },
    set(_, key, value) {
      properties.set(String(key), String(value));
      return true;
    },
  });
}

export class FakeElement extends FakeNode {
  constructor(document, tagName) {
    super(document);
    this.nodeType = ELEMENT_NODE;
    this.localName = tagName.toLowerCase();
    this.tagName = tagName.toUpperCase();
    this.attributes = new Map();
    this.style = styleDeclaration();
    this.scrollTop = 0;
    this.scrollLeft = 0;
    this.inert = false;
    const element = this;
    this.dataset = new Proxy({}, {
      get(_, key) {
        return element.getAttribute(camelToDataAttribute(String(key))) ?? undefined;
      },
      set(_, key, value) {
        element.setAttribute(camelToDataAttribute(String(key)), String(value));
        return true;
      },
      deleteProperty(_, key) {
        element.removeAttribute(camelToDataAttribute(String(key)));
        return true;
      },
      has(_, key) {
        return element.hasAttribute(camelToDataAttribute(String(key)));
      },
      ownKeys() {
        return [...element.attributes.keys()].filter((name) => name.startsWith("data-")).map(dataAttributeToCamel);
      },
      getOwnPropertyDescriptor(_, key) {
        const value = element.getAttribute(camelToDataAttribute(String(key)));
        return value === null ? undefined : { value, enumerable: true, configurable: true, writable: true };
      },
    });
    this.classList = {
      add: (...names) => names.forEach((name) => this.toggleClass(name, true)),
      remove: (...names) => names.forEach((name) => this.toggleClass(name, false)),
      toggle: (name, force) => this.toggleClass(name, force),
      contains: (name) => this.classes().includes(name),
    };
  }

  classes() {
    return (this.getAttribute("class") ?? "").split(/\s+/).filter(Boolean);
  }

  toggleClass(name, force) {
    const classes = this.classes();
    const has = classes.includes(name);
    const next = force === undefined ? !has : force;
    if (next && !has) classes.push(name);
    if (!next && has) classes.splice(classes.indexOf(name), 1);
    this.setAttribute("class", classes.join(" "));
    return next;
  }

  get id() { return this.getAttribute("id") ?? ""; }
  set id(value) { this.setAttribute("id", value); }
  get className() { return this.getAttribute("class") ?? ""; }
  set className(value) { this.setAttribute("class", value); }
  get title() { return this.getAttribute("title") ?? ""; }
  set title(value) { this.setAttribute("title", value); }
  get role() { return this.getAttribute("role"); }
  set role(value) { this.setAttribute("role", value); }
  get hidden() { return this.hasAttribute("hidden"); }
  set hidden(value) { this.toggleAttribute("hidden", Boolean(value)); }
  get disabled() { return this.hasAttribute("disabled"); }
  set disabled(value) { this.toggleAttribute("disabled", Boolean(value)); }
  get tabIndex() { return Number(this.getAttribute("tabindex") ?? -1); }
  set tabIndex(value) { this.setAttribute("tabindex", String(value)); }
  get draggable() { return this.getAttribute("draggable") === "true"; }
  set draggable(value) { this.setAttribute("draggable", String(Boolean(value))); }
  get htmlFor() { return this.getAttribute("for") ?? ""; }
  set htmlFor(value) { this.setAttribute("for", value); }
  get type() { return this.getAttribute("type") ?? (this.localName === "button" ? "submit" : "text"); }
  set type(value) { this.setAttribute("type", value); }
  get open() { return this.hasAttribute("open"); }
  set open(value) { this.toggleAttribute("open", Boolean(value)); }
  get isContentEditable() { return this.getAttribute("contenteditable") === "true"; }

  get children() {
    return this.childNodes.filter((node) => node.nodeType === ELEMENT_NODE);
  }

  get firstElementChild() {
    return this.children[0] ?? null;
  }

  get offsetParent() {
    return this.isConnected && !this.closest("[hidden]") ? this.parentNode : null;
  }

  get offsetTop() { return 0; }
  get offsetHeight() { return 0; }
  get scrollHeight() { return 0; }
  get clientHeight() { return 0; }
  get scrollWidth() { return 0; }

  getAttribute(name) {
    return this.attributes.has(name) ? this.attributes.get(name) : null;
  }

  setAttribute(name, value) {
    const old = this.getAttribute(name);
    this.attributes.set(name, String(value));
    this.ownerDocument?.notifyAttribute(this, name, old);
  }

  removeAttribute(name) {
    if (!this.attributes.has(name)) return;
    const old = this.getAttribute(name);
    this.attributes.delete(name);
    this.ownerDocument?.notifyAttribute(this, name, old);
  }

  hasAttribute(name) {
    return this.attributes.has(name);
  }

  toggleAttribute(name, force) {
    const next = force === undefined ? !this.hasAttribute(name) : force;
    if (next && !this.hasAttribute(name)) this.setAttribute(name, "");
    if (!next) this.removeAttribute(name);
    return next;
  }

  getBoundingClientRect() {
    return { x: 0, y: 0, left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 };
  }

  scrollIntoView() {}
  setPointerCapture() {}
  releasePointerCapture() {}
  hasPointerCapture() { return false; }

  focus() {
    if (!this.isConnected) return;
    this.ownerDocument.activeElement = this;
  }

  blur() {
    if (this.ownerDocument.activeElement === this) this.ownerDocument.activeElement = this.ownerDocument.body;
  }

  click() {
    if (this.disabled) return;
    const event = new FakeEvent("click", { bubbles: true });
    this.dispatchEvent(event);
    if (!event.defaultPrevented && this.localName === "button" && this.type === "submit") {
      this.closest("form")?.requestSubmit?.();
    }
  }

  querySelectorAll(selector) {
    const matcher = compileSelector(selector);
    const found = [];
    const walk = (node) => {
      for (const child of node.children) {
        if (matcher(child, this)) found.push(child);
        walk(child);
      }
    };
    walk(this);
    return found;
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] ?? null;
  }

  matches(selector) {
    return compileSelector(selector)(this, null);
  }

  closest(selector) {
    const matcher = compileSelector(selector);
    for (let node = this; node && node.nodeType === ELEMENT_NODE; node = node.parentNode) {
      if (matcher(node, null)) return node;
    }
    return null;
  }

  cloneInto(document, deep) {
    const clone = document.createElement(this.localName);
    for (const [name, value] of this.attributes) clone.attributes.set(name, value);
    if (deep) for (const child of this.childNodes) clone.appendChild(child.cloneInto(document, true));
    return clone;
  }
}

// ------------------------------------------------------------ form elements

class FakeHTMLElement extends FakeElement {}

class FakeInputElement extends FakeHTMLElement {
  constructor(document, tagName) {
    super(document, tagName);
    this.currentValue = null;
    this.currentChecked = null;
    this.selectionStart = 0;
    this.selectionEnd = 0;
  }

  get value() { return this.currentValue ?? this.getAttribute("value") ?? ""; }
  set value(value) { this.currentValue = String(value); }
  get checked() { return this.currentChecked ?? this.hasAttribute("checked"); }
  set checked(value) { this.currentChecked = Boolean(value); }
  get readOnly() { return this.hasAttribute("readonly"); }
  set readOnly(value) { this.toggleAttribute("readonly", Boolean(value)); }
  get maxLength() { return Number(this.getAttribute("maxlength") ?? -1); }
  set maxLength(value) { this.setAttribute("maxlength", String(value)); }
  select() {}
  setSelectionRange() {}
}

class FakeTextAreaElement extends FakeInputElement {
  get value() { return this.currentValue ?? this.textContent; }
  set value(value) { this.currentValue = String(value); }
}

class FakeOptionElement extends FakeHTMLElement {
  get value() { return this.getAttribute("value") ?? this.textContent; }
  set value(value) { this.setAttribute("value", value); }
  get selected() {
    return this.currentSelected ?? this.hasAttribute("selected");
  }
  set selected(value) {
    const select = this.closest("select");
    if (value && select) for (const option of select.options) option.currentSelected = false;
    this.currentSelected = Boolean(value);
  }
}

class FakeSelectElement extends FakeHTMLElement {
  get options() {
    return this.querySelectorAll("option");
  }

  get selectedIndex() {
    const options = this.options;
    const explicit = options.findIndex((option) => option.selected);
    return explicit >= 0 ? explicit : options.length ? 0 : -1;
  }

  get selectedOptions() {
    const option = this.options[this.selectedIndex];
    return option ? [option] : [];
  }

  get value() {
    return this.options[this.selectedIndex]?.value ?? "";
  }

  set value(value) {
    const options = this.options;
    for (const option of options) option.currentSelected = false;
    const match = options.find((option) => option.value === String(value));
    if (match) match.currentSelected = true;
    else if (options.length) {
      // A value no option carries leaves the select with no selection.
      for (const option of options) option.currentSelected = false;
      this.noSelection = true;
      return;
    }
    this.noSelection = false;
  }
}

class FakeFormElement extends FakeHTMLElement {
  requestSubmit() {
    this.dispatchEvent(new FakeEvent("submit", { bubbles: true }));
  }

  get elements() {
    return this.querySelectorAll("input, select, textarea, button");
  }
}

class FakeProgressElement extends FakeHTMLElement {
  constructor(document, tagName) {
    super(document, tagName);
    this.value = 0;
    this.max = 1;
  }
}

class FakeTimeElement extends FakeHTMLElement {
  get dateTime() { return this.getAttribute("datetime") ?? ""; }
  set dateTime(value) { this.setAttribute("datetime", value); }
}

class FakeLabelElement extends FakeHTMLElement {}
class FakeSVGElement extends FakeElement {}

const elementClasses = {
  HTMLElement: FakeHTMLElement,
  HTMLButtonElement: class extends FakeHTMLElement {},
  HTMLInputElement: FakeInputElement,
  HTMLTextAreaElement: FakeTextAreaElement,
  HTMLSelectElement: FakeSelectElement,
  HTMLOptionElement: FakeOptionElement,
  HTMLFormElement: FakeFormElement,
  HTMLDivElement: class extends FakeHTMLElement {},
  HTMLSpanElement: class extends FakeHTMLElement {},
  HTMLParagraphElement: class extends FakeHTMLElement {},
  HTMLHeadingElement: class extends FakeHTMLElement {},
  HTMLUListElement: class extends FakeHTMLElement {},
  HTMLOListElement: class extends FakeHTMLElement {},
  HTMLLIElement: class extends FakeHTMLElement {},
  HTMLAnchorElement: class extends FakeHTMLElement {},
  HTMLLabelElement: FakeLabelElement,
  HTMLProgressElement: FakeProgressElement,
  HTMLDetailsElement: class extends FakeHTMLElement {},
  HTMLPreElement: class extends FakeHTMLElement {},
  HTMLDListElement: class extends FakeHTMLElement {},
  HTMLTimeElement: FakeTimeElement,
  HTMLImageElement: class extends FakeHTMLElement {},
  HTMLFieldSetElement: class extends FakeHTMLElement {},
  HTMLLegendElement: class extends FakeHTMLElement {},
  HTMLOutputElement: class extends FakeHTMLElement {},
  HTMLDialogElement: class extends FakeHTMLElement {},
  SVGElement: FakeSVGElement,
  SVGSVGElement: class extends FakeSVGElement {},
};

const tagClasses = {
  button: "HTMLButtonElement", input: "HTMLInputElement", textarea: "HTMLTextAreaElement",
  select: "HTMLSelectElement", option: "HTMLOptionElement", form: "HTMLFormElement", div: "HTMLDivElement",
  span: "HTMLSpanElement", p: "HTMLParagraphElement", h1: "HTMLHeadingElement", h2: "HTMLHeadingElement",
  h3: "HTMLHeadingElement", h4: "HTMLHeadingElement", h5: "HTMLHeadingElement", h6: "HTMLHeadingElement",
  ul: "HTMLUListElement", ol: "HTMLOListElement", li: "HTMLLIElement", a: "HTMLAnchorElement",
  label: "HTMLLabelElement", progress: "HTMLProgressElement", details: "HTMLDetailsElement",
  pre: "HTMLPreElement", dl: "HTMLDListElement", time: "HTMLTimeElement", img: "HTMLImageElement",
  fieldset: "HTMLFieldSetElement", legend: "HTMLLegendElement", output: "HTMLOutputElement",
  dialog: "HTMLDialogElement",
};

// ----------------------------------------------------------------- selectors

function parseSimple(source) {
  // A compound selector: tag, #id, .class, [attr], [attr=value], :not(...).
  const parts = [];
  let rest = source;
  const tag = rest.match(/^([a-zA-Z][\w-]*|\*)/);
  if (tag) {
    if (tag[1] !== "*") parts.push((element) => element.localName === tag[1].toLowerCase());
    rest = rest.slice(tag[0].length);
  }
  while (rest) {
    let match;
    if ((match = rest.match(/^#([\w-]+)/))) {
      const id = match[1];
      parts.push((element) => element.getAttribute("id") === id);
    } else if ((match = rest.match(/^\.([\w-]+)/))) {
      const name = match[1];
      parts.push((element) => element.classes().includes(name));
    } else if ((match = rest.match(/^\[([\w-]+)(?:([~^$*]?=)(?:"([^"]*)"|'([^']*)'|([^\]]*)))?\]/))) {
      const [, name, operator, double, single, bare] = match;
      const expected = double ?? single ?? bare;
      parts.push((element) => {
        const value = element.getAttribute(name);
        if (value === null) return false;
        if (!operator) return true;
        if (operator === "=") return value === expected;
        if (operator === "^=") return value.startsWith(expected);
        if (operator === "$=") return value.endsWith(expected);
        if (operator === "*=") return value.includes(expected);
        if (operator === "~=") return value.split(/\s+/).includes(expected);
        return false;
      });
    } else if ((match = rest.match(/^:not\(([^)]*)\)/))) {
      const inner = compileSelector(match[1]);
      parts.push((element) => !inner(element, null));
    } else if ((match = rest.match(/^:scope/))) {
      parts.push((element, scope) => element === scope);
    } else {
      throw new Error(`fake-dom: unsupported selector part "${rest}" in "${source}"`);
    }
    rest = rest.slice(match[0].length);
  }
  return (element, scope) => parts.every((part) => part(element, scope));
}

function splitTopLevel(selector, separator) {
  const out = [];
  let depth = 0;
  let quote = "";
  let current = "";
  for (const ch of selector) {
    if (quote) {
      current += ch;
      if (ch === quote) quote = "";
      continue;
    }
    if (ch === '"' || ch === "'") quote = ch;
    if (ch === "(" || ch === "[") depth += 1;
    if (ch === ")" || ch === "]") depth -= 1;
    if (ch === separator && depth === 0) {
      out.push(current);
      current = "";
    } else {
      current += ch;
    }
  }
  out.push(current);
  return out;
}

function tokenizeComplex(selector) {
  // Returns [compound, combinator, compound, ...] with " " and ">".
  const tokens = [];
  let current = "";
  let depth = 0;
  let quote = "";
  const flush = () => {
    if (current.trim()) tokens.push(current.trim());
    current = "";
  };
  for (let i = 0; i < selector.length; i += 1) {
    const ch = selector[i];
    if (quote) {
      current += ch;
      if (ch === quote) quote = "";
      continue;
    }
    if (ch === '"' || ch === "'") { quote = ch; current += ch; continue; }
    if (ch === "(" || ch === "[") depth += 1;
    if (ch === ")" || ch === "]") depth -= 1;
    if (depth === 0 && (ch === ">" || /\s/.test(ch))) {
      flush();
      if (ch === ">") tokens.push(">");
      else if (tokens.length && tokens[tokens.length - 1] !== ">" && tokens[tokens.length - 1] !== " ") {
        // Whitespace is a descendant combinator unless a ">" follows.
        const next = selector.slice(i).trimStart();
        if (!next.startsWith(">")) tokens.push(" ");
      }
      continue;
    }
    current += ch;
  }
  flush();
  return tokens;
}

const selectorCache = new Map();

export function compileSelector(selector) {
  if (selectorCache.has(selector)) return selectorCache.get(selector);
  const alternatives = splitTopLevel(selector, ",").map((part) => {
    const tokens = tokenizeComplex(part.trim());
    const compounds = [];
    const combinators = [];
    tokens.forEach((token, index) => {
      if (index % 2 === 0) compounds.push(parseSimple(token));
      else combinators.push(token);
    });
    return (element, scope) => {
      const matchFrom = (node, index) => {
        if (!compounds[index](node, scope)) return false;
        if (index === 0) return true;
        const combinator = combinators[index - 1];
        if (combinator === ">") {
          const parent = node.parentNode;
          return Boolean(parent && parent.nodeType === ELEMENT_NODE && matchFrom(parent, index - 1));
        }
        for (let parent = node.parentNode; parent && parent.nodeType === ELEMENT_NODE; parent = parent.parentNode) {
          if (matchFrom(parent, index - 1)) return true;
        }
        return false;
      };
      return matchFrom(element, compounds.length - 1);
    };
  });
  const matcher = (element, scope) => alternatives.some((alternative) => alternative(element, scope));
  selectorCache.set(selector, matcher);
  return matcher;
}

// ------------------------------------------------------------------ parsing

function decodeEntities(text) {
  return text
    .replace(/&nbsp;/g, " ")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&#(\d+);/g, (_, code) => String.fromCodePoint(Number(code)))
    .replace(/&amp;/g, "&");
}

function parseAttributes(source) {
  const attributes = [];
  const pattern = /([^\s"'>\/=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?/g;
  let match;
  while ((match = pattern.exec(source))) {
    attributes.push([match[1].toLowerCase(), decodeEntities(match[2] ?? match[3] ?? match[4] ?? "")]);
  }
  return attributes;
}

export function parseInto(document, parent, html) {
  const stack = [parent];
  const pattern = /<!--[\s\S]*?-->|<!DOCTYPE[^>]*>|<(script|style)\b([^>]*)>([\s\S]*?)<\/\1>|<\/([a-zA-Z][\w-]*)\s*>|<([a-zA-Z][\w-]*)((?:[^>"']|"[^"]*"|'[^']*')*)>|([^<]+)/gi;
  let match;
  while ((match = pattern.exec(html))) {
    const top = stack[stack.length - 1];
    if (match[1]) {
      const element = document.createElement(match[1]);
      for (const [name, value] of parseAttributes(match[2])) element.attributes.set(name, value);
      top.appendChild(element);
    } else if (match[4]) {
      const name = match[4].toLowerCase();
      for (let index = stack.length - 1; index > 0; index -= 1) {
        if (stack[index].localName === name) {
          stack.length = index;
          break;
        }
      }
    } else if (match[5]) {
      const name = match[5].toLowerCase();
      const rawAttributes = match[6] ?? "";
      const selfClosing = /\/\s*$/.test(rawAttributes);
      const element = name === "svg" || top instanceof FakeSVGElement
        ? document.createElementNS("http://www.w3.org/2000/svg", name)
        : document.createElement(name);
      for (const [attribute, value] of parseAttributes(rawAttributes.replace(/\/\s*$/, ""))) {
        element.attributes.set(attribute, value);
      }
      top.appendChild(element);
      if (!selfClosing && !VOID_TAGS.has(name)) stack.push(element);
    } else if (match[7]) {
      if (/\S/.test(match[7]) || top.localName === "pre" || top.localName === "textarea") {
        top.appendChild(document.createTextNode(decodeEntities(match[7])));
      }
    }
  }
}

// ----------------------------------------------------------------- document

export class FakeDocument extends FakeNode {
  constructor() {
    super(null);
    this.ownerDocument = this;
    this.nodeType = 9;
    this.mutationObservers = [];
    this.activeElement = null;
    this.hidden = false;
    this.title = "";
    this.focused = true;
    // Font loading resolves at once: the harness has no faces to wait for.
    this.fonts = { load: async () => [], ready: Promise.resolve() };
    this.documentElement = this.createElement("html");
    this.appendChild(this.documentElement);
    this.head = this.createElement("head");
    this.body = this.createElement("body");
    this.documentElement.append(this.head, this.body);
    this.activeElement = this.body;
  }

  createElement(tagName) {
    const name = String(tagName).toLowerCase();
    const type = elementClasses[tagClasses[name] ?? "HTMLElement"];
    return new type(this, name);
  }

  createElementNS(namespace, tagName) {
    if (namespace === "http://www.w3.org/2000/svg") {
      const type = tagName === "svg" ? elementClasses.SVGSVGElement : elementClasses.SVGElement;
      return new type(this, tagName);
    }
    return this.createElement(tagName);
  }

  createTextNode(data) {
    return new FakeText(this, String(data));
  }

  createDocumentFragment() {
    return new FakeFragment(this);
  }

  get children() {
    return [this.documentElement];
  }

  querySelectorAll(selector) {
    const matcher = compileSelector(selector);
    const found = [];
    const walk = (node) => {
      for (const child of node.children) {
        if (matcher(child, null)) found.push(child);
        walk(child);
      }
    };
    walk(this);
    return found;
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] ?? null;
  }

  getElementById(id) {
    return this.querySelector(`#${CSS_ESCAPE(id)}`);
  }

  hasFocus() {
    return this.focused;
  }

  notifyAttribute(target, attributeName, oldValue) {
    for (const observer of this.mutationObservers) observer.notify(target, attributeName, oldValue);
  }
}

function CSS_ESCAPE(value) {
  return String(value).replace(/[^\w-]/g, (ch) => `\\${ch}`);
}

class FakeMutationObserver {
  constructor(callback, document) {
    this.callback = callback;
    this.document = document;
    this.targets = [];
    this.pending = [];
  }

  observe(target, options = {}) {
    this.targets.push({ target, options });
    if (!this.document.mutationObservers.includes(this)) this.document.mutationObservers.push(this);
  }

  disconnect() {
    this.targets = [];
    this.document.mutationObservers = this.document.mutationObservers.filter((observer) => observer !== this);
  }

  takeRecords() {
    const records = this.pending;
    this.pending = [];
    return records;
  }

  notify(node, attributeName, oldValue) {
    const watching = this.targets.some(({ target, options }) =>
      options.attributes &&
      (target === node || (options.subtree && target.contains(node))) &&
      (!options.attributeFilter || options.attributeFilter.includes(attributeName)));
    if (!watching) return;
    const record = { type: "attributes", target: node, attributeName, oldValue };
    this.pending.push(record);
    if (this.pending.length === 1) {
      queueMicrotask(() => {
        const records = this.takeRecords();
        if (records.length) this.callback(records, this);
      });
    }
  }
}

class FakeStorage {
  constructor() {
    this.values = new Map();
  }

  get length() { return this.values.size; }
  key(index) { return [...this.values.keys()][index] ?? null; }
  getItem(key) { return this.values.has(key) ? this.values.get(key) : null; }
  setItem(key, value) { this.values.set(key, String(value)); }
  removeItem(key) { this.values.delete(key); }
  clear() { this.values.clear(); }
}

/**
 * Installs a fresh document built from `html` and the window globals the
 * controllers read. Returns the fake window plus helpers to flush animation
 * frames and restore the previous globals.
 */
export function installFakeDom(html) {
  const document = new FakeDocument();
  const bodyMatch = html.match(/<body\b[^>]*>([\s\S]*)<\/body>/i);
  parseInto(document, document.body, bodyMatch ? bodyMatch[1] : html);
  const frames = [];
  // Timers are tracked so restore() can cancel them: a debounce left running
  // by one test must not fire into the next test's window and backend.
  const realSetTimeout = globalThis.setTimeout;
  const realClearTimeout = globalThis.clearTimeout;
  const realSetInterval = globalThis.setInterval;
  const realClearInterval = globalThis.clearInterval;
  const timeouts = new Set();
  const intervals = new Set();
  const timers = {
    setTimeout(callback, delay, ...args) {
      const handle = realSetTimeout(() => {
        timeouts.delete(handle);
        callback(...args);
      }, delay);
      timeouts.add(handle);
      return handle;
    },
    clearTimeout(handle) {
      timeouts.delete(handle);
      realClearTimeout(handle);
    },
    setInterval(callback, delay, ...args) {
      const handle = realSetInterval(callback, delay, ...args);
      intervals.add(handle);
      return handle;
    },
    clearInterval(handle) {
      intervals.delete(handle);
      realClearInterval(handle);
    },
  };
  const window = new (class extends EventTargetMixin {})();
  const storage = new FakeStorage();
  Object.assign(window, {
    document,
    location: { hash: "" },
    innerWidth: 1280,
    innerHeight: 800,
    localStorage: storage,
    navigator: { platform: "MacIntel", userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15", language: "en-US", userAgentData: undefined, clipboard: undefined },
    matchMedia: (query) => ({
      matches: false,
      media: query,
      addEventListener() {},
      removeEventListener() {},
      addListener() {},
      removeListener() {},
    }),
    requestAnimationFrame: (callback) => {
      frames.push(callback);
      return frames.length;
    },
    cancelAnimationFrame: () => {},
    ...timers,
    confirm: () => true,
    open: () => null,
  });
  document.defaultView = window;
  const globals = {
    window,
    document,
    localStorage: storage,
    navigator: window.navigator,
    matchMedia: window.matchMedia,
    requestAnimationFrame: window.requestAnimationFrame,
    cancelAnimationFrame: window.cancelAnimationFrame,
    ...timers,
    MutationObserver: class extends FakeMutationObserver {
      constructor(callback) { super(callback, document); }
    },
    ResizeObserver: class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
    Element: FakeElement,
    Node: FakeNode,
    Event: FakeEvent,
    KeyboardEvent: FakeEvent,
    CustomEvent: FakeEvent,
    CSS: { escape: CSS_ESCAPE },
    ...elementClasses,
  };
  const previous = new Map();
  for (const [name, value] of Object.entries(globals)) {
    previous.set(name, Object.getOwnPropertyDescriptor(globalThis, name));
    Object.defineProperty(globalThis, name, { value, configurable: true, writable: true });
  }
  window.window = window;
  for (const [name, value] of Object.entries(globals)) if (!(name in window)) window[name] = value;
  return {
    window,
    document,
    /** Runs every queued animation frame, including frames they queue. */
    flushFrames(limit = 20) {
      for (let round = 0; round < limit && frames.length; round += 1) {
        const batch = frames.splice(0);
        for (const frame of batch) frame(0);
      }
    },
    restore() {
      for (const handle of timeouts) realClearTimeout(handle);
      for (const handle of intervals) realClearInterval(handle);
      timeouts.clear();
      intervals.clear();
      frames.length = 0;
      for (const [name, descriptor] of previous) {
        if (descriptor) Object.defineProperty(globalThis, name, descriptor);
        else delete globalThis[name];
      }
    },
  };
}

/** Dispatches a bubbling event of `type` on `target` with `init` fields. */
export function fire(target, type, init = {}) {
  const event = new FakeEvent(type, { bubbles: true, ...init });
  target.dispatchEvent(event);
  return event;
}
