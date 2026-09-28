import type { ReplaySummary } from "../api";
import { changeReplayFolder, selectReplay, setFilter } from "../actions";
import { findChampion, type Asset } from "../datadragon";
import { h, replace, syncValue } from "../dom";
import { formatDuration, formatRelative, stripExtension, teamLabel } from "../format";
import { state, subscribe } from "../state";

export function mountLibrary(root: HTMLElement) {
  const filter = h("input", {
    class: "input filter",
    type: "search",
    placeholder: "Filter replays",
    title: "Champion, player, patch, server, or match id",
    spellcheck: false,
    attrs: { "aria-label": "Filter replays by champion, player, patch, server, or match id" },
    on: { input: () => setFilter(filter.value) },
  });
  const count = h("span", { class: "library-count" });
  const list = h("ol", { class: "replay-list", attrs: { "aria-label": "Replays" } });
  const footer = h("div", { class: "library-footer" });

  list.addEventListener("keydown", (event) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const buttons = [...list.querySelectorAll<HTMLButtonElement>("button.replay-row")];
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = buttons[index + (event.key === "ArrowDown" ? 1 : -1)];
    if (next) {
      event.preventDefault();
      next.focus();
      next.click();
    }
  });

  const head = h("div", { class: "library-head" }, filter, count);
  root.append(head, list, footer);

  const render = () => {
    syncValue(filter, state.filter);
    const library = state.library;
    head.hidden = !library || library.replays.length === 0;

    if (!state.settings.replayFolder) {
      count.textContent = "";
      replace(list, emptyState(
        "No replay folder",
        "Choose the folder where League saves replays, usually Documents\\League of Legends\\Replays. ChronoRift only reads it.",
        h("button", { class: "button", type: "button", text: "Choose folder…", on: { click: () => void changeReplayFolder() } }),
      ));
      replace(footer);
      return;
    }
    if (state.libraryLoading && !library) {
      count.textContent = "";
      replace(list, emptyState("Reading folder…", ""));
      return;
    }
    if (state.libraryError) {
      count.textContent = "";
      replace(list, emptyState("Folder could not be read", state.libraryError, null, true));
      replace(footer);
      return;
    }
    if (!library) return;

    if (library.replays.length === 0) {
      count.textContent = "";
      replace(list, emptyState(
        "No replays here",
        "This folder has no .rofl files. Change the folder, or use Open file… for a single replay.",
      ));
    } else {
      const query = state.filter.trim().toLowerCase();
      const visible = query
        ? library.replays.filter((replay) => matches(replay, query, state.libraryChampions))
        : library.replays;
      count.textContent = query
        ? `${visible.length} of ${library.replays.length}`
        : String(library.replays.length);
      if (visible.length === 0) {
        replace(list, emptyState("No matches", `Nothing matches “${state.filter.trim()}”.`));
      } else {
        const hadFocus = list.contains(document.activeElement);
        replace(list, ...visible.map((replay) => row(replay, state.libraryChampions)));
        if (hadFocus) list.querySelector<HTMLButtonElement>("button.is-selected")?.focus();
      }
    }

    if (library.errors.length > 0) {
      const details = h("details", { class: "library-errors" },
        h("summary", { text: `${library.errors.length} file${library.errors.length === 1 ? "" : "s"} could not be read` }),
        h("ul", {}, ...library.errors.map((error) =>
          h("li", {}, h("span", { class: "mono", text: error.path }), " ", error.message),
        )),
      );
      replace(footer, details);
    } else {
      replace(footer);
    }
  };

  subscribe(["library", "settings", "selection"], render);
  render();
}

/** Matches champions (id and name in the loaded locale), player names, patch, server, and match id. */
function matches(replay: ReplaySummary, query: string, champions: Map<string, Asset> | null): boolean {
  const champs = [...replay.blueChampions, ...replay.redChampions];
  const fields = [
    replay.fileName,
    replay.patch,
    replay.region,
    replay.matchId,
    replay.gameVersion,
    ...champs,
    ...champs.map((id) => findChampion(champions, id)?.name),
    ...replay.players,
  ];
  return fields.some((field) => Boolean(field) && field!.toLowerCase().includes(query));
}

function row(replay: ReplaySummary, champions: Map<string, Asset> | null) {
  const selected = replay.path === state.selectedPath;
  const title = stripExtension(replay.fileName);

  const meta: string[] = [];
  if (replay.status === "parsed") {
    if (replay.patch) meta.push(replay.patch);
    if (replay.region) meta.push(replay.region);
    if (replay.gameDurationMs) meta.push(formatDuration(replay.gameDurationMs));
  } else {
    meta.push(replay.status === "unsupported" ? "Unsupported format" : "Not a readable replay");
  }

  const winner = teamLabel(replay.winningTeam);
  const score =
    replay.status === "parsed"
      ? h("span", { class: "row-score" },
          h("span", { class: winner === "Blue" ? "strong" : "", text: String(replay.blueKills) }),
          h("span", { class: "dim", text: "–" }),
          h("span", { class: winner === "Red" ? "strong" : "", text: String(replay.redKills) }),
        )
      : null;

  const hasChampions = replay.blueChampions.length > 0 || replay.redChampions.length > 0;

  const button = h("button", {
    class: "replay-row",
    type: "button",
    title: replay.path,
    attrs: selected ? { "aria-current": "true" } : {},
    on: { click: () => void selectReplay(replay.path) },
  },
    h("span", { class: "row-line" },
      h("span", { class: "row-title", text: title }),
      h("span", { class: "row-date", text: formatRelative(replay.modifiedMs) }),
    ),
    hasChampions
      ? h("span", { class: "row-champs" },
          champStrip(replay.blueChampions, champions, winner === "Blue"),
          h("span", { class: "row-vs", text: "vs" }),
          champStrip(replay.redChampions, champions, winner === "Red"),
        )
      : null,
    h("span", { class: "row-line" },
      h("span", { class: replay.status === "parsed" ? "row-meta" : "row-meta row-meta-warn", text: meta.join(" · ") }),
      score,
    ),
  );
  if (selected) button.classList.add("is-selected");
  return h("li", {}, button);
}

function champStrip(ids: string[], champions: Map<string, Asset> | null, winner: boolean) {
  const strip = h("span", { class: winner ? "champ-strip" : "champ-strip is-loser" });
  for (const id of ids) {
    const asset = findChampion(champions, id);
    const monogram = () => h("span", { class: "mini-champ mini-champ-text", title: asset?.name ?? id, text: id.slice(0, 2) });
    if (asset) {
      const img = h("img", { class: "mini-champ", src: asset.image, alt: asset.name, title: asset.name, loading: "lazy", decoding: "async" });
      img.addEventListener("error", () => img.replaceWith(monogram()), { once: true });
      strip.append(img);
    } else {
      strip.append(monogram());
    }
  }
  return strip;
}

function emptyState(title: string, body: string, action: Node | null = null, error = false) {
  return h("li", { class: "empty" },
    h("p", { class: "empty-title", text: title }),
    body ? h("p", { class: error ? "empty-body error-text" : "empty-body", text: body }) : null,
    action,
  );
}
