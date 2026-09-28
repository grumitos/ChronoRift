// Typed wrappers around the Tauri commands described in docs/roadmap.md
// ("Frontend/Backend Contract", "Install pipeline contract (v2)"). Outside Tauri (plain `bun run dev` in a
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
};

export type PlayerStats = {
  name: string;
  champion: string;
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
  /** Replay "major.minor" patch; resolve the concrete release with datadragon.resolveVersion. */
  dataDragonVersion: string | null;
  matchId: string | null;
  gameDurationMs: number | null;
  winningTeam: string | null;
  blueKills: number;
  redKills: number;
  players: PlayerStats[];
  /** Replay metadata JSON; its raw `statsJson` string is replaced by "<parsed into players>". */
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
  /** Replay patch, e.g. "16.17". */
  patch: string;
  /** Replay build, e.g. "16.17.810.4348"; must match the patch. */
  gameVersion: string | null;
  /** Required server; null or unknown fails with `invalidRegion`. */
  region: string | null;
  locale: string;
  /** Absolute; the client goes to <destinationPath>/Game. */
  destinationPath: string;
};

/** "archive" = Riot manifests from the community archive; "sieve" = Riot live (current patch only). */
export type ClientSource = "archive" | "sieve";

export type ClientInstallPlan = {
  planId: string;
  expiresAtMs: number;
  patch: string;
  /** Archive form of the replay build, e.g. "16.17.8104348". */
  requestedVersion: string | null;
  resolvedVersion: string;
  exactBuild: boolean;
  source: ClientSource;
  region: string;
  manifestRegion: string;
  locale: string;
  /** Locale whose asset files are installed (es_AR installs es_MX files). */
  localeAssets: string;
  availableLocales: string[];
  destinationPath: string;
  gamePath: string;
  executablePath: string;
  manifestUrl: string;
  fileCount: number;
  downloadBytes: number;
  installBytes: number;
  requestCount: number;
};

export type ClientInstallResult = {
  patch: string;
  requestedVersion: string | null;
  resolvedVersion: string;
  exactBuild: boolean;
  source: ClientSource;
  region: string;
  manifestRegion: string;
  locale: string;
  destinationPath: string;
  gamePath: string;
  executablePath: string;
  manifestUrl: string;
  /** Whole client. */
  fileCount: number;
  /** Downloaded by this run. */
  downloadBytes: number;
  /** Whole client. */
  installBytes: number;
  requestCount: number;
};

export type ClientRepairPlanRequest = {
  destinationPath: string;
  locale: string;
};

export type ClientRepairPlan = {
  planId: string;
  expiresAtMs: number;
  patch: string;
  resolvedVersion: string;
  exactBuild: boolean;
  source: ClientSource;
  region: string;
  locale: string;
  localeAssets: string;
  destinationPath: string;
  gamePath: string;
  manifestUrl: string;
  stateStatus: "inProgress" | "complete";
  totalFiles: number;
  okFiles: number;
  missingFiles: number;
  wrongSizeFiles: number;
  /** First 50. */
  missingPaths: string[];
  /** First 50. */
  wrongSizePaths: string[];
  downloadBytes: number;
  installBytes: number;
  /** Extra when replacing wrong-size files. */
  replaceDownloadBytes: number;
  replaceInstallBytes: number;
  staleTempFiles: number;
};

export type ClientRepairRequest = {
  planId: string;
  replaceWrongSize: boolean;
};

export type LaunchReplayRequest = {
  replayPath: string;
  executablePath: string;
  locale: string;
};

export type ProgressStage =
  | "resolve"
  | "manifest"
  | "plan"
  | "download"
  | "repair"
  | "complete"
  | "cancelled"
  | "failed";

export type DownloadProgress = {
  destinationPath: string;
  stage: ProgressStage;
  /** Human-readable step, or the error message for "failed". */
  detail: string;
  downloadedBytes: number;
  totalBytes: number | null;
  percent: number | null;
  filesDone: number;
  filesTotal: number;
  /** True on the last event of an operation. */
  done: boolean;
};

export type CommandErrorCode =
  | "invalidRegion"
  | "invalidPatch"
  | "invalidLocale"
  | "localeUnavailable"
  | "invalidDestination"
  | "releaseNotFound"
  | "network"
  | "manifest"
  | "io"
  | "unsafePath"
  | "wouldOverwrite"
  | "notChronoRiftInstall"
  | "localeMismatch"
  | "missingExecutable"
  | "unknownPlan"
  | "cancelled"
  | "busy"
  /** Frontend only: browser preview has no backend for this command. */
  | "unavailable"
  /** Frontend only: a plain string or unexpected value was thrown. */
  | "unknown";

/** Rejection shape of the install commands; anything else is normalized into it. */
export type CommandError = {
  code: CommandErrorCode;
  /** Human readable, safe to show as is. */
  message: string;
  path?: string;
  /** localeUnavailable: locales the manifest offers, sorted. */
  availableLocales?: string[];
};

/** Server ids the installer accepts (src-tauri/src/install/resolve.rs REGIONS). */
export const REGIONS = [
  "BR1", "EUN1", "EUW1", "JP1", "KR", "LA1", "LA2", "ME1", "NA1",
  "OC1", "PH2", "RU", "SG2", "TH2", "TR1", "TW2", "VN2",
] as const;

/** Returns the allowlisted server id for a replay region, or "" when unknown. */
export function knownRegion(region: string | null | undefined): string {
  const upper = (region ?? "").trim().toUpperCase();
  return (REGIONS as readonly string[]).includes(upper) ? upper : "";
}

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
  /** Installs exactly the pinned plan; an expired id fails with `unknownPlan`. */
  installClient: (planId: string) =>
    call<ClientInstallResult>("install_client", { request: { planId } }),
  planClientRepair: (request: ClientRepairPlanRequest) =>
    call<ClientRepairPlan>("plan_client_repair", { request }),
  repairClient: (request: ClientRepairRequest) =>
    call<ClientInstallResult>("repair_client", { request }),
  /** Resolves true when a plan, install, or repair was running. */
  cancelClientInstall: () => call<boolean>("cancel_client_install"),
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

const ERROR_CODES = new Set<string>([
  "invalidRegion", "invalidPatch", "invalidLocale", "localeUnavailable", "invalidDestination",
  "releaseNotFound", "network", "manifest", "io", "unsafePath", "wouldOverwrite",
  "notChronoRiftInstall", "localeMismatch", "missingExecutable", "unknownPlan", "cancelled",
  "busy", "unavailable", "unknown",
]);

/** Turns any thrown value (backend object, string, Error) into a CommandError. */
export function toCommandError(error: unknown): CommandError {
  if (typeof error === "string") return { code: "unknown", message: error };
  if (error instanceof Error) return { code: "unknown", message: error.message };
  if (error && typeof error === "object") {
    const value = error as Record<string, unknown>;
    if (typeof value.message === "string") {
      const code = typeof value.code === "string" && ERROR_CODES.has(value.code) ? value.code : "unknown";
      const result: CommandError = { code: code as CommandErrorCode, message: value.message };
      if (typeof value.path === "string") result.path = value.path;
      if (Array.isArray(value.availableLocales)) {
        result.availableLocales = value.availableLocales.filter((item): item is string => typeof item === "string");
      }
      return result;
    }
  }
  let message: string;
  try {
    message = JSON.stringify(error) ?? String(error);
  } catch {
    message = String(error);
  }
  return { code: "unknown", message };
}

export function errorMessage(error: unknown): string {
  return toCommandError(error).message;
}
