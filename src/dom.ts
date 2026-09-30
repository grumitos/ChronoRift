// Utilidades de DOM: creación de elementos, iconos SVG en línea y búsqueda estricta.

type Child = Node | string | number | null | undefined | false;

type Props = {
  class?: string;
  text?: string | number;
  title?: string;
  dataset?: Record<string, string>;
  attrs?: Record<string, string | number | boolean | null | undefined>;
  style?: Record<string, string>;
  on?: { [K in keyof HTMLElementEventMap]?: (event: HTMLElementEventMap[K]) => void };
};

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Props = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  if (props.class) element.className = props.class;
  if (props.text != null) element.textContent = String(props.text);
  if (props.title) element.title = props.title;
  for (const [key, value] of Object.entries(props.dataset ?? {})) element.dataset[key] = value;
  for (const [key, value] of Object.entries(props.attrs ?? {})) {
    if (value === false || value == null) continue;
    element.setAttribute(key, value === true ? "" : String(value));
  }
  for (const [key, value] of Object.entries(props.style ?? {})) element.style.setProperty(key, value);
  for (const [type, handler] of Object.entries(props.on ?? {})) {
    element.addEventListener(type, handler as EventListener);
  }
  append(element, children);
  return element;
}

export function append(parent: Node, children: Child[]) {
  for (const child of children) {
    if (child == null || child === false) continue;
    parent.appendChild(
      typeof child === "string" || typeof child === "number"
        ? document.createTextNode(String(child))
        : child,
    );
  }
}

type Shape =
  | { d: string }
  | { circle: [number, number, number] }
  | { rect: [number, number, number, number, number] };

const ICONS = {
  search: [{ circle: [11, 11, 6.5] }, { d: "M20 20l-4.2-4.2" }],
  refresh: [{ d: "M20 11.5a8 8 0 1 1-2.35-5.65" }, { d: "M20 4.5v5h-5" }],
  folder: [{ d: "M3 7.5A2 2 0 0 1 5 5.5h4l2 2h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" }],
  file: [{ d: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" }, { d: "M14 3v5h5" }],
  exe: [
    { d: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" },
    { d: "M14 3v5h5" },
    { d: "M9.5 15.5 12 13l2.5 2.5M12 13v5" },
  ],
  copy: [
    { rect: [8.5, 8.5, 11.5, 11.5, 2] },
    { d: "M15.5 8.5V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v7.5a2 2 0 0 0 2 2h2.5" },
  ],
  download: [{ d: "M12 4v11" }, { d: "M7.5 10.5 12 15l4.5-4.5" }, { d: "M5 19.5h14" }],
  close: [{ d: "M6 6l12 12M18 6 6 18" }],
  external: [{ d: "M14 5h5v5" }, { d: "M19 5l-8 8" }, { d: "M18 14v4a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h4" }],
  minimize: [{ d: "M5 12h14" }],
  maximize: [{ rect: [5.5, 5.5, 13, 13, 1.5] }],
  restore: [
    { rect: [5, 8.5, 10.5, 10.5, 1.5] },
    { d: "M8.5 8.5V6.5A1.5 1.5 0 0 1 10 5h7.5A1.5 1.5 0 0 1 19 6.5V14a1.5 1.5 0 0 1-1.5 1.5h-2" },
  ],
  check: [{ d: "M5 12.5l4.5 4.5L19 7.5" }],
  chevron: [{ d: "M7 10l5 5 5-5" }],
  drive: [{ rect: [3, 12, 18, 7.5, 2] }, { d: "M5.5 12 8 5h8l2.5 7" }, { d: "M7.5 15.75h.01" }],
  user: [{ circle: [12, 8, 4] }, { d: "M4.5 20c1.4-3.4 4.3-5 7.5-5s6.1 1.6 7.5 5" }],
  play: [{ d: "M7 4.8v14.4a.8.8 0 0 0 1.2.7l11.3-7.2a.8.8 0 0 0 0-1.4L8.2 4.1A.8.8 0 0 0 7 4.8z" }],
  stop: [{ rect: [6, 6, 12, 12, 2] }],
  // Rebobinar sobre el minimapa: las líneas superior e inferior en L, con los huecos de las bases
  // cortados a 45° sobre la línea central. Cuadrícula 24 con margen 2; grosor t = 2,6, hueco = t,
  // chaflán 18 % (como la loseta del ícono) y separación entre flechas t / 2. Solo relleno.
  logo: [
    { d: "M2 5.6L5.6 2H20.16L17.56 4.6H6.68L4.6 6.68V17.56L2 20.16Z" },
    { d: "M22 18.4L18.4 22H3.84L6.44 19.4H17.32L19.4 17.32V6.44L22 3.84Z" },
    { d: "M11.35 7.9V16.1L6.35 12Z" },
    { d: "M17.65 7.9V16.1L12.65 12Z" },
  ],
} satisfies Record<string, Shape[]>;

export type IconName = keyof typeof ICONS;

const FILLED = new Set<IconName>(["play", "stop", "logo"]);
const SVG_NS = "http://www.w3.org/2000/svg";

export function icon(name: IconName, className = "icon") {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("class", `${className}${FILLED.has(name) ? " icon--filled" : ""}`);
  for (const shape of ICONS[name] as Shape[]) {
    let node: SVGElement;
    if ("d" in shape) {
      node = document.createElementNS(SVG_NS, "path");
      node.setAttribute("d", shape.d);
    } else if ("circle" in shape) {
      node = document.createElementNS(SVG_NS, "circle");
      const [cx, cy, r] = shape.circle;
      node.setAttribute("cx", String(cx));
      node.setAttribute("cy", String(cy));
      node.setAttribute("r", String(r));
    } else {
      node = document.createElementNS(SVG_NS, "rect");
      const [x, y, width, height, rx] = shape.rect;
      node.setAttribute("x", String(x));
      node.setAttribute("y", String(y));
      node.setAttribute("width", String(width));
      node.setAttribute("height", String(height));
      node.setAttribute("rx", String(rx));
    }
    svg.appendChild(node);
  }
  return svg;
}

/** Reemplaza los marcadores `<span data-icon="…">` del HTML estático por su SVG. */
export function hydrateIcons(root: ParentNode = document) {
  for (const slot of root.querySelectorAll<HTMLElement>("[data-icon]")) {
    const name = slot.dataset.icon as IconName;
    const classes = slot.className.split(/\s+/).filter(Boolean);
    if (!classes.includes("icon")) classes.unshift("icon");
    if (name in ICONS) slot.replaceWith(icon(name, classes.join(" ")));
  }
}

export function must<T extends Element>(
  selector: string,
  constructor: { new (): T; prototype: T },
  root: ParentNode = document,
): T {
  const element = root.querySelector(selector);
  if (!(element instanceof constructor)) throw new Error(`Falta el elemento ${selector}`);
  return element;
}
