# ChronoRift Product Guide

## Objective

ChronoRift should make old `.rofl` files easy to inspect and replay without forcing the user to manually hunt for the matching League of Legends client.

The core flow:

1. User opens a `.rofl` file.
2. ChronoRift reads metadata and shows patch, server, suggested locale, match id, and duration.
3. ChronoRift loads DataDragon champion/item assets for the selected locale.
4. ChronoRift shows the client for the replay's build in one strip: state, build, location under the saved clients folder (for example `E:\ChronoRift`), and locale. The user can also point to an existing executable.
5. If client work is requested, ChronoRift resolves the replay's exact build and classifies the work as a new install, resume, repair, or already ready.
6. User checks the plan: build, whether it is the exact build, manifest source, mode, affected and reused file counts, download size, install size, the full destination `<clients folder>\<build>\Game`, and locale.
7. User explicitly confirms the exact plan.
8. Progress stays visible until the client is ready.
9. User explicitly launches the replay. If Riot Vanguard is running, the client strip says so next to Reproducir, and the user exits Vanguard from its tray icon first.

## Product Rules

- Opening a replay never downloads or installs a client.
- Download, install, repair replacement, cleanup of ChronoRift temporary files, and launch are user-triggered.
- The clients folder is always visible, editable, and remembered between sessions. The full destination is shown in the plan before anything is downloaded.
- ChronoRift creates the clients folder and the build folder only when the user confirms a plan.
- The locale is always visible and editable.
- Locale can be suggested from replay server, but never silently forced.
- Static-data loading may happen during inspection, but game-client work never does.
- Failed or unsupported replay formats must say what is unsupported.
- Downloader progress must come from real bytes/files, not fake timers.
- Heavy bundle downloads require an explicit confirmation after the manifest plan is visible.
- Client work can be stopped at any time; completed files stay for resuming.
- The client state shown for each replay (ready, partial, missing) is read from local files only.
- Riot Vanguard is read, never controlled: ChronoRift does not stop it, change its configuration, or ask for elevation. Restarting Windows brings it back for online play.
- Interface copy is labels and data only: no explanatory text. The single clarification kept is that ChronoRift does not modify the League of Legends installation.

## Language And Server Handling

Riot locales use values like `en_US`, `es_MX`, `pt_BR`, `ko_KR`, and `ja_JP`.

ChronoRift maps known replay platform ids to a default locale:

| Region | Suggested locale |
| --- | --- |
| BR1 | `pt_BR` |
| EUN1, EUW1 | `en_GB` |
| JP1 | `ja_JP` |
| KR | `ko_KR` |
| LA1, LA2 | `es_MX` |
| NA1, OC1, PH2, SG2 | `en_US` |
| RU | `ru_RU` |
| TH2 | `th_TH` |
| TR1 | `tr_TR` |
| TW2 | `zh_TW` |
| VN2 | `vi_VN` |

This is only a default. The plan keeps the locale editable because backups and downloaded clients may have different language assets. A client downloaded by ChronoRift only has the files of the locale it was installed with, so it is shown and launched with that locale; its plans (resume, verify) start from it too.

## Downloader/Installer Contract

The downloader resolves the replay build into a concrete `lol-game-client` RMAN manifest, then installs files into `<clients folder>\<build>\Game`.

Inputs:

- Replay patch and exact build, for example `16.18` and `16.18.817.5716`.
- Preferred region/server, when available.
- Locale, defaulted but editable.
- Clients folder (persisted setting).

Outputs:

- Installed game client folder, one per build.
- Detected executable path.
- Installed build metadata (`.chronorift/install.json`, including the download duration).
- Progress events with byte count and stage.

Stages:

1. Resolve the build: Riot Sieve first, then the public riot-manifests index for the exact build ([ADR-004](decisions/ADR-004-historical-builds-and-client-layout.md)).
2. Download the manifest from Riot's CDN.
3. Classify existing targets and show mode, file counts, bytes, source, and destination.
4. Ask for explicit confirmation.
5. Download bundle ranges ([ADR-005](decisions/ADR-005-bundle-range-downloads.md)).
6. Decompress chunks and commit each file as soon as it is complete.
7. Write install metadata and mark the executable ready.

The installer includes neutral files, Windows files, and files matching the visible locale. A partial destination reuses files whose manifest path and size match. Missing files produce a resume plan; wrong-sized files produce a repair plan. Repair replacements are assembled in a ChronoRift temporary folder and committed only after the complete replacement is available. Unknown destination files are left alone, and directory/symlink targets are refused.

If Sieve and the index do not have the exact build but Sieve lists the same patch, the plan offers the newest build of that patch and marks it as not exact.

The current structural check compares expected file size. It does not claim to detect corruption that preserves the same size.

## Match Enrichment

Replay parsing remains local. After inspection, the visible match may be enriched from two patch-scoped static-data sources:

- DataDragon supplies champion and item names and images.
- CommunityDragon supplies summoner-spell and perk names, descriptions, and images.

The selected locale is attempted first, followed by a documented fallback. Static-data failure is non-fatal: identifiers and parsed statistics remain visible, and no client download or launch is triggered.

## Deliberate Non-Goals For First Cut

- No account login.
- No Riot API key.
- No cloud sync.
- No automatic install on file open.
- No custom UI framework until vanilla TypeScript hurts.
- No database until JSON/cache files stop being enough.
