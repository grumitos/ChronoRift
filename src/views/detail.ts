import { errorMessage, openExternal, type PlayerStats, type ReplayInspection, type TeamSummary } from "../api";
import { findChampion, type DataDragonAssets } from "../datadragon";
import { h, replace } from "../dom";
import {
  formatCompact,
  formatDate,
  formatDuration,
  formatNumber,
  positionLabel,
  teamLabel,
} from "../format";
import { profileLinks } from "../profiles";
import { setStatus, state, subscribe } from "../state";
import { createPlayButton, mountClient } from "./client";

type Tab = "overview" | "stats";
let activeTab: Tab = "overview";

export function mountDetail(root: HTMLElement) {
  const content = h("div", { class: "detail-content" });
  const clientHost = h("section", { class: "section client", attrs: { "aria-label": "Game client" } });
  const rawHost = h("div", { class: "detail-raw" });
  const playButton = createPlayButton();
  root.append(content, clientHost, rawHost);
  mountClient(clientHost);

  const render = () => {
    closeMenu?.();
    const replay = state.replay;
    clientHost.hidden = !replay;

    if (!state.selectedPath) {
      replace(content, placeholder("No replay selected", "Pick a replay from the list, or use Open file… for a single .rofl. Selecting a replay only reads it."));
      replace(rawHost);
      return;
    }
    if (state.replayLoading) {
      replace(content, placeholder("Reading replay…", state.selectedPath));
      replace(rawHost);
      return;
    }
    if (state.replayError || !replay) {
      replace(content, placeholder("Replay could not be read", state.replayError ?? "", true));
      replace(rawHost);
      return;
    }

    replace(content, header(replay, playButton), ...body(replay, state.assets));
    replace(rawHost, rawMetadata(replay));
  };

  const renderContentOnly = () => {
    const replay = state.replay;
    if (!replay || state.replayLoading) return;
    replace(content, header(replay, playButton), ...body(replay, state.assets));
  };

  subscribe(["selection", "replay"], render);
  subscribe(["assets"], renderContentOnly);
  render();
}

function placeholder(title: string, body: string, error = false) {
  return h("div", { class: "placeholder" },
    h("p", { class: "placeholder-title", text: title }),
    body ? h("p", { class: error ? "placeholder-body error-text" : "placeholder-body", text: body }) : null,
  );
}

// Header -----------------------------------------------------------------

function header(replay: ReplayInspection, playButton: HTMLElement) {
  const summary = state.library?.replays.find((entry) => entry.path === replay.path);
  const meta = [
    replay.patch ? `Patch ${replay.patch}` : null,
    replay.gameVersion,
    replay.region,
    replay.gameDurationMs ? formatDuration(replay.gameDurationMs) : null,
    replay.matchId,
    summary?.modifiedMs ? formatDate(summary.modifiedMs) : null,
  ].filter((part): part is string => Boolean(part));

  return h("header", { class: "detail-head" },
    h("div", { class: "detail-head-text" },
      h("h1", { class: "detail-title", text: replay.fileName }),
      h("p", { class: "detail-meta", text: meta.join("  ·  ") }),
      resultLine(replay),
    ),
    h("div", { class: "detail-head-actions" }, playButton),
  );
}

function resultLine(replay: ReplayInspection) {
  if (replay.status !== "parsed") {
    const label = replay.status === "unsupported" ? "Unsupported replay format" : "Not a readable replay";
    return h("p", { class: "result-line" }, h("span", { class: "error-text", text: label }));
  }
  const winner = teamLabel(replay.winningTeam);
  const loser = winner === "Blue" ? "Red" : winner === "Red" ? "Blue" : "";
  const notes: string[] = [];
  if (winner) notes.push(`${winner} victory`);
  if (replay.endedInEarlySurrender) notes.push(`${loser} early surrender`);
  else if (replay.endedInSurrender) notes.push(`${loser} surrendered`);

  return h("p", { class: "result-line" },
    h("span", { class: winner === "Blue" ? "team-name is-winner" : "team-name", text: "Blue" }),
    h("span", { class: "result-score" },
      h("span", { text: String(replay.blueKills) }),
      h("span", { class: "dim", text: " – " }),
      h("span", { text: String(replay.redKills) }),
    ),
    h("span", { class: winner === "Red" ? "team-name is-winner" : "team-name", text: "Red" }),
    notes.length ? h("span", { class: "result-note", text: notes.join(" · ") }) : null,
  );
}

// Body -------------------------------------------------------------------

function body(replay: ReplayInspection, assets: DataDragonAssets | null): Node[] {
  const nodes: Node[] = [];
  if (replay.notes.length) {
    nodes.push(h("ul", { class: "notes" }, ...replay.notes.map((note) => h("li", { text: note }))));
  }
  if (state.assetsError) nodes.push(h("p", { class: "notes", text: state.assetsError }));

  if (replay.players.length === 0) {
    nodes.push(placeholder("No match stats", replay.status === "parsed"
      ? "This replay has no end-of-game stats block."
      : "ChronoRift does not guess stats for files it cannot parse."));
    return nodes;
  }

  const blue = replay.players.filter((player) => teamLabel(player.team) === "Blue");
  const red = replay.players.filter((player) => teamLabel(player.team) === "Red");
  const context: MatchContext = {
    replay,
    assets,
    offline: !assets && Boolean(state.assetsError),
    minutes: replay.gameDurationMs ? replay.gameDurationMs / 60_000 : 0,
    maxDamage: Math.max(1, ...replay.players.map((player) => player.damageToChampions)),
  };

  const tabs = h("div", { class: "tabs", attrs: { role: "tablist" } });
  const panel = h("div", { class: "tab-panel", attrs: { role: "tabpanel" } });
  const renderTab = () => {
    replace(tabs, ...(["overview", "stats"] as Tab[]).map((tab) =>
      h("button", {
        class: tab === activeTab ? "tab is-active" : "tab",
        type: "button",
        text: tab === "overview" ? "Overview" : "Stats",
        attrs: { role: "tab", "aria-selected": String(tab === activeTab) },
        on: {
          click: () => {
            activeTab = tab;
            renderTab();
          },
        },
      }),
    ));
    if (activeTab === "overview") {
      replace(panel,
        replay.teams.length ? teamTotals(replay.teams) : null,
        teamTable("Blue", blue, context),
        teamTable("Red", red, context),
      );
    } else {
      replace(panel, statsTable(blue, red, context));
    }
  };
  renderTab();
  nodes.push(tabs, panel);
  return nodes;
}

type MatchContext = {
  replay: ReplayInspection;
  assets: DataDragonAssets | null;
  /** DataDragon failed: text fallbacks, no spell/rune placeholders. */
  offline: boolean;
  minutes: number;
  maxDamage: number;
};

// Team totals ------------------------------------------------------------

function teamTotals(teams: TeamSummary[]) {
  type Column = { label: string; title?: string; className?: string; value: (team: TeamSummary) => string; optional?: (team: TeamSummary) => number };
  const columns: Column[] = [
    { label: "Kills", value: (team) => `${team.kills} / ${team.deaths} / ${team.assists}` },
    { label: "Gold", value: (team) => formatCompact(team.goldEarned) },
    { label: "Damage", title: "Damage to champions", value: (team) => formatCompact(team.damageToChampions), className: "c-total-dmg" },
    { label: "Towers", value: (team) => String(team.towers) },
    { label: "Inhibs", title: "Inhibitors", value: (team) => String(team.inhibitors) },
    { label: "Dragons", value: (team) => String(team.dragons) },
    { label: "Elder", value: (team) => String(team.elderDragons), optional: (team) => team.elderDragons },
    { label: "Barons", value: (team) => String(team.barons) },
    { label: "Heralds", value: (team) => String(team.heralds) },
    { label: "Grubs", title: "Voidgrubs", value: (team) => String(team.voidgrubs), optional: (team) => team.voidgrubs },
    { label: "Atakhan", value: (team) => String(team.atakhan), optional: (team) => team.atakhan },
  ];
  const visible = columns.filter((column) => !column.optional || teams.some((team) => column.optional!(team) > 0));

  return h("div", { class: "table-scroll" }, h("table", { class: "grid totals" },
    h("thead", {}, h("tr", {},
      h("th", { class: "col-team", text: "Team" }),
      ...visible.map((column) => h("th", { class: column.className ? `num ${column.className}` : "num", text: column.label, title: column.title ?? column.label })),
    )),
    h("tbody", {}, ...teams.map((team) => h("tr", {},
      h("th", { class: "col-team", attrs: { scope: "row" } },
        h("span", { class: team.win ? "team-name is-winner" : "team-name", text: teamLabel(team.team) }),
        h("span", { class: "team-result", text: team.win ? "Victory" : "Defeat" }),
      ),
      ...visible.map((column) => h("td", { class: column.className ? `num ${column.className}` : "num", text: column.value(team) })),
    ))),
  ));
}

// Team table -------------------------------------------------------------

function teamTable(team: "Blue" | "Red", players: PlayerStats[], context: MatchContext) {
  if (players.length === 0) return null;
  const win = players.some((player) => player.win);
  const teamKills = players.reduce((sum, player) => sum + player.kills, 0);

  return h("div", { class: "table-scroll" }, h("table", { class: "grid scoreboard" },
    h("thead", {}, h("tr", {},
      h("th", { class: "c-champ team-cell" },
        h("span", { class: win ? "team-name is-winner" : "team-name", text: `${team} team` }),
        h("span", { class: "team-result", text: win ? "Victory" : "Defeat" }),
      ),
      h("th", { class: "c-player" }),
      h("th", { class: "num c-kda", text: "KDA" }),
      h("th", { class: "num c-kp", text: "KP", title: "Kill participation" }),
      h("th", { class: "num c-dmg", text: "Damage", title: "Damage to champions" }),
      h("th", { class: "num c-gold", text: "Gold" }),
      h("th", { class: "num c-cs", text: "CS" }),
      h("th", { class: "num c-vision", text: "Vision", title: "Vision score, wards placed / killed / control wards" }),
      h("th", { class: "c-items items-head", text: "Items" }),
    )),
    h("tbody", {}, ...players.map((player) => playerRow(player, teamKills, context))),
  ));
}

function playerRow(player: PlayerStats, teamKills: number, context: MatchContext) {
  const { assets, minutes, maxDamage } = context;
  const champion = findChampion(assets?.champions, player.champion);
  const championName = champion?.name ?? player.champion;
  const ratio = player.deaths === 0 ? "Perfect" : ((player.kills + player.assists) / player.deaths).toFixed(2);
  const kp = teamKills > 0 ? Math.round(((player.kills + player.assists) / teamKills) * 100) : 0;
  const csPerMin = minutes > 0 ? (player.creepScore / minutes).toFixed(1) : "–";
  const damageShare = Math.max(2, Math.round((player.damageToChampions / maxDamage) * 100));

  return h("tr", {},
    h("td", { class: "cell-champ" }, champCluster(player, championName, champion?.image ?? null, context)),
    h("td", { class: "cell-player" },
      playerName(player, context.replay.region),
      h("span", { class: "sub", text: `${championName} · ${positionLabel(player.position)}` }),
    ),
    h("td", { class: "num" },
      h("span", { class: "kda" },
        String(player.kills), h("span", { class: "dim", text: " / " }),
        h("span", { class: "deaths", text: String(player.deaths) }), h("span", { class: "dim", text: " / " }),
        String(player.assists),
      ),
      h("span", { class: "sub", text: ratio === "Perfect" ? "Perfect" : `${ratio} KDA` }),
    ),
    h("td", { class: "num c-kp", text: `${kp}%` }),
    h("td", { class: "num" },
      h("span", { text: formatCompact(player.damageToChampions), title: formatNumber(player.damageToChampions) }),
      h("span", { class: "bar", attrs: { "aria-hidden": "true" } },
        h("span", { class: "bar-fill", attrs: { style: `width:${damageShare}%` } }),
      ),
    ),
    h("td", { class: "num", text: formatCompact(player.goldEarned), title: formatNumber(player.goldEarned) }),
    h("td", { class: "num" },
      String(player.creepScore),
      h("span", { class: "sub", text: `${csPerMin}/m` }),
    ),
    h("td", { class: "num c-vision", title: "Wards placed / killed / control wards bought" },
      String(player.visionScore),
      h("span", { class: "sub", text: `${player.wardsPlaced}/${player.wardsKilled}/${player.controlWardsBought}` }),
    ),
    h("td", { class: "cell-items" }, itemStrip(player, context)),
  );
}

/** An image that turns into `fallback()` if it fails to load (offline, blocked CDN). */
function icon(props: { class?: string; src: string; label: string }, fallback: () => HTMLElement) {
  const img = h("img", { class: props.class, src: props.src, alt: props.label, title: props.label, loading: "lazy", decoding: "async" });
  img.addEventListener("error", () => img.replaceWith(fallback()), { once: true });
  return img;
}

function monogram(name: string, className: string) {
  return h("span", { class: className, text: name.slice(0, 2), title: name, attrs: { role: "img", "aria-label": name } });
}

function champCluster(
  player: PlayerStats,
  name: string,
  image: string | null,
  context: MatchContext,
) {
  const { assets, offline } = context;
  const portrait = h("span", { class: "champ" },
    image
      ? icon({ src: image, label: name }, () => monogram(name, "champ-fallback"))
      : monogram(name, "champ-fallback"),
    player.level ? h("span", { class: "level", text: String(player.level), title: `Level ${player.level}` }) : null,
  );
  // Without static data there is nothing useful to show for spells and runes.
  if (offline) return h("span", { class: "champ-cluster" }, portrait);
  const spells = h("span", { class: "icon-stack" },
    ...[0, 1].map((index) => {
      const key = player.summonerSpells[index];
      const spell = key ? assets?.spells.get(key) : undefined;
      return smallIcon(spell?.image, spell?.name ?? (key ? `Spell ${key}` : "No spell"));
    }),
  );
  const keystone = player.keystone ?? player.perks[0] ?? null;
  const keystoneAsset = keystone ? assets?.runes.get(keystone) : undefined;
  const subAsset = player.subStyle ? assets?.runes.get(player.subStyle) : undefined;
  const runes = h("span", { class: "icon-stack" },
    smallIcon(keystoneAsset?.image, keystoneAsset?.name ?? (keystone ? `Rune ${keystone}` : "No keystone"), "rune"),
    smallIcon(subAsset?.image, subAsset?.name ?? (player.subStyle ? `Style ${player.subStyle}` : "No secondary style"), "rune rune-sub"),
  );
  return h("span", { class: "champ-cluster" }, portrait, spells, runes);
}

function smallIcon(src: string | undefined, label: string, extra = "") {
  const className = extra ? `icon-sm ${extra}` : "icon-sm";
  const empty = () => h("span", { class: `${className} is-empty`, title: label, attrs: { role: "img", "aria-label": label } });
  return src ? icon({ class: className, src, label }, empty) : empty();
}

function itemStrip(player: PlayerStats, { assets, offline }: MatchContext) {
  const bySlot = new Map(player.items.map((item) => [item.slot, item]));
  const strip = h("span", { class: "items" });
  const slot = (id: string | undefined, trinket = false) => {
    const className = trinket ? "item is-trinket" : "item";
    if (!id || id === "0") return h("span", { class: `${className} is-empty` });
    const asset = assets?.items.get(id);
    const label = asset?.name ?? `Item ${id}`;
    const text = () => h("span", { class: `${className} item-text`, text: id, title: label, attrs: { role: "img", "aria-label": label } });
    if (asset) return icon({ class: className, src: asset.image, label }, text);
    // While static data loads, keep a quiet empty slot instead of flashing ids.
    return assets || offline ? text() : h("span", { class: `${className} is-empty`, title: label });
  };
  for (let index = 0; index < 6; index += 1) {
    const item = bySlot.get(index);
    strip.append(slot(item?.id));
  }
  const trinket = bySlot.get(6);
  strip.append(slot(trinket?.id ?? player.trinket ?? undefined, true));
  return strip;
}

function playerName(player: PlayerStats, region: string | null) {
  const name = player.name || "Unknown";
  const tag = player.riotTagLine ? h("span", { class: "tag", text: `#${player.riotTagLine}` }) : null;
  const links = profileLinks(player.name, player.riotTagLine, region);
  if (links.length === 0) return h("span", { class: "player-name" }, name, tag);

  const button = h("button", {
    class: "player-name is-link",
    type: "button",
    title: "Open public profile",
    attrs: { "aria-haspopup": "menu", "aria-expanded": "false" },
  }, name, tag);
  button.addEventListener("click", (event) => {
    event.stopPropagation();
    openProfileMenu(button, links);
  });
  return button;
}

let closeMenu: (() => void) | null = null;

function openProfileMenu(anchor: HTMLElement, links: { site: string; url: string }[]) {
  const wasOpen = anchor.getAttribute("aria-expanded") === "true";
  closeMenu?.();
  if (wasOpen) return;

  const menu = h("div", { class: "menu", attrs: { role: "menu" } },
    ...links.map((link) => h("button", {
      class: "menu-item",
      type: "button",
      attrs: { role: "menuitem" },
      title: link.url,
      on: {
        click: () => {
          closeMenu?.();
          openExternal(link.url).catch((error) => setStatus(`Could not open link: ${errorMessage(error)}`, true));
        },
      },
    }, `Open on ${link.site}`)),
  );
  const rect = anchor.getBoundingClientRect();
  menu.style.left = `${Math.round(rect.left)}px`;
  menu.style.top = `${Math.round(rect.bottom + 4)}px`;
  document.body.append(menu);
  anchor.setAttribute("aria-expanded", "true");
  (menu.querySelector("button") as HTMLButtonElement | null)?.focus();

  const onDocument = (event: Event) => {
    if (event instanceof KeyboardEvent && event.key !== "Escape") return;
    if (event.type === "mousedown" && menu.contains(event.target as Node)) return;
    closeMenu?.();
    if (event instanceof KeyboardEvent) anchor.focus();
  };
  const onScroll = () => closeMenu?.();
  document.addEventListener("mousedown", onDocument);
  document.addEventListener("keydown", onDocument);
  document.addEventListener("scroll", onScroll, true);
  closeMenu = () => {
    menu.remove();
    anchor.setAttribute("aria-expanded", "false");
    document.removeEventListener("mousedown", onDocument);
    document.removeEventListener("keydown", onDocument);
    document.removeEventListener("scroll", onScroll, true);
    closeMenu = null;
  };
}

// Stats tab --------------------------------------------------------------

type StatRow = { label: string; value: (player: PlayerStats) => number; format?: (value: number) => string };
type StatGroup = { title: string; rows: StatRow[] };

const compact = (value: number) => formatCompact(value);
const plain = (value: number) => String(value);
const seconds = (value: number) => formatDuration(value * 1000);

const STAT_GROUPS: StatGroup[] = [
  {
    title: "Damage dealt",
    rows: [
      { label: "To champions", value: (p) => p.damageToChampions, format: compact },
      { label: "Physical", value: (p) => p.physicalDamageToChampions, format: compact },
      { label: "Magic", value: (p) => p.magicDamageToChampions, format: compact },
      { label: "True", value: (p) => p.trueDamageToChampions, format: compact },
      { label: "To buildings", value: (p) => p.damageToBuildings, format: compact },
      { label: "To objectives", value: (p) => p.damageToObjectives, format: compact },
    ],
  },
  {
    title: "Damage taken and support",
    rows: [
      { label: "Taken", value: (p) => p.damageTaken, format: compact },
      { label: "Self-mitigated", value: (p) => p.damageSelfMitigated, format: compact },
      { label: "Healing", value: (p) => p.healing, format: compact },
      { label: "Healing on allies", value: (p) => p.healingOnTeammates, format: compact },
      { label: "Shielding on allies", value: (p) => p.shieldingOnTeammates, format: compact },
      { label: "Crowd control", value: (p) => p.crowdControlScore },
    ],
  },
  {
    title: "Economy and vision",
    rows: [
      { label: "Gold earned", value: (p) => p.goldEarned, format: compact },
      { label: "Gold spent", value: (p) => p.goldSpent, format: compact },
      { label: "Minions", value: (p) => p.minionsKilled },
      { label: "Jungle monsters", value: (p) => p.neutralMinionsKilled },
      { label: "Vision score", value: (p) => p.visionScore },
      { label: "Wards placed", value: (p) => p.wardsPlaced },
      { label: "Wards killed", value: (p) => p.wardsKilled },
      { label: "Control wards", value: (p) => p.controlWardsBought },
    ],
  },
  {
    title: "Fights",
    rows: [
      { label: "Turret takedowns", value: (p) => p.turretTakedowns },
      { label: "Largest spree", value: (p) => p.largestKillingSpree },
      { label: "Largest multikill", value: (p) => p.largestMultiKill },
      { label: "Double kills", value: (p) => p.doubleKills },
      { label: "Triple kills", value: (p) => p.tripleKills },
      { label: "Quadra kills", value: (p) => p.quadraKills },
      { label: "Penta kills", value: (p) => p.pentaKills },
      { label: "Time dead", value: (p) => p.timeSpentDeadSeconds, format: seconds },
    ],
  },
];

function statsTable(blue: PlayerStats[], red: PlayerStats[], context: MatchContext) {
  const players = [...blue, ...red];
  const columns = players.length;

  const headCells = players.map((player, index) => {
    const champion = findChampion(context.assets?.champions, player.champion);
    const name = champion?.name ?? player.champion;
    const image = champion?.image ?? null;
    const label = `${player.name || "Unknown"} · ${name}`;
    return h("th", { class: index === blue.length ? "num stat-player team-split" : "num stat-player", title: label },
      image
        ? icon({ class: "stat-champ", src: image, label }, () => monogram(name, "stat-champ champ-fallback"))
        : monogram(name, "stat-champ champ-fallback"),
    );
  });

  const rows: Node[] = [];
  for (const group of STAT_GROUPS) {
    rows.push(h("tr", { class: "group-row" }, h("th", { attrs: { colspan: String(columns + 1), scope: "rowgroup" }, text: group.title })));
    for (const row of group.rows) {
      const values = players.map(row.value);
      const max = Math.max(...values);
      rows.push(h("tr", {},
        h("th", { class: "stat-label", attrs: { scope: "row" }, text: row.label }),
        ...values.map((value, index) => {
          const classes = ["num"];
          if (index === blue.length) classes.push("team-split");
          if (max > 0 && value === max) classes.push("is-max");
          if (value === 0) classes.push("dim");
          return h("td", { class: classes.join(" "), text: (row.format ?? plain)(value), title: formatNumber(value) });
        }),
      ));
    }
  }

  return h("div", { class: "table-scroll" },
    h("table", { class: "grid stats" },
      h("thead", {}, h("tr", {}, h("th", { class: "stat-label" }), ...headCells)),
      h("tbody", {}, ...rows),
    ),
  );
}

// Raw metadata -----------------------------------------------------------

function rawMetadata(replay: ReplayInspection) {
  let text: string;
  try {
    text = JSON.stringify(replay.metadata, null, 2) ?? "null";
  } catch (error) {
    text = errorMessage(error);
  }
  return h("details", { class: "raw" },
    h("summary", { text: "Raw metadata" }),
    h("pre", { class: "mono", text }),
  );
}
