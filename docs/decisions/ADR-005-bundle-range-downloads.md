# ADR-005: Download Bundle Ranges, Not Individual Chunks

## Status

Accepted

## Date

2026-09-29

## Context

The first installer sent one HTTP range request per chunk, in sequence, file by file.

An independent reading of the real `16.18.8175716` manifest with `es_MX` gives these numbers:

- 416 of 4,056 files are selected.
- The chunks add up to 27.66 GiB, from 1,082,502 chunk references.
- The unique chunks add up to 23.48 GiB: 4.2 GiB were references to chunks that several files share.
- Merged by contiguity inside each bundle, the unique chunks need only 1,289 ranges, across 1,156 bundles.
- Cold range requests to Riot's CDN took about 0.35 to 0.8 s each.

At that latency the chunk-by-chunk approach would need days. The work is dominated by request latency, not bandwidth.

## Decision

- Group pending chunks by bundle and deduplicate them by `(bundle, offset)`. The plan's download size counts every unique chunk once.
- Merge contiguous chunks into windows of at most 8 MiB, split at chunk boundaries. Order the windows by the first file position they write, so large files are written close to sequentially.
- Keep up to N range requests in flight as independent tasks. N is 8 by default and can be set with `downloadConnections` (1 to 32) in `settings.json`.
- Apply the windows in plan order on a blocking thread: decompress each chunk, check its size, and write it to every file position that uses it. Networking keeps going while the disk works. Memory stays bounded to about N + 1 windows.
- Assemble each file in `.chronorift/tmp/<index>-<name>.part` and commit it as soon as its last byte is written, with the ADR-003 rollback swap when repairing.
- On any error, stop scheduling, abort requests still in flight, keep the files already committed, and remove the uncommitted temporary files. Stale `.part` files from a crashed run are removed at the start of the next run.
- Report progress once per window, with real compressed bytes.
- Record the download duration in the result and in `install.json`.

## Alternatives Considered

### Parallel per-chunk requests

Rejected: even with 32 connections, about 1 M requests would still be latency-bound, and shared chunks would be downloaded twice.

### Whole-bundle downloads

Rejected: the 1,156 bundles total 39.8 GiB, which is 16 GiB more than needed.

### HTTP multi-range requests

Rejected: parsing multipart responses adds complexity, and merged single ranges already remove almost all of the request overhead.

## Consequences

- The request count drops from about 1.08 M to about 3,300 windows (3,773 for the real `16.18.8175716` `es_MX` plan). The install is now bounded by what the CDN delivers.
- Measured on 2026-09-29 for 23.5 GiB to the USB SSD `E:`:
  - Throughput averaged about 110 Mbit/s, with a peak of 204 Mbit/s.
  - A `curl` check with 14 parallel 8 MiB ranges reached 145 Mbit/s, and a single connection reached 39 Mbit/s.
  - With 16 connections, the speed was no better than with 8.
  - The download process used about 0.2 of a CPU core and 140 to 280 MB of RAM.
- Writing in bundle order makes NTFS zero-fill the gaps in partly written files, which can roughly double disk writes. When an install is interrupted, the partly assembled files are discarded. Sparse temporary files, or buffering chunks for sequential writes, would avoid both.
- Resume and repair semantics are unchanged: validation is per file, by path and size.
- Files complete in window order rather than manifest order. An interruption therefore keeps whatever files finished, not a prefix of the manifest.
- `cdragon-rman` 0.2 tests the file-flag bit `id` while current RMAN 2.1 manifests use bit `id - 1`. Selection still uses path markers, and the real-manifest test checks them against the corrected flags.

## Update 2026-09-29

- Leftovers of an interrupted run in `.chronorift/tmp` are removed when the user confirms a plan, including a `ready` one: `.part` files, and the `.backup` of a repair cut between its two renames (the replaced, wrong-size file). Before, only `.part` files were removed, when the download started, so an orphan `.backup` was never removed.
- Planning, opening a replay, or reading the client state never removes anything, and unknown files in `.chronorift/tmp` stay.

## Update 2026-09-29: stalls, retries, and stops

In the real app, a `16.17` install sat at "0 % · 14.8 MiB of 23.7 GiB" for 80 s:

- The process had written about 292 MB, and the network stalled.
- Free space on `E:` (NTFS) dropped about 54 GiB, then came back exactly on Stop.
- Earlier, a full `16.18` install finished at about 13 MB/s, but downloads sometimes died and had to be restarted by hand.

Cause, measured on NTFS (`C:`) with the same `create_new` + `seek` + `write_all` that assembles a `.part`:

| Write of 1 MiB | Time | Allocated |
| --- | --- | --- |
| Normal file, at 2 GiB | 1.95 s | 2,049 MiB |
| Normal file, at 512 MiB | 0.46 s | 513 MiB |
| Normal file, `set_len` first, at 2 GiB | 2.58 s | 2,049 MiB |
| Sparse file, at 2 GiB | under 1 ms | 1 MiB |

- In a normal file, a write beyond the valid data length zero-fills everything before it, synchronously (about 1 GiB/s on that disk). `set_len` does not avoid it, and the writer never called it.
- While the fill runs, the file grows and a directory listing lags behind it: 0.4 s into a write at 4 GiB (3.92 s in total), the listing showed 0 bytes while the file already had 384 MiB. That matches the `.part` files listed at 0 bytes.
- Shared chunks make the first windows write into many files at far offsets. The single writer then zero-fills gigabytes, and the bounded pipeline holds the 8 windows in flight behind it.
- The `16.17` plan totals 29.25 GiB installed, so its `.part` files alone do not explain a 54 GiB drop. An overlapping second install is possible but not proven.

Changes:

- Each `.part` is marked sparse (`FSCTL_SET_SPARSE`) before its first write, best effort: other file systems keep zero-filling. The flag stays on the committed file. Clearing it takes 1 ms after forward writes, but forces a synchronous flush of ranges filled late: 0.58 s per 512 MiB written back to front, against 1.18 s for a plain flush. Partly assembled files are still discarded on an interruption.
- Progress counts bytes as they arrive, with about five events per second, instead of one event per applied window. It never goes back when a retry discards bytes.
- A window is retried silently after a timeout, a connection error, a 5xx or 429, a response that is not 206 (a 200 with the whole bundle, for example), or an incomplete body. One attempt plus five retries, with waits of 2, 4, 8, 16, and 32 s, give about a minute of tolerance. Other 4xx and validation errors, such as a 206 with an unexpected `Content-Length`, are never retried.
- The 300 s total timeout is replaced by a 60 s inactivity timeout (`read_timeout`). A slow transfer never dies, and a stalled one is retried.
- Stop aborts requests in flight within 100 ms instead of waiting for the current window. It also interrupts the plan recomputed at confirmation.
- One install runs at a time: a guard in the process, plus `.chronorift/install.lock` opened without sharing against other processes. The stale-temp cleanup runs only while holding that lock.
