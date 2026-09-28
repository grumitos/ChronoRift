//! Client install pipeline: resolve → plan (pinned by id) → install / repair.
//!
//! Module map (no cycles; `mod.rs` is the only module that knows about all of them):
//! - `resolve`: archive + Sieve lookup, region and version rules
//! - `http`: shared HTTP client and the `Fetch` trait
//! - `manifest`: panic-free RMAN reading and flag-based file selection
//! - `download`: range planning and the concurrent download/write engine
//! - `store`: on-disk layout and versioned `.chronorift` files
//! - `repair`: disk classification for repair plans
//! - `progress`: progress trait and the throttled Tauri emitter
//! - `clients`: discovery of installed clients

pub mod clients;
mod download;
mod error;
mod http;
mod manifest;
mod progress;
mod repair;
mod resolve;
mod store;

pub use error::CommandError;

use crate::locale::normalize_locale;
use download::{DownloadJob, DownloadStats};
use error::InstallError;
use http::{Fetch, HttpClient};
use manifest::{PlannedFile, Selection};
use progress::{Progress, Stage, TauriProgress, Update};
use repair::RepairSelection;
use resolve::{ResolveRequest, Source};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use store::{InstallRecord, InstallState, InstallStatus, Release, StateFile};
use tauri::AppHandle;

const PLAN_TTL_MS: u64 = 30 * 60 * 1000;
const MAX_PLANS: usize = 3;

// ---------------------------------------------------------------------------------------------
// IPC types
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInstallRequest {
    pub patch: String,
    pub game_version: Option<String>,
    pub region: Option<String>,
    pub locale: String,
    pub destination_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInstallPlan {
    pub plan_id: String,
    pub expires_at_ms: u64,
    pub patch: String,
    pub requested_version: Option<String>,
    pub resolved_version: String,
    pub exact_build: bool,
    pub source: Source,
    pub region: String,
    pub manifest_region: String,
    pub locale: String,
    pub locale_assets: String,
    pub available_locales: Vec<String>,
    pub destination_path: String,
    pub game_path: String,
    pub executable_path: String,
    pub manifest_url: String,
    pub file_count: usize,
    pub download_bytes: u64,
    pub install_bytes: u64,
    pub request_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallClientRequest {
    pub plan_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInstallResult {
    pub patch: String,
    pub requested_version: Option<String>,
    pub resolved_version: String,
    pub exact_build: bool,
    pub source: Source,
    pub region: String,
    pub manifest_region: String,
    pub locale: String,
    pub destination_path: String,
    pub game_path: String,
    pub executable_path: String,
    pub manifest_url: String,
    pub file_count: usize,
    /// Bytes downloaded by this run.
    pub download_bytes: u64,
    /// Size of the whole installed client.
    pub install_bytes: u64,
    pub request_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientRepairPlanRequest {
    pub destination_path: String,
    pub locale: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientRepairPlan {
    pub plan_id: String,
    pub expires_at_ms: u64,
    pub patch: String,
    /// Replay build the install was planned for (archive form); `None` when the saved
    /// state does not know it (v1 installs).
    pub requested_version: Option<String>,
    pub resolved_version: String,
    pub exact_build: bool,
    pub source: Source,
    pub region: String,
    pub locale: String,
    pub locale_assets: String,
    pub destination_path: String,
    pub game_path: String,
    pub manifest_url: String,
    pub state_status: InstallStatus,
    pub total_files: usize,
    pub ok_files: usize,
    pub missing_files: usize,
    pub wrong_size_files: usize,
    pub missing_paths: Vec<String>,
    pub wrong_size_paths: Vec<String>,
    pub download_bytes: u64,
    pub install_bytes: u64,
    pub replace_download_bytes: u64,
    pub replace_install_bytes: u64,
    pub stale_temp_files: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientRepairRequest {
    pub plan_id: String,
    #[serde(default)]
    pub replace_wrong_size: bool,
}

// ---------------------------------------------------------------------------------------------
// Managed state
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanKind {
    Install,
    Repair,
}

/// A plan the user saw; install/repair runs exactly this.
struct PinnedPlan {
    id: String,
    kind: PlanKind,
    expires_at_ms: u64,
    root: PathBuf,
    game_dir: String,
    release: Release,
    files: Arc<Vec<PlannedFile>>,
}

#[derive(Default)]
struct PlanCache(Vec<Arc<PinnedPlan>>);

impl PlanCache {
    fn insert(&mut self, plan: PinnedPlan, now: u64) -> Arc<PinnedPlan> {
        self.0.retain(|pinned| pinned.expires_at_ms > now);
        while self.0.len() >= MAX_PLANS {
            self.0.remove(0);
        }
        let plan = Arc::new(plan);
        self.0.push(plan.clone());
        plan
    }

    fn get(&self, id: &str, kind: PlanKind, now: u64) -> Result<Arc<PinnedPlan>, InstallError> {
        self.0
            .iter()
            .find(|plan| plan.id == id && plan.kind == kind && plan.expires_at_ms > now)
            .cloned()
            .ok_or(InstallError::UnknownPlan)
    }

    fn remove(&mut self, id: &str) {
        self.0.retain(|plan| plan.id != id);
    }
}

fn new_plan_id(now: u64) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("plan-{now:x}-{:x}-{count:x}", std::process::id())
}

/// Tauri managed state for client installs.
pub struct InstallRuntime {
    http: HttpClient,
    busy: AtomicBool,
    cancel: Arc<AtomicBool>,
    plans: Mutex<PlanCache>,
}

impl Default for InstallRuntime {
    fn default() -> Self {
        Self {
            http: HttpClient::new(),
            busy: AtomicBool::new(false),
            cancel: Arc::new(AtomicBool::new(false)),
            plans: Mutex::new(PlanCache::default()),
        }
    }
}

/// Holds the busy flag for one plan/install/repair and releases it on drop.
struct BusyGuard<'a>(&'a AtomicBool);

impl<'a> BusyGuard<'a> {
    fn start(busy: &'a AtomicBool, cancel: &AtomicBool) -> Result<Self, InstallError> {
        busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| InstallError::Busy)?;
        cancel.store(false, Ordering::SeqCst);
        Ok(Self(busy))
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl InstallRuntime {
    fn guard(&self) -> Result<BusyGuard<'_>, InstallError> {
        BusyGuard::start(&self.busy, &self.cancel)
    }

    fn pin(&self, plan: PinnedPlan) -> Result<Arc<PinnedPlan>, InstallError> {
        let mut plans = self.plans.lock().map_err(|_| InstallError::UnknownPlan)?;
        Ok(plans.insert(plan, store::now_ms()))
    }

    fn pinned(&self, id: &str, kind: PlanKind) -> Result<Arc<PinnedPlan>, InstallError> {
        let plans = self.plans.lock().map_err(|_| InstallError::UnknownPlan)?;
        plans.get(id, kind, store::now_ms())
    }

    fn forget(&self, id: &str) {
        if let Ok(mut plans) = self.plans.lock() {
            plans.remove(id);
        }
    }

    /// Requests cancel of the running operation. Returns whether one was running.
    pub fn cancel(&self) -> bool {
        self.cancel.store(true, Ordering::SeqCst);
        self.busy.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

fn destination(path: &str) -> Result<PathBuf, InstallError> {
    let path = PathBuf::from(path.trim());
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err(InstallError::InvalidDestination);
    }
    Ok(path)
}

fn resolve_request(request: &ClientInstallRequest) -> Result<ResolveRequest, InstallError> {
    let region = resolve::normalize_region(request.region.as_deref())?;
    let version = match request.game_version.as_deref().map(str::trim) {
        Some(version) if !version.is_empty() => Some(
            resolve::archive_name(version)
                .ok_or_else(|| InstallError::InvalidPatch(version.to_string()))?,
        ),
        _ => None,
    };
    let patch = resolve::normalize_patch(&request.patch);
    let version_patch = version.as_deref().and_then(resolve::normalize_patch);
    let patch = match (patch, version_patch) {
        (Some(patch), Some(from_version)) if patch != from_version => {
            return Err(InstallError::InvalidPatch(format!(
                "{} does not match {}",
                request.patch,
                version.unwrap_or_default()
            )))
        }
        (Some(patch), _) | (None, Some(patch)) => patch,
        (None, None) => return Err(InstallError::InvalidPatch(request.patch.clone())),
    };
    Ok(ResolveRequest {
        region,
        patch,
        version,
    })
}

/// Emits the final `cancelled`/`failed` event for an error outcome.
fn report_outcome<T>(
    progress: &dyn Progress,
    outcome: Result<T, InstallError>,
) -> Result<T, InstallError> {
    if let Err(err) = &outcome {
        let stage = if matches!(err, InstallError::Cancelled) {
            Stage::Cancelled
        } else {
            Stage::Failed
        };
        progress.report(Update::stage(stage, err.to_string()).finished());
    }
    outcome
}

async fn select_files(manifest: Vec<u8>, locale: String) -> Result<Selection, InstallError> {
    tauri::async_runtime::spawn_blocking(move || manifest::parse(&manifest)?.select(&locale))
        .await
        .map_err(|err| InstallError::Manifest(err.to_string()))?
}

fn totals(files: &[PlannedFile]) -> Result<(u64, u64, usize), InstallError> {
    let plan = download::plan_ranges(files, download::COALESCE_GAP, download::MAX_SPAN)?;
    let install_bytes = files.iter().map(|file| file.size).sum();
    Ok((plan.download_bytes, install_bytes, plan.ranges.len()))
}

/// Refuses a destination that already has game files, ChronoRift state, or temp files.
fn preflight_install(
    root: &Path,
    game_path: &Path,
    files: &[PlannedFile],
) -> Result<(), InstallError> {
    let state = store::metadata_dir(root).join(store::STATE_FILE);
    if state.exists() {
        return Err(InstallError::WouldOverwrite(state));
    }
    if let Some(temp) = store::stale_temp_files(root)?.into_iter().next() {
        return Err(InstallError::WouldOverwrite(temp));
    }
    for file in files {
        let target = store::safe_join(game_path, &file.path)?;
        if std::fs::symlink_metadata(&target).is_ok() {
            return Err(InstallError::WouldOverwrite(target));
        }
    }
    Ok(())
}

pub async fn plan_client_install(
    runtime: &InstallRuntime,
    app: &AppHandle,
    request: ClientInstallRequest,
) -> Result<ClientInstallPlan, InstallError> {
    let root = destination(&request.destination_path)?;
    let _guard = runtime.guard()?;
    let progress = TauriProgress::new(app.clone(), &root);
    let outcome = async {
        let locale = normalize_locale(&request.locale)
            .ok_or_else(|| InstallError::InvalidLocale(request.locale.clone()))?;
        let resolve_request = resolve_request(&request)?;
        let resolved =
            resolve::resolve(&runtime.http, &resolve_request, &runtime.cancel, &progress).await?;
        progress.report(Update::stage(Stage::Plan, "Planning locale files"));
        let selection = select_files(resolved.manifest, locale.clone()).await?;
        let game_path = store::game_path(&root, store::GAME_DIR)?;
        preflight_install(&root, &game_path, &selection.files)?;
        let (download_bytes, install_bytes, request_count) = totals(&selection.files)?;

        let now = store::now_ms();
        let release = Release {
            patch: resolve_request.patch,
            resolved_version: resolved.version,
            requested_version: resolve_request.version,
            exact_build: resolved.exact_build,
            source: resolved.source,
            region: resolve_request.region,
            manifest_region: resolved.manifest_region,
            locale,
            manifest_url: resolved.manifest_url,
        };
        let pinned = runtime.pin(PinnedPlan {
            id: new_plan_id(now),
            kind: PlanKind::Install,
            expires_at_ms: now + PLAN_TTL_MS,
            root: root.clone(),
            game_dir: store::GAME_DIR.into(),
            release: release.clone(),
            files: Arc::new(selection.files),
        })?;
        progress.report(Update::stage(Stage::Plan, "Plan ready").finished());
        Ok(ClientInstallPlan {
            plan_id: pinned.id.clone(),
            expires_at_ms: pinned.expires_at_ms,
            patch: release.patch,
            requested_version: release.requested_version,
            resolved_version: release.resolved_version,
            exact_build: release.exact_build,
            source: release.source,
            region: release.region,
            manifest_region: release.manifest_region,
            locale: release.locale,
            locale_assets: selection.locale_assets,
            available_locales: selection.available_locales,
            destination_path: root.display().to_string(),
            game_path: game_path.display().to_string(),
            executable_path: game_path.join(store::GAME_EXE).display().to_string(),
            manifest_url: release.manifest_url,
            file_count: pinned.files.len(),
            download_bytes,
            install_bytes,
            request_count,
        })
    }
    .await;
    report_outcome(&progress, outcome)
}

pub async fn install_client(
    runtime: &InstallRuntime,
    app: &AppHandle,
    request: InstallClientRequest,
) -> Result<ClientInstallResult, InstallError> {
    let _guard = runtime.guard()?;
    let plan = runtime.pinned(&request.plan_id, PlanKind::Install)?;
    let progress = Arc::new(TauriProgress::new(app.clone(), &plan.root));
    let outcome = async {
        let game_path = store::game_path(&plan.root, &plan.game_dir)?;
        preflight_install(&plan.root, &game_path, &plan.files)?;
        let stats = run_download(
            runtime,
            &plan,
            plan.files.to_vec(),
            HashSet::new(),
            Stage::Download,
            progress.clone(),
        )
        .await?;
        finish(&plan, stats, progress.as_ref())
    }
    .await;
    if outcome.is_ok() {
        runtime.forget(&plan.id);
    }
    report_outcome(progress.as_ref(), outcome)
}

pub async fn plan_client_repair(
    runtime: &InstallRuntime,
    app: &AppHandle,
    request: ClientRepairPlanRequest,
) -> Result<ClientRepairPlan, InstallError> {
    let root = destination(&request.destination_path)?;
    let _guard = runtime.guard()?;
    let progress = TauriProgress::new(app.clone(), &root);
    let outcome = async {
        let locale = normalize_locale(&request.locale)
            .ok_or_else(|| InstallError::InvalidLocale(request.locale.clone()))?;
        let state = store::read_state(&root)?
            .ok_or_else(|| InstallError::NotChronoRiftInstall(root.clone()))?;
        if state.release.locale != locale {
            return Err(InstallError::LocaleMismatch {
                installed: state.release.locale,
                requested: locale,
            });
        }
        // Repair uses the manifest this install was made from; it never re-resolves.
        let url = &state.release.manifest_url;
        if resolve::valid_manifest_url(url).is_none() {
            return Err(InstallError::Manifest(format!(
                "{url} is not a Riot manifest URL"
            )));
        }
        progress.report(Update::stage(Stage::Manifest, "Downloading manifest"));
        let fetched = runtime
            .http
            .get(
                url,
                resolve::MANIFEST_LIMIT,
                &runtime.cancel,
                &|done, total| {
                    progress.report(
                        Update::stage(Stage::Manifest, "Downloading manifest").bytes(done, total),
                    );
                },
            )
            .await?;
        if !fetched.is_success() {
            return Err(InstallError::Network(format!(
                "{url} returned HTTP {}",
                fetched.status
            )));
        }
        progress.report(Update::stage(Stage::Plan, "Checking installed files"));
        let selection = select_files(fetched.body, locale).await?;
        let game_path = store::game_path(&root, &state.game_dir)?;
        let conditions = repair::check_files(&game_path, &selection.files)?;
        let checked = RepairSelection::new(&selection.files, &conditions);
        let owned =
            |files: &[&PlannedFile]| files.iter().map(|file| (*file).clone()).collect::<Vec<_>>();
        let (download_bytes, install_bytes, _) = totals(&owned(&checked.missing))?;
        let (replace_download_bytes, replace_install_bytes, _) =
            totals(&owned(&checked.wrong_size))?;
        let summary = (
            checked.ok,
            checked.missing.len(),
            checked.wrong_size.len(),
            repair::capped_paths(&checked.missing),
            repair::capped_paths(&checked.wrong_size),
        );
        let stale_temp_files = store::stale_temp_files(&root)?.len();

        let now = store::now_ms();
        let total_files = selection.files.len();
        let pinned = runtime.pin(PinnedPlan {
            id: new_plan_id(now),
            kind: PlanKind::Repair,
            expires_at_ms: now + PLAN_TTL_MS,
            root: root.clone(),
            game_dir: state.game_dir.clone(),
            release: state.release.clone(),
            files: Arc::new(selection.files),
        })?;
        progress.report(Update::stage(Stage::Plan, "Repair plan ready").finished());
        let release = state.release;
        Ok(ClientRepairPlan {
            plan_id: pinned.id.clone(),
            expires_at_ms: pinned.expires_at_ms,
            patch: release.patch,
            requested_version: release.requested_version,
            resolved_version: release.resolved_version,
            exact_build: release.exact_build,
            source: release.source,
            region: release.region,
            locale: release.locale,
            locale_assets: selection.locale_assets,
            destination_path: root.display().to_string(),
            game_path: game_path.display().to_string(),
            manifest_url: release.manifest_url,
            state_status: state.status,
            total_files,
            ok_files: summary.0,
            missing_files: summary.1,
            wrong_size_files: summary.2,
            missing_paths: summary.3,
            wrong_size_paths: summary.4,
            download_bytes,
            install_bytes,
            replace_download_bytes,
            replace_install_bytes,
            stale_temp_files,
        })
    }
    .await;
    report_outcome(&progress, outcome)
}

pub async fn repair_client(
    runtime: &InstallRuntime,
    app: &AppHandle,
    request: ClientRepairRequest,
) -> Result<ClientInstallResult, InstallError> {
    let _guard = runtime.guard()?;
    let plan = runtime.pinned(&request.plan_id, PlanKind::Repair)?;
    let progress = Arc::new(TauriProgress::new(app.clone(), &plan.root));
    let outcome = async {
        match store::read_state(&plan.root)? {
            Some(state) if state.release.manifest_url == plan.release.manifest_url => {}
            _ => return Err(InstallError::NotChronoRiftInstall(plan.root.clone())),
        }
        let game_path = store::game_path(&plan.root, &plan.game_dir)?;
        // Disk may have changed since the plan; classify again.
        let conditions = repair::check_files(&game_path, &plan.files)?;
        let checked = RepairSelection::new(&plan.files, &conditions);
        let fetch = checked.to_fetch(request.replace_wrong_size);
        let replace = if request.replace_wrong_size {
            checked
                .wrong_size
                .iter()
                .map(|file| file.path.clone())
                .collect()
        } else {
            HashSet::new()
        };
        store::remove_stale_temp_files(&plan.root)?;
        let stats = run_download(
            runtime,
            &plan,
            fetch,
            replace,
            Stage::Repair,
            progress.clone(),
        )
        .await?;
        finish(&plan, stats, progress.as_ref())
    }
    .await;
    if outcome.is_ok() {
        runtime.forget(&plan.id);
    }
    report_outcome(progress.as_ref(), outcome)
}

fn state_for(plan: &PinnedPlan, status: InstallStatus, started_at_ms: u64) -> InstallState {
    InstallState {
        version: store::STORE_VERSION,
        status,
        release: plan.release.clone(),
        game_dir: plan.game_dir.clone(),
        files: plan
            .files
            .iter()
            .map(|file| StateFile {
                path: file.path.clone(),
                size: file.size,
            })
            .collect(),
        started_at_ms,
        updated_at_ms: store::now_ms(),
    }
}

/// Writes the in-progress state, then downloads `files`.
async fn run_download(
    runtime: &InstallRuntime,
    plan: &PinnedPlan,
    files: Vec<PlannedFile>,
    replace_existing: HashSet<String>,
    stage: Stage,
    progress: Arc<TauriProgress>,
) -> Result<DownloadStats, InstallError> {
    let started = store::now_ms();
    store::write_state(
        &plan.root,
        &state_for(plan, InstallStatus::InProgress, started),
    )?;
    let job = DownloadJob {
        game_path: store::game_path(&plan.root, &plan.game_dir)?,
        temp_dir: store::temp_dir(&plan.root),
        bundle_base: download::bundle_base_from_manifest_url(&plan.release.manifest_url)?,
        files,
        replace_existing,
        stage,
    };
    download::download(runtime.http.clone(), job, runtime.cancel.clone(), progress).await
}

/// Verifies the executable, writes install.json, marks the state complete.
fn finish(
    plan: &PinnedPlan,
    stats: DownloadStats,
    progress: &dyn Progress,
) -> Result<ClientInstallResult, InstallError> {
    let game_path = store::game_path(&plan.root, &plan.game_dir)?;
    let executable = game_path.join(store::GAME_EXE);
    if !executable.is_file() {
        return Err(InstallError::MissingExecutable(executable));
    }
    let install_bytes = plan.files.iter().map(|file| file.size).sum();
    let now = store::now_ms();
    store::write_record(
        &plan.root,
        &InstallRecord {
            version: store::STORE_VERSION,
            release: plan.release.clone(),
            game_dir: plan.game_dir.clone(),
            file_count: plan.files.len(),
            download_bytes: stats.downloaded_bytes,
            install_bytes,
            installed_at_ms: now,
        },
    )?;
    let started = store::read_state(&plan.root)?
        .map(|state| state.started_at_ms)
        .unwrap_or(now);
    store::write_state(
        &plan.root,
        &state_for(plan, InstallStatus::Complete, started),
    )?;
    progress.report(
        Update::stage(Stage::Complete, "Client ready")
            .bytes(stats.downloaded_bytes, Some(stats.downloaded_bytes))
            .files(stats.files as u64, stats.files as u64)
            .finished(),
    );
    let release = plan.release.clone();
    Ok(ClientInstallResult {
        patch: release.patch,
        requested_version: release.requested_version,
        resolved_version: release.resolved_version,
        exact_build: release.exact_build,
        source: release.source,
        region: release.region,
        manifest_region: release.manifest_region,
        locale: release.locale,
        destination_path: plan.root.display().to_string(),
        game_path: game_path.display().to_string(),
        executable_path: executable.display().to_string(),
        manifest_url: release.manifest_url,
        file_count: plan.files.len(),
        download_bytes: stats.downloaded_bytes,
        install_bytes,
        request_count: stats.requests,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(region: Option<&str>, patch: &str, version: Option<&str>) -> ClientInstallRequest {
        ClientInstallRequest {
            patch: patch.into(),
            game_version: version.map(Into::into),
            region: region.map(Into::into),
            locale: "es_MX".into(),
            destination_path: "/clients/16.17".into(),
        }
    }

    #[test]
    fn builds_resolve_requests_strictly() {
        let ok = resolve_request(&request(Some("la2"), "16.17", Some("16.17.810.4348"))).unwrap();
        assert_eq!(
            ok,
            ResolveRequest {
                region: "LA2".into(),
                patch: "16.17".into(),
                version: Some("16.17.8104348".into()),
            }
        );
        let no_version = resolve_request(&request(Some("NA1"), "16.17.1", None)).unwrap();
        assert_eq!(no_version.patch, "16.17");
        assert_eq!(no_version.version, None);

        let err = resolve_request(&request(None, "16.17", None)).unwrap_err();
        assert_eq!(err.code(), "invalidRegion");
        assert!(err.to_string().contains("Choose a server"));
        let err = resolve_request(&request(Some("NA1"), "16.18", Some("16.17.810.4348")));
        assert_eq!(err.unwrap_err().code(), "invalidPatch");
        let err = resolve_request(&request(Some("NA1"), "latest", None));
        assert_eq!(err.unwrap_err().code(), "invalidPatch");
    }

    #[test]
    fn destination_must_be_absolute() {
        assert!(destination("").is_err());
        assert!(destination("relative/dir").is_err());
        let absolute = std::env::temp_dir();
        assert_eq!(
            destination(&absolute.display().to_string()).unwrap(),
            absolute
        );
    }

    fn pinned(id: &str, kind: PlanKind, expires_at_ms: u64) -> PinnedPlan {
        PinnedPlan {
            id: id.into(),
            kind,
            expires_at_ms,
            root: PathBuf::from("/c"),
            game_dir: store::GAME_DIR.into(),
            release: store::tests::release(),
            files: Arc::new(Vec::new()),
        }
    }

    #[test]
    fn plan_cache_pins_by_id_kind_and_expiry() {
        let mut cache = PlanCache::default();
        cache.insert(pinned("a", PlanKind::Install, 100), 0);
        cache.insert(pinned("b", PlanKind::Repair, 100), 0);
        assert!(cache.get("a", PlanKind::Install, 50).is_ok());
        assert!(matches!(
            cache.get("a", PlanKind::Repair, 50),
            Err(InstallError::UnknownPlan)
        ));
        assert!(cache.get("a", PlanKind::Install, 100).is_err(), "expired");
        assert!(cache.get("zzz", PlanKind::Install, 0).is_err());
        for id in ["c", "d", "e"] {
            cache.insert(pinned(id, PlanKind::Install, 1000), 0);
        }
        assert_eq!(cache.0.len(), MAX_PLANS);
        assert!(
            cache.get("a", PlanKind::Install, 0).is_err(),
            "oldest evicted"
        );
        cache.remove("e");
        assert!(cache.get("e", PlanKind::Install, 0).is_err());
        assert_ne!(new_plan_id(1), new_plan_id(1));
    }

    #[test]
    fn busy_guard_is_exclusive_and_resets_cancel() {
        let runtime = InstallRuntime::default();
        runtime.cancel.store(true, Ordering::SeqCst);
        let guard = runtime.guard().unwrap();
        assert!(!runtime.cancel.load(Ordering::SeqCst));
        assert!(matches!(runtime.guard(), Err(InstallError::Busy)));
        assert!(runtime.cancel(), "reports a running operation");
        drop(guard);
        assert!(!runtime.cancel());
        assert!(runtime.guard().is_ok());
    }

    #[test]
    fn preflight_refuses_existing_state_temp_or_targets() {
        let root = store::tests::temp_root("preflight");
        let game = root.join(store::GAME_DIR);
        let files = vec![PlannedFile {
            path: "DATA/a.wad".into(),
            size: 1,
            chunks: Vec::new(),
        }];
        assert!(preflight_install(&root, &game, &files).is_ok());

        std::fs::create_dir_all(game.join("DATA")).unwrap();
        std::fs::write(game.join("DATA/a.wad"), b"x").unwrap();
        let target = preflight_install(&root, &game, &files).unwrap_err();
        std::fs::create_dir_all(store::temp_dir(&root)).unwrap();
        std::fs::write(store::temp_dir(&root).join("x"), b"x").unwrap();
        let temp = preflight_install(&root, &game, &files).unwrap_err();
        store::write_state(
            &root,
            &state_for(
                &pinned("a", PlanKind::Install, 0),
                InstallStatus::InProgress,
                0,
            ),
        )
        .unwrap();
        let state = preflight_install(&root, &game, &files).unwrap_err();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(matches!(target, InstallError::WouldOverwrite(path) if path.ends_with("a.wad")));
        assert!(matches!(temp, InstallError::WouldOverwrite(path) if path.ends_with("x")));
        assert!(
            matches!(state, InstallError::WouldOverwrite(path) if path.ends_with(store::STATE_FILE))
        );
    }

    /// `CHRONORIFT_MANIFEST_SAMPLE=/path/16.17.manifest cargo test -- --ignored --nocapture`
    #[test]
    #[ignore = "needs CHRONORIFT_MANIFEST_SAMPLE pointing at a real game-client manifest"]
    fn measures_request_reduction_on_real_manifest() {
        let path = std::env::var("CHRONORIFT_MANIFEST_SAMPLE")
            .expect("set CHRONORIFT_MANIFEST_SAMPLE to a real lol-game-client manifest");
        let manifest = manifest::parse(&std::fs::read(path).unwrap()).unwrap();
        let files = manifest.select("es_MX").unwrap().files;
        let refs = files.iter().map(|file| file.chunks.len()).sum::<usize>();
        let ref_bytes = files
            .iter()
            .flat_map(|file| &file.chunks)
            .map(|chunk| u64::from(chunk.bundle_size))
            .sum::<u64>();
        let plan =
            download::plan_ranges(&files, download::COALESCE_GAP, download::MAX_SPAN).unwrap();
        println!(
            "es_MX: {} files, {refs} chunk refs ({} MiB), {} unique chunks, {} requests ({} MiB on the wire)",
            files.len(),
            ref_bytes >> 20,
            plan.unique_chunks,
            plan.ranges.len(),
            plan.download_bytes >> 20,
        );
        assert_eq!(plan.chunk_refs, refs);
        assert!(
            plan.ranges.len() * 100 < refs,
            "coalescing cuts requests >100x"
        );
        assert!(
            plan.download_bytes < ref_bytes,
            "dedupe beats per-ref downloads"
        );
    }

    /// Hits GitHub and Riot's CDN: resolves the 16.17 sample replay build for LA2 through the
    /// archive and installs one small file with the real engine.
    #[test]
    #[ignore = "network: raw.githubusercontent.com and lol.secure.dyn.riotcdn.net"]
    fn network_installs_a_small_file_from_an_archived_build() {
        use progress::NoProgress;
        tauri::async_runtime::block_on(async {
            let http = HttpClient::new();
            let cancel = Arc::new(AtomicBool::new(false));
            let request =
                resolve_request(&request(Some("LA2"), "16.17", Some("16.17.810.4348"))).unwrap();
            let resolved = resolve::resolve(&http, &request, &cancel, &NoProgress)
                .await
                .unwrap();
            assert_eq!(resolved.source, Source::Archive);
            assert!(resolved.exact_build);
            assert_eq!(resolved.version, "16.17.8104348");
            let selection = manifest::parse(&resolved.manifest)
                .unwrap()
                .select("es_MX")
                .unwrap();
            let small = selection
                .files
                .iter()
                .find(|file| file.path == "LeagueCrashHandler64.exe")
                .expect("16.17 ships LeagueCrashHandler64.exe")
                .clone();
            let expected = small.size;
            let root = store::tests::temp_root("network");
            let job = DownloadJob {
                game_path: root.join(store::GAME_DIR),
                temp_dir: store::temp_dir(&root),
                bundle_base: download::bundle_base_from_manifest_url(&resolved.manifest_url)
                    .unwrap(),
                files: vec![small.clone()],
                replace_existing: HashSet::new(),
                stage: Stage::Download,
            };
            let stats = download::download(http, job, cancel, Arc::new(NoProgress))
                .await
                .unwrap();
            let bytes = std::fs::read(root.join(store::GAME_DIR).join(&small.path)).unwrap();
            let written = bytes.len() as u64;
            std::fs::remove_dir_all(&root).unwrap();
            println!(
                "{} via {} ({}): {} = {written} bytes in {} request(s), {} bytes downloaded",
                resolved.version,
                resolved.manifest_region,
                resolved.manifest_url,
                small.path,
                stats.requests,
                stats.downloaded_bytes
            );
            assert_eq!(written, expected);
            assert_eq!(&bytes[..2], b"MZ", "a Windows executable");
        });
    }
}
