# ChronoRift

ChronoRift is a desktop replay library for old League of Legends `.rofl` files. It inspects replay metadata, shows the required patch, lets the user choose an existing or downloaded game client, and launches the replay with a visible locale.

The goal is ReplayBook-style usability plus an explicit game-client downloader/installer. Opening a replay must never download or install anything by itself.

## Current Status

Implemented:

- Tauri 2 desktop scaffold with Bun-managed vanilla TypeScript UI.
- Classic ROFL metadata inspection in Rust.
- ROFL2 trailer metadata/stat parsing for current replay files.
- Replay match view with DataDragon champion/item assets plus CommunityDragon spell and perk details resolved by patch and locale.
- Disk cache for DataDragon and CommunityDragon: data and images go through the `lolasset` scheme and each file is downloaded once. Versioned files are then served from disk, also offline. The CSP blocks direct CDN access, so nothing skips the cache. A missing versioned file (a 404, or the 403 DataDragon answers with) is remembered for 24 hours, only for its exact file name (the CDNs are case-sensitive, NTFS is not), and temporary files left by an interrupted write are removed at the next start. Redirects are only followed within the same origin, here and for Riot's endpoints.
- Commands that read the disk, validate a folder, or start the game run off the main thread, so a slow or network drive never freezes the window.
- Backend errors are in neutral Spanish, including the common network failures of client downloads (timeout, connection, refused redirect, HTTP status, incomplete response); other details appended by Windows or a library are kept as they come. Reproducir only starts an executable named `League of Legends.exe`. Confirming a client plan removes the `.part` and `.backup` files that an interrupted install left in `.chronorift\tmp`.
- Instant replay switching: the library scan keeps each replay it already parsed, the UI reads every replay and warms every patch's data in the background, and selecting a match renders it at once without a loading state.
- Manual League executable launch path with editable Riot locale.
- Explicit League client install, resume, and structural repair from Riot RMAN manifests with visible destination, locale, build, manifest source, plan, and progress.
- Historical builds: Riot Sieve first, then the public riot-manifests index for the replay's exact build. Manifests and bundles always come from Riot's CDN.
- Persisted clients folder (for example `E:\ChronoRift`), one folder per build: `<clients folder>\<build>\Game\League of Legends.exe`.
- Bundle-range downloads: shared chunks are fetched once, contiguous chunks are requested together, and 8 ranges run in parallel. The parallel count can be set with `downloadConnections` (1 to 32) in `settings.json`.
- Client work can be stopped from the UI between ranges; completed files stay on disk and the next plan resumes.
- Local client state per build (ready, partial, missing), read from the clients folder without network access or writes.
- Library view: the Riot ID present in every parsed replay is treated as the library owner, so each row shows that player's result, champion, KDA, and the client state of the build. Files that cannot be read are listed at the end in grey with the reason; they cannot be selected and only appear under "Todas".
- Match view: a summary row (result and score, date, duration, patch, region, and the match id, which opens the match on League of Graphs), a client strip (state, build, location, locale, and the actions: choose executable, prepare/resume/progress, Reproducir; a failed launch or folder action is shown under it), tabs with the replay file name, size, and "Mostrar en carpeta", a scoreboard with team objectives, a stats tab (team totals, damage to champions by type, per-player detail), and a file tab. A replay that could not be parsed shows the reason instead of the scoreboard.
- Role quest slot: since the 2026 role quests every player has a `ROLE_BOUND_ITEM` (bot laners' boots, the support's Control Ward, or the quest reward of other roles). The scoreboard shows it between the six items and the trinket; replays without it keep seven slots.
- A client downloaded by ChronoRift is shown and launched with the locale it was installed with, because it only has that locale's files. A chosen executable uses the replay's suggested locale or the one picked in the plan.
- Clients view: downloads folder, builds used by the library, their size and state.
- Monochrome UI with bundled Geist and Geist Mono (OFL) fonts, a strict CSP, no global Tauri object, and the opener plugin limited to revealing files in the file manager and to opening League of Graphs match pages.
- Riot Vanguard state, read-only: while its `vgk` driver is loaded, the client strip shows "Vanguard activo" next to Reproducir. The state is read again when the window gains focus and after a failed launch.
- Frameless window: the top bar is the title bar (drag to move, double-click to maximize) with its own minimize, maximize, and close buttons. The Windows 11 snap-layout flyout of the native maximize button is not available; `Win + Z`, `Win + arrows`, and edge snapping still work.
- Product and architecture docs.

Known limits:

- Existing client files are validated by manifest path and expected size; same-size content corruption is not detected yet.
- ROFL2 payload packet decoding is not implemented; trailer metadata and end-of-game stats power the match view.
- Stopping or interrupting an install keeps completed files only; files that were partly assembled are downloaded again on resume.
- Chunks arrive in bundle order, so NTFS zero-fills the gaps in partly written files. This can roughly double disk writes during an install.
- Builds that are neither in Sieve nor in the riot-manifests index cannot be resolved. If the exact build is missing but Sieve lists the same patch, the plan offers the newest build of that patch and warns that it may not play the replay.
- Riot Vanguard blocks launches of a separate League installation while its `vgk` kernel driver is loaded: `CreateProcess` fails with access denied (os error 5), even elevated. ChronoRift only reads the driver state and never stops or reconfigures it ([ADR-006](docs/decisions/ADR-006-vanguard-read-only.md)). To watch a replay, exit Vanguard from its tray icon; `vgk` starts with Windows, so a restart brings it back for online play. On 2026-09-29, with `vgk` and `vgc` stopped, the downloaded `16.18.817.5716` client played `LA2-1625390199.rofl`.

## Commands

| Command | Description |
| --- | --- |
| `bun install` | Install frontend and Tauri CLI dependencies. |
| `bun run dev` | Start Vite only. |
| `bun run tauri dev` | Start the desktop app in development. |
| `bun run build` | Type-check and build the frontend. |
| `bun test` | Run the frontend suites: asset pipeline and the UI flow (`index.html` + `src/main.ts` on happy-dom with mocked Tauri IPC). |
| `bun run tauri build` | Build the release app (`src-tauri/target/release/chronorift.exe`) and its per-user NSIS installer in Spanish (`src-tauri/target/release/bundle/nsis/ChronoRift_<version>_x64-setup.exe`). |
| `cargo test --manifest-path src-tauri/Cargo.toml` | Run Rust tests, including the client E2E against a local HTTP CDN. No network or real files needed. |
| `cargo fmt --manifest-path src-tauri/Cargo.toml --check` | Check Rust formatting. |
| `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets` | Lint Rust code. |

Optional suites against real data are skipped unless their variables are set:

| Variable | Test | What it checks |
| --- | --- | --- |
| `CHRONORIFT_REPLAY_DIRECTORY` | `rofl::tests::real_replays_match_an_independent_read_and_stay_untouched` | Every `.rofl` in the folder parses, key fields match an independent read of the trailer JSON, and the folder is unchanged afterwards. |
| `CHRONORIFT_MANIFEST_SAMPLE` (+ optional `CHRONORIFT_MANIFEST_LOCALE`) | `client::tests::real_manifest_selection_matches_rman_flags` | The path-based locale selection skips no file that the RMAN flags require, and prints download size and range-request count. |

Example (PowerShell): `$env:CHRONORIFT_REPLAY_DIRECTORY="$env:USERPROFILE\Documents\League of Legends\Replays"; cargo test --manifest-path src-tauri/Cargo.toml real_ -- --nocapture`

Two harnesses have real side effects. They are `#[ignore]` and only run when asked for by name with `--ignored`:

| Test | Requires | What it does |
| --- | --- | --- |
| `client::tests::real_install_downloads_the_replay_build` | `CHRONORIFT_ALLOW_REAL_DOWNLOAD=1`, `CHRONORIFT_REAL_CLIENTS_ROOT`, `CHRONORIFT_REAL_GAME_VERSION` (optional: `CHRONORIFT_REAL_REGION`, `CHRONORIFT_REAL_LOCALE`, `CHRONORIFT_REAL_CONNECTIONS`) | Plans against Riot. It aborts unless the plan is the exact build inside the clients folder, then confirms and installs the plan as shown and prints progress and throughput. If it is interrupted, running it again resumes. |
| `launcher::tests::real_launch_starts_the_game_for_a_replay` | `CHRONORIFT_REAL_LAUNCH_EXE`, `CHRONORIFT_REAL_LAUNCH_REPLAY` (optional: `CHRONORIFT_REAL_LOCALE`), and Riot Vanguard stopped | Starts `League of Legends.exe` with the replay, using the same arguments as the app. |

```powershell
$env:CHRONORIFT_ALLOW_REAL_DOWNLOAD="1"; $env:CHRONORIFT_REAL_CLIENTS_ROOT="E:\ChronoRift"
$env:CHRONORIFT_REAL_GAME_VERSION="16.18.817.5716"; $env:CHRONORIFT_REAL_REGION="LA2"; $env:CHRONORIFT_REAL_LOCALE="es_MX"
cargo test --manifest-path src-tauri/Cargo.toml real_install_downloads_the_replay_build -- --ignored --nocapture
```

## Architecture

- `src-tauri/src/rofl.rs` parses replay metadata.
- `src-tauri/src/library.rs` scans one replay folder level.
- `src-tauri/src/client.rs` resolves and installs the matching League game client.
- `src-tauri/src/settings.rs` persists the clients folder in the app config directory.
- `src-tauri/src/launcher.rs` launches a replay with a chosen League executable and locale.
- `src-tauri/src/vanguard.rs` reads the state of Riot Vanguard's driver without changing it.
- `src/main.ts` is the UI controller: library, match tabs, client panel, clients view, and launch.
- `src/types.ts`, `src/format.ts`, and `src/dom.ts` hold the IPC contracts, number/date formatting, and DOM helpers with inline SVG icons.
- `src/game-assets.ts` resolves DataDragon and CommunityDragon match assets without blocking replay inspection, through the disk cache.
- `src-tauri/src/asset_cache.rs` serves the `lolasset` scheme: each DataDragon/CommunityDragon file is downloaded once and kept on disk.
- `docs/` holds project guide and ADRs.

See [docs/product-guide.md](docs/product-guide.md) and [docs/decisions/ADR-001-stack.md](docs/decisions/ADR-001-stack.md).

## Sources Used For Design

- ReplayBook: https://github.com/fraxiinus/ReplayBook
- ManifestDownloader: https://github.com/Morilli/ManifestDownloader
- Riot manifests reference: https://github.com/Morilli/riot-manifests
- Tauri docs: https://v2.tauri.app/start/
- Bun docs: https://bun.sh/docs

## Non-Endorsement

ChronoRift is not endorsed by Riot Games and does not reflect Riot Games' views. League of Legends and Riot Games are Riot Games trademarks.
