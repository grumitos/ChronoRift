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

Known ceiling: ROFL2 payload packet decoding is not implemented yet. Metadata and end-of-game stats are available.

### `launcher`

Launches `League of Legends.exe` with:

- replay path
- `-GameBaseDir`
- replay launch flags based on ReplayBook behavior
- explicit `-Locale`

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
