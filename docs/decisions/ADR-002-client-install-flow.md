# ADR-002: Client Download And Install Must Be Explicit

## Status

Accepted

## Date

2026-06-30

## Context

The app exists to remove friction from old replay playback. The risky part is downloading and installing old game clients. Doing that silently would be surprising, expensive in disk space, and potentially wrong if the user wants a custom destination or language.

## Decision

ChronoRift may detect the required patch and suggest locale automatically, but it must not download, install, overwrite, delete, or launch without a visible user action.

The first installer resolves Riot's Windows `lol-game-client` artifact. That RMAN manifest already contains `League of Legends.exe` and locale-tagged game files, so ChronoRift filters neutral files, `windows` files, and files tagged with the selected locale instead of downloading every language or duplicating `lol-standalone-client-content`.

The UI must show:

- required patch
- server/region when available
- suggested locale
- editable locale
- install destination
- progress and current stage
- final result: the client's destination folder, with an action that opens it with `League of Legends.exe` selected

## Alternatives Considered

### Auto-install on replay open

Rejected. It hides disk/network side effects behind a read action.

### Require users to install clients manually

Rejected. It preserves ReplayBook's main friction point.

### Install only into ChronoRift's version folder

Rejected. Users need control over disk layout and storage devices.

## Consequences

- More visible UI state is needed.
- Downloader state must be durable enough to recover from interruption.
- Locale suggestion must be editable and treated as a default, not truth.
- The initial installer refused all existing targets. [ADR-003](ADR-003-resume-repair.md) extends this decision with an explicit, previewed resume/repair flow while preserving the user-action boundary.
- [ADR-004](ADR-004-historical-builds-and-client-layout.md) replaces the free-form destination with a persisted clients folder and one `<build>\Game` folder per build. The full destination is still shown before confirmation. It also adds the riot-manifests index as a second source for the manifest URL.
