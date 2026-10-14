# Architecture

## Runtime Shape

ChronoRift uses Tauri 2:

- Rust owns filesystem, replay parsing, download/install, process launch, and validation.
- TypeScript owns screen state and user interaction.
- Bun is only the developer package manager.
- Commands that touch the disk, the network, or a process are asynchronous and never run on the main thread, so a slow or network drive does not freeze the window.
- Error and notice texts that reach the UI are in the interface language, neutral Spanish or English, through `crate::i18n` (`tr!`): the language saved in `settings.json`, or the Windows UI language until one is chosen ([ADR-008](decisions/ADR-008-languages.md)). The UI's own texts live in `src/i18n.ts`. Network failures of client planning and downloads are described with the host when they are a timeout, a connection failure, a refused redirect, an HTTP status, or an incomplete response; any other network error keeps reqwest's text. Other appended details, such as Windows errors (already localized) or file-format details, are kept as they come.

The installed user app must not require Bun, Node, Rust, or .NET.

`bun run tauri build` produces `chronorift.exe`, which uses the Windows GUI subsystem and opens without a console, and a single NSIS installer (`bundle.targets: ["nsis"]`). The Rust library is built only as an `rlib` for that executable; the `staticlib` and `cdylib` types of Tauri's template are only needed for iOS and Android. The installer is per user (`installMode: currentUser`, no administrator rights) and in Spanish and English: a language dialog opens at start, preselected from the Windows UI language. The product name is `ChronoRift`; the identifier stays `app.chronorift.desktop`, so `settings.json` and the WebView data keep their folders across builds. The installer also ships `THIRD_PARTY_NOTICES.md` (the OFL-1.1 license of the bundled Geist fonts) next to the executable, and its pages come from the custom template `src-tauri/windows/installer.nsi` with its texts in `src-tauri/windows/spanish.nsh` (neutral Spanish) and `src-tauri/windows/english.nsh`.

## Modules

### `rofl`

Reads classic ROFL headers and extracts metadata JSON. It also reads modern ROFL2 trailer metadata and parses `statsJson` into player summaries. It returns a small `ReplayInspection` object for the UI, with the status `parsed` or `invalid`; the raw metadata JSON is not part of it. Only the header and the metadata block are read: the payload is never loaded, header-declared lengths are checked against the file size before anything is allocated, and a metadata block larger than 4 MiB is rejected (real ones are about 120 KiB).

`statsJson` is read one player at a time, without copying the text or the list. A list with more than 32 players (Arena has 16) is not a real match: reading stops at the 33rd, so a crafted file cannot exhaust memory. The replay keeps its other facts but has no players and no teams, and a note says why.

ROFL2 facts come from these places: the build (`16.18.817.5716`) comes from the length-prefixed version string in the header (the length is byte 14, after the 6-byte signature and 8 more bytes); if that string is not a dotted version, the first dotted number in the first 128 bytes is used. Duration and player stats come from the trailer JSON (`gameLength`, `statsJson`), and region and match id come from the file name (`LA2-1625354615.rofl`, or `<prefix>-LA2-1625354615.rofl`) because the trailer has no such fields. The region must be one of the 17 known Riot platform ids and the match id must be numeric; otherwise the replay has neither.

Each player carries the end-of-game stats the UI shows: KDA, level, CS, gold, damage to champions (total and physical/magic/true), damage taken and self-mitigated, healing and shielding on teammates, time dead, vision, objective kills, items, summoner spells, perks, keystone, and rune trees. Item slots 0–5 are the inventory (`ITEM0`–`ITEM5`), 6 is the trinket (`ITEM6`), and 7 is `ROLE_BOUND_ITEM`, the role quest item added in 2026: bot laners' boots, the support's Control Ward, or the quest reward of the other roles. Positions outside Summoner's Rift (ARAM and similar modes) stay empty instead of being guessed. Team summaries add kills, gold, damage, objectives, and destroyed towers and inhibitors; the last two come from the opposing team's `FRIENDLY_TURRET_LOST` / `FRIENDLY_DAMPEN_LOST`, which include structures taken by minions.

Known ceiling: ROFL2 payload packet decoding is not implemented yet. Metadata and end-of-game stats are available.

### `library`

Scans one folder level for `.rofl` files (any extension case) and reports each file as an entry or as a failure, newest first. Each entry includes a compact player list (Riot ID, champion, team, position, KDA, result). The UI treats the Riot ID present in every parsed replay as the library owner; with fewer than two replays, or more than one such ID, there is no owner.

The scan already reads every replay in full, so it keeps those inspections in a `ReplayCache` keyed by path together with each file's size and modified time. An inspection holds only what the UI shows, not the raw metadata JSON, which was most of every `inspect_replay` answer. `inspect_replay` answers from that cache while both still match, and reads the file again as soon as either changes. Each scan replaces the whole cache, since the UI shows one library: replays that were removed, renamed, or no longer parse, and those of other folders, leave it. After a scan, the UI reads every replay in list order (three at a time) and keeps the results in memory, and it also warms the DataDragon and CommunityDragon data of every patch in the library. Selecting a replay is therefore synchronous: the match renders at once, with names and icons, without a loading state. A replay that is not ready yet keeps the previous match on screen with `aria-busy` until the new one arrives.

### `launcher`

Launches `League of Legends.exe` with these League command-line arguments, in this order:

- the replay path
- `-GameBaseDir=<folder>`: the folder that contains `Game`, as in a Riot install
- `-SkipRads`, `-SkipBuild`, `-EnableLNP`, `-UseNewX3D=1`, and `-UseNewX3DFramebuffers=1`
- `-Locale=<xx_YY>`: for a client downloaded by ChronoRift, the locale in its `install.json` (it only has that locale's files); for a chosen executable, the replay's suggested locale or the one picked in the plan

Paths and locale are validated before any process is created, and the executable must be named `League of Legends.exe` (any case). Launch is Windows-only because the League client replay flow is Windows-only in practice.

`launch_replay` keeps the game process, and a background thread waits for it. When it exits, the backend emits the `replay-exited` event with `{ replayPath, exitCode }`: `replayPath` is the path the UI sent, and `exitCode` is `null` if the wait failed. The UI uses it to leave the playing state. If the game exits at once, the event can arrive before `invoke("launch_replay")` resolves, so the UI must listen from startup and must not mark a replay as playing once its exit has arrived.

### `vanguard`

Reads the state of Riot Vanguard's kernel driver (`vgk`) from the Service Control Manager with query-only rights (`SC_MANAGER_CONNECT`, `SERVICE_QUERY_STATUS`); a standard user has them. It reports `running` (any state but stopped, pending states included), `stopped`, `missing`, or `unknown` when the query fails; other platforms always report `unknown`. It never starts, stops, or reconfigures a service ([ADR-006](decisions/ADR-006-vanguard-read-only.md)).

While `vgk` is loaded, Windows refuses to start `League of Legends.exe` from a separate installation (os error 5), even elevated. The UI reads the state at startup, when the window gains focus, and after a failed launch. When the replay has an executable and `vgk` is running, the client strip shows "Vanguard activo" next to Reproducir. The button stays enabled, and a launch error is shown under the strip.

### `settings`

Persists `settings.json` in the Tauri app config directory (`%APPDATA%\app.chronorift.desktop` on Windows). It holds the clients folder, the optional `downloadConnections`, and `assetPrefetch`: whether the images of the library's patches are downloaded at once (`true`) or when each match is opened (`false`); it has no value until the user answers the first-run question, and a file from an older version loads without it. `set_asset_prefetch` saves it through the same `settings::update`. It also holds `language` (`"es"` or `"en"`, `null` until chosen), saved by `set_language`, which also switches the backend texts at once. The commands return the settings plus `systemLanguage`, the Windows UI language (`GetUserDefaultUILanguage`: Spanish when its primary language is Spanish, otherwise English), which is never written to the file. Saving the clients folder validates the path (it must be absolute and on an existing drive) but creates nothing. If the user picks a drive root such as `E:\`, the folder becomes `E:\ChronoRift`. Saving reads, changes, and writes the file under one lock and keeps the other values; a corrupt file is replaced, while a file that cannot be read is left untouched and the error is shown.

### `client`

Resolves `lol-game-client` releases for the replay build and the Windows platform ([ADR-004](decisions/ADR-004-historical-builds-and-client-layout.md)):

1. Riot Sieve for the replay region, when it lists the exact build.
2. Otherwise the public riot-manifests index (`LoL/<region>/windows/lol-game-client/<build>.txt`, trying the replay region, then `NA1` and `EUW1`). The index only supplies the manifest URL.
3. Otherwise, only when no region of the index has the build (404), the newest build of the same patch that Sieve lists, marked `exactBuild: false`.

Any other index failure (network, 5xx, 429, a URL outside Riot's CDN) moves on to the next region. If no region gives a URL, that failure is returned instead of the non-exact plan; when Sieve failed too, the error names both.

Every manifest URL must be HTTPS under `*.riotcdn.net`. Redirects are only followed within the origin (scheme, host, and port) of each request, so manifests and bundles never come from another host; Sieve and the index follow the same rule. The plan reports the source (`sieve` or `manifestIndex`) and the manifest URL. Bodies are read in chunks with a cap, with or without `Content-Length`: 128 MiB for a manifest, 4 MiB for Sieve and the index.

The destination is `<clients folder>\<build>\Game`, where `<build>` uses the replay format (`16.18.817.5716`) derived from Riot's artifact version (`16.18.8175716`). The RMAN manifest is downloaded only after a user action, and files are filtered to neutral + `windows` + the selected locale by the RMAN flags of each file: a file with locale flags is installed only if it carries the requested locale as is (`es_AR` is not mapped to `es_MX`), and a file with platform flags only if it carries `windows`. A locale that no flag offers is `UnavailableLocale`. Only a manifest where no file carries flags falls back to the markers in the file name (`Aatrox.es_MX.wad.client`). Directory paths are built by ChronoRift, not by `cdragon-rman`: a parent cycle or a missing parent is a corrupt manifest.

Before client files are changed, the module classifies the destination as `install`, `resume`, `repair`, or `ready`. Files with the expected manifest size are reused, missing files are downloaded, and wrong-sized files are repair candidates. A destination with no reusable file but with partly downloaded files kept by a stopped run is a `resume` too; `repair` wins whenever a target has the wrong size. The exact mode, affected file count, destination, locale, and byte totals are returned for confirmation. On a resume the totals only count what is still missing: `downloadBytes` leaves out the chunks already written in kept `.part` files, and `installBytes` is what is left to write. Applying the plan requires the same confirmed mode, resolved version, destination, locale, and manifest URL: `ConfirmedPlan` is `{ mode, resolvedVersion, destinationPath, locale, manifestUrl }`. If any of them changed, a new review is required. Planning reads the destination (files, the download journal, the saved manifest) but never writes to disk.

Downloads follow [ADR-005](decisions/ADR-005-bundle-range-downloads.md):

- Pending chunks are grouped by bundle and deduplicated. Contiguous chunks are merged into range requests of at most 8 MiB.
- Windows are ordered so that files finish one after another. Each chunk counts for the file where it lands (the first by path). The next windows are those of the started file with the fewest windows left; when no file is started, the next file by path starts. Requesting by bundle used to leave hundreds of `.part` files half written, and on NTFS that reserves far more space than it writes (below).
- Up to 8 windows (or `downloadConnections`) are in flight as independent tasks. A range request must answer 206 with the expected `Content-Length` before its body is read.
- Each window is written as soon as it arrives, on a blocking thread, so a window waiting for a retry no longer holds the others. Memory stays bounded to about `connections + 2` windows: the ones in flight or waiting for the disk, plus the one being written. When the disk is slower than the network, fewer requests are sent.
- A window is retried silently when it fails with a timeout, a connection error, a 5xx or 429, a response that is not 206, or an incomplete body. The waits are 2, 4, 8, 16, and 32 s: one attempt plus five retries, about a minute. Other 4xx and validation errors (size, decompression, URL) are never retried. When the retries run out, the install stops with the last error, for example `Falló la descarga: HTTP 503 (<host>)`.
- The HTTP client has no total timeout, so a slow but live transfer keeps going. `read_timeout` (60 s) bounds the wait for the headers and every pause between body bytes, so a stalled connection fails and is retried. The connect timeout is 30 s.
- A chunk is written into every file that uses it and is already half written. If none is, it lands in the first one by path, and the other files copy it from the disk when they start. So a chunk shared by hundreds of files does not leave them all half written, and a file whose chunks all landed elsewhere is assembled from local copies, without network. Copies only happen within a run.
- Each file is assembled in `.chronorift/tmp/<index>-<name>.part`, where the index is the file's position in the manifest selection, stable between runs. Each `.part` is marked sparse (`FSCTL_SET_SPARSE`) and, when the flag took, extended to its final size before its first write. In a normal NTFS file a far write first zero-fills everything before it, synchronously: 1 MiB written at 2 GiB took about 2 s and reserved 2 GiB. That fill used to block the single writer and, behind it, every window in flight. A sparse file, though, reserves space in whole 64 KiB units (16 clusters of 4 KiB) at write time, before anything is flushed: 2,000 scattered 28 KiB writes reserved 4.57 times what they wrote, and 4 KiB writes 16 times; contiguous writes, even out of order, reserved exactly what they wrote. The order above keeps the extra reservation to the few files in progress. The flag stays on the committed file, because clearing it forces an immediate flush of ranges filled late. File systems without sparse files keep zero-filling and are not extended up front.
- A complete file is flushed (`sync_data`) and then renamed into place, so after a power cut a file with the right size has no holes. Repair swaps a completed file through a rollback backup instead of modifying the old file in place.
- `.chronorift/journal.json` records the half-written `.part` files and the ranges of each one that reached the disk, together with the plan they belong to (manifest URL, resolved version, locale, destination) and, per file, its size, its `.part` name, whether it is sparse, and the `.part` creation time. At least every 10 s, and when the download stops or fails, a checkpoint flushes the `.part` files written since the previous one and only then replaces the journal (written aside, flushed, and renamed over the old one). A journal therefore never records bytes that did not reach the disk; a power cut loses at most the last 10 s. Without half-written files, the journal is removed.
- When a plan is confirmed (also a `ready` one), and only after the destination's lock is taken, the journal is read again; if it changed since the plan was computed, a new review is required. Then the `.part` files the journal keeps for this plan stay, and every other `.part`, every `.backup`, and a journal of another plan are removed. A `.backup` is the replaced wrong-size file of a repair cut between its two renames. A journal entry is only trusted for the same plan, the same file and size, and a `.part` that still exists with the recorded creation time and length. Unknown files stay, and planning or opening a replay never removes anything.
- The downloaded manifest is saved as `.chronorift/manifest/<id>.manifest` when an install is confirmed, for Riot URLs ending in a 16-digit hexadecimal manifest id. Planning again and resuming read it instead of downloading it; it is used only if it parses and declares the id of its URL, otherwise it is downloaded again. It is removed when the client is complete.

Progress counts compressed bytes as they arrive, not when a window is written, so a slow disk never holds it at 0 %. There is an event when the download starts, then at most one every 200 ms while bytes arrive or windows are written, and a last one with the total. On a resume the count starts from the compressed size of the chunks already in kept `.part` files, and the total is that plus what is missing, so the percentage does not restart at 0. The value never goes back: the bytes of a failed attempt are discounted, and the progress waits until the retry catches up. The event shape and its stage codes are unchanged. RMAN parsing runs behind a panic guard because `cdragon-rman` panics on malformed tables.

When a confirmed install starts (any mode but `ready`), the plan is written to `.chronorift/plan.json`. A previous `install.json` stays until the first file is added or replaced, so a download that fails before that leaves a ready client ready. `install.json` is written again, and `plan.json` removed, when the client is complete. A confirmed `ready` plan keeps the existing `install.json`, with the statistics of the download that built the client, and writes one only if it is missing.

One install runs at a time:

- In the process, the shared `CancelSignal` state rejects a second `install_client` at once with `Ya hay una descarga en curso.`, before any request and without touching the first install's stop flag. A reloaded window is covered here: the backend and its state survive a reload.
- Across processes (a second instance of the app), the install holds `.chronorift/install.lock` in the destination, opened without sharing, from the confirmation to the end, and removes it when done. If another process holds it, the install is rejected with the same text before the temporary files are cleaned, so the cleanup never touches another install's files.

`cancel_client_install` sets the shared stop flag. The install checks it every 100 ms while it waits for the network or the disk, also while the plan is recomputed at confirmation. In-flight requests and their retry waits are aborted at once, a window that is being written finishes, windows that arrived but were not written yet are dropped, and a last checkpoint keeps the half-written files and their journal. Completed files and kept `.part` files are used by the next `resume` plan, which only requests the missing chunks. The install ends with `Cancelled` ("Descarga detenida"). A failure after the retries keeps them the same way.

`client_statuses` and `list_clients` read the clients folder without network access and without creating anything:

- `ready`: `install.json` exists and so does `League of Legends.exe`.
- `partial`: a confirmed plan or game files exist. `installedBytes` adds the committed game files and the journaled bytes of kept `.part` files (of the confirmed plan's manifest, whose target does not exist yet); `totalInstallBytes` is the plan total, so their ratio is the partial percentage after a stop.
- `missing`: nothing is there.

When the exact build has no `ready` client, `client_statuses` reports the newest `ready` build of the same patch instead, with `exactBuild: false` and that build's `gameBuild` and executable; the plan installs such a build when the exact one cannot be resolved. Exact matches and every `list_clients` entry have `exactBuild: true`. An unreadable version is skipped. If the drive or share of the clients folder does not exist (an unplugged USB disk), both commands fail with `La carpeta de clientes no está disponible: <path>`; a folder that does not exist yet on a present drive only means that nothing is installed.

Texts of this module (errors and progress details) come in the interface language through `crate::i18n` (`tr!`); the examples above are the Spanish ones.

### Match assets

The UI resolves the replay patch to Riot's latest matching DataDragon version, then loads champion and item JSON in the interface language (`es_MX`, or `es_ES` on a Spain-Spanish system) with an `en_US` fallback. It also requests patch-scoped CommunityDragon spell, perk, and rune-tree data with a neutral fallback; rune trees are optional. The game client locale chosen in the plan is independent of these names. These fetches only enrich the match view; a failure leaves identifiers visible and never starts client work or blocks replay launch.

Every DataDragon and CommunityDragon request, JSON and images alike, goes through the `lolasset` URI scheme (`http://lolasset.localhost/<source>/<path>` on Windows, `source` being `ddragon` or `cdragon`). The Rust handler (`asset_cache`) is asynchronous and keeps files under `<app cache dir>\assets` ([ADR-007](decisions/ADR-007-asset-disk-cache.md)):

- A path with a version (`cdn/16.18.1/…`, `16.18/…`) never changes: it is downloaded once and served from disk from then on, also offline.
- Other paths (`api/versions.json`) are refreshed after 12 hours; if the network fails, the stale copy is served.
- A 404 on a versioned path is remembered for 24 hours, so the locale fallback does not reach the network on every run. DataDragon (an S3 bucket) answers a missing file with 403, so on a versioned DataDragon path a 403 counts as a 404 and is remembered too; a 403 from CommunityDragon, or on a path without a version, stays an error. The marker only counts for the exact file name: NTFS ignores case but the CDNs do not, so a 404 for one spelling never hides a file with another.
- Concurrent requests for the same file wait for one download, and at most six on-demand downloads run at a time; `prefetch_assets` has its own pool (about 24, at most 8 to CommunityDragon) and shares the per-file locks. A download larger than 32 MiB is refused while it is read, with or without `Content-Length`.
- Each file is written to `.<name>.<n>.tmp` next to it and renamed into place. At startup, a background pass removes such temporary files older than 10 minutes, left by an interrupted run; newer ones may belong to another running instance.
- Responses carry `X-Content-Type-Options: nosniff`: the type comes from the file extension.
- The webview treats `lolasset.localhost` as a local origin, and it serves third-party SVGs. Every response carries `Content-Security-Policy: default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; sandbox`, so an SVG opened as a document runs no script and gets an opaque origin.
- CORS is only for the UI. `Access-Control-Allow-Origin` repeats the request's `Origin` when it is one of the app's origins: `http://tauri.localhost` (the packaged app on Windows), `https://tauri.localhost`, `tauri://localhost`, or Vite's `http://localhost:1420`. Errors carry it too, so the UI sees a 404 and tries the next locale. Other origins, `null` included, get no CORS header, and responses carry `Vary: Origin`. A request without `Origin` (an `<img>`, or a GET if WebView2 does not pass the header, which is not confirmed) gets the UI's own origin: `http://tauri.localhost` in the packaged app, Vite's in development. That fixed value admits no other origin.
- If the HTTP client with this policy (timeouts, redirects within the origin) cannot be built, the cache stays offline: it serves what is on disk, and any other request is an upstream error that is not remembered as missing.
- Only the two known origins are reachable: requests go only to them, and redirects are only followed within the same origin. Every path segment is validated as a Windows file name before touching the disk, including device names such as `NUL .png` or `COM¹`.

### Startup

- The window is created with `backgroundColor: #0E0E10`, the `--bg-0` token. tao fills the window with it on `WM_ERASEBKGND`, and wry passes it to WebView2 as the controller's `DefaultBackgroundColor` before the controller is created (`ICoreWebView2ControllerOptions3`, and again on the controller), so nothing paints white before the page. The window is not created hidden: a failed bundle can never leave an invisible window.
- `index.html` carries a static splash with its style inline (the CSP allows inline styles, not scripts): the logo (the paths of `ICONS.logo`) and a three-dot loader, static under `prefers-reduced-motion`. It paints before the bundle runs. The splash is a drag region, and the window buttons stay above it; while it shows, the rest of the app is inert.
- `main.ts` fades it out (160 ms) on the first meaningful render: the remembered library loaded or failed, or there is none. It hides in a `finally`, with a 5 s backup for a scan that hangs; there is no minimum display time.
- The first time a library with patches loads and `assetPrefetch` has no value, the splash stays up (or comes back over a folder opened later) and asks before showing it. It waits up to 4 s for the patches' JSON to count the images exactly; the size is an estimate from per-kind averages measured on patch 16.18 (`AVERAGE_ICON_BYTES`). A patch without data is not counted, and if none has data (offline) nothing is asked. "Descargar todo" hands each patch's image paths to the backend in one `prefetch_assets` call. The cache downloads them on its own pool (about 24 requests in flight, at most 8 to CommunityDragon), separate from the on-demand permits, so a match opened meanwhile never waits behind it; files already on disk cost no request, a 404 counts as done, and 12 network errors in a row end the call as offline. Progress arrives as `asset-prefetch-progress`, and `cancel_asset_prefetch` ends a running call. Each image takes about 0.35 s of server time, so the size of the pool, not the bandwidth, sets the speed. A patch whose images all resolved is remembered in `localStorage` (`chronorift.prefetchedPatches`). Each library read retries the patches that failed, once.
- After the library shows, the images its replays use and the list portraits are decoded in the background (`new Image()` and `decode()`, two at a time, up to 3 000 kept referenced). An `<img>` with the same URL is then complete as soon as it is created and paints in the same frame; one that is not gets `is-pending` and fades in on load.

### Webview security

- CSP allows scripts, styles, and fonts from the app only. Images and `fetch` may reach only the `lolasset` cache scheme, never the CDNs directly, so nothing can skip the disk cache; IPC is allowed. The development CSP adds the Vite server.
- `withGlobalTauri` is off: the UI imports `@tauri-apps/api` modules.
- Fonts (Geist, Geist Mono) ship with the app; nothing is loaded from font CDNs.
- The opener permissions are `reveal_item_in_dir` ("Mostrar en carpeta" and "Abrir carpeta") and `open_url` scoped to `https://www.leagueofgraphs.com/match/*`. The match id opens the match there, built from the replay's platform (`LA2` → `las`) and Riot game id. OP.GG was considered, but its match pages need an internal OP.GG key that a replay does not carry.
- The window has no native decorations. The top bar is a `data-tauri-drag-region`, and the only window permissions beyond the defaults are start dragging, maximize on double-click, minimize, toggle maximize, and close.
- The minimum window size is 1116×689 so that the UI gets at least 1100×680. Without decorations and with `shadow: true`, tao (Tauri's window library) applies the minimum to the whole window but keeps the invisible resize borders outside the web view: `SM_CXSIZEFRAME` + `SM_CXPADDEDBORDER` (8 px at 100 % scaling) on the left, right, and bottom, plus 1 px at the top on Windows 11.
- There is no generic download command: every download goes through a confirmed client plan against Riot's CDN.

## Data Storage

Start with simple files:

- app settings JSON (`settings.json` in the app config directory; implemented)
- known clients JSON (not implemented yet)
- install metadata JSON per client folder (`<build>\Game\.chronorift\install.json`; implemented)
- confirmed plan of an unfinished install (`<build>\Game\.chronorift\plan.json`; implemented)
- lock of a running install (`<build>\Game\.chronorift\install.lock`, empty, only while it runs; implemented)
- download journal of half-written files (`<build>\Game\.chronorift\journal.json`, only while a download is unfinished; implemented)
- manifest of an unfinished install (`<build>\Game\.chronorift\manifest\<id>.manifest`, removed when the client is complete; implemented)
- DataDragon and CommunityDragon files (`<app cache dir>\assets\<source>\…`, `%LOCALAPPDATA%\app.chronorift.desktop` on Windows; implemented)

Use SQLite only when search, filtering, or indexing becomes painful.

## Safety Boundaries

- Never delete user-selected client folders automatically.
- Never replace an existing client file without an explicit repair plan and matching confirmation.
- Never delete unknown files in a user-selected destination.
- Never launch a replay if the replay path or executable path does not exist, or if the executable is not `League of Legends.exe`.
- Never accept arbitrary launch locale strings. Keep `xx_YY` validation.
- Never hide download/install progress.
- Never fetch a manifest or bundle outside Riot's CDN, whatever index supplied the URL. Redirects to another origin are refused.
- Never start, stop, or reconfigure a Windows service. Riot Vanguard is only read.
- Never create the clients folder or a build folder before the user confirms a plan.
