//! On-disk layout and the versioned `.chronorift` files. These types are storage only; IPC
//! types live in `install::mod` and are built from them.
//!
//! Layout (v2): `<root>/Game/League of Legends.exe`, `<root>/.chronorift/install.json`,
//! `<root>/.chronorift/install-state.json`, temp files in `<root>/.chronorift/tmp`.
//! v2 stores paths relative to `<root>` and derives absolute paths from where the file was
//! found. v1 files (absolute paths, game files directly in `<root>`) are still read.

use super::{error::InstallError, resolve::Source};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const METADATA_DIR: &str = ".chronorift";
pub const RECORD_FILE: &str = "install.json";
pub const STATE_FILE: &str = "install-state.json";
pub const TEMP_DIR: &str = "tmp";
pub const GAME_DIR: &str = "Game";
pub const GAME_EXE: &str = "League of Legends.exe";
pub const STORE_VERSION: u32 = 2;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

pub fn metadata_dir(root: &Path) -> PathBuf {
    root.join(METADATA_DIR)
}

pub fn temp_dir(root: &Path) -> PathBuf {
    metadata_dir(root).join(TEMP_DIR)
}

/// Joins a `/`-separated relative path under `base`, refusing anything that could escape it.
pub fn safe_join(base: &Path, relative: &str) -> Result<PathBuf, InstallError> {
    let mut path = base.to_path_buf();
    for part in relative.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.contains('\\')
            || part.contains(':')
            || part.contains('\0')
        {
            return Err(InstallError::UnsafePath(relative.to_string()));
        }
        path.push(part);
    }
    Ok(path)
}

/// `<root>/<game_dir>`; an empty `game_dir` is the v1 layout (files directly in `<root>`).
pub fn game_path(root: &Path, game_dir: &str) -> Result<PathBuf, InstallError> {
    if game_dir.is_empty() {
        Ok(root.to_path_buf())
    } else {
        safe_join(root, game_dir)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallStatus {
    InProgress,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateFile {
    /// Relative to the game folder.
    pub path: String,
    pub size: u64,
}

/// What was installed and from where: `install.json` and `install-state.json` share it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub patch: String,
    pub resolved_version: String,
    pub requested_version: Option<String>,
    pub exact_build: bool,
    pub source: Source,
    pub region: String,
    pub manifest_region: String,
    pub locale: String,
    pub manifest_url: String,
}

/// `<root>/.chronorift/install-state.json` (v2), written at install/repair start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallState {
    pub version: u32,
    pub status: InstallStatus,
    #[serde(flatten)]
    pub release: Release,
    /// Game folder relative to the install root: `Game`, or `""` for v1 installs.
    pub game_dir: String,
    pub files: Vec<StateFile>,
    pub started_at_ms: u64,
    pub updated_at_ms: u64,
}

/// `<root>/.chronorift/install.json` (v2), written when an install/repair completes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallRecord {
    pub version: u32,
    #[serde(flatten)]
    pub release: Release,
    pub game_dir: String,
    pub file_count: usize,
    pub download_bytes: u64,
    pub install_bytes: u64,
    pub installed_at_ms: u64,
}

/// v1 `install-state.json` (Milestone 2): absolute destination, files relative to it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StateV1 {
    status: InstallStatus,
    patch: String,
    resolved_version: String,
    region: String,
    locale: String,
    manifest_url: String,
    files: Vec<StateFile>,
    started_at_ms: u64,
    updated_at_ms: u64,
}

/// v1 `install.json` (the old `ClientInstallResult`), absolute paths.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordV1 {
    patch: String,
    resolved_version: String,
    region: String,
    locale: String,
    executable_path: String,
    manifest_url: String,
    file_count: usize,
    download_bytes: u64,
    install_bytes: u64,
}

fn v1_release(
    patch: String,
    version: String,
    region: String,
    locale: String,
    url: String,
) -> Release {
    Release {
        patch,
        resolved_version: version,
        requested_version: None,
        exact_build: false,
        source: Source::Sieve,
        manifest_region: region.clone(),
        region,
        locale,
        manifest_url: url,
    }
}

/// A read `install.json` with absolute paths derived from `root`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredInstall {
    pub root: PathBuf,
    pub record: InstallRecord,
    pub game_path: PathBuf,
    pub executable_path: PathBuf,
}

fn file_version(value: &serde_json::Value) -> u64 {
    value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1)
}

fn invalid(path: &Path, err: impl ToString) -> InstallError {
    InstallError::Io {
        source: io::Error::new(io::ErrorKind::InvalidData, err.to_string()),
        path: Some(path.to_path_buf()),
    }
}

fn read_json(path: &Path) -> Result<Option<serde_json::Value>, InstallError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|err| invalid(path, err)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(InstallError::io_at(path)(err)),
    }
}

pub fn read_state(root: &Path) -> Result<Option<InstallState>, InstallError> {
    let path = metadata_dir(root).join(STATE_FILE);
    let Some(value) = read_json(&path)? else {
        return Ok(None);
    };
    let state = match file_version(&value) {
        1 => {
            let v1: StateV1 = serde_json::from_value(value).map_err(|err| invalid(&path, err))?;
            InstallState {
                version: STORE_VERSION,
                status: v1.status,
                release: v1_release(
                    v1.patch,
                    v1.resolved_version,
                    v1.region,
                    v1.locale,
                    v1.manifest_url,
                ),
                game_dir: String::new(),
                files: v1.files,
                started_at_ms: v1.started_at_ms,
                updated_at_ms: v1.updated_at_ms,
            }
        }
        2 => serde_json::from_value(value).map_err(|err| invalid(&path, err))?,
        other => return Err(invalid(&path, format!("unknown state version {other}"))),
    };
    game_path(root, &state.game_dir)?;
    Ok(Some(state))
}

pub fn read_record(root: &Path) -> Result<Option<StoredInstall>, InstallError> {
    let path = metadata_dir(root).join(RECORD_FILE);
    let Some(value) = read_json(&path)? else {
        return Ok(None);
    };
    let (record, executable_path) = match file_version(&value) {
        1 => {
            let v1: RecordV1 = serde_json::from_value(value).map_err(|err| invalid(&path, err))?;
            let executable = PathBuf::from(&v1.executable_path);
            let record = InstallRecord {
                version: STORE_VERSION,
                release: v1_release(
                    v1.patch,
                    v1.resolved_version,
                    v1.region,
                    v1.locale,
                    v1.manifest_url,
                ),
                game_dir: String::new(),
                file_count: v1.file_count,
                download_bytes: v1.download_bytes,
                install_bytes: v1.install_bytes,
                installed_at_ms: 0,
            };
            (record, executable)
        }
        2 => {
            let record: InstallRecord =
                serde_json::from_value(value).map_err(|err| invalid(&path, err))?;
            let executable = game_path(root, &record.game_dir)?.join(GAME_EXE);
            (record, executable)
        }
        other => return Err(invalid(&path, format!("unknown record version {other}"))),
    };
    let game_path = match executable_path.parent() {
        Some(parent) => parent.to_path_buf(),
        None => root.to_path_buf(),
    };
    Ok(Some(StoredInstall {
        root: root.to_path_buf(),
        record,
        game_path,
        executable_path,
    }))
}

/// Writes JSON beside `path`, syncs it, and renames it into place.
fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<(), InstallError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(InstallError::io_at(parent))?;
    let partial = path.with_extension("json.partial");
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|err| invalid(path, err))?;
    bytes.push(b'\n');
    let mut file = File::create(&partial).map_err(InstallError::io_at(&partial))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(InstallError::io_at(&partial))?;
    drop(file);
    fs::rename(&partial, path).map_err(InstallError::io_at(path))
}

pub fn write_state(root: &Path, state: &InstallState) -> Result<(), InstallError> {
    write_json_atomic(&metadata_dir(root).join(STATE_FILE), state)
}

pub fn write_record(root: &Path, record: &InstallRecord) -> Result<(), InstallError> {
    write_json_atomic(&metadata_dir(root).join(RECORD_FILE), record)
}

/// Files (not directories) directly inside `<root>/.chronorift/tmp`. Missing dir → empty.
pub fn stale_temp_files(root: &Path) -> Result<Vec<PathBuf>, InstallError> {
    let dir = temp_dir(root);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(InstallError::io_at(&dir)(err)),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(InstallError::io_at(&dir))?;
        let is_dir = entry
            .file_type()
            .map_err(InstallError::io_at(&entry.path()))?
            .is_dir();
        if !is_dir {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

pub fn remove_stale_temp_files(root: &Path) -> Result<usize, InstallError> {
    let files = stale_temp_files(root)?;
    for file in &files {
        fs::remove_file(file).map_err(InstallError::io_at(file))?;
    }
    Ok(files.len())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn temp_root(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "chronorift-{name}-{}-{}-{}",
            std::process::id(),
            now_ms(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub(crate) fn release() -> Release {
        Release {
            patch: "16.17".into(),
            resolved_version: "16.17.8104348".into(),
            requested_version: Some("16.17.8104348".into()),
            exact_build: true,
            source: Source::Archive,
            region: "LA2".into(),
            manifest_region: "LA2".into(),
            locale: "es_MX".into(),
            manifest_url: "https://lol.secure.dyn.riotcdn.net/channels/public/releases/A.manifest"
                .into(),
        }
    }

    #[test]
    fn safe_join_rejects_escapes() {
        let root = Path::new("D:/Clients/16.13");
        assert!(safe_join(root, "DATA/FINAL/file.wad").is_ok());
        for bad in [
            "../escape.exe",
            "C:/escape.exe",
            "DATA\\x",
            "a//b",
            "",
            "./a",
        ] {
            assert!(safe_join(root, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn v2_files_store_relative_paths_and_roundtrip() {
        let root = temp_root("store-v2");
        let state = InstallState {
            version: STORE_VERSION,
            status: InstallStatus::InProgress,
            release: release(),
            game_dir: GAME_DIR.into(),
            files: vec![StateFile {
                path: GAME_EXE.into(),
                size: 4,
            }],
            started_at_ms: 1,
            updated_at_ms: 2,
        };
        write_state(&root, &state).unwrap();
        let record = InstallRecord {
            version: STORE_VERSION,
            release: release(),
            game_dir: GAME_DIR.into(),
            file_count: 1,
            download_bytes: 3,
            install_bytes: 4,
            installed_at_ms: 5,
        };
        write_record(&root, &record).unwrap();
        let raw = fs::read_to_string(metadata_dir(&root).join(RECORD_FILE)).unwrap();
        let read_back = read_state(&root).unwrap().unwrap();
        let stored = read_record(&root).unwrap().unwrap();

        // Moving the folder keeps it valid: nothing absolute is stored.
        let moved = root.with_extension("moved");
        fs::rename(&root, &moved).unwrap();
        let moved_record = read_record(&moved).unwrap().unwrap();
        fs::remove_dir_all(&moved).unwrap();

        assert_eq!(read_back, state);
        assert_eq!(stored.record, record);
        assert_eq!(stored.executable_path, root.join(GAME_DIR).join(GAME_EXE));
        assert_eq!(
            moved_record.executable_path,
            moved.join(GAME_DIR).join(GAME_EXE)
        );
        assert!(!raw.contains(&root.display().to_string()));
        assert!(raw.contains("\"version\": 2"));
        assert!(raw.contains("\"manifestUrl\""));
        assert!(raw.contains("\"exactBuild\": true"));
        assert!(raw.contains("\"source\": \"archive\""));
    }

    #[test]
    fn reads_v1_state_and_record() {
        let root = temp_root("store-v1");
        let meta = metadata_dir(&root);
        fs::create_dir_all(&meta).unwrap();
        let exe = root.join(GAME_EXE);
        fs::write(
            meta.join(STATE_FILE),
            serde_json::json!({
                "version": 1, "status": "complete", "patch": "14.9",
                "resolvedVersion": "14.9.5831234", "region": "LA2", "locale": "es_MX",
                "manifestUrl": "https://example.invalid/releases/A.manifest",
                "destinationPath": root.display().to_string(),
                "files": [{ "path": "League of Legends.exe", "size": 4 }],
                "startedAtMs": 10, "updatedAtMs": 20
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            meta.join(RECORD_FILE),
            serde_json::json!({
                "patch": "14.9", "resolvedVersion": "14.9.5831234", "region": "LA2",
                "locale": "es_MX", "destinationPath": root.display().to_string(),
                "executablePath": exe.display().to_string(),
                "manifestUrl": "https://example.invalid/releases/A.manifest",
                "fileCount": 1, "downloadBytes": 2, "installBytes": 3
            })
            .to_string(),
        )
        .unwrap();

        let state = read_state(&root).unwrap().unwrap();
        let stored = read_record(&root).unwrap().unwrap();
        fs::write(meta.join(STATE_FILE), b"{ nope").unwrap();
        let corrupt = read_state(&root);
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(state.status, InstallStatus::Complete);
        assert_eq!(state.game_dir, "");
        assert_eq!(state.release.locale, "es_MX");
        assert_eq!(state.files[0].size, 4);
        assert!(!state.release.exact_build);
        assert_eq!(stored.executable_path, exe);
        assert_eq!(stored.game_path, root);
        assert_eq!(stored.record.release.resolved_version, "14.9.5831234");
        assert_eq!(corrupt.unwrap_err().code(), "io");
    }

    #[test]
    fn missing_files_read_as_none() {
        let root = temp_root("store-none");
        assert!(read_state(&root).unwrap().is_none());
        assert!(read_record(&root).unwrap().is_none());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn detects_and_removes_only_temp_files() {
        let root = temp_root("store-tmp");
        assert!(stale_temp_files(&root).unwrap().is_empty());
        let tmp = temp_dir(&root);
        fs::create_dir_all(tmp.join("nested")).unwrap();
        fs::write(tmp.join("a.partial"), b"1").unwrap();
        fs::write(tmp.join("b.partial"), b"2").unwrap();
        fs::write(root.join("keep.wad"), b"3").unwrap();

        let found = stale_temp_files(&root).unwrap();
        let removed = remove_stale_temp_files(&root).unwrap();
        let kept = tmp.join("nested").is_dir() && root.join("keep.wad").is_file();
        let left = stale_temp_files(&root).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(found.len(), 2);
        assert_eq!(removed, 2);
        assert!(kept);
        assert!(left.is_empty());
    }
}
