// Typed wrappers around the Tauri commands described in docs/roadmap.md
// ("Frontend/Backend Contract"). Outside Tauri (plain `bun run dev` in a
// browser) every call is routed to ./mock.ts instead.

export type ReplayFormat = "classicRofl" | "rofl2" | "unknown";
export type ReplayStatus = "parsed" | "unsupported" | "invalid";

export type ReplaySummary = {
  path: string;
  fileName: string;
  modifiedMs: number | null;
  sizeBytes: number;
  format: ReplayFormat;
  status: ReplayStatus;
  gameVersion: string | null;
  patch: string | null;
  region: string | null;
  matchId: string | null;
  gameDurationMs: number | null;
  winningTeam: string | null;
  blueKills: number;
  redKills: number;
  /** DataDragon champion ids in player order, e.g. ["Urgot", "LeeSin"]. */
  blueChampions: string[];
  redChampions: string[];
};

export type ReplayLibrary = {
  folder: string;
  replays: ReplaySummary[];
  errors: { path: string; message: string }[];
};

export type PlayerItem = {
  slot: number;
  id: string;
  imageUrl: string | null;
};

export type PlayerStats = {
  name: string;
  champion: string;
  championImageUrl: string | null;
  team: string;
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
  riotTagLine: string | null;
  puuid: string | null;
  level: number;
  minionsKilled: number;
  neutralMinionsKilled: number;
  goldSpent: number;
  physicalDamageToChampions: number;
  magicDamageToChampions: number;
  trueDamageToChampions: number;
  damageTaken: number;
  damageSelfMitigated: number;
  damageToBuildings: number;
  damageToObjectives: number;
  healing: number;
  healingOnTeammates: number;
  shieldingOnTeammates: number;
  crowdControlScore: number;
  wardsPlaced: number;
  wardsKilled: number;
  controlWardsBought: number;
  turretTakedowns: number;
  largestKillingSpree: number;
  largestMultiKill: number;
  doubleKills: number;
  tripleKills: number;
  quadraKills: number;
  pentaKills: number;
  timeSpentDeadSeconds: number;
  keystone: string | null;
  primaryStyle: string | null;
  subStyle: string | null;
  statPerks: string[];
  trinket: string | null;
};

export type TeamSummary = {
  team: "Blue" | "Red" | string;
  win: boolean;
  kills: number;
  deaths: number;
  assists: number;
  goldEarned: number;
  damageToChampions: number;
  towers: number;
  inhibitors: number;
  dragons: number;
  elderDragons: number;
  barons: number;
  heralds: number;
  voidgrubs: number;
  atakhan: number;
};

export type ReplayInspection = {
  path: string;
  fileName: string;
  format: ReplayFormat;
  status: ReplayStatus;
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
  /** Blue then Red; empty when the replay carries no stats. */
  teams: TeamSummary[];
  endedInSurrender: boolean;
  endedInEarlySurrender: boolean;
};

export type Settings = {
  replayFolder: string | null;
  clientsRoot: string | null;
  executablePath: string | null;
  locale: string | null;
};

export type InstalledClient = {
  patch: string;
  resolvedVersion: string;
  region: string;
  locale: string;
  destinationPath: string;
  executablePath: string;
  executableExists: boolean;
};

export type ClientInstallRequest = {
  patch: string;
  gameVersion: string | null;
  region: string | null;
  locale: string;
  destinationPath: string;
};

export type ClientInstallPlan = {
  patch: string;
  resolvedVersion: string;
  region: string;
  locale: string;
  destinationPath: string;
  manifestUrl: string;
  fileCount: number;
  downloadBytes: number;
  installBytes: number;
};

export type ClientInstallResult = ClientInstallPlan & {
  executablePath: string;
};

export type LaunchReplayRequest = {
  replayPath: string;
  executablePath: string;
  locale: string;
};

export type DownloadProgress = {
  destinationPath: string;
  downloadedBytes: number;
  totalBytes: number | null;
  percent: number | null;
  done: boolean;
  stage?: string;
  detail?: string;
};

export const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export const PREVIEW_UNAVAILABLE = "Not available in browser preview";

type Mock = typeof import("./mock");
let mockModule: Promise<Mock> | null = null;
function mock(): Promise<Mock> {
  mockModule ??= import("./mock");
  return mockModule;
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(command, args);
  }
  const m = await mock();
  return m.handle(command, args ?? {}) as Promise<T>;
}

export const api = {
  loadSettings: () => call<Settings>("load_settings"),
  saveSettings: (settings: Settings) => call<Settings>("save_settings", { settings }),
  scanReplayFolder: (path: string) => call<ReplayLibrary>("scan_replay_folder", { path }),
  inspectReplay: (path: string) => call<ReplayInspection>("inspect_replay", { path }),
  findInstalledClients: (root: string) =>
    call<InstalledClient[]>("find_installed_clients", { root }),
  planClientInstall: (request: ClientInstallRequest) =>
    call<ClientInstallPlan>("plan_client_install", { request }),
  installClient: (request: ClientInstallRequest) =>
    call<ClientInstallResult>("install_client", { request }),
  launchReplay: (request: LaunchReplayRequest) =>
    call<null>("launch_replay", { request }),
};

/** Folder picker. Resolves to null when the user cancels. */
export async function pickFolder(title: string): Promise<string | null> {
  if (!isTauri) throw new Error(PREVIEW_UNAVAILABLE);
  const { open } = await import("@tauri-apps/plugin-dialog");
  const selected = await open({ directory: true, multiple: false, title });
  return typeof selected === "string" ? selected : null;
}

export async function pickFile(
  title: string,
  filter: { name: string; extensions: string[] },
): Promise<string | null> {
  if (!isTauri) throw new Error(PREVIEW_UNAVAILABLE);
  const { open } = await import("@tauri-apps/plugin-dialog");
  const selected = await open({ multiple: false, title, filters: [filter] });
  return typeof selected === "string" ? selected : null;
}

/** Subscribes to installer progress. A no-op in browser preview. */
export async function onDownloadProgress(
  handler: (progress: DownloadProgress) => void,
): Promise<() => void> {
  if (!isTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<DownloadProgress>("download-progress", (event) => handler(event.payload));
}

/** Opens an http(s) URL in the system browser. Only call from a click handler. */
export async function openExternal(url: string): Promise<void> {
  if (!/^https:\/\//.test(url)) throw new Error("Refusing to open a non-https URL");
  if (!isTauri) {
    window.open(url, "_blank", "noopener,noreferrer");
    return;
  }
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(url);
}

export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}
