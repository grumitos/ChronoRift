# ADR-001: Use Rust, Tauri 2, Bun, And Vanilla TypeScript

## Status

Accepted

## Date

2026-06-30

## Context

ChronoRift needs native filesystem access, replay binary parsing, hidden download work with visible progress, client installation, and launching League executables. It should avoid becoming full Windows-only or requiring .NET unless necessary.

ReplayBook is C#/.NET/WPF and GPL-3.0. ManifestDownloader is C and MIT. ChronoRift is a clean rewrite, using those projects as behavioral references.

## Decision

Use:

- Rust for native core.
- Tauri 2 for desktop shell.
- Bun for frontend package scripts.
- Vanilla TypeScript, HTML, and CSS for the initial UI.

## Alternatives Considered

### Go + Wails

Good option. Rejected for now because Rust/Tauri gives a stronger fit for binary parsing, installer safety, and Tauri's mature desktop permission model.

### Electron

Rejected because shipping Chromium and Node is unnecessary for a replay utility.

### .NET + Avalonia

Technically viable. Rejected for now because it pulls the project closer to ReplayBook's stack and user runtime assumptions.

### React/Vue/Svelte

Rejected for the first cut. The UI does not need component machinery yet.

## Consequences

- Developer machines need Rust, Bun, and platform build tools.
- User machines get a bundled desktop app and WebView2 on Windows if missing.
- Native logic stays testable with `cargo test`.
- UI stays small until repeated views justify a framework.
