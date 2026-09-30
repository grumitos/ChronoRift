# ADR-003: Resume And Repair Require A Destination-Aware Plan

## Status

Accepted

## Date

2026-07-13

## Context

Large historical League clients can be interrupted after many valid files have already been written. Refusing every existing target forces users to discard useful downloads, while blindly overwriting a destination hides destructive work and can damage a client the user selected manually.

The RMAN data exposed by the current parser provides manifest paths, target sizes, and bundle chunks. It does not expose a ready-to-use full-file checksum through ChronoRift's existing dependencies.

## Decision

Every user-requested client plan classifies the selected destination as:

- `install`: no selected manifest files exist.
- `resume`: matching files can be reused and other files are missing.
- `repair`: at least one selected file exists with the wrong size.
- `ready`: every selected file exists with the expected size.

The plan reports the exact mode, locale, destination, affected files, reused files, repair files, download bytes, and install bytes. Applying client work must carry the same confirmed mode. If the destination changes enough to alter the mode, ChronoRift rejects the operation and requires a new plan.

Matching files are reused. Missing files are downloaded. Repair candidates are downloaded to `.chronorift/tmp`, then swapped into place through a temporary rollback backup only after the replacement is complete. ChronoRift ignores unknown files and refuses to replace non-file targets.

This is structural verification by path and size, not a claim of content-integrity verification.

## Alternatives Considered

### Redownload every file

Rejected because it wastes bandwidth and prevents practical recovery from interrupted multi-gigabyte installs.

### Overwrite existing files during a normal install

Rejected because it crosses the explicit repair boundary and makes destination mistakes destructive.

### Delete and recreate the destination

Rejected because ChronoRift must never delete a user-selected client folder automatically or remove unknown files.

### Add full-file hashing now

Deferred. It would require a verified hash contract or another dependency and would read every installed byte before planning. Path-and-size validation provides useful resume behavior without claiming stronger integrity than it can prove.

## Consequences

- Interrupted installs can continue without redownloading structurally valid files. Since [ADR-005](ADR-005-bundle-range-downloads.md), chunks arrive by bundle range, but each file is still committed only when it is complete, so resume granularity stays per file.
- The confirmation carries the resolved build and destination as well as the mode ([ADR-004](ADR-004-historical-builds-and-client-layout.md)).
- Repair is visible and distinguishable from installation.
- A plan can become stale and must then be reviewed again.
- Same-size corruption remains a known limitation.
