// Tiny element helper. Text always goes through textContent; never innerHTML.

type Child = Node | string | number | null | undefined | false;

type Handlers = {
  [E in keyof HTMLElementEventMap]?: (event: HTMLElementEventMap[E]) => void;
};

export type Props = {
  class?: string;
  text?: string;
  attrs?: Record<string, string>;
  on?: Handlers;
  [property: string]: unknown;
};

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Props = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value === undefined) continue;
    if (key === "class") element.className = String(value);
    else if (key === "text") element.textContent = String(value);
    else if (key === "attrs") {
      for (const [name, attr] of Object.entries(value as Record<string, string>)) {
        element.setAttribute(name, attr);
      }
    } else if (key === "on") {
      for (const [name, handler] of Object.entries(value as Record<string, EventListener>)) {
        element.addEventListener(name, handler);
      }
    } else {
      (element as unknown as Record<string, unknown>)[key] = value;
    }
  }
  append(element, children);
  return element;
}

export function append(parent: Node, children: Child[]) {
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    parent.appendChild(child instanceof Node ? child : document.createTextNode(String(child)));
  }
}

export function replace(parent: Element, ...children: Child[]) {
  parent.replaceChildren();
  append(parent, children);
}

/** Sets an input's value unless the user is typing in it. */
export function syncValue(input: HTMLInputElement, value: string) {
  if (document.activeElement !== input && input.value !== value) input.value = value;
}
