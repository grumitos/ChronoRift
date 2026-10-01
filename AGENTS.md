# Agent Guide

## Project Rules

- Clean rewrite only. Do not copy ReplayBook GPL code into this repo.
- Opening a replay must never download, install, overwrite, delete, or launch anything.
- User-triggered client work must show destination, locale, progress, and result.
- Prefer Rust stdlib and existing dependencies before adding new ones.
- Keep the UI vanilla TypeScript until repetition justifies a framework.
- Tauri versions follow the policy shared with FairTranslator (README, "Version Policy"): exact pins, bumps agreed with that project, packages at least 7 days old, no general `cargo update` while Tauri is pinned to an older minor.

## Commands

- Install deps: `bun install`
- Frontend build: `bun run build`
- Frontend tests: `bun test`
- Desktop dev: `bun run tauri dev`
- Rust tests: `cargo test --manifest-path src-tauri/Cargo.toml`
- Rust format check: `cargo fmt --manifest-path src-tauri/Cargo.toml --check`
- Rust lint: `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets`

## Definition Of Done

- Rust behavior has a small `cargo test` check.
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets` reports no warnings.
- Frontend compiles with `bun run build`.
- Frontend tests pass with `bun test`.
- Docs are updated when product flow or architecture changes.
- No generated build output is committed.
