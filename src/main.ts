import "./styles.css";
import { init } from "./actions";
import { h } from "./dom";
import { mountDetail } from "./views/detail";
import { mountLibrary } from "./views/library";
import { mountStatusBar, mountTopBar } from "./views/shell";

const app = document.querySelector<HTMLElement>("#app");
if (!app) throw new Error("Missing #app");

const topBar = h("header", { class: "topbar" });
const library = h("aside", { class: "library", attrs: { "aria-label": "Replay library" } });
const detail = h("main", { class: "detail" });
const statusBar = h("footer", { class: "statusbar" });

app.append(topBar, h("div", { class: "panes" }, library, detail), statusBar);

mountTopBar(topBar);
mountLibrary(library);
mountDetail(detail);
mountStatusBar(statusBar);

void init();
