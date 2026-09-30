// Contratos con los comandos de Rust (serde en camelCase).

export type ReplayStatus = "parsed" | "unsupported" | "invalid";
export type ReplayFormat = "classicRofl" | "rofl2" | "unknown";
export type ClientWorkMode = "install" | "resume" | "repair" | "ready";
export type ReleaseSource = "sieve" | "manifestIndex";
export type LocalClientState = "ready" | "partial" | "missing";
export type VanguardState = "running" | "stopped" | "missing" | "unknown";

export type LibraryPlayer = {
  name: string;
  champion: string;
  team: string;
  position: string;
  kills: number;
  deaths: number;
  assists: number;
  win: boolean;
};

export type ReplayLibraryEntry = {
  path: string;
  fileName: string;
  fileSize: number;
  modifiedAtMs: number | null;
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
  playerCount: number;
  players: LibraryPlayer[];
};

export type ReplayLibrary = {
  folderPath: string;
  entries: ReplayLibraryEntry[];
  failures: Array<{ path: string; message: string }>;
};

export type PlayerItem = { slot: number; id: string; imageUrl: string | null };

export type PlayerStats = {
  name: string;
  gameName: string;
  tagLine: string;
  champion: string;
  championImageUrl: string | null;
  team: string;
  position: string;
  win: boolean;
  level: number;
  kills: number;
  deaths: number;
  assists: number;
  creepScore: number;
  goldEarned: number;
  damageToChampions: number;
  physicalDamageToChampions: number;
  magicDamageToChampions: number;
  trueDamageToChampions: number;
  damageTaken: number;
  damageSelfMitigated: number;
  healShieldOnTeammates: number;
  timeDeadSeconds: number;
  visionScore: number;
  turretKills: number;
  dragonKills: number;
  baronKills: number;
  heraldKills: number;
  voidgrubKills: number;
  items: PlayerItem[];
  summonerSpells: string[];
  perks: string[];
  keystone: string | null;
  primaryStyle: string | null;
  subStyle: string | null;
};

export type TeamSummary = {
  team: string;
  win: boolean;
  kills: number;
  goldEarned: number;
  damageToChampions: number;
  towers: number;
  inhibitors: number;
  dragons: number;
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
  teams: TeamSummary[];
  metadata: unknown;
  notes: string[];
};

export type ConfirmedPlan = {
  mode: ClientWorkMode;
  resolvedVersion: string;
  destinationPath: string;
};

export type ClientRequest = {
  patch: string;
  gameVersion: string | null;
  region: string | null;
  locale: string;
  clientsRoot: string;
  confirmed: ConfirmedPlan | null;
};

export type ClientInstallPlan = {
  mode: ClientWorkMode;
  patch: string;
  resolvedVersion: string;
  gameBuild: string;
  exactBuild: boolean;
  source: ReleaseSource;
  region: string;
  locale: string;
  clientsRoot: string;
  destinationPath: string;
  manifestUrl: string;
  fileCount: number;
  affectedFileCount: number;
  reusedFileCount: number;
  repairedFileCount: number;
  downloadBytes: number;
  installBytes: number;
  totalInstallBytes: number;
};

export type ClientInstallResult = ClientInstallPlan & {
  executablePath: string;
  downloadSeconds: number;
};

export type DownloadProgress = {
  destinationPath: string;
  downloadedBytes: number;
  totalBytes: number | null;
  percent: number | null;
  done: boolean;
  stage: string;
  detail: string;
};

export type LocalClientStatus = {
  gameBuild: string;
  destinationPath: string;
  state: LocalClientState;
  executablePath: string | null;
  locale: string | null;
  installedBytes: number;
  totalInstallBytes: number | null;
};

export type AppSettings = {
  clientsRoot: string | null;
  downloadConnections?: number | null;
};
