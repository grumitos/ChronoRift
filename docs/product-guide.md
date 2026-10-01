# ChronoRift Product Guide

## Objective

ChronoRift should make old `.rofl` files easy to inspect and replay without forcing the user to manually hunt for the matching League of Legends client.

The core flow:

1. User opens a replay folder. ChronoRift lists its `.rofl` files by file date, newest first (on the same date, the higher match id first), and the folder is remembered for the next start. The first time a folder with patches loads, before showing it, ChronoRift asks whether to download every image of those patches now or when each match is opened (see Match Enrichment). On the very first start the same splash also asks the interface language (see Interface Language).
2. User picks a replay. ChronoRift reads its metadata and shows the result, patch, server, match id, duration, and the file's date labeled "Descargada": when the replay was downloaded or saved, not when the game was played.
3. ChronoRift loads DataDragon champion/item assets for the interface language.
4. ChronoRift shows the client for the replay's build in one strip: state, build, location under the saved clients folder (for example `E:\ChronoRift`), and locale. The user can also point to an existing executable; only a file named `League of Legends.exe` is accepted, and another one is rejected as soon as it is picked.
5. If client work is requested, ChronoRift resolves the replay's exact build and classifies the work as a new install, resume, repair, or already ready.
6. User checks the plan: build, whether it is the exact build, manifest source, mode, affected and reused file counts, download size, install size, the full destination `<clients folder>\<build>\Game`, and locale.
7. User explicitly confirms the exact plan, including its locale and manifest.
8. Progress stays visible until the client is ready, with the percent to one decimal and the current speed (see Download Progress). Detener shows the client as stopped at once ("Incompleto" with its percent on disk, no progress bar, no top-bar indicator) while the backend finishes stopping in the background, and a neutral line under the strip says that the downloaded part is kept and Reanudar continues from there. Reanudar waits for the stop to end; what was downloaded, partly assembled files included, stays for it. When the stop ends, the plan is recomputed from what is on disk; while it loads, the drawer already says "Reanudar". A download that finished just as Detener was pressed shows "Listo".
9. User explicitly launches the replay. The strip shows "Reproduciendo" until the game closes. If Riot Vanguard is running, the strip says "Vanguard activo" next to Reproducir, also while a game is open, and the user exits Vanguard from its tray icon first.

If a download fails after its automatic retries (transient network failures are retried for about a minute), the reason stays under the client strip, with the drawer open or closed, until the next attempt. The plan is then recomputed on its own, so Reanudar opens a plan that is ready to confirm. One download runs at a time: while it runs, the actions that would start another build are disabled.

## Product Rules

- Opening a replay never downloads or installs a client.
- Download, install, repair replacement, cleanup of ChronoRift temporary files, and launch are user-triggered.
- The clients folder is always visible, editable, and remembered between sessions. The full destination is shown in the plan before anything is downloaded.
- ChronoRift creates the clients folder and the build folder only when the user confirms a plan.
- The locale is always visible. It is chosen in the plan, before anything is downloaded. A client downloaded by ChronoRift is then shown and launched with the locale it was installed with, and the strip does not edit it.
- Locale can be suggested from replay server, but never silently forced.
- Static-data loading may happen during inspection, but game-client work never does.
- A replay that cannot be read, or whose statistics were omitted (more than 32 players), says why and shows "—" instead of an invented score.
- Downloader progress must come from real bytes/files, not fake timers, and never shows 100 % before the client is ready. Its percent has one decimal and its speed is measured, never estimated from the plan.
- Heavy bundle downloads require an explicit confirmation after the manifest plan is visible.
- Client work can be stopped at any time; completed files stay for resuming. A failed download is never silent.
- The client state shown for each replay (ready, partial, missing) is read from local files only. Until it has been read, the state is neutral. If the clients folder is unavailable (for example a disconnected USB drive), the strip says so instead of "No instalado", and it is read again when the window regains focus.
- When a newer build of the same patch is installed instead of the exact one, the strip shows that build marked "no exacta" and launches it.
- A replay folder that cannot be read stays remembered, because a sleeping network drive or an unplugged USB disk comes back; "Actualizar" retries it.
- Riot Vanguard is read, never controlled: ChronoRift does not stop it, change its configuration, or ask for elevation. Restarting Windows brings it back for online play.
- Interface copy is labels and data only: no explanatory text. Two clarifications are kept: that ChronoRift does not modify the League of Legends installation, and, after Detener, that the downloaded part is kept and Reanudar continues from there. The first-run setup is the language label with its two options and, when it applies, the image question: a short question title, its data, and two button labels.
- Every text exists in Spanish and English and follows the chosen language at once, without a reload (see Interface Language).
- The window never paints white: from its creation it has the app's dark background, and a splash with the logo and a sober three-dot loader shows until the library (or its error, or the empty state) is ready. There is no minimum display time. The window buttons stay visible and usable over the splash from the first paint, including the first-run setup.
- Text that appears during a download never moves the rest of the interface: its space is reserved.
- Color is semantic and never the only signal: green for a victory and a ready client, red for a defeat and for errors, amber for an incomplete client, Riot Vanguard, or an unavailable drive, and blue for a download in progress. Blue and red team marks go before "Lado azul" and "Lado rojo" and on AZUL and ROJO; team red is a mark, defeat red a chip.
- ChronoRift's own color is a muted Hextech gold, used sparingly: the single primary action of each view (Reproducir, and Descargar or Reanudar in the client drawer), progress fills, the line under the active tab and section, and the completed steps of a download. Everything else stays monochrome.

## Interface Language

The interface is in Spanish (neutral, with "tú") or English.

- The language in use is the chosen one (`settings.language`) or, while none was chosen (`null`), the system language the backend reports (`systemLanguage`).
- It is asked once, on the splash: the first start shows "Idioma" with Español and English, the system language preselected. When the image question applies (a library with patches and no saved image choice), both go on the same screen and answering the images also saves the language; otherwise "Continuar" saves it. Users who already answered the images get only the language question, once.
- It can be changed in Clientes, next to the image choice. Picking a language applies it at once and saves it (`set_language`); after that, backend messages arrive in that language too. If it cannot be saved, it still applies for the session.
- Numbers, sizes, and dates follow the language: "24,5 MiB", "20,4 %", and "20 sept 2025, 0:01" in Spanish; "24.5 MiB", "20.4%", and "Sep 20, 2025, 12:01 AM" in English.
- Champion, item, and rune names follow it too: the open match and the library reload their DataDragon and CommunityDragon data in the new locale and keep the previous names until it arrives. Images do not depend on the language, so the patches already downloaded stay downloaded.
- "Build", KDA, KP, CS, positions (TOP, JG, MID, BOT, SUP), Riot locales such as `es_MX`, and proper names stay the same in both languages.

## Download Progress

A client download of tens of GiB advances slowly at home speeds (at 6 MB/s each 1 % of 24 GiB takes about 40 s), so progress must never look stuck:

- The percent has one decimal ("0,4 %" / "0.4%"), truncated, so it reaches 100 only when the client is ready.
- The speed is the average of the last 5 s of real bytes ("6,1 MB/s") and appears after the first measured second. The time left is the remaining bytes at that speed.
- The drawer shows the percent, the bytes, the speed, and the time left. The top bar shows the percent and the speed (the time left in its tooltip). The client strip and the Clientes row show the percent in the state text and the speed next to their mini meter.
- Every one of these values has its space reserved: nothing moves when they appear or change.
- After Detener, the strip and Clientes show what is on disk over the full install size, partly assembled files included ("Incompleto · 20,4 %"; in Clientes, "12,3 GiB de 29,2 GiB").

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

This is only a default. The plan keeps the locale editable because backups and downloaded clients may have different language assets. A client downloaded by ChronoRift only has the files of the locale it was installed with, so it is shown and launched with that locale; its plans (resume, verify) start from it too. A chosen executable uses the replay's suggested locale or the one picked in the plan.

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

The locale of the interface language (`es_MX`, or `es_ES` on an es-ES system, and `en_US` in English) is attempted first, followed by a documented fallback. Static-data failure is non-fatal: identifiers and parsed statistics remain visible, and no client download or launch is triggered.

Images are asked about once, on the splash, the first time a library with patches loads: "¿Descargar todas las imágenes ahora?", with the patches, the number of images, and the estimated size (a patch has about 1 160 images and 12 MiB), and two answers:

- "Descargar todo" downloads every champion, item, summoner-spell, rune, and rune-tree icon of the library's patches into the disk cache, the images the library's replays use first. The backend downloads each patch in one call, many images in parallel, and reports its progress. The splash shows the percentage and the patches done; "Continuar" hides it and the download goes on in the top-bar indicator. A file the CDN does not have counts as done. Errors are never shown: when the network is gone the download stops and resumes the next time the folder is read. Patches that appear later are downloaded in the background.
- "Al abrir cada partida" loads the images of each match when it is shown, and stops a download in progress.

The answer is saved and can be changed in Clientes. With either answer, the images the library's replays use are decoded in the background once the library is shown, so switching matches paints them in the same frame. An image that is not ready yet appears with a brief fade in its fixed-size slot; the match never waits for images and never shows a loading state for them.

## Deliberate Non-Goals For First Cut

- No account login.
- No Riot API key.
- No cloud sync.
- No automatic install on opening a folder or a replay.
- No custom UI framework until vanilla TypeScript hurts.
- No database until JSON/cache files stop being enough.
