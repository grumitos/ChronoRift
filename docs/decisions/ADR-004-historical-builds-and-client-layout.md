# ADR-004: Resolve Historical Builds And Keep One Folder Per Build

## Status

Accepted

## Date

2026-09-29

## Context

Riot Sieve (`sieve.services.riotcdn.net`) only lists the builds that are live right now. On 2026-09-29 it returned three `16.19` builds for LA2 and NA1. The seven sample replays needed `16.17.810.4348` and `16.18.816.5012` / `16.18.817.5716`, so every client plan failed with "manifest not found". That contradicts the product goal of playing old `.rofl` files.

Riot keeps old manifests on its CDN. The public [Morilli/riot-manifests](https://github.com/Morilli/riot-manifests) repository records them as `LoL/<region>/windows/lol-game-client/<artifact version>.txt`, and each file holds one manifest URL. Checked samples:

- `16.18.8175716` points to `https://lol.secure.dyn.riotcdn.net/channels/public/releases/344E097FEB027691.manifest`, which Riot's CDN still serves.
- `16.19.8217343` points to exactly the URL that Sieve returns for that build.
- The `lol-game-client` manifest URL is the same in every region.
- All 458 LA2 entries (10.10 to 16.19) use a seven-digit third component, which is build and revision (`817` + `5716`).

Users also asked for one persistent download location on a large drive, organized by version, instead of choosing a folder for each install.

## Decision

Resolution order for a replay build (`16.18.817.5716` becomes artifact `16.18.8175716`; the revision is zero-padded to four digits):

1. The exact build in Sieve for the replay region.
2. The exact build in the riot-manifests index, trying the replay region, then `NA1`, then `EUW1`.
3. The newest Sieve build of the same patch, flagged `exactBuild: false` so the plan shows that it may not play the replay.

The index only supplies a URL. Every manifest URL must be HTTPS under `*.riotcdn.net` and end in `.manifest`, whichever source provided it. Bundles are derived from that URL, so all game bytes come from Riot's CDN. The plan shows the source (`Riot Sieve` or `riot-manifests`) and the manifest URL.

Layout:

- The user sets a clients folder once. It is stored in `settings.json` in the app config directory. If the user picks a drive root such as `E:\`, ChronoRift uses `E:\ChronoRift`.
- Each build installs to `<clients folder>\<build>\Game\`. `<build>` uses the replay format (`16.18.817.5716`) derived from the resolved artifact, so the folder name always describes what is installed. When the exact build is missing, the folder is named after the newest build of the patch that is actually installed. Only digits and dots are accepted, so the name cannot escape the clients folder.
- The `Game` subfolder mirrors Riot's layout. `-GameBaseDir` therefore points at the build folder, and the game's `Config` and `Logs` stay inside it instead of being shared between builds.
- Nothing is created until the user confirms a plan. The confirmation must match the recalculated mode, resolved version, and destination.

## Alternatives Considered

### Sieve only

Rejected: it cannot install any build older than the live patch.

### Manual manifest URL field

Kept as a possible fallback for builds missing from both sources. Rejected as the main path because it moves Riot internals into the UI.

### Artifact version as folder name (`16.18.8175716`)

Rejected for display consistency. The replay and `League of Legends.exe` both show `16.18.817.5716`, and the conversion is deterministic for every known artifact.

## Consequences

- Any build listed in the riot-manifests index can be planned, as long as Riot's CDN still serves its manifest and bundles.
- ChronoRift depends on a third-party index for the URL of old builds. A compromised index can at most point to another Riot CDN manifest. The plan shows the source and URL, and the exact-build flag stays visible.
- Existing ad-hoc destinations from earlier versions are not migrated. The plan simply proposes the new per-build folder.
