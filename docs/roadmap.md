# Roadmap

This roadmap turns the promises in the README and [product-guide.md](product-guide.md) into shippable milestones. Each milestone must pass the Definition Of Done in `AGENTS.md`.

## Milestone 1 — Usable library (current)

Goal: the app opens to something useful and the whole core flow is visible.

- Replay library: choose a folder, list its `.rofl` files with patch, server, duration, score, and result. Read-only scan.
- Replay detail: header, scoreboard, client panel, raw metadata collapsed.
- Settings persisted to `settings.json` in the app config dir: replay folder, clients root, executable, locale.
- Known clients: detect ChronoRift-installed clients (`.chronorift/install.json`) under the clients root and suggest the one matching the replay patch (product-guide step 4).
- Install plan shown inside the app with an explicit confirm button. No `window.confirm`.
- Browser preview (`bun run dev`) works with sample data; client work is disabled there.
- Remove the unused `download_artifact` command (arbitrary URL to arbitrary path).
- New UI: minimal, monochrome, utility-first.

Pulled forward from Milestone 3 at the owner's request: the detail pane is a full post-game view (per-team objectives, KDA/KP, CS/min, damage share, runes, spells, vision) in the style of Porofessor/op.gg. See "Match stats" below.

## Milestone 2 — Resume and repair installs

Goal: an interrupted or damaged install can be finished without starting over or overwriting blindly.

- Write `.chronorift/install-state.json` at install start (manifest URL, version, locale, file list) and mark it complete at the end.
- `plan_client_repair`: compare planned files with disk (existence and size), report `ok`, `missing`, `wrongSize`, and stale temp files.
- `repair_client`: explicit user action that downloads only missing/wrong files. Replacing a wrong-size file requires the plan to list it and the user to confirm.
- Clean `.chronorift/tmp` only for files ChronoRift itself was writing.
- Cancel button for a running install (cooperative cancel flag checked between chunks).
- UI: detect partial installs in the destination and offer "Review repair" instead of failing with `WouldOverwrite`.

## Milestone 3 — Richer match view

Goal: the "CommunityDragon-enriched match view beyond the scoreboard" from the README.

- Summoner spells and keystone/secondary rune icons (DataDragon `summoner.json`, `runesReforged.json`, CommunityDragon fallback for old patches).
- Per-team totals (gold, damage, vision) and simple bars for damage share.
- Item tooltips with names in the selected locale.
- Graceful offline mode: text fallbacks when static data is unavailable.

## Milestone 4 — Polish and release

- Library search/filter by champion, player, patch.
- Remember window size and last selected replay.
- Windows bundle smoke test (`bun run tauri build`) and release notes.

## Out Of Scope For Now

- ROFL2 packet/payload decoding (timeline, positions).
- Account login, Riot API, cloud sync.

## Frontend/Backend Contract (Milestone 1)

All payloads are camelCase JSON.

| Command | Args | Returns |
| --- | --- | --- |
| `inspect_replay` | `{ path }` | `ReplayInspection` (unchanged) |
| `scan_replay_folder` | `{ path }` | `ReplayLibrary` |
| `load_settings` | none | `Settings` |
| `save_settings` | `{ settings }` | `Settings` |
| `find_installed_clients` | `{ root }` | `InstalledClient[]` |
| `plan_client_install` | `{ request }` | `ClientInstallPlan` (unchanged) |
| `install_client` | `{ request }` | `ClientInstallResult` (unchanged) |
| `launch_replay` | `{ request }` | `null` (unchanged) |

```ts
type ReplayLibrary = {
  folder: string;
  replays: ReplaySummary[];          // newest first by modifiedMs
  errors: { path: string; message: string }[];
};

type ReplaySummary = {
  path: string;
  fileName: string;
  modifiedMs: number | null;         // unix epoch ms
  sizeBytes: number;
  format: "classicRofl" | "rofl2" | "unknown";
  status: "parsed" | "unsupported" | "invalid";
  gameVersion: string | null;
  patch: string | null;
  region: string | null;
  matchId: string | null;
  gameDurationMs: number | null;
  winningTeam: string | null;
  blueKills: number;
  redKills: number;
  blueChampions: string[];           // champion ids in player order, e.g. ["Urgot", "LeeSin"]
  redChampions: string[];
};

type Settings = {
  replayFolder: string | null;
  clientsRoot: string | null;
  executablePath: string | null;
  locale: string | null;
};

// Parsed from <root>/.chronorift/install.json and <root>/*/.chronorift/install.json
type InstalledClient = {
  patch: string;
  resolvedVersion: string;
  region: string;
  locale: string;
  destinationPath: string;
  executablePath: string;
  executableExists: boolean;
};
```

### Match stats (post-game view like Porofessor/op.gg)

`ReplayInspection` gains `teams` and `PlayerStats` gains more end-of-game stats read from the ROFL2 `statsJson` (classic ROFL uses the same keys). Missing keys default to `0` / `null` / empty so older replays still render.

```ts
type ReplayInspection = /* existing fields */ & {
  teams: TeamSummary[];              // Blue then Red; empty when no stats
  endedInSurrender: boolean;         // GAME_ENDED_IN_SURRENDER or GAME_ENDED_IN_EARLY_SURRENDER
  endedInEarlySurrender: boolean;
};

type TeamSummary = {
  team: "Blue" | "Red";
  win: boolean;
  kills: number; deaths: number; assists: number;
  goldEarned: number;
  damageToChampions: number;
  towers: number;                    // sum TURRETS_KILLED
  inhibitors: number;                // sum BARRACKS_KILLED
  dragons: number;                   // sum DRAGON_KILLS
  elderDragons: number;              // sum ELDER_DRAGON_KILLS
  barons: number;                    // sum BARON_KILLS
  heralds: number;                   // sum RIFT_HERALD_KILLS
  voidgrubs: number;                 // sum HORDE_KILLS
  atakhan: number;                   // sum ATAKHAN_KILLS
};

type PlayerStats = /* existing fields */ & {
  riotTagLine: string | null;        // RIOT_ID_TAG_LINE
  puuid: string | null;              // PUUID
  level: number;                     // LEVEL
  minionsKilled: number;             // MINIONS_KILLED
  neutralMinionsKilled: number;      // NEUTRAL_MINIONS_KILLED
  goldSpent: number;
  physicalDamageToChampions: number;
  magicDamageToChampions: number;
  trueDamageToChampions: number;
  damageTaken: number;               // TOTAL_DAMAGE_TAKEN
  damageSelfMitigated: number;
  damageToBuildings: number;         // TOTAL_DAMAGE_DEALT_TO_BUILDINGS
  damageToObjectives: number;        // TOTAL_DAMAGE_DEALT_TO_OBJECTIVES
  healing: number;                   // TOTAL_HEAL
  healingOnTeammates: number;
  shieldingOnTeammates: number;      // TOTAL_DAMAGE_SHIELDED_ON_TEAMMATES
  crowdControlScore: number;         // TIME_CCING_OTHERS
  wardsPlaced: number;
  wardsKilled: number;
  controlWardsBought: number;        // VISION_WARDS_BOUGHT_IN_GAME
  turretTakedowns: number;
  largestKillingSpree: number;
  largestMultiKill: number;
  doubleKills: number; tripleKills: number; quadraKills: number; pentaKills: number;
  timeSpentDeadSeconds: number;      // TOTAL_TIME_SPENT_DEAD
  keystone: string | null;           // KEYSTONE_ID, else PERK0
  primaryStyle: string | null;       // PERK_PRIMARY_STYLE
  subStyle: string | null;           // PERK_SUB_STYLE
  statPerks: string[];               // STAT_PERK_0..2
  trinket: string | null;            // ITEM6 (also still in items)
};
```

Player profiles: public sites address players by Riot ID + region, not PUUID, so the UI builds links from `name#riotTagLine` and the replay region (e.g. `https://www.op.gg/summoners/las/Name-TAG`, `https://www.leagueofgraphs.com/summoner/las/Name-TAG`) and opens them in the system browser only on click (opener plugin). PUUID is kept in the data for future lookups. No link is shown when the tag line or region is unknown.

The frontend derives kill participation, KDA ratio, CS/min, gold/min, and damage share from these.

The `download-progress` event keeps its current shape.

## Frontend/Backend Contract (Milestone 2)

All payloads are camelCase JSON. M1 commands are unchanged except that `install_client` now also writes install state, honors cancel, and is exclusive with `repair_client`.

| Command | Args | Returns |
| --- | --- | --- |
| `plan_client_repair` | `{ request: ClientInstallRequest }` | `ClientRepairPlan` (read-only) |
| `repair_client` | `{ request: ClientRepairRequest }` | `ClientInstallResult` |
| `cancel_client_install` | none | `boolean` (true when an install/repair was running) |

```ts
type ClientRepairRequest = {
  install: ClientInstallRequest;
  replaceWrongSize: boolean;         // default false; wrong-size files are left untouched unless true
};

type ClientRepairPlan = {
  patch: string;
  resolvedVersion: string;
  region: string;
  locale: string;
  destinationPath: string;
  manifestUrl: string;
  totalFiles: number;
  okFiles: number;                   // exists with the exact manifest size
  missingFiles: number;
  wrongSizeFiles: number;            // exists but size differs (or is not a regular file)
  missingPaths: string[];            // first 50
  wrongSizePaths: string[];          // first 50
  downloadBytes: number;             // to fetch missing files
  installBytes: number;              // written for missing files
  replaceDownloadBytes: number;      // extra if replaceWrongSize
  replaceInstallBytes: number;       // extra if replaceWrongSize
  staleTempFiles: number;            // files in <dest>/.chronorift/tmp (deleted by repair)
  hasPriorState: boolean;            // <dest>/.chronorift/install-state.json exists
  priorStateMatches: boolean;        // same manifestUrl + locale
};

// <dest>/.chronorift/install-state.json, written at install/repair start
type InstallState = {
  version: 1;
  status: "inProgress" | "complete";
  patch: string;
  resolvedVersion: string;
  region: string;
  locale: string;
  manifestUrl: string;
  destinationPath: string;
  files: { path: string; size: number }[];
  startedAtMs: number;
  updatedAtMs: number;
};
```

Behavior:

- `install_client` / `plan_client_install` still refuse existing targets (or stale temp files) with the message `"<path> already exists. Review repair to resume or fix this install."`; the UI should offer "Review repair" (`plan_client_repair`) when it sees it.
- `repair_client` downloads missing files, replaces wrong-size files only when `replaceWrongSize` is true, deletes only files directly in `<dest>/.chronorift/tmp`, then writes `install.json` (full-client totals), marks state `complete`, and fails if `League of Legends.exe` is still missing.
- Only one install/repair runs at a time; a second call fails with `"Another install is running"`.
- Cancel is cooperative (checked between chunks and files). On cancel the current temp file is removed, state stays `inProgress`, a final `download-progress` event is emitted with `stage: "cancelled"` and `done: true`, and the command fails with `"Install cancelled"`.
- `download-progress` keeps its shape. Stages: `resolve`, `manifest`, `plan`, `download` (install) or `repair` (repair), `complete`, `cancelled`.
