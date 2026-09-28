# ChronoRift Product Guide

## Objective

ChronoRift should make old `.rofl` files easy to inspect and replay without forcing the user to manually hunt for the matching League of Legends client.

The core flow:

1. User chooses a replays folder once; ChronoRift lists every `.rofl` in it (read-only).
2. Selecting a replay shows the match view: result, team objectives, and per-player stats.
3. ChronoRift loads DataDragon champion, item, spell, and rune assets for the selected locale.
4. To watch it, ChronoRift checks whether a matching client is already installed.
5. If missing, the user picks server, locale, and destination and asks for an install plan.
6. The plan shows the resolved build (and whether it is the exact build of the replay), source, available locales, file count, download size, install size, and destination.
7. User explicitly confirms the install; progress stays visible and can be cancelled.
8. Interrupted or damaged installs are resumed through an explicit repair plan.
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

The downloader resolves the replay's build into a concrete `lol-game-client` RMAN manifest, then installs files into `<destination>/Game/`.

Riot's live release service only lists the current patch, so old builds are resolved from the public manifest archive (Morilli/riot-manifests) first: the exact build, then other builds of the same patch, then the live service. When the exact build is not available the plan says so; ChronoRift never swaps builds silently. The plan also lists the locales the manifest offers and fails with the available list if the chosen locale is missing.

Inputs:

- Replay patch, for example `14.8`.
- Server. Required; suggested from the replay file name when known.
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
