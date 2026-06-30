# ChronoRift Product Guide

## Objective

ChronoRift should make old `.rofl` files easy to inspect and replay without forcing the user to manually hunt for the matching League of Legends client.

The core flow:

1. User opens a `.rofl` file.
2. ChronoRift reads metadata and shows patch, server, suggested locale, match id, and duration.
3. ChronoRift loads DataDragon champion/item assets for the selected locale.
4. ChronoRift checks whether a matching client is already known.
5. If missing, ChronoRift shows a download/install action with destination folder and locale visible.
6. User explicitly starts download/install.
7. Progress stays visible until the client is ready.
8. User explicitly launches the replay.

## Product Rules

- Opening a replay never downloads or installs a client.
- Download, install, overwrite, delete, and launch are user-triggered.
- The install folder is always visible and editable.
- The locale is always visible and editable.
- Locale can be suggested from replay server, but never silently forced.
- Static-data loading may happen during inspection, but game-client work never does.
- Failed or unsupported replay formats must say what is unsupported.
- Downloader progress must come from real bytes/files, not fake timers.

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

This is only a default. The launch UI keeps the locale editable because backups and downloaded clients may have different language assets.

## Downloader/Installer Contract

The downloader must resolve a required patch into a concrete client manifest, then install files into the user-selected destination.

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
2. Plan files.
3. Download files.
4. Verify hashes.
5. Extract/decompress.
6. Write install metadata.
7. Mark executable ready.

## Deliberate Non-Goals For First Cut

- No account login.
- No Riot API key.
- No cloud sync.
- No automatic install on file open.
- No custom UI framework until vanilla TypeScript hurts.
- No database until JSON/cache files stop being enough.
