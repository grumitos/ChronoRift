import { isTauri } from "../api";
import { changeReplayFolder, openReplayFile } from "../actions";
import { h } from "../dom";
import { state, subscribe } from "../state";

export function mountTopBar(root: HTMLElement) {
  const folder = h("span", { class: "topbar-path mono" });
  root.append(
    h("span", { class: "app-name", text: "ChronoRift" }),
    folder,
    h("span", { class: "topbar-actions" },
      h("button", { class: "button", type: "button", text: "Change folder…", on: { click: () => void changeReplayFolder() } }),
      h("button", { class: "button", type: "button", text: "Open file…", on: { click: () => void openReplayFile() } }),
    ),
  );
  const render = () => {
    const path = state.settings.replayFolder;
    folder.textContent = path ?? "No replay folder";
    folder.title = path ?? "";
    folder.classList.toggle("is-empty", !path);
  };
  subscribe(["settings"], render);
  render();
}

export function mountStatusBar(root: HTMLElement) {
  const text = h("span", { class: "status-text", attrs: { role: "status", "aria-live": "polite" } });
  root.append(text);
  if (!isTauri) root.append(h("span", { class: "status-mode", text: "Browser preview · sample data" }));
  const render = () => {
    text.textContent = state.status.text;
    text.title = state.status.text;
    text.classList.toggle("error-text", state.status.error);
  };
  subscribe(["status"], render);
  render();
}
