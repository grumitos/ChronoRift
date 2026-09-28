// Sample data for the browser preview (`bun run dev` without Tauri).
// Dialogs, installs, and launches are deliberately unavailable here.

import {
  PREVIEW_UNAVAILABLE,
  type InstalledClient,
  type PlayerItem,
  type PlayerStats,
  type ReplayInspection,
  type ReplayLibrary,
  type ReplaySummary,
  type Settings,
  type TeamSummary,
} from "./api";

type Role = "TOP" | "JUNGLE" | "MIDDLE" | "BOTTOM" | "UTILITY";
type Kind = "ad" | "ap" | "tank" | "enchanter";

type Build = {
  spells: [string, string];
  items: number[];
  trinket: number;
  keystone: number;
  primary: number;
  sub: number;
  kind: Kind;
};

// Spell keys: 4 Flash, 14 Ignite, 12 Teleport, 11 Smite, 7 Heal, 3 Exhaust, 6 Ghost, 21 Barrier.
const BUILDS: Record<string, Build> = {
  KSante: { spells: ["4", "12"], items: [3068, 3047, 3075, 3143, 3083, 3065], trinket: 3340, keystone: 8437, primary: 8400, sub: 8300, kind: "tank" },
  Aatrox: { spells: ["4", "12"], items: [6692, 3071, 3047, 3053, 6333, 3026], trinket: 3340, keystone: 8010, primary: 8000, sub: 8400, kind: "ad" },
  Darius: { spells: ["4", "6"], items: [3071, 3047, 3053, 6333, 3742, 3065], trinket: 3340, keystone: 8010, primary: 8000, sub: 8400, kind: "ad" },
  Ornn: { spells: ["4", "12"], items: [3068, 3047, 3075, 3065, 3083, 3143], trinket: 3340, keystone: 8437, primary: 8400, sub: 8300, kind: "tank" },
  Gwen: { spells: ["4", "12"], items: [3115, 3020, 4645, 3089, 3157, 3135], trinket: 3340, keystone: 8010, primary: 8000, sub: 8400, kind: "ap" },
  LeeSin: { spells: ["4", "11"], items: [6692, 3111, 3071, 3053, 6333, 3814], trinket: 3364, keystone: 8010, primary: 8000, sub: 8100, kind: "ad" },
  Viego: { spells: ["11", "4"], items: [3153, 3047, 3071, 3053, 6333, 3026], trinket: 3364, keystone: 8010, primary: 8000, sub: 8100, kind: "ad" },
  Vi: { spells: ["4", "11"], items: [3071, 3111, 3053, 6333, 3742, 3814], trinket: 3364, keystone: 8010, primary: 8000, sub: 8300, kind: "ad" },
  Graves: { spells: ["4", "11"], items: [3142, 3814, 3158, 3031, 3036, 6694], trinket: 3364, keystone: 8128, primary: 8100, sub: 8000, kind: "ad" },
  Sejuani: { spells: ["4", "11"], items: [3068, 3047, 3075, 3143, 3065, 3110], trinket: 3364, keystone: 8439, primary: 8400, sub: 8300, kind: "tank" },
  Ahri: { spells: ["4", "14"], items: [6655, 3020, 3157, 3089, 3135, 4645], trinket: 3340, keystone: 8112, primary: 8100, sub: 8200, kind: "ap" },
  Orianna: { spells: ["4", "12"], items: [6655, 3020, 3089, 3157, 3135, 3165], trinket: 3340, keystone: 8229, primary: 8200, sub: 8300, kind: "ap" },
  Sylas: { spells: ["4", "14"], items: [3152, 3020, 3157, 4645, 3089, 3135], trinket: 3340, keystone: 8010, primary: 8000, sub: 8200, kind: "ap" },
  Viktor: { spells: ["4", "12"], items: [6655, 3020, 3089, 3157, 3135, 4645], trinket: 3340, keystone: 8229, primary: 8200, sub: 8300, kind: "ap" },
  Yasuo: { spells: ["4", "12"], items: [6672, 3006, 3031, 3046, 3072, 3026], trinket: 3340, keystone: 8008, primary: 8000, sub: 8400, kind: "ad" },
  Jinx: { spells: ["4", "7"], items: [6672, 3006, 3031, 3046, 3036, 3072], trinket: 3363, keystone: 8021, primary: 8000, sub: 8400, kind: "ad" },
  Kaisa: { spells: ["4", "7"], items: [6672, 3006, 3124, 3115, 3031, 3036], trinket: 3363, keystone: 8005, primary: 8000, sub: 8100, kind: "ad" },
  Ezreal: { spells: ["4", "7"], items: [3508, 3158, 3042, 3071, 6694, 3036], trinket: 3363, keystone: 8010, primary: 8000, sub: 8300, kind: "ad" },
  Caitlyn: { spells: ["4", "7"], items: [3031, 3006, 3094, 3046, 3036, 3072], trinket: 3363, keystone: 8021, primary: 8000, sub: 8300, kind: "ad" },
  Thresh: { spells: ["4", "3"], items: [3877, 3111, 3190, 3107, 3222, 3050], trinket: 3364, keystone: 8439, primary: 8400, sub: 8300, kind: "tank" },
  Nautilus: { spells: ["4", "14"], items: [3877, 3047, 3190, 3050, 3107, 3143], trinket: 3364, keystone: 8439, primary: 8400, sub: 8300, kind: "tank" },
  Lulu: { spells: ["4", "3"], items: [3876, 3158, 6617, 3107, 3222, 3504], trinket: 3364, keystone: 8214, primary: 8200, sub: 8400, kind: "enchanter" },
  Leona: { spells: ["4", "14"], items: [3877, 3047, 3190, 3050, 3075, 3083], trinket: 3364, keystone: 8439, primary: 8400, sub: 8300, kind: "tank" },
  Milio: { spells: ["4", "3"], items: [3876, 3158, 6617, 3107, 3222, 3504], trinket: 3364, keystone: 8214, primary: 8200, sub: 8400, kind: "enchanter" },
};

const ROLES: Role[] = ["TOP", "JUNGLE", "MIDDLE", "BOTTOM", "UTILITY"];

type Lineup = [name: string, tag: string, champion: string][];

type Sample = {
  fileName: string;
  region: string;
  gameVersion: string;
  hoursAgo: number;
  minutes: number;
  winner: "Blue" | "Red";
  blueKills: number;
  redKills: number;
  surrender?: "normal" | "early";
  seed: number;
  blue: Lineup;
  red: Lineup;
};

const SAMPLES: Sample[] = [
  {
    fileName: "KR-7412093385.rofl",
    region: "KR",
    gameVersion: "15.6.672.1234",
    hoursAgo: 3,
    minutes: 34.6,
    winner: "Blue",
    blueKills: 29,
    redKills: 21,
    seed: 7,
    blue: [["Hide on bush", "KR1", "KSante"], ["Oner", "T1", "LeeSin"], ["Faker", "KR1", "Ahri"], ["Gumayusi", "T1", "Jinx"], ["Keria", "T1", "Thresh"]],
    red: [["Kiin", "KR1", "Aatrox"], ["Canyon", "GEN", "Viego"], ["Chovy", "GEN", "Orianna"], ["Peyz", "GEN", "Kaisa"], ["Lehends", "KR2", "Nautilus"]],
  },
  {
    fileName: "NA1-5023318841.rofl",
    region: "NA1",
    gameVersion: "14.15.604.8123",
    hoursAgo: 27,
    minutes: 26.9,
    winner: "Red",
    blueKills: 14,
    redKills: 31,
    surrender: "normal",
    seed: 11,
    blue: [["tarzaned", "NA1", "Darius"], ["Spica", "NA1", "Graves"], ["Palafox", "NA1", "Sylas"], ["Berserker", "C9", "Ezreal"], ["Vulcan", "NA1", "Leona"]],
    red: [["Dhokla", "NA1", "Ornn"], ["Blaber", "C9", "Vi"], ["Jojopyun", "NA1", "Viktor"], ["Doublelift", "NA1", "Caitlyn"], ["CoreJJ", "TL", "Lulu"]],
  },
  {
    fileName: "EUW1-6891234567.rofl",
    region: "EUW1",
    gameVersion: "14.8.581.2283",
    hoursAgo: 75,
    minutes: 31.4,
    winner: "Blue",
    blueKills: 32,
    redKills: 18,
    seed: 23,
    blue: [["BrokenBlade", "EUW", "Gwen"], ["Yike", "G2", "Sejuani"], ["Caps", "EUW", "Orianna"], ["Hans Sama", "G2", "Kaisa"], ["Mikyx", "G2", "Milio"]],
    red: [["Oscarinin", "FNC", "KSante"], ["Razork", "EUW", "LeeSin"], ["Humanoid", "FNC", "Ahri"], ["Noah", "FNC", "Jinx"], ["Jun", "FNC", "Nautilus"]],
  },
  {
    fileName: "LA2-1456789012.rofl",
    region: "LA2",
    gameVersion: "13.24.550.1150",
    hoursAgo: 24 * 41,
    minutes: 15.2,
    winner: "Red",
    blueKills: 5,
    redKills: 19,
    surrender: "early",
    seed: 31,
    blue: [["Tito", "LAS", "Aatrox"], ["Mañana", "LAS", "Vi"], ["Ñoño", "LAS", "Yasuo"], ["Pollo", "7777", "Caitlyn"], ["Chispa", "LAS", "Thresh"]],
    red: [["Rakyz", "LAS", "Ornn"], ["Grell", "ISG", "Graves"], ["Plugo", "LAS", "Viktor"], ["Snaker", "LAS", "Ezreal"], ["Ackerman", "LAS", "Leona"]],
  },
];

const FOLDER = "C:\\Users\\Player\\Documents\\League of Legends\\Replays";
const CLIENTS_ROOT = "D:\\LoL Clients";

function rng(seed: number) {
  let value = seed * 9301 + 49297;
  return () => {
    value = (value * 9301 + 49297) % 233280;
    return value / 233280;
  };
}

function distribute(total: number, weights: number[], random: () => number): number[] {
  const noisy = weights.map((weight) => weight * (0.6 + random() * 0.8));
  const sum = noisy.reduce((a, b) => a + b, 0);
  const result = noisy.map((weight) => Math.floor((weight / sum) * total));
  let rest = total - result.reduce((a, b) => a + b, 0);
  for (let i = 0; rest > 0; i = (i + 1) % result.length, rest -= 1) result[i] += 1;
  return result;
}

const KILL_WEIGHTS = [0.18, 0.2, 0.25, 0.3, 0.07];
const DEATH_WEIGHTS = [0.22, 0.2, 0.18, 0.2, 0.2];
const CS_PER_MIN = [7.1, 6.0, 8.0, 8.6, 1.3];
const DMG_PER_MIN = [850, 650, 1050, 1100, 420];
const TAKEN_PER_MIN = [1150, 1050, 650, 600, 720];

function buildTeam(
  sample: Sample,
  team: "Blue" | "Red",
  lineup: Lineup,
  random: () => number,
): PlayerStats[] {
  const minutes = sample.minutes;
  const win = sample.winner === team;
  const teamKills = team === "Blue" ? sample.blueKills : sample.redKills;
  const enemyKills = team === "Blue" ? sample.redKills : sample.blueKills;
  const kills = distribute(teamKills, KILL_WEIGHTS, random);
  const deaths = distribute(enemyKills, DEATH_WEIGHTS, random);
  const oldPatch = sample.gameVersion.startsWith("13.");

  return lineup.map(([name, tag, champion], index) => {
    const role = ROLES[index];
    const build = BUILDS[champion];
    const r = () => 0.8 + random() * 0.4;
    const k = kills[index];
    const d = deaths[index];
    const kp = role === "UTILITY" ? 0.62 + random() * 0.2 : 0.42 + random() * 0.3;
    const a = Math.max(0, Math.round(teamKills * kp) - k);
    const cs = Math.round(CS_PER_MIN[index] * minutes * r());
    const neutral = role === "JUNGLE" ? Math.round(cs * 0.82) : Math.round(cs * 0.04);
    const damage = Math.round((DMG_PER_MIN[index] * minutes + k * 450) * r());
    const [physicalShare, magicShare] =
      build.kind === "ap" || build.kind === "enchanter" ? [0.08, 0.84] : build.kind === "tank" ? [0.45, 0.4] : [0.86, 0.06];
    const physical = Math.round(damage * physicalShare);
    const magic = Math.round(damage * magicShare);
    const gold = Math.round((480 + minutes * (role === "UTILITY" ? 215 : 300) + k * 300 + a * 110 + cs * 18) * (win ? 1.05 : 0.95));
    const wardsPlaced = Math.round(minutes * (role === "UTILITY" ? 1.55 : role === "JUNGLE" ? 0.5 : 0.36) * r());
    const wardsKilled = Math.round(minutes * (role === "UTILITY" ? 0.35 : role === "JUNGLE" ? 0.25 : 0.1) * r());
    const controlWards = Math.round(minutes * (role === "UTILITY" ? 0.24 : 0.08) * r());
    const tanky = build.kind === "tank";
    const items: PlayerItem[] = [];
    const itemCount = Math.min(6, Math.max(2, Math.floor(minutes / 5.5)));
    for (let slot = 0; slot < itemCount; slot += 1) {
      let id = build.items[slot];
      if (oldPatch && [3877, 3876, 3869, 3870].includes(id)) id = 3860;
      items.push({ slot, id: String(id), imageUrl: null });
    }
    items.push({ slot: 6, id: String(build.trinket), imageUrl: null });
    const multi = k >= 8 ? 3 : k >= 4 ? 2 : k > 0 ? 1 : 0;
    const player: PlayerStats = {
      name,
      riotTagLine: tag,
      puuid: null,
      champion,
      championImageUrl: null,
      team,
      position: role,
      win,
      level: Math.min(18, Math.round(minutes * 0.42 + 3.5 - (role === "UTILITY" ? 2 : 0) + (win ? 1 : 0) + random())),
      kills: k,
      deaths: d,
      assists: a,
      creepScore: cs,
      minionsKilled: cs - neutral,
      neutralMinionsKilled: neutral,
      goldEarned: gold,
      goldSpent: gold - Math.round(random() * 700),
      damageToChampions: damage,
      physicalDamageToChampions: physical,
      magicDamageToChampions: magic,
      trueDamageToChampions: damage - physical - magic,
      damageTaken: Math.round(TAKEN_PER_MIN[index] * minutes * (tanky ? 1.4 : 1) * r()),
      damageSelfMitigated: Math.round(TAKEN_PER_MIN[index] * minutes * (tanky ? 1.6 : 0.6) * r()),
      damageToBuildings: Math.round((role === "UTILITY" ? 60 : role === "JUNGLE" ? 110 : 260) * minutes * r() * (win ? 1.4 : 0.6)),
      damageToObjectives: Math.round((role === "JUNGLE" ? 950 : 240) * minutes * r() * (win ? 1.3 : 0.7)),
      healing: Math.round((build.kind === "enchanter" ? 260 : 140) * minutes * r()),
      healingOnTeammates: build.kind === "enchanter" ? Math.round(190 * minutes * r()) : Math.round(random() * 400),
      shieldingOnTeammates: build.kind === "enchanter" ? Math.round(240 * minutes * r()) : tanky && role === "UTILITY" ? Math.round(60 * minutes * r()) : 0,
      crowdControlScore: Math.round((tanky ? 1.6 : 0.55) * minutes * r()),
      visionScore: Math.round(wardsPlaced * 1.1 + wardsKilled * 1.4 + controlWards * 1.8 + minutes * 0.2),
      wardsPlaced,
      wardsKilled,
      controlWardsBought: controlWards,
      turretTakedowns: win ? Math.round(random() * 3) + (role === "UTILITY" ? 0 : 1) : Math.round(random() * 1.2),
      largestKillingSpree: Math.max(0, Math.round(k * 0.5)),
      largestMultiKill: multi,
      doubleKills: multi >= 2 ? Math.max(1, Math.round(k / 5)) : 0,
      tripleKills: multi >= 3 ? 1 : 0,
      quadraKills: 0,
      pentaKills: 0,
      timeSpentDeadSeconds: Math.round(d * (8 + minutes * 0.9)),
      items,
      trinket: String(build.trinket),
      summonerSpells: [...build.spells],
      perks: [String(build.keystone)],
      keystone: String(build.keystone),
      primaryStyle: String(build.primary),
      subStyle: String(build.sub),
      statPerks: ["5008", "5008", "5002"],
    };
    return player;
  });
}

function teamSummary(sample: Sample, team: "Blue" | "Red", players: PlayerStats[], random: () => number): TeamSummary {
  const win = sample.winner === team;
  const sum = (key: keyof PlayerStats) =>
    players.reduce((total, player) => total + (player[key] as number), 0);
  const early = sample.surrender === "early";
  const [major] = sample.gameVersion.split(".").map(Number);
  return {
    team,
    win,
    kills: sum("kills"),
    deaths: sum("deaths"),
    assists: sum("assists"),
    goldEarned: sum("goldEarned"),
    damageToChampions: sum("damageToChampions"),
    towers: win ? (early ? 5 : 8 + Math.round(random() * 3)) : early ? 0 : 2 + Math.round(random() * 2),
    inhibitors: win ? (early ? 0 : 1 + Math.round(random() * 2)) : 0,
    dragons: win ? (early ? 2 : 3 + Math.round(random())) : early ? 0 : 1,
    elderDragons: 0,
    barons: win && !early ? 1 : 0,
    heralds: win ? 1 : early || major < 14 ? 0 : 0,
    voidgrubs: major >= 14 ? (win ? 4 : 2) : 0,
    atakhan: major >= 15 && win ? 1 : 0,
  };
}

function inspection(sample: Sample): ReplayInspection {
  const random = rng(sample.seed);
  const blue = buildTeam(sample, "Blue", sample.blue, random);
  const red = buildTeam(sample, "Red", sample.red, random);
  const [major, minor] = sample.gameVersion.split(".");
  const patch = `${major}.${minor}`;
  const durationMs = Math.round(sample.minutes * 60_000);
  const matchId = sample.fileName.replace(/\.rofl$/, "");
  return {
    path: `${FOLDER}\\${sample.fileName}`,
    fileName: sample.fileName,
    format: "rofl2",
    status: "parsed",
    gameVersion: sample.gameVersion,
    patch,
    region: sample.region,
    suggestedLocale: SUGGESTED_LOCALES[sample.region] ?? "en_US",
    dataDragonVersion: `${patch}.1`,
    matchId,
    gameDurationMs: durationMs,
    winningTeam: sample.winner,
    blueKills: sample.blueKills,
    redKills: sample.redKills,
    players: [...blue, ...red],
    teams: [teamSummary(sample, "Blue", blue, random), teamSummary(sample, "Red", red, random)],
    endedInSurrender: sample.surrender !== undefined,
    endedInEarlySurrender: sample.surrender === "early",
    metadata: {
      gameLength: durationMs,
      gameVersion: sample.gameVersion,
      lastGameChunkId: Math.round(sample.minutes),
      lastKeyFrameId: Math.round(sample.minutes / 2),
      statsJson: `[10 player entries omitted in preview]`,
    },
    notes: [],
  };
}

const SUGGESTED_LOCALES: Record<string, string> = {
  BR1: "pt_BR",
  EUN1: "en_GB",
  EUW1: "en_GB",
  JP1: "ja_JP",
  KR: "ko_KR",
  LA1: "es_MX",
  LA2: "es_MX",
  NA1: "en_US",
};

const now = Date.now();
const inspections = new Map<string, ReplayInspection>();
const summaries: ReplaySummary[] = [];

SAMPLES.forEach((sample) => {
  const replay = inspection(sample);
  inspections.set(replay.path, replay);
  summaries.push({
    path: replay.path,
    fileName: replay.fileName,
    modifiedMs: now - sample.hoursAgo * 3_600_000,
    sizeBytes: Math.round(sample.minutes * 520_000),
    format: replay.format,
    status: replay.status,
    gameVersion: replay.gameVersion,
    patch: replay.patch,
    region: replay.region,
    matchId: replay.matchId,
    gameDurationMs: replay.gameDurationMs,
    winningTeam: replay.winningTeam,
    blueKills: replay.blueKills,
    redKills: replay.redKills,
    blueChampions: sample.blue.map(([, , champion]) => champion),
    redChampions: sample.red.map(([, , champion]) => champion),
  });
});

function brokenReplay(
  fileName: string,
  status: "unsupported" | "invalid",
  hoursAgo: number,
  note: string,
): ReplaySummary {
  const path = `${FOLDER}\\${fileName}`;
  const summary: ReplaySummary = {
    path,
    fileName,
    modifiedMs: now - hoursAgo * 3_600_000,
    sizeBytes: status === "invalid" ? 4_096 : 11_204_311,
    format: status === "invalid" ? "unknown" : "rofl2",
    status,
    gameVersion: null,
    patch: null,
    region: status === "invalid" ? null : "EUN1",
    matchId: status === "invalid" ? null : fileName.replace(/\.rofl$/, ""),
    gameDurationMs: null,
    winningTeam: null,
    blueKills: 0,
    redKills: 0,
    blueChampions: [],
    redChampions: [],
  };
  inspections.set(path, {
    ...summary,
    suggestedLocale: summary.region ? SUGGESTED_LOCALES[summary.region] ?? null : null,
    dataDragonVersion: null,
    players: [],
    teams: [],
    endedInSurrender: false,
    endedInEarlySurrender: false,
    metadata: null,
    notes: [note],
  });
  return summary;
}

summaries.splice(
  3,
  0,
  brokenReplay(
    "EUN1-3398120476.rofl",
    "unsupported",
    24 * 9,
    "ROFL2 trailer found but its stats block uses an unknown layout; metadata was not guessed.",
  ),
);
summaries.push(
  brokenReplay("replay (copy).rofl", "invalid", 24 * 120, "File is not a League replay: magic header missing."),
);

const clients: InstalledClient[] = [
  {
    patch: "14.8",
    resolvedVersion: "14.8.5812283",
    region: "EUW1",
    locale: "en_GB",
    destinationPath: `${CLIENTS_ROOT}\\14.8-en_GB`,
    executablePath: `${CLIENTS_ROOT}\\14.8-en_GB\\Game\\League of Legends.exe`,
    executableExists: true,
  },
];

const startEmpty = new URLSearchParams(window.location.search).has("empty");
let settings: Settings = {
  replayFolder: startEmpty ? null : FOLDER,
  clientsRoot: startEmpty ? null : CLIENTS_ROOT,
  executablePath: null,
  locale: null,
};

export async function handle(command: string, args: Record<string, unknown>): Promise<unknown> {
  switch (command) {
    case "load_settings":
      return { ...settings };
    case "save_settings":
      settings = { ...(args.settings as Settings) };
      return { ...settings };
    case "scan_replay_folder": {
      const library: ReplayLibrary = {
        folder: String(args.path),
        replays: summaries.map((summary) => ({ ...summary })),
        errors: [],
      };
      return library;
    }
    case "inspect_replay": {
      const replay = inspections.get(String(args.path));
      if (!replay) throw new Error(`Replay not found: ${String(args.path)}`);
      return structuredClone(replay);
    }
    case "find_installed_clients":
      return clients.map((client) => ({ ...client }));
    case "plan_client_install":
    case "install_client":
    case "launch_replay":
      throw new Error(PREVIEW_UNAVAILABLE);
    default:
      throw new Error(`Unknown command: ${command}`);
  }
}
