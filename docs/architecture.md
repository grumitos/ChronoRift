# Architecture

## Runtime Shape

ChronoRift uses Tauri 2:

- Rust owns filesystem, replay parsing, download/install, process launch, and validation.
- TypeScript owns screen state and user interaction.
- Bun is only the developer package manager.

The installed user app must not require Bun, Node, Rust, or .NET.

## Modules

### `rofl`

Reads classic ROFL headers and extracts metadata JSON. It also reads modern ROFL2 trailer metadata and parses `statsJson` into player summaries. It returns a small `ReplayInspection` object for the UI.

- Every length read from a replay is bounded (offset + length within the file, metadata at most 16 MB) before anything is allocated. ROFL2 reads only the header, the 4-byte trailer length at the end of the file, and the metadata JSON.
- ROFL2 game version comes from the length-prefixed header field (magic, 8 bytes, `u8` length, version string).
- Region and match id come from metadata when present, otherwise from file names shaped `<anything>-<PLATFORM>-<digits>.rofl` or `<PLATFORM>-<digits>.rofl`; unknown platforms give no region.
- Error model: I/O failures are errors. Files that were read but are not usable replays return status `invalid` (or `unsupported` for ROFL2 files whose trailer is unreadable) with a reason in `notes`.
- `dataDragonVersion` is the replay's `major.minor` patch; the UI resolves the concrete DataDragon release and builds image URLs. The raw `statsJson` string in `metadata` is replaced by `"<parsed into players>"`.

Known ceiling: ROFL2 payload packet decoding is not implemented yet. Metadata and end-of-game stats are available.

### `launcher`

Validates the request (absolute, existing `.rofl` replay; canonical, existing `League of Legends.exe` directly inside a `Game` folder; well-formed locale), then launches the executable from its `Game` folder with:

- replay path
- `-GameBaseDir=<parent of Game>` (official installs `<LoL>/Game/...`, ChronoRift installs `<dest>/Game/...`)
- `-SkipBuild -EnableLNP -UseNewX3D=1 -UseNewX3DFramebuffers=1`
- explicit `-Locale`

Argument building (`build_launch_args`) is a pure function tested on every platform; only the spawn is Windows-only.

Launch is Windows-only because the League client replay flow is Windows-only in practice.

### `install` (client install pipeline)

`src-tauri/src/install/` owns everything between "the user wants this replay's client" and a ready `League of Legends.exe`. Contract: [roadmap.md](roadmap.md#install-pipeline-contract-v2).

- `resolve`: finds a `lol-game-client` manifest. Riot Sieve only serves the current patch, so the community archive [Morilli/riot-manifests](https://github.com/Morilli/riot-manifests) comes first: the exact build (`16.17.810.4348` → `LoL/<REGION>/windows/lol-game-client/16.17.8104348.txt`, then the same file under `NA1`/`EUW1`, since the game client is identical across regions), then other archived builds of the same patch, newest first (one GitHub contents-API listing per region dir, only when the exact file is missing), then Sieve. Manifests that answer 403/404 are skipped. Archive entries must be `https://*.riotcdn.net/**/releases/*.manifest`. The result records `source`, `exactBuild`, `resolvedVersion`, and which region's entry was used; nothing falls back silently. The server must be one of 17 known platform ids; there is no default.
- `http`: one shared `reqwest` client (connect/read timeouts, `ChronoRift/0.1` user agent) behind a small `Fetch` trait so resolution is unit-tested with a fake. Sieve/archive bodies are capped at 1 MB, manifests at 64 MB; cancel is checked between body chunks.
- `manifest`: reads RMAN 2.0/2.1 through `cdragon-rman` inside `catch_unwind` into owned tables, with checked chunk/directory lookups and cycle-safe paths, so malformed manifests are errors, not panics. Files are selected by flag bitmask (flag id `n` is bit `n-1`): neutral + `windows` + the requested locale. The path-name heuristic is only a fallback for manifests without flags. The plan reports the manifest's locales and which locale's assets are installed (`es_AR` → `es_MX`).
- `download`: dedupes chunk refs by chunk id, groups them by bundle, coalesces chunks at most 64 KiB apart into ranged GETs of at most 8 MiB, and runs 8 at a time (tokio `JoinSet`). Retryable failures (network, 403, 429, 5xx) are retried 3 times with backoff. Each range is decompressed on the blocking pool, checked against the chunk's `target_size`, and written at its file offset(s) into `<dest>/.chronorift/tmp`. When a file's last chunk lands it is synced, size-checked, and renamed into place. Existing targets are never replaced unless repair allows that path. On failure or cancel, incomplete temp files are removed; finished files stay for repair. Chunk hashes are not verified (the chunk-id hash scheme could not be confirmed against real chunks); zstd decoding and exact sizes are checked instead.
- `store`: layout and versioned files. Install to `<dest>/Game/` (official layout, launcher uses `-GameBaseDir=<dest>`); `<dest>/.chronorift/install.json` and `install-state.json` are version 2 with paths relative to `<dest>`. Version 1 files (absolute paths, game files directly in `<dest>`) are still read.
- `repair`: disk classification (ok / missing / wrong size) for repair plans.
- `progress`: the engine reports through a `Progress` trait; the Tauri implementation emits `download-progress` at most ~10 times per second, always on stage changes, and always a final `complete`/`cancelled`/`failed` event.
- `clients`: discovers installed clients under the clients root from `install.json` (v1 or v2).

Plans are pinned: `plan_client_install` / `plan_client_repair` keep the selected files in memory under a `planId` (30 min, at most 3 plans), and `install_client` / `repair_client` run exactly that plan. Only one plan/install/repair runs at a time.

### DataDragon assets

The UI resolves the replay patch to Riot's latest matching DataDragon version, then loads champion and item JSON for the visible locale with an `en_US` fallback. These fetches only enrich the scoreboard; they do not install or launch anything.

## Data Storage

Start with simple files:

- app settings JSON
- known clients JSON
- install metadata JSON per client folder (`.chronorift/install.json`, `.chronorift/install-state.json`, schema version 2)

Use SQLite only when search, filtering, or indexing becomes painful.

## Safety Boundaries

- Never delete user-selected client folders automatically.
- Never overwrite an existing install without confirmation.
- Never launch a replay if the replay path or executable path does not exist.
- Never accept arbitrary launch locale strings. Keep `xx_YY` validation.
- Never hide download/install progress.
