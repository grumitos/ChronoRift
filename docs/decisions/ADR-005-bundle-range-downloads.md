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

## Update 2026-09-30: disk reservation, head-of-line blocking, and resume inside files

A real `16.17.810.4348` install (415 files, 23.7 GiB download, 29.2 GiB installed) to `E:` (NTFS, 4 KiB clusters) showed:

- The process wrote about 1.24 GB while free space on `E:` dropped about 6.24 GB. On a second run, 603 MB written for a 5.4 GB drop. The space came back when Stop deleted the `.part` files.
- After 57 s, 334 `.part` files were being assembled at once, and none was complete, so Stop kept nothing.
- Writes stopped completely for 22 s while the download was running.
- Every resume downloaded the manifest again.

Cause of the reservation, measured on `C:` (NTFS, 4 KiB clusters). Each row is 2,000 writes into one sparse file, 256 KiB apart unless noted. "Reserved" is the drop in free space, which matched `GetCompressedFileSizeW` once flushed:

| Writes | Written | Reserved | Ratio |
| --- | --- | --- | --- |
| 4 KiB, inside one 64 KiB unit | 7.8 MiB | 125 MiB | 16.0 |
| 28 KiB, inside one unit | 54.7 MiB | 125 MiB | 2.29 |
| 28 KiB, across a unit boundary | 54.7 MiB | 250 MiB | 4.57 |
| 64 KiB, across a unit boundary | 125 MiB | 250 MiB | 2.00 |
| 28 KiB, contiguous, shuffled order | 54.7 MiB | 54.8 MiB | 1.00 |
| Normal file, 28 KiB, 256 KiB apart | 54.7 MiB | 500 MiB | 9.13 (zero-filled) |

- NTFS reserves a sparse file in whole compression units of 16 clusters (64 KiB) when the data is written, before anything is flushed. During the writes, `GetCompressedFileSizeW` still showed about 2 MiB, but free space had already dropped.
- The real manifest averages about 28 KiB per chunk reference (29 GiB over about 1.08 M references). Bundle order scatters such writes across hundreds of files, so almost every write reserves one or two fresh units. That predicts 2.3 to 4.6 times the written bytes, close to the 5 times observed.
- The reservation never exceeds the file sizes, and it shrinks back to the exact size as files complete. With bundle order, though, files hardly ever completed.

Head-of-line blocking, confirmed on the local test CDN before any change. With the bundle of the first window in plan order held and 3 connections, the old loop sent exactly 3 requests and wrote nothing for 3 s. It waited for windows in order, so the finished fetches sat in the queue behind the stuck one, and no new request was sent either. A window in retry backoff (2 + 4 + 8 s plus the request times) therefore stops both the network and the disk. This is consistent with the 22 s pause, but it was not proven on the user's PC.

Changes:

- **File-by-file order.** Windows are unchanged (same merging, same request count). They are ordered so that files finish one after another:
  - Each chunk counts for the file where it lands, the first by path.
  - The next windows belong to the started file with the fewest windows left; when none is started, the next file by path starts.
  - A chunk is written into every file that uses it and is already half written. If none is, it lands in the first one by path, and the other files copy it from the disk when they start. A file whose chunks all landed elsewhere is assembled from local copies.
  - On a synthetic client, a pure model of the measured 64 KiB reservation gives: by bundle, 47 of 48 files half written at once and 7.5 MiB reserved beyond what was written; file by file, 3 files and 0.95 MiB, with the same 770 requests. The synthetic client has 28 KiB chunks on average, four versions of bundles packed in path order, superseded chunks left as gaps, and a block shared by every file. The env-gated real-manifest test prints the same comparison for a real manifest; it has not been run on one yet.
  - There is no hard cap on open files: when a bundle really mixes many files, a cap would only cut the download down to one connection.
- **Write on arrival.** Each window is written as soon as it arrives, so a stuck window holds only its own files. Memory stays bounded to about `connections + 2` windows. The test CDN now holds the first-in-order bundle while the later windows are written and their file is committed.
- **Resume inside files.** Stop, a failure after the retries, or a crash keep the `.part` files. `.chronorift/journal.json` records the ranges of each one that reached the disk. A resume requests only the missing chunks, and its progress starts from the bytes already on disk. See [ADR-003](ADR-003-resume-repair.md).
- **Pre-extended `.part` files.** A `.part` is extended to its final size only when the sparse flag took, so FAT and exFAT get no up-front allocation.
- **Saved manifest.** The manifest is saved as `.chronorift/manifest/<id>.manifest` when an install is confirmed. Planning again and resuming reuse it if it parses and declares the id of its URL. It is removed when the client is complete.
