# ChronoRift

ChronoRift is a desktop replay library for old League of Legends `.rofl` files. It inspects replay metadata, shows the required patch, lets the user choose an existing or downloaded game client, and launches the replay with a visible locale.

The goal is ReplayBook-style usability plus an explicit game-client downloader/installer. Opening a replay must never download or install anything by itself.

## Current Status

Implemented:

- Tauri 2 desktop scaffold with Bun-managed vanilla TypeScript UI.
- Classic ROFL metadata inspection in Rust.
- Manual League executable launch path with editable Riot locale.
- Download job plumbing with progress events.
- Product and architecture docs.

Not implemented yet:

- Riot manifest resolution for a specific patch.
- RMAN install/extract pipeline.
- Static-data rich match view from CommunityDragon/DataDragon.

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

- `src-tauri/src/rofl.rs` parses replay metadata.
- `src-tauri/src/launcher.rs` launches a replay with a chosen League executable and locale.
- `src-tauri/src/installer.rs` owns visible download progress events.
- `src/main.ts` is the thin UI controller.
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
