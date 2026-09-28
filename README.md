# ChronoRift

ChronoRift is a desktop replay viewer for League of Legends `.rofl` files. It lists every replay in your replays folder, shows a post-game match view (players, KDA, damage, gold, CS, vision, runes, items, objectives), and can download the exact game client build a replay needs so you can watch it.

Opening or scanning replays never downloads, installs, or launches anything. Client work only happens after you review an install plan and confirm it.

## Current Status

Implemented:

- Replay library: read-only scan of a replays folder with champions, patch, server, duration, and score per match; filter by champion, player, patch, server, or match id.
- Match view: team objectives, per-player stats, runes, spells, items with localized names on hover, a full stats comparison, and profile links (op.gg, League of Graphs) by Riot ID and region. Works offline with text fallbacks.
- Remembers the last selected replay and the window size.
- Replay parsing for classic ROFL and ROFL2 (header, trailer metadata, end-of-game stats) with bounded reads.
- Client install for old patches: exact build from the public manifest archive (Morilli/riot-manifests), Riot's live service for the current patch, with a visible notice when only a nearby build exists.
- Install plan with locale availability, sizes, destination, and explicit confirmation; concurrent, coalesced downloads with real progress; cancel; resume/repair of interrupted installs.
- Launch with a chosen `Game\League of Legends.exe` and locale (Windows).
- Browser preview (`bun run dev`) with sample data.

Not done yet: a Windows bundle smoke test and release notes, and a CommunityDragon fallback for very old patches. See [docs/roadmap.md](docs/roadmap.md).

## Commands

| Command | Description |
| --- | --- |
| `bun install` | Install frontend and Tauri CLI dependencies. |
| `bun run dev` | Start Vite only. |
| `bun run tauri dev` | Start the desktop app in development. |
| `bun run build` | Type-check and build the frontend. |
| `bun run tauri build` | Build the desktop bundle. |
| `cargo test --manifest-path src-tauri/Cargo.toml` | Run Rust tests. |
| `cargo fmt --manifest-path src-tauri/Cargo.toml --check` | Check Rust formatting. |

## Architecture

- `src-tauri/src/rofl.rs` parses replay metadata and stats.
- `src-tauri/src/library.rs` scans a replays folder (read-only).
- `src-tauri/src/install/` resolves manifests, plans, downloads, repairs, and records client installs.
- `src-tauri/src/launcher.rs` validates and launches a replay.
- `src-tauri/src/settings.rs` and `locale.rs` hold persisted settings and locale validation.
- `src/` is the vanilla TypeScript UI (`api.ts` for commands, `views/` for screens, `datadragon.ts` for static assets).
- `docs/` holds the product guide, architecture, roadmap, and ADRs.

See [docs/product-guide.md](docs/product-guide.md), [docs/architecture.md](docs/architecture.md), and [docs/decisions/ADR-001-stack.md](docs/decisions/ADR-001-stack.md).

## Sources Used For Design

- ReplayBook: https://github.com/fraxiinus/ReplayBook
- ManifestDownloader: https://github.com/Morilli/ManifestDownloader
- Riot manifests reference: https://github.com/Morilli/riot-manifests
- Tauri docs: https://v2.tauri.app/start/
- Bun docs: https://bun.sh/docs

## Non-Endorsement

ChronoRift is not endorsed by Riot Games and does not reflect Riot Games' views. League of Legends and Riot Games are Riot Games trademarks.
