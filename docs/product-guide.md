# ChronoRift Product Guide

## Objective

ChronoRift should make old `.rofl` files easy to inspect and replay without forcing the user to manually hunt for the matching League of Legends client.

The core flow:

1. User opens a `.rofl` file.
2. ChronoRift reads metadata and shows patch, server, suggested locale, match id, and duration.
3. ChronoRift loads DataDragon champion/item assets for the selected locale.
4. ChronoRift checks whether a matching client is already known.
5. If missing, ChronoRift shows a download/install action with destination folder and locale visible.
6. User checks the install plan and sees file count, download size, install size, destination, and locale.
7. User explicitly confirms download/install.
8. Progress stays visible until the client is ready.
9. User explicitly launches the replay.

## Product Rules

- Opening a replay never downloads or installs a client.
- Download, install, overwrite, delete, and launch are user-triggered.
- The install folder is always visible and editable.
- The locale is always visible and editable.
- Locale can be suggested from replay server, but never silently forced.
- Static-data loading may happen during inspection, but game-client work never does.
- Failed or unsupported replay formats must say what is unsupported.
- Downloader progress must come from real bytes/files, not fake timers.
- Heavy bundle downloads require an explicit confirmation after the manifest plan is visible.

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
| ME1 | `ar_AE` |
| NA1 | `en_US` |
| OC1 | `en_AU` |
| PH2 | `en_PH` |
| RU | `ru_RU` |
| SG2 | `en_SG` |
| TH2 | `th_TH` |
| TR1 | `tr_TR` |
| TW2 | `zh_TW` |
| VN2 | `vi_VN` |

This is only a default. The launch UI keeps the locale editable because backups and downloaded clients may have different language assets.

## Downloader/Installer Contract

The downloader resolves a required patch into a concrete `lol-game-client` RMAN manifest, then installs files into the user-selected destination.

Inputs:

- Replay patch, for example `14.8`.
- Preferred region/server, when available.
- Locale, defaulted but editable.
- Destination folder.

Outputs:

- Installed game client folder.
- Detected executable path.
- Installed patch metadata.
- Progress events with byte count and stage.

Stages:

1. Resolve manifest.
2. Download manifest.
3. Plan files and show file count/bytes.
4. Ask for explicit confirmation.
5. Download file chunks.
6. Extract/decompress.
7. Write install metadata.
8. Mark executable ready.

The first installer includes neutral files, Windows files, and files matching the visible locale. It refuses to overwrite existing files. An interrupted or damaged install is handled by an explicit repair flow:

- ChronoRift writes `.chronorift/install-state.json` while installing and marks it complete at the end.
- "Review repair" compares the planned files with disk and shows missing files, wrong-size files, and stale temp files, with sizes.
- Repair downloads only missing files. Wrong-size files are replaced only when the user explicitly allows it in that plan.
- Only ChronoRift's own temp files in `.chronorift/tmp` are ever deleted.
- A running install or repair can be cancelled; only one runs at a time.

## Deliberate Non-Goals For First Cut

- No account login.
- No Riot API key.
- No cloud sync.
- No automatic install on file open.
- No custom UI framework until vanilla TypeScript hurts.
- No database until JSON/cache files stop being enough.
