//! Download engine: chunk refs are deduplicated by chunk id, grouped by bundle, and nearby
//! chunks are coalesced into one ranged GET. Ranges run with bounded concurrency; each range
//! is decompressed off the async runtime and written at its file offsets into temp files,
//! which are size-checked, synced, and renamed into place when their last chunk lands.

use super::{
    error::InstallError,
    http::{check_cancel, HttpClient, RangeError},
    manifest::PlannedFile,
    progress::{Progress, Stage, Update},
    store::safe_join,
};
use reqwest::Url;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    future::Future,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::task::JoinSet;

/// Chunks at most this far apart in a bundle share one request (the gap is downloaded and
/// discarded).
pub const COALESCE_GAP: u64 = 64 * 1024;
/// Upper bound for one request, which also bounds memory: CONCURRENCY × MAX_SPAN.
pub const MAX_SPAN: u64 = 8 << 20;
pub const CONCURRENCY: usize = 8;
const ATTEMPTS: u32 = 4;
const RETRY_BASE: Duration = Duration::from_millis(500);
/// Temp files kept open at once; others are reopened on demand.
const MAX_OPEN_FILES: usize = 64;

/// Where one decompressed chunk goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub file: usize,
    pub offset: u64,
}

/// One chunk inside a fetched range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeChunk {
    /// Offset of the compressed chunk within the range body.
    pub start: u64,
    pub size: u32,
    pub target_size: u32,
    pub targets: Vec<Target>,
}

/// One ranged GET: `[start, end)` of a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRange {
    pub bundle_id: u64,
    pub start: u64,
    pub end: u64,
    pub chunks: Vec<RangeChunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadPlan {
    pub ranges: Vec<FetchRange>,
    /// Bytes on the wire, including coalesced gaps.
    pub download_bytes: u64,
    pub chunk_refs: usize,
    pub unique_chunks: usize,
    /// Chunk writes each file needs before it is complete.
    pub writes_per_file: Vec<usize>,
}

fn manifest_error(message: impl Into<String>) -> InstallError {
    InstallError::Manifest(message.into())
}

/// Dedupes chunks by id, groups them by bundle, and coalesces nearby ones into ranges.
pub fn plan_ranges(
    files: &[PlannedFile],
    gap: u64,
    max_span: u64,
) -> Result<DownloadPlan, InstallError> {
    struct Unique {
        bundle_id: u64,
        start: u64,
        end: u64,
        size: u32,
        target_size: u32,
        targets: Vec<Target>,
    }
    let mut index: HashMap<u64, usize> = HashMap::new();
    let mut uniques: Vec<Unique> = Vec::new();
    let mut chunk_refs = 0usize;
    let mut writes_per_file = Vec::with_capacity(files.len());

    for (file_index, file) in files.iter().enumerate() {
        let mut offset = 0u64;
        for chunk in &file.chunks {
            if chunk.bundle_size == 0 {
                return Err(manifest_error(format!("{} has an empty chunk", file.path)));
            }
            let target = Target {
                file: file_index,
                offset,
            };
            offset = offset
                .checked_add(u64::from(chunk.target_size))
                .ok_or_else(|| manifest_error("file offset overflow"))?;
            let slot = *index.entry(chunk.id).or_insert_with(|| {
                uniques.push(Unique {
                    bundle_id: chunk.bundle_id,
                    start: chunk.bundle_offset,
                    end: 0,
                    size: chunk.bundle_size,
                    target_size: chunk.target_size,
                    targets: Vec::new(),
                });
                uniques.len() - 1
            });
            let unique = &mut uniques[slot];
            unique.end = unique
                .start
                .checked_add(u64::from(unique.size))
                .ok_or_else(|| manifest_error("bundle offset overflow"))?;
            unique.targets.push(target);
            chunk_refs += 1;
        }
        if offset != file.size {
            return Err(manifest_error(format!(
                "{} chunks add up to {offset} bytes, expected {}",
                file.path, file.size
            )));
        }
        writes_per_file.push(file.chunks.len());
    }

    uniques.sort_by_key(|unique| (unique.bundle_id, unique.start));
    let unique_chunks = uniques.len();
    let mut ranges: Vec<FetchRange> = Vec::new();
    for unique in uniques {
        let extend = ranges.last().is_some_and(|range| {
            range.bundle_id == unique.bundle_id
                && unique.start <= range.end.saturating_add(gap)
                && unique.end.max(range.end) - range.start <= max_span
        });
        if !extend {
            ranges.push(FetchRange {
                bundle_id: unique.bundle_id,
                start: unique.start,
                end: unique.end,
                chunks: Vec::new(),
            });
        }
        let range = ranges.last_mut().expect("a range was just ensured");
        range.end = range.end.max(unique.end);
        range.chunks.push(RangeChunk {
            start: unique.start - range.start,
            size: unique.size,
            target_size: unique.target_size,
            targets: unique.targets,
        });
    }
    // Roughly file order, so early files complete early (and survive an interruption).
    ranges.sort_by_key(|range| {
        range
            .chunks
            .iter()
            .flat_map(|chunk| chunk.targets.iter().map(|target| target.file))
            .min()
            .unwrap_or(usize::MAX)
    });
    let download_bytes = ranges.iter().map(|range| range.end - range.start).sum();
    Ok(DownloadPlan {
        ranges,
        download_bytes,
        chunk_refs,
        unique_chunks,
        writes_per_file,
    })
}

/// `https://…/channels/public/releases/X.manifest` → `https://…/channels/public/bundles`.
pub fn bundle_base_from_manifest_url(manifest_url: &str) -> Result<String, InstallError> {
    let mut url = Url::parse(manifest_url)
        .map_err(|_| manifest_error(format!("invalid manifest URL {manifest_url}")))?;
    let path = url.path().to_string();
    let index = path
        .rfind("/releases/")
        .ok_or_else(|| manifest_error(format!("no bundle URL for {manifest_url}")))?;
    url.set_path(&format!("{}/bundles", &path[..index]));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string().trim_end_matches('/').to_string())
}

/// Fetches byte ranges of bundles. Implemented by [`HttpClient`]; faked in tests.
pub trait BundleSource: Clone + Send + Sync + 'static {
    fn get_range(
        &self,
        url: &str,
        start: u64,
        end: u64,
        cancel: &AtomicBool,
        on_delta: &(dyn Fn(u64) + Sync),
    ) -> impl Future<Output = Result<Vec<u8>, RangeError>> + Send;
}

impl BundleSource for HttpClient {
    fn get_range(
        &self,
        url: &str,
        start: u64,
        end: u64,
        cancel: &AtomicBool,
        on_delta: &(dyn Fn(u64) + Sync),
    ) -> impl Future<Output = Result<Vec<u8>, RangeError>> + Send {
        HttpClient::get_range(self, url, start, end, cancel, on_delta)
    }
}

pub struct DownloadJob {
    /// Absolute game folder the manifest paths are relative to.
    pub game_path: PathBuf,
    pub temp_dir: PathBuf,
    pub bundle_base: String,
    pub files: Vec<PlannedFile>,
    /// Manifest paths whose existing file may be replaced (repair); anything else must not
    /// exist.
    pub replace_existing: HashSet<String>,
    pub stage: Stage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadStats {
    pub requests: usize,
    pub downloaded_bytes: u64,
    pub files: usize,
}

struct Slot {
    target: PathBuf,
    temp: PathBuf,
    size: u64,
    replace: bool,
}

#[derive(Default)]
struct SlotState {
    handle: Option<Arc<File>>,
    created: bool,
    remaining: usize,
    finished: bool,
}

/// Temp-file bookkeeping shared by the range workers.
struct Writer {
    slots: Vec<Slot>,
    states: Mutex<Vec<SlotState>>,
    files_done: AtomicU64,
}

fn lock_error() -> InstallError {
    InstallError::Io {
        source: std::io::Error::other("install writer lock poisoned"),
        path: None,
    }
}

impl Writer {
    fn new(job: &DownloadJob, writes_per_file: &[usize]) -> Result<Self, InstallError> {
        let slots = job
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let target = safe_join(&job.game_path, &file.path)?;
                // Index prefix keeps names unique even when sanitizing collides.
                let name = format!("{index:05}-{}", temp_name(&file.path));
                Ok(Slot {
                    target,
                    temp: job.temp_dir.join(name),
                    size: file.size,
                    replace: job.replace_existing.contains(&file.path),
                })
            })
            .collect::<Result<Vec<_>, InstallError>>()?;
        let states = writes_per_file
            .iter()
            .map(|remaining| SlotState {
                remaining: *remaining,
                ..SlotState::default()
            })
            .collect();
        Ok(Self {
            slots,
            states: Mutex::new(states),
            files_done: AtomicU64::new(0),
        })
    }

    /// Refuses to start if any target exists and may not be replaced.
    fn preflight(&self) -> Result<(), InstallError> {
        for slot in &self.slots {
            if !slot.replace && fs::symlink_metadata(&slot.target).is_ok() {
                return Err(InstallError::WouldOverwrite(slot.target.clone()));
            }
        }
        Ok(())
    }

    fn open(&self, file: usize, state: &mut SlotState) -> Result<Arc<File>, InstallError> {
        let slot = &self.slots[file];
        let handle = if state.created {
            OpenOptions::new().write(true).open(&slot.temp)
        } else {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&slot.temp)
        }
        .map_err(InstallError::io_at(&slot.temp))?;
        state.created = true;
        Ok(Arc::new(handle))
    }

    fn handle(&self, file: usize) -> Result<Arc<File>, InstallError> {
        let mut states = self.states.lock().map_err(|_| lock_error())?;
        if let Some(handle) = &states[file].handle {
            return Ok(handle.clone());
        }
        let open = states.iter().filter(|state| state.handle.is_some()).count();
        if open >= MAX_OPEN_FILES {
            // Writers holding a clone keep theirs alive; this only drops the cached ones.
            for state in states.iter_mut() {
                state.handle = None;
            }
        }
        let handle = self.open(file, &mut states[file])?;
        states[file].handle = Some(handle.clone());
        Ok(handle)
    }

    /// Writes one chunk; finalizes the file when it was its last one.
    fn write(&self, target: Target, data: &[u8]) -> Result<(), InstallError> {
        let slot = &self.slots[target.file];
        target
            .offset
            .checked_add(data.len() as u64)
            .filter(|end| *end <= slot.size)
            .ok_or_else(|| manifest_error("chunk past end of file"))?;
        let handle = self.handle(target.file)?;
        write_at(&handle, data, target.offset).map_err(InstallError::io_at(&slot.temp))?;
        drop(handle);
        let finished = {
            let mut states = self.states.lock().map_err(|_| lock_error())?;
            let state = &mut states[target.file];
            state.remaining = state
                .remaining
                .checked_sub(1)
                .ok_or_else(|| manifest_error("chunk written twice"))?;
            (state.remaining == 0).then(|| state.handle.take())
        };
        match finished {
            Some(handle) => self.finalize(target.file, handle),
            None => Ok(()),
        }
    }

    /// Syncs, size-checks, and renames a complete temp file into place.
    fn finalize(&self, file: usize, handle: Option<Arc<File>>) -> Result<(), InstallError> {
        let slot = &self.slots[file];
        let handle = match handle {
            Some(handle) => handle,
            None => {
                let mut states = self.states.lock().map_err(|_| lock_error())?;
                self.open(file, &mut states[file])?
            }
        };
        handle.sync_all().map_err(InstallError::io_at(&slot.temp))?;
        let written = handle
            .metadata()
            .map_err(InstallError::io_at(&slot.temp))?
            .len();
        drop(handle);
        if written != slot.size {
            return Err(InstallError::Io {
                source: std::io::Error::other(format!(
                    "wrote {written} bytes, expected {}",
                    slot.size
                )),
                path: Some(slot.target.clone()),
            });
        }
        if !slot.replace && fs::symlink_metadata(&slot.target).is_ok() {
            return Err(InstallError::WouldOverwrite(slot.target.clone()));
        }
        if let Some(parent) = slot.target.parent() {
            fs::create_dir_all(parent).map_err(InstallError::io_at(parent))?;
        }
        fs::rename(&slot.temp, &slot.target).map_err(InstallError::io_at(&slot.target))?;
        let mut states = self.states.lock().map_err(|_| lock_error())?;
        states[file].finished = true;
        drop(states);
        self.files_done.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Finalizes files that need no chunks (empty files).
    fn finalize_empty(&self) -> Result<(), InstallError> {
        let empty = {
            let states = self.states.lock().map_err(|_| lock_error())?;
            states
                .iter()
                .enumerate()
                .filter(|(_, state)| state.remaining == 0 && !state.finished)
                .map(|(index, _)| index)
                .collect::<Vec<_>>()
        };
        empty
            .into_iter()
            .try_for_each(|file| self.finalize(file, None))
    }

    fn all_finished(&self) -> bool {
        self.states
            .lock()
            .map(|states| states.iter().all(|state| state.finished))
            .unwrap_or(false)
    }

    /// Removes temp files of files that did not complete. Finished files stay (repair can
    /// resume from them).
    fn cleanup(&self) {
        let Ok(mut states) = self.states.lock() else {
            return;
        };
        for (slot, state) in self.slots.iter().zip(states.iter_mut()) {
            state.handle = None;
            if state.created && !state.finished {
                let _ = fs::remove_file(&slot.temp);
            }
        }
    }
}

#[cfg(unix)]
fn write_at(file: &File, data: &[u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(data, offset)
}

#[cfg(windows)]
fn write_at(file: &File, mut data: &[u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !data.is_empty() {
        let written = file.seek_write(data, offset)?;
        if written == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        data = &data[written..];
        offset += written as u64;
    }
    Ok(())
}

fn temp_name(manifest_path: &str) -> String {
    manifest_path
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Decompresses every chunk of a fetched range and writes it to all of its targets.
fn write_range(writer: &Writer, range: &FetchRange, body: &[u8]) -> Result<(), InstallError> {
    let mut decompressor =
        zstd::bulk::Decompressor::new().map_err(|err| manifest_error(err.to_string()))?;
    for chunk in &range.chunks {
        let start = usize::try_from(chunk.start).map_err(|_| manifest_error("range overflow"))?;
        let compressed = start
            .checked_add(chunk.size as usize)
            .and_then(|end| body.get(start..end))
            .ok_or_else(|| manifest_error("chunk outside fetched range"))?;
        let data = decompressor
            .decompress(compressed, chunk.target_size as usize)
            .map_err(|err| {
                InstallError::Network(format!(
                    "bundle {:016X}: chunk did not decompress: {err}",
                    range.bundle_id
                ))
            })?;
        if data.len() != chunk.target_size as usize {
            return Err(InstallError::Network(format!(
                "bundle {:016X}: chunk is {} bytes, expected {}",
                range.bundle_id,
                data.len(),
                chunk.target_size
            )));
        }
        for target in &chunk.targets {
            writer.write(*target, &data)?;
        }
    }
    Ok(())
}

struct Shared<S> {
    source: S,
    writer: Writer,
    bundle_base: String,
    cancel: Arc<AtomicBool>,
    /// Set when another range failed; workers stop early.
    stop: AtomicBool,
    downloaded: AtomicU64,
    total: u64,
    files_total: u64,
    stage: Stage,
    progress: Arc<dyn Progress>,
}

impl<S> Shared<S> {
    fn report(&self) {
        self.progress.report(
            Update::stage(self.stage, "Downloading client files")
                .bytes(self.downloaded.load(Ordering::Relaxed), Some(self.total))
                .files(
                    self.writer.files_done.load(Ordering::Relaxed),
                    self.files_total,
                ),
        );
    }

    fn check(&self) -> Result<(), InstallError> {
        check_cancel(&self.cancel)?;
        check_cancel(&self.stop)
    }
}

async fn fetch_range<S: BundleSource>(
    shared: &Shared<S>,
    range: &FetchRange,
) -> Result<Vec<u8>, InstallError> {
    let url = format!("{}/{:016X}.bundle", shared.bundle_base, range.bundle_id);
    let mut attempt = 0;
    loop {
        shared.check()?;
        let this_attempt = AtomicU64::new(0);
        let on_delta = |delta: u64| {
            this_attempt.fetch_add(delta, Ordering::Relaxed);
            shared.downloaded.fetch_add(delta, Ordering::Relaxed);
            shared.report();
        };
        let result = shared
            .source
            .get_range(&url, range.start, range.end, &shared.cancel, &on_delta)
            .await;
        match result {
            Ok(body) => return Ok(body),
            Err(RangeError { retryable, error }) => {
                // Do not count bytes of a failed attempt.
                shared
                    .downloaded
                    .fetch_sub(this_attempt.load(Ordering::Relaxed), Ordering::Relaxed);
                attempt += 1;
                if !retryable || attempt >= ATTEMPTS {
                    return Err(error);
                }
                let wait = RETRY_BASE * 2u32.pow(attempt - 1);
                let mut waited = Duration::ZERO;
                while waited < wait {
                    shared.check()?;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    waited += Duration::from_millis(100);
                }
            }
        }
    }
}

async fn run_range<S: BundleSource>(
    shared: Arc<Shared<S>>,
    range: FetchRange,
) -> Result<(), InstallError> {
    let body = fetch_range(&shared, &range).await?;
    shared.check()?;
    let worker = shared.clone();
    tokio::task::spawn_blocking(move || write_range(&worker.writer, &range, &body))
        .await
        .map_err(|err| InstallError::Io {
            source: std::io::Error::other(err.to_string()),
            path: None,
        })??;
    shared.report();
    Ok(())
}

/// Downloads and writes every file of `job`. On failure or cancel, temp files of incomplete
/// files are removed; completed files stay in place.
pub async fn download<S: BundleSource>(
    source: S,
    job: DownloadJob,
    cancel: Arc<AtomicBool>,
    progress: Arc<dyn Progress>,
) -> Result<DownloadStats, InstallError> {
    let plan = plan_ranges(&job.files, COALESCE_GAP, MAX_SPAN)?;
    let writer = Writer::new(&job, &plan.writes_per_file)?;
    writer.preflight()?;
    fs::create_dir_all(&job.temp_dir).map_err(InstallError::io_at(&job.temp_dir))?;
    let requests = plan.ranges.len();
    let shared = Arc::new(Shared {
        source,
        writer,
        bundle_base: job.bundle_base.clone(),
        cancel,
        stop: AtomicBool::new(false),
        downloaded: AtomicU64::new(0),
        total: plan.download_bytes,
        files_total: job.files.len() as u64,
        stage: job.stage,
        progress,
    });
    shared.report();

    let outcome = run_ranges(&shared, plan.ranges).await;
    let outcome = outcome.and_then(|()| {
        if shared.writer.all_finished() {
            Ok(())
        } else {
            Err(manifest_error("some files did not receive all chunks"))
        }
    });
    if outcome.is_err() {
        shared.writer.cleanup();
    }
    outcome?;
    shared.report();
    Ok(DownloadStats {
        requests,
        downloaded_bytes: shared.downloaded.load(Ordering::Relaxed),
        files: job.files.len(),
    })
}

async fn run_ranges<S: BundleSource>(
    shared: &Arc<Shared<S>>,
    ranges: Vec<FetchRange>,
) -> Result<(), InstallError> {
    shared.writer.finalize_empty()?;
    let mut pending = ranges.into_iter();
    let mut running = JoinSet::new();
    let mut first_error: Option<InstallError> = None;
    loop {
        while first_error.is_none() && running.len() < CONCURRENCY {
            let Some(range) = pending.next() else {
                break;
            };
            running.spawn(run_range(shared.clone(), range));
        }
        let Some(joined) = running.join_next().await else {
            break;
        };
        let result = joined.unwrap_or_else(|err| {
            Err(InstallError::Io {
                source: std::io::Error::other(err.to_string()),
                path: None,
            })
        });
        if let Err(err) = result {
            shared.stop.store(true, Ordering::SeqCst);
            // Prefer the real cause over the "stopped" errors it triggers in other workers.
            if first_error.is_none() || matches!(first_error, Some(InstallError::Cancelled)) {
                first_error = Some(err);
            }
        }
        if first_error.is_none() && shared.cancel.load(Ordering::SeqCst) {
            shared.stop.store(true, Ordering::SeqCst);
            first_error = Some(InstallError::Cancelled);
        }
    }
    if shared.cancel.load(Ordering::SeqCst) {
        return Err(InstallError::Cancelled);
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::{manifest::ChunkRef, progress::NoProgress, store::tests::temp_root};
    use std::path::Path;

    fn chunk(id: u64, bundle_id: u64, offset: u64, size: u32, target: u32) -> ChunkRef {
        ChunkRef {
            id,
            bundle_id,
            bundle_offset: offset,
            bundle_size: size,
            target_size: target,
        }
    }

    fn planned(path: &str, chunks: Vec<ChunkRef>) -> PlannedFile {
        PlannedFile {
            path: path.to_string(),
            size: chunks.iter().map(|c| u64::from(c.target_size)).sum(),
            chunks,
        }
    }

    #[test]
    fn coalesces_nearby_chunks_and_splits_on_gap_bundle_and_span() {
        let files = vec![planned(
            "a",
            vec![
                chunk(1, 7, 0, 100, 200),
                chunk(2, 7, 100, 100, 200),
                // 64 KiB gap exactly: still coalesced.
                chunk(3, 7, 200 + COALESCE_GAP, 10, 20),
                // One byte more than the gap: new range.
                chunk(4, 7, 210 + 2 * COALESCE_GAP + 1, 10, 20),
                chunk(5, 8, 0, 10, 20),
            ],
        )];
        let plan = plan_ranges(&files, COALESCE_GAP, MAX_SPAN).unwrap();
        let spans = plan
            .ranges
            .iter()
            .map(|r| (r.bundle_id, r.start, r.end, r.chunks.len()))
            .collect::<Vec<_>>();
        assert_eq!(
            spans,
            vec![
                (7, 0, 210 + COALESCE_GAP, 3),
                (7, 210 + 2 * COALESCE_GAP + 1, 220 + 2 * COALESCE_GAP + 1, 1),
                (8, 0, 10, 1),
            ]
        );
        assert_eq!(plan.ranges[0].chunks[2].start, 200 + COALESCE_GAP);
        assert_eq!(plan.download_bytes, 210 + COALESCE_GAP + 10 + 10);

        let capped = plan_ranges(&files, COALESCE_GAP, 150).unwrap();
        assert_eq!(
            capped.ranges.len(),
            5,
            "span cap splits even adjacent chunks"
        );
    }

    #[test]
    fn dedupes_chunks_across_and_within_files() {
        let shared = chunk(1, 7, 0, 100, 200);
        let files = vec![
            planned("a", vec![shared, chunk(2, 7, 100, 50, 60), shared]),
            planned("b", vec![shared]),
        ];
        let plan = plan_ranges(&files, 0, MAX_SPAN).unwrap();
        assert_eq!(plan.chunk_refs, 4);
        assert_eq!(plan.unique_chunks, 2);
        assert_eq!(plan.writes_per_file, vec![3, 1]);
        assert_eq!(plan.ranges.len(), 1);
        assert_eq!(plan.download_bytes, 150);
        assert_eq!(
            plan.ranges[0].chunks[0].targets,
            vec![
                Target { file: 0, offset: 0 },
                Target {
                    file: 0,
                    offset: 260
                },
                Target { file: 1, offset: 0 },
            ]
        );
    }

    #[test]
    fn offsets_are_overflow_safe() {
        let near_end = chunk(1, 7, u64::MAX - 5, 10, 20);
        assert!(plan_ranges(&[planned("a", vec![near_end])], 0, MAX_SPAN).is_err());
        let empty = chunk(1, 7, 0, 0, 0);
        assert!(plan_ranges(&[planned("a", vec![empty])], 0, MAX_SPAN).is_err());
        let mut wrong_size = planned("a", vec![chunk(1, 7, 0, 10, 20)]);
        wrong_size.size = 21;
        assert!(plan_ranges(&[wrong_size], 0, MAX_SPAN).is_err());
        // The gap test saturates instead of overflowing.
        let files = vec![planned(
            "a",
            vec![chunk(1, 7, 0, 10, 20), chunk(2, 7, u64::MAX - 20, 10, 20)],
        )];
        assert_eq!(
            plan_ranges(&files, u64::MAX, MAX_SPAN)
                .unwrap()
                .ranges
                .len(),
            2
        );
    }

    #[test]
    fn derives_bundle_base_url() {
        assert_eq!(
            bundle_base_from_manifest_url(
                "https://lol.secure.dyn.riotcdn.net/channels/public/releases/9A.manifest"
            )
            .unwrap(),
            "https://lol.secure.dyn.riotcdn.net/channels/public/bundles"
        );
        assert!(bundle_base_from_manifest_url("https://x.riotcdn.net/9A.manifest").is_err());
    }

    /// In-memory bundles; counts requests and can fail the first N attempts.
    #[derive(Clone)]
    struct FakeBundles {
        bundles: Arc<HashMap<String, Vec<u8>>>,
        requests: Arc<AtomicU64>,
        fail_first: Arc<AtomicU64>,
    }

    impl BundleSource for FakeBundles {
        async fn get_range(
            &self,
            url: &str,
            start: u64,
            end: u64,
            cancel: &AtomicBool,
            on_delta: &(dyn Fn(u64) + Sync),
        ) -> Result<Vec<u8>, RangeError> {
            check_cancel(cancel).map_err(|error| RangeError {
                retryable: false,
                error,
            })?;
            self.requests.fetch_add(1, Ordering::SeqCst);
            if self
                .fail_first
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err(RangeError {
                    retryable: true,
                    error: InstallError::Network("HTTP 403".into()),
                });
            }
            let body = self.bundles.get(url).expect("known bundle");
            let slice = body[start as usize..end as usize].to_vec();
            on_delta(slice.len() as u64);
            Ok(slice)
        }
    }

    /// Builds one bundle from raw chunk payloads; returns chunk refs in the given order.
    fn bundle(bundle_id: u64, payloads: &[&[u8]]) -> (Vec<u8>, Vec<ChunkRef>) {
        let mut body = Vec::new();
        let mut refs = Vec::new();
        for (index, payload) in payloads.iter().enumerate() {
            let compressed = zstd::bulk::compress(payload, 3).unwrap();
            refs.push(chunk(
                bundle_id * 100 + index as u64,
                bundle_id,
                body.len() as u64,
                compressed.len() as u32,
                payload.len() as u32,
            ));
            body.extend_from_slice(&compressed);
        }
        (body, refs)
    }

    fn job(root: &Path, files: Vec<PlannedFile>, replace: &[&str]) -> DownloadJob {
        DownloadJob {
            game_path: root.join("Game"),
            temp_dir: root.join(".chronorift").join("tmp"),
            bundle_base: "https://cdn.test/channels/public/bundles".into(),
            files,
            replace_existing: replace.iter().map(|path| path.to_string()).collect(),
            stage: Stage::Download,
        }
    }

    fn fake(bundles: Vec<(u64, Vec<u8>)>, fail_first: u64) -> FakeBundles {
        FakeBundles {
            bundles: Arc::new(
                bundles
                    .into_iter()
                    .map(|(id, body)| {
                        (
                            format!("https://cdn.test/channels/public/bundles/{id:016X}.bundle"),
                            body,
                        )
                    })
                    .collect(),
            ),
            requests: Arc::new(AtomicU64::new(0)),
            fail_first: Arc::new(AtomicU64::new(fail_first)),
        }
    }

    fn run(
        source: FakeBundles,
        job: DownloadJob,
        cancel: bool,
    ) -> Result<DownloadStats, InstallError> {
        tauri::async_runtime::block_on(download(
            source,
            job,
            Arc::new(AtomicBool::new(cancel)),
            Arc::new(NoProgress),
        ))
    }

    #[test]
    fn writes_files_from_coalesced_ranges_with_dedupe_and_retry() {
        let root = temp_root("engine");
        let (b1, r1) = bundle(1, &[b"hello ", b"world", b"!!"]);
        let (b2, r2) = bundle(2, &[&[7u8; 5000]]);
        let files = vec![
            planned("League of Legends.exe", vec![r1[0], r1[1], r1[0]]),
            planned("DATA/FINAL/a.wad.client", vec![r2[0], r1[2]]),
            planned("DATA/empty.bin", vec![]),
        ];
        let source = fake(vec![(1, b1), (2, b2)], 1);
        let stats = run(source.clone(), job(&root, files, &[]), false).unwrap();
        let game = root.join("Game");
        let exe = fs::read(game.join("League of Legends.exe")).unwrap();
        let wad = fs::read(game.join("DATA/FINAL/a.wad.client")).unwrap();
        let empty = fs::read(game.join("DATA/empty.bin")).unwrap();
        let temp_left = fs::read_dir(root.join(".chronorift/tmp")).unwrap().count();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(exe, b"hello worldhello ");
        assert_eq!(&wad[..5000], &[7u8; 5000][..]);
        assert_eq!(&wad[5000..], b"!!");
        assert!(empty.is_empty());
        assert_eq!(temp_left, 0);
        assert_eq!(stats.requests, 2, "one request per bundle");
        assert_eq!(source.requests.load(Ordering::SeqCst), 3, "one retried 403");
        assert_eq!(stats.files, 3);
    }

    #[test]
    fn refuses_to_overwrite_unless_allowed() {
        let root = temp_root("engine-overwrite");
        let (b1, r1) = bundle(1, &[b"new"]);
        let game = root.join("Game");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("a.bin"), b"old!").unwrap();
        let files = vec![planned("a.bin", vec![r1[0]])];

        let refused = run(
            fake(vec![(1, b1.clone())], 0),
            job(&root, files.clone(), &[]),
            false,
        );
        let untouched = fs::read(game.join("a.bin")).unwrap();
        let replaced = run(fake(vec![(1, b1)], 0), job(&root, files, &["a.bin"]), false);
        let after = fs::read(game.join("a.bin")).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert!(matches!(refused, Err(InstallError::WouldOverwrite(_))));
        assert_eq!(untouched, b"old!");
        assert!(replaced.is_ok());
        assert_eq!(after, b"new");
    }

    #[test]
    fn cancel_and_corrupt_data_clean_up_temp_files() {
        let root = temp_root("engine-fail");
        let (b1, r1) = bundle(1, &[b"abc"]);
        let files = vec![planned("a.bin", vec![r1[0]])];
        let cancelled = run(fake(vec![(1, b1)], 0), job(&root, files.clone(), &[]), true);

        let corrupt = fake(vec![(1, vec![0u8; r1[0].bundle_size as usize])], 0);
        let failed = run(corrupt, job(&root, files, &[]), false);
        let temp_left = fs::read_dir(root.join(".chronorift/tmp")).unwrap().count();
        let installed = root.join("Game/a.bin").exists();
        fs::remove_dir_all(&root).unwrap();

        assert!(matches!(cancelled, Err(InstallError::Cancelled)));
        assert_eq!(failed.unwrap_err().code(), "network");
        assert_eq!(temp_left, 0);
        assert!(!installed);
    }
}
