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

### `installer`

Owns generic visible download progress for direct HTTP(S) artifacts.

### `client`

Resolves `lol-game-client` releases from Riot Sieve for the replay region and Windows platform. It prefers the exact replay build when available, downloads the RMAN manifest only after user action, filters files to neutral + `windows` + selected locale, downloads bundle byte ranges, decompresses Zstandard chunks, and writes `League of Legends.exe` plus install metadata into the chosen destination.

### DataDragon assets

The UI resolves the replay patch to Riot's latest matching DataDragon version, then loads champion and item JSON for the visible locale with an `en_US` fallback. These fetches only enrich the scoreboard; they do not install or launch anything.

## Data Storage

Start with simple files:

- app settings JSON
- known clients JSON
- install metadata JSON per client folder

Use SQLite only when search, filtering, or indexing becomes painful.

## Safety Boundaries

- Never delete user-selected client folders automatically.
- Never overwrite an existing install without confirmation.
- Never launch a replay if the replay path or executable path does not exist.
- Never accept arbitrary launch locale strings. Keep `xx_YY` validation.
- Never hide download/install progress.
