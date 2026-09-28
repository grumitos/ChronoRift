# Roadmap

This roadmap turns the promises in the README and [product-guide.md](product-guide.md) into shippable milestones. Each milestone must pass the Definition Of Done in `AGENTS.md`.

## Milestone 1 — Usable library (done)

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

## Milestone 2 — Resume and repair installs (done)

Goal: an interrupted or damaged install can be finished without starting over or overwriting blindly.

- Write `.chronorift/install-state.json` at install start (manifest URL, version, locale, file list) and mark it complete at the end.
- `plan_client_repair`: compare planned files with disk (existence and size), report `ok`, `missing`, `wrongSize`, and stale temp files.
- `repair_client`: explicit user action that downloads only missing/wrong files. Replacing a wrong-size file requires the plan to list it and the user to confirm.
- Clean `.chronorift/tmp` only for files ChronoRift itself was writing.
- Cancel button for a running install (cooperative cancel flag checked between chunks).
- UI: detect partial installs in the destination and offer "Review repair" instead of failing with `WouldOverwrite`.

The UI shows the repair plan (ok / missing / wrong size, stale temp files, opt-in replacement), a cancel button with progress, and "Review repair" for `wouldOverwrite`, `cancelled`, and interrupted installs.

## Milestone 2b — Install pipeline rebuild (done)

Found in the project audit: Riot's live service only lists the current patch, and the downloader issued one request per chunk. Rebuilt as `src-tauri/src/install/`: archive-first resolution with exact-build reporting, plan pinning by `planId`, coalesced concurrent downloads (about 250× fewer requests), size-verified atomic writes, `Game/` layout, repair pinned to the saved manifest, structured errors. See "Install pipeline contract (v2)", which supersedes the Milestone 1 and 2 install commands below.

## Milestone 3 — Richer match view (done, one item open)

Goal: the "CommunityDragon-enriched match view beyond the scoreboard" from the README.

- Done: summoner spells and keystone/secondary rune icons (DataDragon `summoner.json`, `runesReforged.json`).
- Done: per-team totals (kills, gold, damage, objectives) and bars for damage share; a Stats tab compares all ten players.
- Done: champion, item, spell, and rune icons carry `title` and `alt` text with names in the selected locale (`en_US` fallback).
- Done: offline mode. When DataDragon cannot be reached, champions show a two-letter monogram, items show their id in a small box, spells and runes are left out, and the detail pane shows one quiet note, "Static data unavailable (offline?)". Images that fail to load fall back the same way.
- Open: CommunityDragon fallback for patches DataDragon does not cover (runes from before Runes Reforged, patch 7.22). Those replays show empty rune slots with the rune id in the title.

## Milestone 4 — Polish and release (in progress)

- Done: library filter matches champion ids and names (selected locale), player Riot ID game names, patch, server, and match id.
- Done: remember the last selected replay (restored on startup when it is still in the library) and the window size (restored on startup, saved 600 ms after the last resize, not while maximized).
- Open: Windows bundle smoke test (`bun run tauri build`) and release notes.

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
| `launch_replay` | `{ request }` | `null`; rejects with `CommandError` (see "Contract updates (Milestones 3–4)") |

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
  players: string[];                 // Riot ID game names in player order (both teams), empty names skipped
};

type Settings = {
  replayFolder: string | null;
  clientsRoot: string | null;
  executablePath: string | null;
  locale: string | null;
  lastReplay: string | null;         // last selected replay path
  windowWidth: number | null;        // inner size in logical px; clamped to >= 900
  windowHeight: number | null;       // clamped to >= 600
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
  legacyLayout: boolean;             // executable is not <destinationPath>/Game/League of Legends.exe (v1); cannot be launched
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

## Install pipeline contract (v2)

Supersedes the install/repair parts of the Milestone 1 and 2 contracts (`plan_client_install`, `install_client`, `plan_client_repair`, `repair_client`, `InstallState`, the `download-progress` shape). `find_installed_clients` and `cancel_client_install` keep their shapes. All payloads are camelCase JSON. Architecture notes: [architecture.md](architecture.md#install-client-install-pipeline).

| Command | Args | Returns | Errors (`code`) |
| --- | --- | --- | --- |
| `plan_client_install` | `{ request: ClientInstallRequest }` | `ClientInstallPlan` | `busy`, `invalidRegion`, `invalidPatch`, `invalidLocale`, `localeUnavailable`, `invalidDestination`, `releaseNotFound`, `network`, `manifest`, `wouldOverwrite`, `unsafePath`, `io`, `cancelled` |
| `install_client` | `{ request: { planId } }` | `ClientInstallResult` | `busy`, `unknownPlan`, `wouldOverwrite`, `network`, `manifest`, `io`, `missingExecutable`, `cancelled` |
| `plan_client_repair` | `{ request: ClientRepairPlanRequest }` | `ClientRepairPlan` | `busy`, `invalidDestination`, `invalidLocale`, `localeUnavailable`, `notChronoRiftInstall`, `localeMismatch`, `network`, `manifest`, `io`, `cancelled` |
| `repair_client` | `{ request: ClientRepairRequest }` | `ClientInstallResult` | `busy`, `unknownPlan`, `notChronoRiftInstall`, `wouldOverwrite`, `network`, `manifest`, `io`, `missingExecutable`, `cancelled` |
| `cancel_client_install` | none | `boolean` (true when a plan/install/repair was running) | — |

```ts
// Failures of the four commands above, and of launch_replay, reject with this object (not a string).
type CommandError = {
  code:
    | "invalidRegion" | "invalidPatch" | "invalidLocale" | "localeUnavailable"
    | "invalidDestination" | "releaseNotFound" | "network" | "manifest" | "io"
    | "unsafePath" | "wouldOverwrite" | "notChronoRiftInstall" | "localeMismatch"
    | "missingExecutable" | "unknownPlan" | "cancelled" | "busy"
    // launch_replay only:
    | "invalidReplay" | "invalidExecutable" | "unsupportedPlatform" | "spawn";
  message: string;                   // human readable, safe to show as is
  path?: string;                     // io, wouldOverwrite, notChronoRiftInstall, missingExecutable
  availableLocales?: string[];       // localeUnavailable: what the manifest offers (sorted)
};

type ClientInstallRequest = {
  patch: string;                     // "16.17" (a full version is accepted too)
  gameVersion: string | null;        // replay build, e.g. "16.17.810.4348"; must match patch
  region: string | null;             // required: BR1 EUN1 EUW1 JP1 KR LA1 LA2 ME1 NA1 OC1 PH2 RU SG2 TH2 TR1 TW2 VN2
                                     // null/other → invalidRegion "Choose a server…" (no NA1 default)
  locale: string;                    // xx_YY; must be offered by the manifest (else localeUnavailable)
  destinationPath: string;           // absolute; the client goes to <destinationPath>/Game
};

type ClientInstallPlan = {
  planId: string;                    // pass to install_client; valid ~30 min, until installed, or until 3 newer plans
  expiresAtMs: number;
  patch: string;                     // "16.17"
  requestedVersion: string | null;   // archive form, "16.17.8104348"
  resolvedVersion: string;           // archive form of the build that will be installed
  exactBuild: boolean;               // resolvedVersion === requestedVersion; show a warning when false
  source: "archive" | "sieve";       // archive = github.com/Morilli/riot-manifests; sieve = Riot (current patch only)
  region: string;                    // requested server
  manifestRegion: string;            // archive region dir the manifest came from (e.g. NA1 when LA2 lacked it)
  locale: string;                    // requested locale
  localeAssets: string;              // locale whose asset files are installed: es_AR → "es_MX", en_GB → "en_US"
  availableLocales: string[];        // locales this manifest offers, sorted
  destinationPath: string;
  gamePath: string;                  // <destinationPath>/Game
  executablePath: string;            // <destinationPath>/Game/League of Legends.exe
  manifestUrl: string;
  fileCount: number;
  downloadBytes: number;             // bytes on the wire after dedupe + range coalescing
  installBytes: number;              // bytes written to disk
  requestCount: number;              // HTTP range requests the download needs
};

type ClientInstallResult = {
  patch: string;
  requestedVersion: string | null;
  resolvedVersion: string;
  exactBuild: boolean;
  source: "archive" | "sieve";
  region: string;
  manifestRegion: string;
  locale: string;
  destinationPath: string;
  gamePath: string;
  executablePath: string;            // launch this; GameBaseDir = destinationPath
  manifestUrl: string;
  fileCount: number;                 // whole client
  downloadBytes: number;             // downloaded by this run
  installBytes: number;              // whole client
  requestCount: number;              // requests made by this run
};

type ClientRepairPlanRequest = {
  destinationPath: string;           // must contain .chronorift/install-state.json (else notChronoRiftInstall)
  locale: string;                    // must equal the installed locale (else localeMismatch)
};

type ClientRepairPlan = {
  planId: string;
  expiresAtMs: number;
  patch: string;
  requestedVersion: string | null;   // replay build from install-state.json; null when the state does not know it (v1)
  resolvedVersion: string;
  exactBuild: boolean;
  source: "archive" | "sieve";
  region: string;
  locale: string;
  localeAssets: string;
  destinationPath: string;
  gamePath: string;                  // <dest>/Game (or <dest> for v1 installs)
  manifestUrl: string;               // from install-state.json; repair never re-resolves
  stateStatus: "inProgress" | "complete";
  totalFiles: number;
  okFiles: number;
  missingFiles: number;
  wrongSizeFiles: number;
  missingPaths: string[];            // first 50
  wrongSizePaths: string[];          // first 50
  downloadBytes: number;             // to fetch missing files
  installBytes: number;
  replaceDownloadBytes: number;      // extra if replaceWrongSize
  replaceInstallBytes: number;
  staleTempFiles: number;            // deleted by repair_client
};

type ClientRepairRequest = {
  planId: string;
  replaceWrongSize: boolean;         // default false
};

// Event "download-progress"
type DownloadProgress = {
  destinationPath: string;
  stage: "resolve" | "manifest" | "plan" | "download" | "repair" | "complete" | "cancelled" | "failed";
  detail: string;                    // human-readable step, or the error message for "failed"
  downloadedBytes: number;
  totalBytes: number | null;
  percent: number | null;
  filesDone: number;
  filesTotal: number;
  done: boolean;                     // true on the last event of an operation
};
```

Behavior:

- Resolution order: archive exact build (request region dir, then `NA1`, `EUW1`) → archive builds of the same patch, newest first (skipping manifests Riot answers 403/404 for) → Riot Sieve (current patch only). `exactBuild: false` or `source: "sieve"` must be visible in the plan UI. When nothing is downloadable the error is `releaseNotFound` with the reasons in `message`.
- Planning downloads only the manifest (checked for cancel) and never writes to the destination. It fails with `wouldOverwrite` (and `path`) if any target file, `.chronorift/install-state.json`, or a temp file already exists; the UI should then offer "Review repair".
- `install_client` installs exactly the pinned plan; unknown or expired ids fail with `unknownPlan` (plan again).
- Progress events are throttled to about 10 per second. Stage changes and the final event are always sent: `plan` with `done: true` when a plan is ready, `complete` after install/repair, `cancelled` on cancel, `failed` on any other error.
- Cancel is checked between network reads (manifest and bundles) and between ranges. Files already completed stay; their temp files are cleaned; `install-state.json` stays `inProgress`.
- Repair reads `install-state.json` (v1 or v2), downloads the manifest at its `manifestUrl`, replaces wrong-size files only with `replaceWrongSize`, deletes only files directly in `.chronorift/tmp`, and ends like an install.

On disk (`<dest>/.chronorift/`), version 2, paths relative to `<dest>`:

```ts
type Release = {
  patch: string; resolvedVersion: string; requestedVersion: string | null; exactBuild: boolean;
  source: "archive" | "sieve"; region: string; manifestRegion: string; locale: string; manifestUrl: string;
};
type InstallState = Release & {     // install-state.json, written at install/repair start
  version: 2;
  status: "inProgress" | "complete";
  gameDir: "Game" | "";              // "" = v1 layout (files directly in <dest>)
  files: { path: string; size: number }[];   // relative to gameDir
  startedAtMs: number;
  updatedAtMs: number;
};
type InstallRecord = Release & {     // install.json, written on completion
  version: 2;
  gameDir: "Game" | "";
  fileCount: number;
  downloadBytes: number;
  installBytes: number;
  installedAtMs: number;
};
```

Version 1 files (no `version` in `install.json`, `version: 1` in the state; absolute `destinationPath`/`executablePath`) are still read by `find_installed_clients` and repair.

## Contract updates (Milestones 3–4)

- `ReplaySummary.players`, `Settings.lastReplay` / `windowWidth` / `windowHeight`, `InstalledClient.legacyLayout`, and `ClientRepairPlan.requestedVersion` are shown inline above.
- `ReplayInspection.notes` only holds things the user should know: why a file is invalid or unsupported, or that the version was guessed. A normal read has no notes.
- `launch_replay` rejects with the same `CommandError` shape as the install commands:

| Code | When | `path` |
| --- | --- | --- |
| `invalidReplay` | replay path not absolute, not `.rofl`, or missing | replay |
| `invalidExecutable` | executable missing, not `League of Legends.exe`, or not in a `Game` folder | executable |
| `invalidLocale` | locale not shaped `xx_YY` | — |
| `unsupportedPlatform` | not Windows | — |
| `spawn` | the process could not be started | — |

- Capabilities: `core:window:allow-set-size`, `allow-inner-size`, and `allow-scale-factor` for the window size restore.
