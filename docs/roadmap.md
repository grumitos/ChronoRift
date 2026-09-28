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

The `download-progress` event keeps its current shape.
