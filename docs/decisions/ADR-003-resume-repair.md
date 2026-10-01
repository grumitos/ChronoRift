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

## Update 2026-09-30: resume inside each file

Resume granularity is now the chunk, not the file. Stopping a 23.7 GiB download after a minute used to keep nothing: hundreds of files were half written, and their `.part` files were deleted ([ADR-005](ADR-005-bundle-range-downloads.md)).

- **What is kept.** Stop, a failure after the retries, and a crash keep the `.part` files.
- **The journal.** `.chronorift/journal.json` records which ranges of each `.part` reached the disk. It also records the plan it belongs to (manifest URL, resolved version, locale, destination) and, for each file:
  - its size;
  - its `.part` name, built from its stable position in the manifest selection;
  - whether it is sparse;
  - the `.part` creation time.
- **Durability.**
  - A checkpoint runs at least every 10 s and when the download stops or fails.
  - It flushes (`sync_data`) every `.part` written since the previous checkpoint. Only then does it replace the journal: the journal is written aside, flushed, and renamed over the old one.
  - So the journal never claims bytes that are not on disk, and a power cut loses at most the last 10 s.
  - A complete file is flushed before it is renamed into place, so after a power cut a file with the right size has no holes.
- **Trust.** A journal entry is used only when all of these hold:
  - the plan matches;
  - the manifest path, size, and `.part` name match the pending file;
  - the ranges are ordered and inside the file;
  - the `.part` still exists with the recorded creation time;
  - its length is the full size (sparse) or covers the ranges (not sparse).
  - Any other `.part` or journal is removed when a plan is confirmed. The journal is read again once the destination's lock is taken, and a change since planning requires a new review.
- **The resume plan.**
  - A placement fully covered by a trusted range is done.
  - `downloadBytes` counts only chunks that still have a missing placement, so a chunk shared with a done placement is fetched again.
  - `installBytes` is what is left to write.
  - A destination with only kept `.part` files is a `resume`, while `repair` still wins when a target has the wrong size.
- **Progress.** Progress starts from the compressed size of the chunks already on disk. `client_statuses` adds the journaled bytes to `installedBytes`.
- **Unchanged.** Committed files are still validated by path and size only. Kept `.part` data is trusted through the journal rules above, not by content.
