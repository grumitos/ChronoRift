# Agent Guide

## Project Rules

- Clean rewrite only. Do not copy ReplayBook GPL code into this repo.
- Opening a replay must never download, install, overwrite, delete, or launch anything.
- User-triggered client work must show destination, locale, progress, and result.
- Prefer Rust stdlib and existing dependencies before adding new ones.
- Keep the UI vanilla TypeScript until repetition justifies a framework.

## Commands

- Install deps: `bun install`
- Frontend build: `bun run build`
- Desktop dev: `bun run tauri dev`
- Rust tests: `cargo test --manifest-path src-tauri/Cargo.toml`
- Rust format check: `cargo fmt --manifest-path src-tauri/Cargo.toml --check`

## Definition Of Done

- Rust behavior has a small `cargo test` check.
- Frontend compiles with `bun run build`.
- Docs are updated when product flow or architecture changes.
- No generated build output is committed.
