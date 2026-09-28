//! Durable install state, repair planning, and the explicit repair action (Milestone 2).
//!
//! Planning is read-only. Repair only writes missing files, replaces wrong-size files when the
//! user allowed it, and deletes files inside `<dest>/.chronorift/tmp`.

use crate::client::{
    self, BusyGuard, ClientInstallError, ClientInstallPlan, ClientInstallRequest,
    ClientInstallResult, DownloadJob, InstallBusy, InstallCancel, InstallFile, PlannedInstall,
    METADATA_DIR,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};

pub(crate) const STATE_FILE: &str = "install-state.json";
const STATE_VERSION: u32 = 1;
const PATH_LIST_CAP: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallStatus {
    InProgress,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateFile {
    pub path: String,
    pub size: u64,
}

/// `<dest>/.chronorift/install-state.json`, written at install/repair start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallState {
    pub version: u32,
    pub status: InstallStatus,
    pub patch: String,
    pub resolved_version: String,
    pub region: String,
    pub locale: String,
    pub manifest_url: String,
    pub destination_path: String,
    pub files: Vec<StateFile>,
    pub started_at_ms: u64,
    pub updated_at_ms: u64,
}

impl InstallState {
    pub(crate) fn new(plan: &ClientInstallPlan, files: &[InstallFile], now_ms: u64) -> Self {
        Self {
            version: STATE_VERSION,
            status: InstallStatus::InProgress,
            patch: plan.patch.clone(),
            resolved_version: plan.resolved_version.clone(),
            region: plan.region.clone(),
            locale: plan.locale.clone(),
            manifest_url: plan.manifest_url.clone(),
            destination_path: plan.destination_path.clone(),
            files: files
                .iter()
                .map(|file| StateFile {
                    path: file.path.clone(),
                    size: u64::from(file.filesize),
                })
                .collect(),
            started_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }

    pub(crate) fn mark_complete(&mut self, now_ms: u64) {
        self.status = InstallStatus::Complete;
        self.updated_at_ms = now_ms;
    }

    fn matches(&self, manifest_url: &str, locale: &str) -> bool {
        self.manifest_url == manifest_url && self.locale == locale
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn state_path(destination: &Path) -> PathBuf {
    destination.join(METADATA_DIR).join(STATE_FILE)
}

/// Writes the state next to its final path and renames it into place.
pub(crate) fn write_install_state(
    destination: &Path,
    state: &InstallState,
) -> Result<(), ClientInstallError> {
    let path = state_path(destination);
    fs::create_dir_all(destination.join(METADATA_DIR))?;
    let partial = path.with_extension("json.partial");
    let mut bytes = serde_json::to_vec_pretty(state).map_err(ClientInstallError::Metadata)?;
    bytes.push(b'\n');
    fs::write(&partial, bytes)?;
    fs::rename(&partial, &path)?;
    Ok(())
}

pub(crate) fn read_install_state(destination: &Path) -> Option<InstallState> {
    let bytes = fs::read(state_path(destination)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Files (not directories) directly inside `<dest>/.chronorift/tmp`. Missing dir → empty.
pub(crate) fn stale_temp_files(destination: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(client::temp_dir(destination)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

fn remove_stale_temp_files(destination: &Path) -> io::Result<usize> {
    let files = stale_temp_files(destination)?;
    for file in &files {
        fs::remove_file(file)?;
    }
    Ok(files.len())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileCondition {
    Ok,
    Missing,
    WrongSize,
}

/// Classifies each `(manifest path, expected size)` against disk. Read-only.
/// Anything present that is not a regular file of exactly that size is `WrongSize`.
pub(crate) fn check_files<'a>(
    destination: &Path,
    files: impl IntoIterator<Item = (&'a str, u64)>,
) -> Result<Vec<FileCondition>, ClientInstallError> {
    files
        .into_iter()
        .map(|(path, size)| {
            let target = client::resolve_install_path(destination, path)?;
            Ok(match fs::metadata(&target) {
                Ok(meta) if meta.is_file() && meta.len() == size => FileCondition::Ok,
                Ok(_) => FileCondition::WrongSize,
                Err(err) if err.kind() == io::ErrorKind::NotFound => FileCondition::Missing,
                Err(_) => FileCondition::WrongSize,
            })
        })
        .collect()
}

#[derive(Debug, Default)]
struct RepairSelection<'a> {
    ok: usize,
    missing: Vec<&'a InstallFile>,
    wrong_size: Vec<&'a InstallFile>,
}

impl<'a> RepairSelection<'a> {
    fn new(files: &'a [InstallFile], conditions: &[FileCondition]) -> Self {
        let mut selection = Self::default();
        for (file, condition) in files.iter().zip(conditions) {
            match condition {
                FileCondition::Ok => selection.ok += 1,
                FileCondition::Missing => selection.missing.push(file),
                FileCondition::WrongSize => selection.wrong_size.push(file),
            }
        }
        selection
    }

    /// Files to fetch: missing ones, plus wrong-size ones only when replacement is allowed.
    fn to_fetch(&self, replace_wrong_size: bool) -> Vec<&'a InstallFile> {
        let mut files = self.missing.clone();
        if replace_wrong_size {
            files.extend(self.wrong_size.iter().copied());
        }
        files
    }
}

fn totals(files: &[&InstallFile]) -> (u64, u64) {
    files.iter().fold((0, 0), |(download, install), file| {
        (
            download + file.download_bytes(),
            install + u64::from(file.filesize),
        )
    })
}

fn capped_paths(files: &[&InstallFile]) -> Vec<String> {
    files
        .iter()
        .take(PATH_LIST_CAP)
        .map(|file| file.path.clone())
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientRepairPlan {
    pub patch: String,
    pub resolved_version: String,
    pub region: String,
    pub locale: String,
    pub destination_path: String,
    pub manifest_url: String,
    pub total_files: usize,
    pub ok_files: usize,
    pub missing_files: usize,
    pub wrong_size_files: usize,
    /// First 50 missing paths.
    pub missing_paths: Vec<String>,
    /// First 50 wrong-size paths.
    pub wrong_size_paths: Vec<String>,
    /// Bytes to fetch for missing files.
    pub download_bytes: u64,
    /// Bytes written for missing files.
    pub install_bytes: u64,
    /// Extra bytes to fetch if wrong-size files are replaced.
    pub replace_download_bytes: u64,
    /// Extra bytes written if wrong-size files are replaced.
    pub replace_install_bytes: u64,
    pub stale_temp_files: usize,
    pub has_prior_state: bool,
    pub prior_state_matches: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientRepairRequest {
    pub install: ClientInstallRequest,
    #[serde(default)]
    pub replace_wrong_size: bool,
}

pub async fn plan_client_repair(
    app: AppHandle,
    request: ClientInstallRequest,
) -> Result<ClientRepairPlan, ClientInstallError> {
    let planned = client::resolve_install(&app, &request).await?;
    build_repair_plan(&planned)
}

fn build_repair_plan(planned: &PlannedInstall) -> Result<ClientRepairPlan, ClientInstallError> {
    let plan = &planned.plan;
    let destination = Path::new(&plan.destination_path);
    let conditions = check_files(destination, file_sizes(&planned.files))?;
    let selection = RepairSelection::new(&planned.files, &conditions);
    let (download_bytes, install_bytes) = totals(&selection.missing);
    let (replace_download_bytes, replace_install_bytes) = totals(&selection.wrong_size);
    let prior = read_install_state(destination);

    Ok(ClientRepairPlan {
        patch: plan.patch.clone(),
        resolved_version: plan.resolved_version.clone(),
        region: plan.region.clone(),
        locale: plan.locale.clone(),
        destination_path: plan.destination_path.clone(),
        manifest_url: plan.manifest_url.clone(),
        total_files: planned.files.len(),
        ok_files: selection.ok,
        missing_files: selection.missing.len(),
        wrong_size_files: selection.wrong_size.len(),
        missing_paths: capped_paths(&selection.missing),
        wrong_size_paths: capped_paths(&selection.wrong_size),
        download_bytes,
        install_bytes,
        replace_download_bytes,
        replace_install_bytes,
        stale_temp_files: stale_temp_files(destination)?.len(),
        has_prior_state: state_path(destination).is_file(),
        prior_state_matches: prior
            .is_some_and(|state| state.matches(&plan.manifest_url, &plan.locale)),
    })
}

fn file_sizes(files: &[InstallFile]) -> impl Iterator<Item = (&str, u64)> {
    files
        .iter()
        .map(|file| (file.path.as_str(), u64::from(file.filesize)))
}

pub async fn repair_client(
    app: AppHandle,
    request: ClientRepairRequest,
) -> Result<ClientInstallResult, ClientInstallError> {
    let busy = app.state::<InstallBusy>();
    let cancel = app.state::<InstallCancel>();
    let _guard = BusyGuard::start(&busy.0, &cancel.0)?;
    let destination = PathBuf::from(request.install.destination_path.trim());
    let outcome = run_repair(&app, &request, &cancel.0).await;
    client::report_cancel(&app, &destination, outcome)
}

async fn run_repair(
    app: &AppHandle,
    request: &ClientRepairRequest,
    cancel: &AtomicBool,
) -> Result<ClientInstallResult, ClientInstallError> {
    let planned = client::resolve_install(app, &request.install).await?;
    let destination = PathBuf::from(&planned.plan.destination_path);
    let bundle_base = client::bundle_base_from_manifest_url(&planned.plan.manifest_url)?;
    let conditions = check_files(&destination, file_sizes(&planned.files))?;
    let selection = RepairSelection::new(&planned.files, &conditions);
    let fetch = selection.to_fetch(request.replace_wrong_size);
    let replace: HashSet<&str> = if request.replace_wrong_size {
        selection
            .wrong_size
            .iter()
            .map(|file| file.path.as_str())
            .collect()
    } else {
        HashSet::new()
    };
    let (download_bytes, _) = totals(&fetch);

    fs::create_dir_all(&destination)?;
    let mut state = InstallState::new(&planned.plan, &planned.files, now_ms());
    write_install_state(&destination, &state)?;
    remove_stale_temp_files(&destination)?;

    client::download_files(
        app,
        &destination,
        &DownloadJob {
            bundle_base: &bundle_base,
            files: &fetch,
            total_download_bytes: download_bytes,
            stage: "repair",
            replace_existing: &replace,
            cancel,
        },
    )
    .await?;

    client::finish_install(app, &destination, &planned.plan, &mut state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdragon_rman::BundleChunk;
    use std::sync::atomic::Ordering;

    fn temp_dest(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "chronorift-repair-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file(path: &str, filesize: u32, chunk: u32) -> InstallFile {
        InstallFile {
            path: path.to_string(),
            filesize,
            chunks: vec![BundleChunk {
                bundle_id: 1,
                bundle_offset: 0,
                bundle_size: chunk,
                target_size: filesize,
            }],
        }
    }

    fn plan(destination: &Path, files: &[InstallFile]) -> PlannedInstall {
        PlannedInstall {
            plan: ClientInstallPlan {
                patch: "14.9".to_string(),
                resolved_version: "14.9.5831234".to_string(),
                region: "LA2".to_string(),
                locale: "es_MX".to_string(),
                destination_path: destination.display().to_string(),
                manifest_url: "https://example.invalid/releases/A.manifest".to_string(),
                file_count: files.len(),
                download_bytes: 0,
                install_bytes: 0,
            },
            files: files.to_vec(),
        }
    }

    #[test]
    fn classifies_ok_missing_and_wrong_size() {
        let dest = temp_dest("classify");
        fs::create_dir_all(dest.join("DATA")).unwrap();
        fs::write(dest.join("League of Legends.exe"), b"abcd").unwrap();
        fs::write(dest.join("DATA").join("a.wad"), b"ab").unwrap();
        fs::create_dir_all(dest.join("dir.wad")).unwrap();

        let conditions = check_files(
            &dest,
            [
                ("League of Legends.exe", 4),
                ("DATA/a.wad", 5),
                ("DATA/b.wad", 1),
                ("dir.wad", 0),
            ],
        )
        .unwrap();
        let unsafe_path = check_files(&dest, [("../escape", 1)]);
        fs::remove_dir_all(&dest).unwrap();

        assert_eq!(
            conditions,
            vec![
                FileCondition::Ok,
                FileCondition::WrongSize,
                FileCondition::Missing,
                FileCondition::WrongSize,
            ]
        );
        assert!(matches!(
            unsafe_path,
            Err(ClientInstallError::UnsafeManifestPath(_))
        ));
    }

    #[test]
    fn repair_plan_counts_bytes_and_prior_state() {
        let dest = temp_dest("plan");
        let files = vec![
            file("League of Legends.exe", 4, 3),
            file("DATA/a.wad", 5, 4),
            file("DATA/b.wad", 7, 6),
        ];
        let planned = plan(&dest, &files);
        fs::write(dest.join("League of Legends.exe"), b"abcd").unwrap();
        fs::create_dir_all(dest.join("DATA")).unwrap();
        fs::write(dest.join("DATA").join("a.wad"), b"ab").unwrap();
        fs::create_dir_all(client::temp_dir(&dest)).unwrap();
        fs::write(client::temp_dir(&dest).join("DATA_b.wad"), b"x").unwrap();

        let before = build_repair_plan(&planned).unwrap();
        let state = InstallState::new(&planned.plan, &files, 1);
        write_install_state(&dest, &state).unwrap();
        let after = build_repair_plan(&planned).unwrap();
        fs::remove_dir_all(&dest).unwrap();

        assert_eq!(before.total_files, 3);
        assert_eq!(before.ok_files, 1);
        assert_eq!(before.missing_files, 1);
        assert_eq!(before.missing_paths, vec!["DATA/b.wad"]);
        assert_eq!(before.wrong_size_files, 1);
        assert_eq!(before.wrong_size_paths, vec!["DATA/a.wad"]);
        assert_eq!((before.download_bytes, before.install_bytes), (6, 7));
        assert_eq!(
            (before.replace_download_bytes, before.replace_install_bytes),
            (4, 5)
        );
        assert_eq!(before.stale_temp_files, 1);
        assert!(!before.has_prior_state && !before.prior_state_matches);
        assert!(after.has_prior_state && after.prior_state_matches);

        let json = serde_json::to_value(&after).unwrap();
        assert!(json.get("wrongSizePaths").is_some());
        assert!(json.get("priorStateMatches").is_some());
    }

    #[test]
    fn wrong_size_files_are_fetched_only_when_allowed() {
        let files = vec![file("a", 1, 1), file("b", 2, 2), file("c", 3, 3)];
        let conditions = [
            FileCondition::Ok,
            FileCondition::Missing,
            FileCondition::WrongSize,
        ];
        let selection = RepairSelection::new(&files, &conditions);

        let keep: Vec<_> = selection
            .to_fetch(false)
            .into_iter()
            .map(|f| f.path.as_str())
            .collect();
        let replace: Vec<_> = selection
            .to_fetch(true)
            .into_iter()
            .map(|f| f.path.as_str())
            .collect();
        assert_eq!(keep, vec!["b"]);
        assert_eq!(replace, vec!["b", "c"]);
    }

    #[test]
    fn detects_and_removes_only_temp_files() {
        let dest = temp_dest("stale");
        assert!(stale_temp_files(&dest).unwrap().is_empty());

        let tmp = client::temp_dir(&dest);
        fs::create_dir_all(tmp.join("nested")).unwrap();
        fs::write(tmp.join("a.partial"), b"1").unwrap();
        fs::write(tmp.join("b.partial"), b"2").unwrap();
        fs::write(dest.join("keep.wad"), b"3").unwrap();

        let found = stale_temp_files(&dest).unwrap();
        let removed = remove_stale_temp_files(&dest).unwrap();
        let nested_kept = tmp.join("nested").is_dir();
        let game_file_kept = dest.join("keep.wad").is_file();
        let left = stale_temp_files(&dest).unwrap();
        fs::remove_dir_all(&dest).unwrap();

        assert_eq!(found.len(), 2);
        assert_eq!(removed, 2);
        assert!(nested_kept && game_file_kept);
        assert!(left.is_empty());
    }

    #[test]
    fn install_state_roundtrips_as_camel_case_json() {
        let dest = temp_dest("state");
        let files = vec![file("League of Legends.exe", 4, 3)];
        let planned = plan(&dest, &files);
        let mut state = InstallState::new(&planned.plan, &files, 10);
        write_install_state(&dest, &state).unwrap();
        let first = read_install_state(&dest).unwrap();
        state.mark_complete(20);
        write_install_state(&dest, &state).unwrap();
        let raw = fs::read_to_string(state_path(&dest)).unwrap();
        let second = read_install_state(&dest).unwrap();
        fs::remove_dir_all(&dest).unwrap();

        assert_eq!(first.status, InstallStatus::InProgress);
        assert_eq!(first.files[0].size, 4);
        assert_eq!(second, state);
        assert_eq!(second.started_at_ms, 10);
        assert_eq!(second.updated_at_ms, 20);
        assert!(raw.contains("\"status\": \"complete\""));
        assert!(raw.contains("\"manifestUrl\""));
        assert!(raw.contains("\"startedAtMs\""));
        assert!(raw.contains("\"version\": 1"));
    }

    #[test]
    fn busy_guard_is_exclusive_and_resets_cancel() {
        let busy = AtomicBool::new(false);
        let cancel = AtomicBool::new(true);

        let guard = BusyGuard::start(&busy, &cancel).unwrap();
        assert!(!cancel.load(Ordering::SeqCst));
        assert!(matches!(
            BusyGuard::start(&busy, &cancel),
            Err(ClientInstallError::Busy)
        ));
        assert!(client::check_cancel(&cancel).is_ok());
        cancel.store(true, Ordering::SeqCst);
        let err = client::check_cancel(&cancel).unwrap_err();
        assert_eq!(err.to_string(), "Install cancelled");
        drop(guard);
        assert!(BusyGuard::start(&busy, &cancel).is_ok());
        assert_eq!(
            ClientInstallError::Busy.to_string(),
            "Another install is running"
        );
    }

    #[test]
    fn would_overwrite_points_to_repair() {
        let message =
            ClientInstallError::WouldOverwrite(PathBuf::from("D:/Clients/a.wad")).to_string();
        assert!(message.contains("a.wad"));
        assert!(message.contains("Review repair"));
    }
}
