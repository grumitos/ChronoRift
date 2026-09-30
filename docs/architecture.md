# Architecture

## Runtime Shape

ChronoRift uses Tauri 2:

- Rust owns filesystem, replay parsing, download/install, process launch, and validation.
- TypeScript owns screen state and user interaction.
- Bun is only the developer package manager.
- Commands that touch the disk, the network, or a process are asynchronous and never run on the main thread, so a slow or network drive does not freeze the window.
- Error and notice texts that reach the UI are in neutral Spanish. Network failures of client planning and downloads are described in Spanish with the host when they are a timeout, a connection failure, a refused redirect, an HTTP status, or an incomplete response; any other network error keeps reqwest's text. Other appended details, such as Windows errors (already localized) or file-format details, are kept as they come.

The installed user app must not require Bun, Node, Rust, or .NET.

`bun run tauri build` produces `chronorift.exe`, which uses the Windows GUI subsystem and opens without a console, and a single NSIS installer (`bundle.targets: ["nsis"]`). The Rust library is built only as an `rlib` for that executable; the `staticlib` and `cdylib` types of Tauri's template are only needed for iOS and Android. The installer is per user (`installMode: currentUser`, no administrator rights) and in Spanish. The product name is `ChronoRift`; the identifier stays `app.chronorift.desktop`, so `settings.json` and the WebView data keep their folders across builds.

## Modules

### `rofl`

Reads classic ROFL headers and extracts metadata JSON. It also reads modern ROFL2 trailer metadata and parses `statsJson` into player summaries. It returns a small `ReplayInspection` object for the UI. Only the header and the metadata block are read: the payload is never loaded, and header-declared lengths are checked against the file size before anything is allocated.

ROFL2 facts come from these places: the build (`16.18.817.5716`) comes from the length-prefixed version string in the header, duration and player stats come from the trailer JSON (`gameLength`, `statsJson`), and region and match id come from the file name (`LA2-1625354615.rofl`) because the trailer has no such fields.

Each player carries the end-of-game stats the UI shows: KDA, level, CS, gold, damage to champions (total and physical/magic/true), damage taken and self-mitigated, healing and shielding on teammates, time dead, vision, objective kills, items, summoner spells, perks, keystone, and rune trees. Item slots 0–5 are the inventory (`ITEM0`–`ITEM5`), 6 is the trinket (`ITEM6`), and 7 is `ROLE_BOUND_ITEM`, the role quest item added in 2026: bot laners' boots, the support's Control Ward, or the quest reward of the other roles. Positions outside Summoner's Rift (ARAM and similar modes) stay empty instead of being guessed. Team summaries add kills, gold, damage, objectives, and destroyed towers and inhibitors; the last two come from the opposing team's `FRIENDLY_TURRET_LOST` / `FRIENDLY_DAMPEN_LOST`, which include structures taken by minions.

Known ceiling: ROFL2 payload packet decoding is not implemented yet. Metadata and end-of-game stats are available.

### `library`

Scans one folder level for `.rofl` files (any extension case) and reports each file as an entry or as a failure, newest first. Each entry includes a compact player list (Riot ID, champion, team, position, KDA, result). The UI treats the Riot ID present in every parsed replay as the library owner; with fewer than two replays, or more than one such ID, there is no owner.

The scan already reads every replay in full, so it keeps those inspections in a `ReplayCache` keyed by path together with each file's size and modified time. `inspect_replay` answers from that cache while both still match, and reads the file again as soon as either changes. Each scan replaces the whole cache, since the UI shows one library: replays that were removed, renamed, or no longer parse, and those of other folders, leave it. After a scan, the UI reads every replay in list order (three at a time) and keeps the results in memory, and it also warms the DataDragon and CommunityDragon data of every patch in the library. Selecting a replay is therefore synchronous: the match renders at once, with names and icons, without a loading state. A replay that is not ready yet keeps the previous match on screen with `aria-busy` until the new one arrives.

### `launcher`

Launches `League of Legends.exe` with:

- replay path
- `-GameBaseDir` (the folder that contains `Game`, as in a Riot install)
- replay launch flags based on ReplayBook behavior
- explicit `-Locale`: for a client downloaded by ChronoRift, the locale in its `install.json` (it only has that locale's files); for a chosen executable, the replay's suggested locale or the one picked in the plan

Paths and locale are validated before any process is created, and the executable must be named `League of Legends.exe` (any case). Launch is Windows-only because the League client replay flow is Windows-only in practice.

### `vanguard`

Reads the state of Riot Vanguard's kernel driver (`vgk`) from the Service Control Manager with query-only rights (`SC_MANAGER_CONNECT`, `SERVICE_QUERY_STATUS`); a standard user has them. It reports `running` (any state but stopped, pending states included), `stopped`, `missing`, or `unknown` when the query fails; other platforms always report `unknown`. It never starts, stops, or reconfigures a service ([ADR-006](decisions/ADR-006-vanguard-read-only.md)).

While `vgk` is loaded, Windows refuses to start `League of Legends.exe` from a separate installation (os error 5), even elevated. The UI reads the state at startup, when the window gains focus, and after a failed launch. When the replay has an executable and `vgk` is running, the client strip shows "Vanguard activo" next to Reproducir. The button stays enabled, and a launch error is shown under the strip.

### `settings`

Persists `settings.json` in the Tauri app config directory (`%APPDATA%\app.chronorift.desktop` on Windows). It holds the clients folder and the optional `downloadConnections`. Saving validates the path (it must be absolute and on an existing drive) but creates nothing. If the user picks a drive root such as `E:\`, the folder becomes `E:\ChronoRift`. Saving reads, changes, and writes the file under one lock and keeps the other values; a corrupt file is replaced, while a file that cannot be read is left untouched and the error is shown.

### `client`

Resolves `lol-game-client` releases for the replay build and the Windows platform ([ADR-004](decisions/ADR-004-historical-builds-and-client-layout.md)):

1. Riot Sieve for the replay region, when it lists the exact build.
2. Otherwise the public riot-manifests index (`LoL/<region>/windows/lol-game-client/<build>.txt`, trying the replay region, then `NA1` and `EUW1`). The index only supplies the manifest URL.
3. Otherwise the newest build of the same patch that Sieve lists, marked `exactBuild: false`.

Every manifest URL must be HTTPS under `*.riotcdn.net`. Redirects are only followed within the origin (scheme, host, and port) of each request, so manifests and bundles never come from another host; Sieve and the index follow the same rule. The plan reports the source (`sieve` or `manifestIndex`) and the manifest URL.

The destination is `<clients folder>\<build>\Game`, where `<build>` uses the replay format (`16.18.817.5716`) derived from Riot's artifact version (`16.18.8175716`). The RMAN manifest is downloaded only after a user action, and files are filtered to neutral + `windows` + the selected locale.

Before client files are changed, the module classifies the destination as `install`, `resume`, `repair`, or `ready`. Files with the expected manifest size are reused, missing files are downloaded, and wrong-sized files are repair candidates. The exact mode, affected file count, destination, locale, and byte totals are returned for confirmation. Applying the plan requires the same confirmed mode, resolved version, and destination. If any of them changed, a new review is required. Planning never writes to disk.

Downloads follow [ADR-005](decisions/ADR-005-bundle-range-downloads.md):

- Pending chunks are grouped by bundle and deduplicated.
- Contiguous chunks are merged into range requests of at most 8 MiB, ordered by the first write they produce.
- Each file is assembled in `.chronorift/tmp` and committed as soon as it is complete.
- Repair swaps a completed file through a rollback backup instead of modifying the old file in place.
- When a plan is confirmed (also a `ready` one), the `.part` files and `.backup` files that an interrupted run left in `.chronorift/tmp` are removed. A `.backup` is the replaced wrong-size file of a repair cut between its two renames. Unknown files stay, and planning or opening a replay never removes anything.

Progress is reported once per range with real byte counts. RMAN parsing runs behind a panic guard because `cdragon-rman` panics on malformed tables.

When a confirmed install starts (any mode but `ready`), the plan is written to `.chronorift/plan.json` and any previous `install.json` is removed; `install.json` is written again, and `plan.json` removed, only when the client is complete. `cancel_client_install` sets a shared flag that the downloader checks between ranges: the install stops with `Cancelled`, in-flight requests are aborted, partly assembled files are discarded, and completed files stay for the next `resume` plan.

`client_statuses` and `list_clients` read the clients folder without network access and without creating anything:

- `ready`: `install.json` exists and so does `League of Legends.exe`.
- `partial`: a confirmed plan or game files exist. The percentage uses the plan total.
- `missing`: nothing is there.

### Match assets

The UI resolves the replay patch to Riot's latest matching DataDragon version, then loads champion and item JSON in the interface language (`es_MX`, or `es_ES` on a Spain-Spanish system) with an `en_US` fallback. It also requests patch-scoped CommunityDragon spell, perk, and rune-tree data with a neutral fallback; rune trees are optional. The game client locale chosen in the plan is independent of these names. These fetches only enrich the match view; a failure leaves identifiers visible and never starts client work or blocks replay launch.

Every DataDragon and CommunityDragon request, JSON and images alike, goes through the `lolasset` URI scheme (`http://lolasset.localhost/<source>/<path>` on Windows, `source` being `ddragon` or `cdragon`). The Rust handler (`asset_cache`) is asynchronous and keeps files under `<app cache dir>\assets` ([ADR-007](decisions/ADR-007-asset-disk-cache.md)):

- A path with a version (`cdn/16.18.1/…`, `16.18/…`) never changes: it is downloaded once and served from disk from then on, also offline.
- Other paths (`api/versions.json`) are refreshed after 12 hours; if the network fails, the stale copy is served.
- A 404 on a versioned path is remembered for 24 hours, so the locale fallback does not reach the network on every run. DataDragon (an S3 bucket) answers a missing file with 403, so on a versioned DataDragon path a 403 counts as a 404 and is remembered too; a 403 from CommunityDragon, or on a path without a version, stays an error. The marker only counts for the exact file name: NTFS ignores case but the CDNs do not, so a 404 for one spelling never hides a file with another.
- Concurrent requests for the same file wait for one download, and at most six downloads run at a time. A download larger than 32 MiB is refused while it is read, with or without `Content-Length`.
- Each file is written to `.<name>.<n>.tmp` next to it and renamed into place. At startup, a background pass removes such temporary files older than 10 minutes, left by an interrupted run; newer ones may belong to another running instance.
- Responses carry `X-Content-Type-Options: nosniff`: the type comes from the file extension.
- Only the two known origins are reachable: requests go only to them, and redirects are only followed within the same origin. Every path segment is validated as a Windows file name before touching the disk, including device names such as `NUL .png` or `COM¹`.

### Webview security

- CSP allows scripts, styles, and fonts from the app only. Images and `fetch` may reach only the `lolasset` cache scheme, never the CDNs directly, so nothing can skip the disk cache; IPC is allowed. The development CSP adds the Vite server.
- `withGlobalTauri` is off: the UI imports `@tauri-apps/api` modules.
- Fonts (Geist, Geist Mono) ship with the app; nothing is loaded from font CDNs.
- The opener permissions are `reveal_item_in_dir` ("Mostrar en carpeta" and "Abrir carpeta") and `open_url` scoped to `https://www.leagueofgraphs.com/match/*`. The match id opens the match there, built from the replay's platform (`LA2` → `las`) and Riot game id. OP.GG was considered, but its match pages need an internal OP.GG key that a replay does not carry.
- The window has no native decorations. The top bar is a `data-tauri-drag-region`, and the only window permissions beyond the defaults are start dragging, maximize on double-click, minimize, toggle maximize, and close.
- There is no generic download command: every download goes through a confirmed client plan against Riot's CDN.

## Data Storage

Start with simple files:

- app settings JSON (`settings.json` in the app config directory; implemented)
- known clients JSON (not implemented yet)
- install metadata JSON per client folder (`<build>\Game\.chronorift\install.json`; implemented)
- confirmed plan of an unfinished install (`<build>\Game\.chronorift\plan.json`; implemented)
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
