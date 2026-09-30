# ChronoRift

ChronoRift is a desktop replay library for old League of Legends `.rofl` files. It inspects replay metadata, shows the required patch, lets the user choose an existing or downloaded game client, and launches the replay with a visible locale.

The goal is ReplayBook-style usability plus an explicit game-client downloader/installer. Opening a replay must never download or install anything by itself.

## Current Status

Implemented:

- Tauri 2 desktop scaffold with Bun-managed vanilla TypeScript UI.
- Classic ROFL metadata inspection in Rust.
- ROFL2 trailer metadata/stat parsing for current replay files. The build comes from the header's length-prefixed version string. A crafted file cannot exhaust memory: metadata over 4 MiB is rejected, and a `statsJson` with more than 32 players is dropped with a note after reading its 33rd entry. `inspect_replay` answers carry only what the UI shows, not the raw metadata JSON.
- Replay match view with DataDragon champion/item assets plus CommunityDragon spell and perk details resolved by patch and locale.
- Disk cache for DataDragon and CommunityDragon: data and images go through the `lolasset` scheme and each file is downloaded once. Versioned files are then served from disk, also offline. The CSP blocks direct CDN access, so nothing skips the cache. A missing versioned file (a 404, or the 403 DataDragon answers with) is remembered for 24 hours, only for its exact file name (the CDNs are case-sensitive, NTFS is not), and temporary files left by an interrupted write are removed at the next start. Redirects are only followed within the same origin, here and for Riot's endpoints. Every cache response carries a sandboxing CSP, since third-party SVGs are served from a local origin, and CORS is granted only to the app's own origins. If the HTTP client cannot be built with that policy, the cache serves only what is on disk.
- Commands that read the disk, validate a folder, or start the game run off the main thread, so a slow or network drive never freezes the window.
- Backend errors are in neutral Spanish, including the common network failures of client downloads (timeout, connection, refused redirect, HTTP status, incomplete response); other details appended by Windows or a library are kept as they come. Reproducir only starts an executable named `League of Legends.exe`. Confirming a client plan removes the `.part` and `.backup` files that an interrupted install left in `.chronorift\tmp`.
- Instant replay switching: the library scan keeps each replay it already parsed, the UI reads every replay and warms every patch's data in the background, and selecting a match renders it at once without a loading state. The images each replay of the library uses (and the list portraits) are then decoded in the background, so switching paints them in the same frame; an image that is not in memory yet appears with a 120 ms fade in its fixed-size slot, without a loader.
- Startup without a white flash: the window is created with the app's dark background (`backgroundColor`), and `index.html` paints a static splash (logo and a three-dot loader, static with reduced motion) before the interface loads. It fades out as soon as the library is shown, fails, or there is no remembered folder, with a 5 s backup.
- Image choice, asked once: the first time a library with patches loads, the splash asks "¿Descargar todas las imágenes ahora?" with the patch count, image count, and estimated size (1 163 images and about 12 MiB per patch). "Descargar todo" downloads every champion, item, summoner-spell, rune, and rune-tree icon of those patches through the disk cache, six at a time, with percent and patches done on the splash; "Continuar" moves it to the top bar. A file the CDN does not have counts as done, and an error never blocks anything: after twelve errors in a row it stops silently and resumes when the folder is read again. With "Descargar todo", the patches that appear later are downloaded in the background with the top-bar indicator. "Al abrir cada partida" keeps loading on demand. The choice is saved in `settings.json` (`assetPrefetch`) and can be changed in Clientes.
- Manual League executable launch path with editable Riot locale. After Reproducir, a background thread waits for the game and emits `replay-exited` (replay path and exit code) when it closes.
- Explicit League client install, resume, and structural repair from Riot RMAN manifests with visible destination, locale, build, manifest source, plan, and progress.
- Historical builds: Riot Sieve first, then the public riot-manifests index for the replay's exact build. Manifests and bundles always come from Riot's CDN.
- Persisted clients folder (for example `E:\ChronoRift`), one folder per build: `<clients folder>\<build>\Game\League of Legends.exe`.
- Bundle-range downloads: shared chunks are fetched once, contiguous chunks are requested together, and 8 ranges run in parallel. The parallel count can be set with `downloadConnections` (1 to 32) in `settings.json`. Confirming requires the same mode, build, destination, locale, and manifest as the reviewed plan.
- Resilient downloads: a range that fails with a timeout, a connection error, a 5xx or 429, or an incomplete answer is retried silently, one attempt plus five retries with waits of 2, 4, 8, 16, and 32 s. Only then does the install stop with the error; completed files stay and the next plan resumes. There is no total time limit: a transfer that receives nothing for 60 s counts as stalled and is retried. Progress follows the bytes as they arrive, about five times per second. On NTFS, partly assembled files are sparse, so a chunk far into a large file is written at once instead of first zero-filling the gap.
- Client work can be stopped from the UI at any moment: requests in flight are aborted within a tenth of a second, completed files stay on disk, and the next plan resumes. One install runs at a time, also after a window reload and across instances (a second instance finds `.chronorift\install.lock` held in the destination); another one is rejected with "Ya hay una descarga en curso.".
- Local client state per build (ready, partial, missing), read from the clients folder without network access or writes. When the exact build has no ready client, the newest ready build of the same patch is reported with `exactBuild: false`. If the drive of the clients folder is missing, the state query fails with "La carpeta de clientes no está disponible: <path>"; a folder that does not exist yet on a present drive only means that nothing is installed.
- Library view: replays are sorted by file date, newest first, with the higher match id first on the same date. The Riot ID present in every parsed replay is treated as the library owner, so each row shows that player's result, champion, KDA, and the client state of the build; that state stays neutral until it has been read from disk. Choosing a row with the keyboard keeps the focus on it. Files that cannot be read are listed at the end in grey with the reason; they cannot be selected and only appear under "Todas". A folder that cannot be read (a sleeping network drive, an unplugged USB disk) stays remembered, and "Actualizar" retries it.
- Match view: a summary row (result and score, the file's download date labeled "Descargada", duration, patch, region, and the match id, which opens the match on League of Graphs), a client strip (state, build, location, locale, and the actions: choose executable, prepare/resume/progress, Reproducir; a failed launch, download, executable choice, or folder action is shown under it), tabs with the replay file name, size, and "Mostrar en carpeta", a scoreboard with team objectives, a stats tab (team totals, damage to champions by type, per-player detail), and a file tab. A replay without statistics (unreadable, or with more than 32 players) shows "—" instead of a score and the reason instead of the scoreboard. The client drawer is a dialog: the match behind it is inert and Escape closes it, while the library stays usable.
- Download failures stay visible under the client strip, with the drawer open or closed, until the next attempt; the plan is then recomputed on its own, so Reanudar opens a plan ready to confirm. Detener shows the stopped state at once: the drawer closes, the progress bar and the top-bar indicator go away, and the build shows "Incompleto" with Reanudar. The backend finishes stopping in the background; Reanudar, or any other download, waits for it before planning. When it ends, the client state is read from disk again and the resume plan is recomputed, so Reanudar opens a plan ready to confirm. If the download finished just as Detener was pressed, the build shows "Listo". One download runs at a time: while it runs, the actions that would start another build are disabled. Progress never shows 100 % before the client is ready.
- Nothing moves when a download starts, advances, or stops: the client strip keeps a fixed-width status cell and a fixed-width action button (its mini meter sits under the status text, out of the flow), the location path is the part that ellipsizes, the drawer reserves the line of the file in progress, and the top-bar percentage has a fixed width. At the minimum window size the strip stays on one row while downloading.
- A chosen executable must be named `League of Legends.exe`; another file is rejected when it is picked, with the same message as the backend.
- Role quest slot: since the 2026 role quests every player has a `ROLE_BOUND_ITEM` (bot laners' boots, the support's Control Ward, or the quest reward of other roles). The scoreboard shows it between the six items and the trinket; replays without it keep seven slots.
- A client downloaded by ChronoRift is shown and launched with the locale it was installed with, because it only has that locale's files. A chosen executable uses the replay's suggested locale or the one picked in the plan. When a newer build of the same patch is installed instead of the exact one, the strip shows that build with a "no exacta" chip and launches it.
- Clients view: downloads folder, the image choice ("Descargar todo" or "Al abrir cada partida"), builds used by the library, their size and state, with the running download's progress updated live. An unavailable clients folder (for example a disconnected USB drive) is reported there and under the client strip, which shows "No disponible" instead of "No instalado" and keeps Reproducir disabled; the statuses are read again when the window regains focus.
- Monochrome UI with a few semantic accents, each next to its word, letter, or glyph: victory green and defeat red (result chips, V/D badges, the filter counts), the blue and red team marks and score labels, the client state (ready green; incomplete, Vanguard, or unavailable drive amber; downloading blue), and error lines in red. The identity color, a muted Hextech gold (`--brand`, #C8AA6E), is limited to the single primary action of each view (Reproducir; Descargar or Reanudar in the drawer), progress fills, the 2 px line of the active tab and section, and the completed steps of a download. Every tinted text is above 4.5:1 on its background. The other buttons, segmented controls, borders, body text, the focus ring, and the logo stay monochrome. Bundled Geist and Geist Mono (OFL) fonts, a strict CSP, no global Tauri object, and the opener plugin limited to revealing files in the file manager and to opening League of Graphs match pages.
- Riot Vanguard state, read-only: while its `vgk` driver is loaded, the client strip shows "Vanguard activo" next to Reproducir. The state is read again when the window gains focus and after a failed launch. "Reproduciendo" lasts until the game closes (`replay-exited`) and never hides "Vanguard activo".
- Frameless window: the top bar is the title bar (drag to move, double-click to maximize) with its own minimize, maximize, and close buttons. The Windows 11 snap-layout flyout of the native maximize button is not available; `Win + Z`, `Win + arrows`, and edge snapping still work. The installed app ignores F5, Ctrl+R, and Ctrl+Shift+R: a reload would lose the state of a running download.
- Product and architecture docs.

Known limits:

- Existing client files are validated by manifest path and expected size; same-size content corruption is not detected yet.
- ROFL2 payload packet decoding is not implemented; trailer metadata and end-of-game stats power the match view.
- Stopping or interrupting an install keeps completed files only; files that were partly assembled are downloaded again on resume.
- Chunks arrive in bundle order. On NTFS the partly written files are sparse, so no gap is zero-filled. File systems without sparse files (exFAT, FAT32) still zero-fill the gaps, which can stall the start of an install and roughly double disk writes.
- Builds that are neither in Sieve nor in the riot-manifests index cannot be resolved. If the index has no entry for the exact build (404) but Sieve lists the same patch, the plan offers the newest build of that patch and warns that it may not play the replay. Any other index error is shown instead.
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
| `launcher::tests::real_launch_starts_the_game_for_a_replay` | `CHRONORIFT_REAL_LAUNCH_EXE`, `CHRONORIFT_REAL_LAUNCH_REPLAY` (optional: `CHRONORIFT_REAL_LOCALE`), and Riot Vanguard stopped | Starts `League of Legends.exe` with the replay, using the same arguments as the app, then waits until the game closes and prints the `replay-exited` payload. |

```powershell
$env:CHRONORIFT_ALLOW_REAL_DOWNLOAD="1"; $env:CHRONORIFT_REAL_CLIENTS_ROOT="E:\ChronoRift"
$env:CHRONORIFT_REAL_GAME_VERSION="16.18.817.5716"; $env:CHRONORIFT_REAL_REGION="LA2"; $env:CHRONORIFT_REAL_LOCALE="es_MX"
cargo test --manifest-path src-tauri/Cargo.toml real_install_downloads_the_replay_build -- --ignored --nocapture
```

## Architecture

- `src-tauri/src/rofl.rs` parses replay metadata.
- `src-tauri/src/library.rs` scans one replay folder level.
- `src-tauri/src/client.rs` resolves and installs the matching League game client.
- `src-tauri/src/settings.rs` persists the clients folder and the image choice in the app config directory.
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
