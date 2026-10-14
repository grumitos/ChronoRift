# Agent Guide

## Project Rules

- Clean rewrite only. Do not copy ReplayBook GPL code into this repo.
- Opening a replay must never download, install, overwrite, delete, or launch anything.
- User-triggered client work must show destination, locale, progress, and result.
- Prefer Rust stdlib and existing dependencies before adding new ones.
- Keep the UI vanilla TypeScript until repetition justifies a framework.
- Tauri versions follow the policy shared with FairTranslator (README, "Version Policy"): exact pins, the same Tauri set in both apps, packages at least 7 days old (crates a bump brings into the lock included), `rust-version` equal to the highest of the locked graph, and no general `cargo update`. ChronoRift may bump on its own with these rules and tells FairTranslator the versions it chose. The full rule is `VERSION_POLICY.md` in the shared design kit.

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
