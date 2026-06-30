import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

type ReplayInspection = {
  path: string;
  fileName: string;
  format: "classicRofl" | "rofl2" | "unknown";
  status: "parsed" | "unsupported" | "invalid";
  gameVersion: string | null;
  patch: string | null;
  region: string | null;
  suggestedLocale: string | null;
  dataDragonVersion: string | null;
  matchId: string | null;
  gameDurationMs: number | null;
  winningTeam: string | null;
  blueKills: number;
  redKills: number;
  players: PlayerStats[];
  metadata: unknown;
  notes: string[];
};

type PlayerStats = {
  name: string;
  champion: string;
  championImageUrl: string | null;
  team: "Blue" | "Red" | string;
  position: string;
  win: boolean;
  kills: number;
  deaths: number;
  assists: number;
  creepScore: number;
  goldEarned: number;
  damageToChampions: number;
  visionScore: number;
  items: PlayerItem[];
  summonerSpells: string[];
  perks: string[];
};

type PlayerItem = {
  slot: number;
  id: string;
  imageUrl: string | null;
};

type DataDragonAssets = {
  champions: Map<string, ChampionAsset>;
  items: Map<string, ItemAsset>;
};

type ChampionAsset = {
  id: string;
  name: string;
  image: string;
};

type ItemAsset = {
  id: string;
  name: string;
  image: string;
};

type ChampionJson = {
  data: Record<string, { id: string; name: string; image: { full: string } }>;
};

type ItemJson = {
  data: Record<string, { name: string; image: { full: string } }>;
};

type LaunchReplayRequest = {
  replayPath: string;
  executablePath: string;
  locale: string;
};

type DownloadProgress = {
  destinationPath: string;
  downloadedBytes: number;
  totalBytes: number | null;
  percent: number | null;
  done: boolean;
};

const state: { replay: ReplayInspection | null } = { replay: null };

const openReplayButton = must("#open-replay", HTMLButtonElement);
const chooseInstallFolderButton = must("#choose-install-folder", HTMLButtonElement);
const downloadClientButton = must("#download-client", HTMLButtonElement);
const playReplayButton = must("#play-replay", HTMLButtonElement);
const installFolderInput = must("#install-folder", HTMLInputElement);
const exePathInput = must("#exe-path", HTMLInputElement);
const localeInput = must("#locale", HTMLInputElement);
const fileName = must("#file-name", HTMLElement);
const statusPill = must("#status-pill", HTMLElement);
const patch = must("#patch", HTMLElement);
const version = must("#version", HTMLElement);
const matchId = must("#match-id", HTMLElement);
const duration = must("#duration", HTMLElement);
const score = must("#score", HTMLElement);
const region = must("#region", HTMLElement);
const suggestedLocale = must("#suggested-locale", HTMLElement);
const blueScore = must("#blue-score", HTMLElement);
const redScore = must("#red-score", HTMLElement);
const blueTeam = must("#blue-team", HTMLElement);
const redTeam = must("#red-team", HTMLElement);
const clientState = must("#client-state", HTMLElement);
const progressBar = must("#progress-bar", HTMLElement);
const progressText = must("#progress-text", HTMLElement);
const metadata = must("#metadata", HTMLElement);
const note = must("#note", HTMLElement);
const dataDragonCache = new Map<string, Promise<DataDragonAssets>>();
let dataDragonVersions: Promise<string[]> | null = null;

openReplayButton.addEventListener("click", chooseReplay);
chooseInstallFolderButton.addEventListener("click", chooseInstallFolder);
downloadClientButton.addEventListener("click", downloadClient);
playReplayButton.addEventListener("click", playReplay);
exePathInput.addEventListener("input", updateActions);
installFolderInput.addEventListener("input", updateActions);
localeInput.addEventListener("change", () => {
  if (state.replay) void enrichScoreboard(state.replay);
});

void listen<DownloadProgress>("download-progress", (event) => {
  renderProgress(event.payload);
});

async function chooseReplay() {
  setBusy("Opening");
  try {
    const selected = await open({
      multiple: false,
      filters: [{ name: "League replay", extensions: ["rofl"] }],
    });

    if (typeof selected !== "string") {
      setStatus("Idle", "muted");
      return;
    }

    await inspectReplay(selected);
  } catch (error) {
    showError(error);
  }
}

async function chooseInstallFolder() {
  const selected = await open({ directory: true, multiple: false });
  if (typeof selected === "string") {
    installFolderInput.value = selected;
    updateActions();
  }
}

async function inspectReplay(path: string) {
  setBusy("Reading");
  try {
    const replay = await invoke<ReplayInspection>("inspect_replay", { path });
    state.replay = replay;
    renderReplay(replay);
  } catch (error) {
    showError(error);
  }
}

async function downloadClient() {
  const replay = state.replay;
  const installFolder = installFolderInput.value.trim();
  if (!replay?.patch || !installFolder) return;

  clientState.textContent = `Installer ready for ${replay.patch}`;
  progressText.textContent =
    "Manifest resolver is the next slice; download job plumbing is wired.";
  progressBar.style.width = "0%";

  // ponytail: real progress command exists; Riot manifest resolution lands before this button is enabled.
  downloadClientButton.disabled = true;
}

async function playReplay() {
  if (!state.replay) return;

  const request: LaunchReplayRequest = {
    replayPath: state.replay.path,
    executablePath: exePathInput.value.trim(),
    locale: localeInput.value.trim() || "en_US",
  };

  setBusy("Launching");
  try {
    await invoke("launch_replay", { request });
    setStatus("Launched", "ok");
  } catch (error) {
    showError(error);
  }
}

function renderReplay(replay: ReplayInspection) {
  fileName.textContent = replay.fileName;
  patch.textContent = replay.patch ?? "-";
  version.textContent = replay.gameVersion ?? "-";
  matchId.textContent = replay.matchId ?? "-";
  duration.textContent = formatDuration(replay.gameDurationMs);
  score.textContent = `${replay.blueKills} - ${replay.redKills}`;
  region.textContent = replay.region ?? "-";
  suggestedLocale.textContent = replay.suggestedLocale ?? "-";
  blueScore.textContent = `${replay.blueKills}${replay.winningTeam === "Blue" ? " W" : ""}`;
  redScore.textContent = `${replay.redKills}${replay.winningTeam === "Red" ? " W" : ""}`;
  metadata.textContent = JSON.stringify(replay.metadata, null, 2);
  note.textContent = replay.notes.join(" ");

  if (replay.suggestedLocale) {
    localeInput.value = replay.suggestedLocale;
  }
  renderScoreboard(replay.players);
  void enrichScoreboard(replay);

  if (replay.status === "parsed") {
    setStatus("Parsed", "ok");
    clientState.textContent = replay.patch
      ? `Needs League client ${replay.patch}`
      : "Patch unknown";
    progressText.textContent =
      "No download starts automatically. Choose a folder, review locale, then install or point to an existing executable.";
  } else if (replay.status === "unsupported") {
    setStatus("Unsupported", "warn");
    clientState.textContent = "Unsupported replay format";
    progressText.textContent = "ROFL2 parser is intentionally not guessed yet.";
  } else {
    setStatus("Invalid", "bad");
    clientState.textContent = "Invalid replay";
    progressText.textContent = "ChronoRift could not read this file.";
  }

  updateActions();
}

function updateActions() {
  const replayReady = state.replay?.status === "parsed";
  playReplayButton.disabled = !replayReady || !exePathInput.value.trim();
  // Disabled until the Riot manifest resolver is implemented. Keeping it visible avoids hidden automation.
  downloadClientButton.disabled = true;
}

function renderScoreboard(players: PlayerStats[]) {
  replaceChildren(blueTeam, players.filter((player) => player.team === "Blue"), null);
  replaceChildren(redTeam, players.filter((player) => player.team === "Red"), null);
}

async function enrichScoreboard(replay: ReplayInspection) {
  if (!replay.dataDragonVersion || replay.status !== "parsed") return;

  const activePath = replay.path;
  try {
    note.textContent = "Loading DataDragon assets...";
    const assets = await loadDataDragonAssets(
      replay.dataDragonVersion,
      localeInput.value.trim() || "en_US",
    );

    if (state.replay?.path !== activePath) return;

    replaceChildren(
      blueTeam,
      replay.players.filter((player) => player.team === "Blue"),
      assets,
    );
    replaceChildren(
      redTeam,
      replay.players.filter((player) => player.team === "Red"),
      assets,
    );
    note.textContent = replay.notes.join(" ");
  } catch (error) {
    if (state.replay?.path === activePath) {
      note.textContent = `DataDragon unavailable: ${String(error)}`;
    }
  }
}

function replaceChildren(
  container: HTMLElement,
  players: PlayerStats[],
  assets: DataDragonAssets | null,
) {
  container.textContent = "";
  for (const player of players) {
    container.appendChild(renderPlayer(player, assets));
  }
}

function renderPlayer(player: PlayerStats, assets: DataDragonAssets | null) {
  const row = document.createElement("article");
  row.className = "player-row";
  const champion = resolveChampion(player, assets);

  const champ = document.createElement("div");
  champ.className = "champion-cell";
  if (champion.image) {
    const image = document.createElement("img");
    image.src = champion.image;
    image.alt = champion.name;
    image.loading = "lazy";
    champ.appendChild(image);
  }

  const identity = document.createElement("div");
  identity.className = "player-identity";
  const name = document.createElement("strong");
  name.textContent = player.name || "Unknown";
  const subtitle = document.createElement("span");
  subtitle.textContent = `${player.position} / ${champion.name}`;
  identity.append(name, subtitle);

  const stats = document.createElement("div");
  stats.className = "player-stats";
  stats.textContent = `${player.kills}/${player.deaths}/${player.assists}`;

  const detail = document.createElement("div");
  detail.className = "player-detail";
  detail.textContent = `${player.creepScore} CS · ${formatCompact(player.goldEarned)} gold · ${formatCompact(player.damageToChampions)} dmg · ${player.visionScore} vision`;

  const items = document.createElement("div");
  items.className = "item-row";
  for (const item of player.items) {
    const itemAsset = assets?.items.get(item.id);
    const itemBox = document.createElement("span");
    itemBox.className = "item-box";
    itemBox.title = itemAsset?.name ?? `Item ${item.id}`;
    if (itemAsset?.image || item.imageUrl) {
      const image = document.createElement("img");
      image.src = itemAsset?.image ?? item.imageUrl ?? "";
      image.alt = itemAsset?.name ?? `Item ${item.id}`;
      image.loading = "lazy";
      itemBox.appendChild(image);
    } else {
      itemBox.textContent = item.id;
    }
    items.appendChild(itemBox);
  }

  row.append(champ, identity, stats, detail, items);
  return row;
}

async function loadDataDragonAssets(version: string, locale: string) {
  const resolvedVersion = await resolveDataDragonVersion(version);
  const cacheKey = `${resolvedVersion}:${locale}`;
  if (!dataDragonCache.has(cacheKey)) {
    dataDragonCache.set(cacheKey, fetchDataDragonAssets(resolvedVersion, locale));
  }
  return dataDragonCache.get(cacheKey)!;
}

async function resolveDataDragonVersion(version: string) {
  const patch = version.split(".").slice(0, 2).join(".");
  dataDragonVersions ??= fetch(
    "https://ddragon.leagueoflegends.com/api/versions.json",
  ).then((response) => {
    if (!response.ok) throw new Error("DataDragon versions unavailable");
    return response.json() as Promise<string[]>;
  });

  return dataDragonVersions
    .then((versions) => versions.find((candidate) => candidate.startsWith(`${patch}.`)) ?? version)
    .catch(() => version);
}

async function fetchDataDragonAssets(version: string, locale: string): Promise<DataDragonAssets> {
  const [champions, items] = await Promise.all([
    fetchDataDragonJson<ChampionJson>(version, locale, "champion.json"),
    fetchDataDragonJson<ItemJson>(version, locale, "item.json"),
  ]);

  return {
    champions: championMap(version, champions),
    items: itemMap(version, items),
  };
}

async function fetchDataDragonJson<T>(version: string, locale: string, file: string): Promise<T> {
  const urls = [
    `https://ddragon.leagueoflegends.com/cdn/${version}/data/${locale}/${file}`,
    `https://ddragon.leagueoflegends.com/cdn/${version}/data/en_US/${file}`,
  ];

  for (const url of urls) {
    const response = await fetch(url);
    if (response.ok) return (await response.json()) as T;
  }

  throw new Error(`${file} missing for ${version}`);
}

function championMap(version: string, json: ChampionJson) {
  const map = new Map<string, ChampionAsset>();
  for (const champion of Object.values(json.data)) {
    const asset = {
      id: champion.id,
      name: champion.name,
      image: `https://ddragon.leagueoflegends.com/cdn/${version}/img/champion/${champion.image.full}`,
    };
    map.set(normalizeKey(champion.id), asset);
    map.set(normalizeKey(champion.name), asset);
  }
  return map;
}

function itemMap(version: string, json: ItemJson) {
  const map = new Map<string, ItemAsset>();
  for (const [id, item] of Object.entries(json.data)) {
    map.set(id, {
      id,
      name: item.name,
      image: `https://ddragon.leagueoflegends.com/cdn/${version}/img/item/${item.image.full}`,
    });
  }
  return map;
}

function resolveChampion(player: PlayerStats, assets: DataDragonAssets | null) {
  const asset = assets?.champions.get(normalizeKey(player.champion));
  return {
    name: asset?.name ?? player.champion,
    image: asset?.image ?? player.championImageUrl,
  };
}

function normalizeKey(value: string) {
  return value.toLowerCase().replace(/[^a-z0-9]/g, "");
}

function renderProgress(progress: DownloadProgress) {
  const percent = progress.percent ?? 0;
  progressBar.style.width = `${Math.max(0, Math.min(percent, 100))}%`;
  progressText.textContent = progress.done
    ? `Downloaded ${progress.destinationPath}`
    : `${formatBytes(progress.downloadedBytes)} / ${
        progress.totalBytes ? formatBytes(progress.totalBytes) : "unknown"
      }`;
}

function formatCompact(value: number) {
  return value >= 1000 ? `${(value / 1000).toFixed(1)}k` : `${value}`;
}

function formatDuration(ms: number | null) {
  if (!ms) return "-";
  const seconds = Math.round(ms / 1000);
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, "0")}`;
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
}

function setBusy(label: string) {
  setStatus(label, "muted");
}

function setStatus(label: string, tone: "muted" | "ok" | "warn" | "bad") {
  statusPill.textContent = label;
  statusPill.className = `pill ${tone}`;
}

function showError(error: unknown) {
  note.textContent = String(error);
  setStatus("Error", "bad");
}

function must<T extends Element>(selector: string, constructor: new () => T) {
  const element = document.querySelector(selector);
  if (!(element instanceof constructor)) {
    throw new Error(`Missing element: ${selector}`);
  }
  return element;
}
