use cdragon_rman::{BundleChunk, Rman};
use reqwest::{header, Url};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    error::Error,
    fmt::{self, Display},
    fs::{self, File, OpenOptions},
    io::{Cursor, Write},
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Emitter};

const INSTALL_PROGRESS_EVENT: &str = "download-progress";
const PRODUCT: &str = "lol";
const PLATFORM: &str = "windows";
const GAME_ARTIFACT: &str = "lol-game-client";
const GAME_EXE: &str = "League of Legends.exe";
const PLATFORM_MARKERS: [&str; 2] = ["windows", "macos"];

#[derive(Debug, Deserialize)]
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
pub struct ClientInstallResult {
    pub patch: String,
    pub resolved_version: String,
    pub region: String,
    pub locale: String,
    pub destination_path: String,
    pub executable_path: String,
    pub manifest_url: String,
    pub file_count: usize,
    pub download_bytes: u64,
    pub install_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInstallPlan {
    pub patch: String,
    pub resolved_version: String,
    pub region: String,
    pub locale: String,
    pub destination_path: String,
    pub manifest_url: String,
    pub file_count: usize,
    pub download_bytes: u64,
    pub install_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientInstallProgress {
    destination_path: String,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    percent: Option<f64>,
    done: bool,
    stage: String,
    detail: String,
}

#[derive(Debug)]
pub enum ClientInstallError {
    InvalidPatch,
    InvalidLocale(String),
    InvalidDestination,
    InvalidManifestUrl(String),
    MissingBundleBase(String),
    Resolve(reqwest::Error),
    InvalidResolveResponse(serde_json::Error),
    Metadata(serde_json::Error),
    ReleaseNotFound {
        region: String,
        patch: String,
        version: Option<String>,
    },
    Download(reqwest::Error),
    InvalidChunkSize {
        expected: u32,
        actual: usize,
    },
    Io(std::io::Error),
    Rman(cdragon_rman::RmanError),
    Zstd(std::io::Error),
    UnsafeManifestPath(String),
    WouldOverwrite(PathBuf),
    MissingExecutable(PathBuf),
    Emit(tauri::Error),
}

impl Display for ClientInstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPatch => write!(f, "Invalid replay patch"),
            Self::InvalidLocale(locale) => write!(f, "Invalid Riot locale: {locale}"),
            Self::InvalidDestination => write!(f, "Choose a client install destination first"),
            Self::InvalidManifestUrl(url) => write!(f, "Invalid manifest URL: {url}"),
            Self::MissingBundleBase(url) => {
                write!(f, "Could not derive bundle URL from manifest URL: {url}")
            }
            Self::Resolve(err) => write!(f, "Could not resolve Riot client manifests: {err}"),
            Self::InvalidResolveResponse(err) => {
                write!(f, "Riot manifest response had an unexpected shape: {err}")
            }
            Self::Metadata(err) => write!(f, "Could not write install metadata: {err}"),
            Self::ReleaseNotFound {
                region,
                patch,
                version,
            } => {
                if let Some(version) = version {
                    write!(
                        f,
                        "No {GAME_ARTIFACT} manifest found for {region} {version} ({patch})"
                    )
                } else {
                    write!(f, "No {GAME_ARTIFACT} manifest found for {region} {patch}")
                }
            }
            Self::Download(err) => write!(f, "Client download failed: {err}"),
            Self::InvalidChunkSize { expected, actual } => write!(
                f,
                "Client bundle response had an unexpected size: expected {expected} bytes, got {actual}"
            ),
            Self::Io(err) => write!(f, "Client install failed: {err}"),
            Self::Rman(err) => write!(f, "Could not parse Riot manifest: {err}"),
            Self::Zstd(err) => write!(f, "Could not decompress client chunk: {err}"),
            Self::UnsafeManifestPath(path) => {
                write!(f, "Manifest contains an unsafe file path: {path}")
            }
            Self::WouldOverwrite(path) => write!(
                f,
                "Refusing to overwrite existing file. Choose an empty folder or remove: {}",
                path.display()
            ),
            Self::MissingExecutable(path) => write!(
                f,
                "Install finished but the League executable was not found at {}",
                path.display()
            ),
            Self::Emit(err) => write!(f, "Could not publish install progress: {err}"),
        }
    }
}

impl Error for ClientInstallError {}

impl From<std::io::Error> for ClientInstallError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<cdragon_rman::RmanError> for ClientInstallError {
    fn from(value: cdragon_rman::RmanError) -> Self {
        Self::Rman(value)
    }
}

pub async fn install_client(
    app: AppHandle,
    request: ClientInstallRequest,
) -> Result<ClientInstallResult, ClientInstallError> {
    let planned = build_install_plan(&app, &request).await?;
    let destination = PathBuf::from(&planned.plan.destination_path);
    let bundle_base = bundle_base_from_manifest_url(&planned.plan.manifest_url)?;
    fs::create_dir_all(&destination)?;

    download_files(
        &app,
        &destination,
        &bundle_base,
        &planned.files,
        planned.plan.download_bytes,
    )
    .await?;

    let executable = destination.join(GAME_EXE);
    if !executable.is_file() {
        return Err(ClientInstallError::MissingExecutable(executable));
    }

    let result = ClientInstallResult {
        patch: planned.plan.patch,
        resolved_version: planned.plan.resolved_version,
        region: planned.plan.region,
        locale: planned.plan.locale,
        destination_path: planned.plan.destination_path,
        executable_path: executable.display().to_string(),
        manifest_url: planned.plan.manifest_url,
        file_count: planned.plan.file_count,
        download_bytes: planned.plan.download_bytes,
        install_bytes: planned.plan.install_bytes,
    };
    write_install_metadata(&destination, &result)?;
    emit_progress(
        &app,
        &destination,
        "complete",
        "Client ready",
        result.download_bytes,
        Some(result.download_bytes),
        true,
    )?;
    Ok(result)
}

pub async fn plan_client_install(
    app: AppHandle,
    request: ClientInstallRequest,
) -> Result<ClientInstallPlan, ClientInstallError> {
    build_install_plan(&app, &request)
        .await
        .map(|planned| planned.plan)
}

#[derive(Debug, Clone)]
struct SelectedRelease {
    version: String,
    manifest_url: String,
}

#[derive(Debug, Clone)]
struct InstallFile {
    path: String,
    filesize: u32,
    chunks: Vec<BundleChunk>,
}

#[derive(Debug, Clone)]
struct PlannedInstall {
    plan: ClientInstallPlan,
    files: Vec<InstallFile>,
}

#[derive(Debug, Deserialize)]
struct SieveResponse {
    releases: Vec<SieveRelease>,
}

#[derive(Debug, Deserialize)]
struct SieveRelease {
    release: Release,
    download: Download,
}

#[derive(Debug, Deserialize)]
struct Release {
    labels: HashMap<String, LabelValues>,
}

#[derive(Debug, Deserialize)]
struct LabelValues {
    values: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Download {
    url: String,
}

async fn build_install_plan(
    app: &AppHandle,
    request: &ClientInstallRequest,
) -> Result<PlannedInstall, ClientInstallError> {
    let patch = normalize_patch(&request.patch).ok_or(ClientInstallError::InvalidPatch)?;
    let locale = normalize_locale(&request.locale)
        .ok_or_else(|| ClientInstallError::InvalidLocale(request.locale.clone()))?;
    let region = request
        .region
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("NA1")
        .trim()
        .to_ascii_uppercase();
    let destination = PathBuf::from(request.destination_path.trim());
    if destination.as_os_str().is_empty() {
        return Err(ClientInstallError::InvalidDestination);
    }

    emit_progress(
        app,
        &destination,
        "resolve",
        "Resolving Riot manifest",
        0,
        None,
        false,
    )?;

    let target_version = request
        .game_version
        .as_deref()
        .and_then(normalize_game_version);
    let release = resolve_game_release(&region, &patch, target_version.as_deref()).await?;
    let manifest = download_manifest(app, &destination, &release.manifest_url).await?;
    emit_progress(
        app,
        &destination,
        "plan",
        "Planning locale files",
        0,
        None,
        false,
    )?;

    let rman = read_rman_manifest(manifest)?;
    let files = select_install_files(&rman, &locale)?;
    preflight_targets(&destination, &files)?;
    let (download_bytes, install_bytes) = install_totals(&files);

    Ok(PlannedInstall {
        plan: ClientInstallPlan {
            patch,
            resolved_version: release.version,
            region,
            locale,
            destination_path: destination.display().to_string(),
            manifest_url: release.manifest_url,
            file_count: files.len(),
            download_bytes,
            install_bytes,
        },
        files,
    })
}

fn install_totals(files: &[InstallFile]) -> (u64, u64) {
    let download_bytes = files
        .iter()
        .flat_map(|file| file.chunks.iter())
        .map(|chunk| u64::from(chunk.bundle_size))
        .sum::<u64>();
    let install_bytes = files
        .iter()
        .map(|file| u64::from(file.filesize))
        .sum::<u64>();
    (download_bytes, install_bytes)
}

async fn resolve_game_release(
    region: &str,
    patch: &str,
    target_version: Option<&str>,
) -> Result<SelectedRelease, ClientInstallError> {
    let url = format!(
        "https://sieve.services.riotcdn.net/api/v1/products/{PRODUCT}/version-sets/{region}?q[platform]={PLATFORM}"
    );
    let text = reqwest::get(&url)
        .await
        .map_err(ClientInstallError::Resolve)?
        .error_for_status()
        .map_err(ClientInstallError::Resolve)?
        .text()
        .await
        .map_err(ClientInstallError::Resolve)?;
    let response: SieveResponse =
        serde_json::from_str(&text).map_err(ClientInstallError::InvalidResolveResponse)?;

    select_release(&response.releases, patch, target_version).ok_or_else(|| {
        ClientInstallError::ReleaseNotFound {
            region: region.to_string(),
            patch: patch.to_string(),
            version: target_version.map(ToOwned::to_owned),
        }
    })
}

fn select_release(
    releases: &[SieveRelease],
    patch: &str,
    target_version: Option<&str>,
) -> Option<SelectedRelease> {
    let candidates = releases
        .iter()
        .filter_map(|release| {
            let artifact = first_label(&release.release.labels, "riot:artifact_type_id")?;
            if artifact != GAME_ARTIFACT {
                return None;
            }
            let version = first_label(&release.release.labels, "riot:artifact_version_id")?;
            let version = version.split('+').next()?.to_string();
            (version_patch(&version).as_deref() == Some(patch)).then_some(SelectedRelease {
                version,
                manifest_url: release.download.url.clone(),
            })
        })
        .collect::<Vec<_>>();

    if let Some(target_version) = target_version {
        if let Some(exact) = candidates
            .iter()
            .find(|candidate| candidate.version == target_version)
        {
            return Some(exact.clone());
        }
    }

    candidates
        .into_iter()
        .max_by_key(|candidate| version_revision(&candidate.version).unwrap_or(0))
}

fn first_label<'a>(labels: &'a HashMap<String, LabelValues>, key: &str) -> Option<&'a str> {
    labels.get(key)?.values.first().map(String::as_str)
}

async fn download_manifest(
    app: &AppHandle,
    destination: &Path,
    manifest_url: &str,
) -> Result<Vec<u8>, ClientInstallError> {
    let mut response = reqwest::get(manifest_url)
        .await
        .map_err(ClientInstallError::Download)?
        .error_for_status()
        .map_err(ClientInstallError::Download)?;
    let total = response.content_length();
    let mut bytes = Vec::new();

    emit_progress(
        app,
        destination,
        "manifest",
        "Downloading Riot manifest",
        0,
        total,
        false,
    )?;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(ClientInstallError::Download)?
    {
        bytes.extend_from_slice(&chunk);
        emit_progress(
            app,
            destination,
            "manifest",
            "Downloading Riot manifest",
            bytes.len() as u64,
            total,
            false,
        )?;
    }

    Ok(bytes)
}

fn read_rman_manifest(mut manifest: Vec<u8>) -> Result<Rman, ClientInstallError> {
    // ponytail: Riot current RMAN manifests use minor version 2.1; cdragon-rman 0.2 rejects it
    // but the table layout used here is still compatible. Real-manifest test guards this shim.
    if manifest.get(..6) == Some(b"RMAN\x02\x01") {
        manifest[5] = 0;
    }
    Rman::read(Cursor::new(manifest)).map_err(ClientInstallError::Rman)
}

fn select_install_files(rman: &Rman, locale: &str) -> Result<Vec<InstallFile>, ClientInstallError> {
    let locale_markers = rman
        .iter_flags()
        .filter_map(|flag| normalize_locale(flag.flag))
        .map(|flag| locale_asset_marker(&flag).to_string())
        .collect::<Vec<_>>();
    let marker = locale_asset_marker(locale);
    if !locale_markers.iter().any(|flag| flag == marker) {
        return Err(ClientInstallError::InvalidLocale(locale.to_string()));
    }

    let directories = rman.dir_paths();
    let chunks = rman.bundle_chunks();
    let mut files = Vec::new();
    for file in rman.iter_files() {
        let path = file.path(&directories);
        if !path_matches_locale(&path, marker, &locale_markers) {
            continue;
        }
        let chunks = file
            .iter_chunks()
            .map(|chunk_id| chunks[&chunk_id].clone())
            .collect::<Vec<_>>();
        files.push(InstallFile {
            path,
            filesize: file.filesize,
            chunks,
        });
    }

    Ok(files)
}

fn path_matches_locale(path: &str, marker: &str, locale_markers: &[String]) -> bool {
    for known in locale_markers {
        if PLATFORM_MARKERS.contains(&known.as_str()) {
            continue;
        }
        if known != marker && has_marker(path, known) {
            return false;
        }
    }

    let has_known_platform = PLATFORM_MARKERS
        .iter()
        .any(|platform| has_marker(path, platform));
    !has_known_platform || has_marker(path, PLATFORM)
}

fn has_marker(path: &str, marker: &str) -> bool {
    path.contains(&format!(".{marker}."))
        || path.contains(&format!("/{marker}/"))
        || path.ends_with(&format!(".{marker}"))
}

fn locale_asset_marker(locale: &str) -> &str {
    match locale {
        "en_AU" | "en_GB" | "en_PH" | "en_SG" | "en_US" | "id_ID" | "zh_MY" => "en_US",
        "es_AR" | "es_MX" => "es_MX",
        other => other,
    }
}

fn preflight_targets(destination: &Path, files: &[InstallFile]) -> Result<(), ClientInstallError> {
    for file in files {
        let target = resolve_install_path(destination, &file.path)?;
        if target.exists() {
            return Err(ClientInstallError::WouldOverwrite(target));
        }
    }
    Ok(())
}

async fn download_files(
    app: &AppHandle,
    destination: &Path,
    bundle_base: &str,
    files: &[InstallFile],
    total_download_bytes: u64,
) -> Result<(), ClientInstallError> {
    let client = reqwest::Client::new();
    let temp_dir = destination.join(".chronorift").join("tmp");
    fs::create_dir_all(&temp_dir)?;
    let mut downloaded_bytes = 0;

    for file in files {
        let target = resolve_install_path(destination, &file.path)?;
        let temp_path = temp_dir.join(temp_name_for_manifest_path(&file.path));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => file,
            Err(err) => return Err(ClientInstallError::Io(err)),
        };

        for chunk in &file.chunks {
            let chunk_result = download_bundle_range(&client, bundle_base, chunk).await;
            let compressed = match chunk_result {
                Ok(bytes) => bytes,
                Err(err) => {
                    let _ = fs::remove_file(&temp_path);
                    return Err(err);
                }
            };
            downloaded_bytes += compressed.len() as u64;
            emit_progress(
                app,
                destination,
                "download",
                &file.path,
                downloaded_bytes,
                Some(total_download_bytes),
                false,
            )?;

            let decompressed = match zstd::bulk::decompress(&compressed, chunk.target_size as usize)
            {
                Ok(bytes) => bytes,
                Err(err) => {
                    let _ = fs::remove_file(&temp_path);
                    return Err(ClientInstallError::Zstd(err));
                }
            };
            if let Err(err) = output.write_all(&decompressed) {
                let _ = fs::remove_file(&temp_path);
                return Err(ClientInstallError::Io(err));
            }
        }
        if let Err(err) = output.flush() {
            let _ = fs::remove_file(&temp_path);
            return Err(ClientInstallError::Io(err));
        }
        drop(output);
        fs::rename(&temp_path, target)?;
    }

    Ok(())
}

async fn download_bundle_range(
    client: &reqwest::Client,
    bundle_base: &str,
    chunk: &BundleChunk,
) -> Result<Vec<u8>, ClientInstallError> {
    let from = chunk.bundle_offset;
    let to = chunk.bundle_offset + chunk.bundle_size - 1;
    let url = format!("{bundle_base}/{:016X}.bundle", chunk.bundle_id);
    let bytes = client
        .get(url)
        .header(header::RANGE, format!("bytes={from}-{to}"))
        .send()
        .await
        .map_err(ClientInstallError::Download)?
        .error_for_status()
        .map_err(ClientInstallError::Download)?
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(ClientInstallError::Download)?;
    if bytes.len() != chunk.bundle_size as usize {
        return Err(ClientInstallError::InvalidChunkSize {
            expected: chunk.bundle_size,
            actual: bytes.len(),
        });
    }
    Ok(bytes)
}

fn write_install_metadata(
    destination: &Path,
    result: &ClientInstallResult,
) -> Result<(), ClientInstallError> {
    let metadata_dir = destination.join(".chronorift");
    fs::create_dir_all(&metadata_dir)?;
    let mut file = File::create(metadata_dir.join("install.json"))?;
    serde_json::to_writer_pretty(&mut file, result).map_err(ClientInstallError::Metadata)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn emit_progress(
    app: &AppHandle,
    destination: &Path,
    stage: &str,
    detail: &str,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    done: bool,
) -> Result<(), ClientInstallError> {
    let percent = total_bytes
        .filter(|total| *total > 0)
        .map(|total| downloaded_bytes as f64 / total as f64 * 100.0);
    app.emit(
        INSTALL_PROGRESS_EVENT,
        ClientInstallProgress {
            destination_path: destination.display().to_string(),
            downloaded_bytes,
            total_bytes,
            percent,
            done,
            stage: stage.to_string(),
            detail: detail.to_string(),
        },
    )
    .map_err(ClientInstallError::Emit)
}

fn normalize_patch(patch: &str) -> Option<String> {
    let mut parts = patch.trim().split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    (major.chars().all(|ch| ch.is_ascii_digit()) && minor.chars().all(|ch| ch.is_ascii_digit()))
        .then(|| format!("{major}.{minor}"))
}

fn normalize_game_version(version: &str) -> Option<String> {
    let version = version.trim().split('+').next()?.trim();
    let parts = version.split('.').collect::<Vec<_>>();
    if parts.len() >= 4 {
        return Some(format!(
            "{}.{}.{}{}",
            parts[0], parts[1], parts[2], parts[3]
        ));
    }
    (!version.is_empty()).then(|| version.to_string())
}

fn version_patch(version: &str) -> Option<String> {
    let mut parts = version.split('.');
    Some(format!("{}.{}", parts.next()?, parts.next()?))
}

fn version_revision(version: &str) -> Option<u64> {
    version.split('.').nth(2)?.parse().ok()
}

fn normalize_locale(locale: &str) -> Option<String> {
    let trimmed = locale.trim();
    let (language, territory) = trimmed.split_once('_')?;
    (language.len() == 2
        && territory.len() == 2
        && language.chars().all(|ch| ch.is_ascii_alphabetic())
        && territory.chars().all(|ch| ch.is_ascii_alphabetic()))
    .then(|| {
        format!(
            "{}_{}",
            language.to_ascii_lowercase(),
            territory.to_ascii_uppercase()
        )
    })
}

fn bundle_base_from_manifest_url(manifest_url: &str) -> Result<String, ClientInstallError> {
    let mut url = Url::parse(manifest_url)
        .map_err(|_| ClientInstallError::InvalidManifestUrl(manifest_url.to_string()))?;
    let path = url.path();
    let Some(index) = path.rfind("/releases/") else {
        return Err(ClientInstallError::MissingBundleBase(
            manifest_url.to_string(),
        ));
    };
    let base_path = format!("{}/bundles", &path[..index]);
    url.set_path(&base_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn resolve_install_path(
    destination: &Path,
    manifest_path: &str,
) -> Result<PathBuf, ClientInstallError> {
    let mut path = PathBuf::from(destination);
    for part in manifest_path.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.contains('\\')
            || part.contains(':')
        {
            return Err(ClientInstallError::UnsafeManifestPath(
                manifest_path.to_string(),
            ));
        }
        path.push(part);
    }
    Ok(path)
}

fn temp_name_for_manifest_path(manifest_path: &str) -> String {
    manifest_path
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_replay_game_version_to_artifact_version() {
        assert_eq!(
            normalize_game_version("16.13.790.6961"),
            Some("16.13.7906961".to_string())
        );
        assert_eq!(
            normalize_game_version("16.13.7906961+branch.releases-16-13"),
            Some("16.13.7906961".to_string())
        );
    }

    #[test]
    fn normalizes_locale_shape_without_forcing_region_choice() {
        assert_eq!(normalize_locale("es_mx"), Some("es_MX".to_string()));
        assert_eq!(normalize_locale("ko_KR"), Some("ko_KR".to_string()));
        assert_eq!(normalize_locale("english"), None);
    }

    #[test]
    fn filters_files_for_locale_and_windows() {
        let markers = ["windows", "en_US", "es_MX", "fr_FR", "macos"]
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();

        assert!(path_matches_locale(
            "League of Legends.exe",
            "es_MX",
            &markers
        ));
        assert!(path_matches_locale(
            "DATA/FINAL/Bootstrap.windows.wad.client",
            "es_MX",
            &markers
        ));
        assert!(path_matches_locale(
            "DATA/FINAL/Champions/Aatrox.es_MX.wad.client",
            "es_MX",
            &markers
        ));
        assert!(!path_matches_locale(
            "DATA/FINAL/Champions/Aatrox.fr_FR.wad.client",
            "es_MX",
            &markers
        ));
        assert!(!path_matches_locale(
            "DATA/FINAL/Champions/Aatrox.fr_FR.windows.wad.client",
            "es_MX",
            &markers
        ));
        assert!(!path_matches_locale(
            "DATA/FINAL/Bootstrap.macos.wad.client",
            "es_MX",
            &markers
        ));
        assert_eq!(locale_asset_marker("en_GB"), "en_US");
    }

    #[test]
    fn derives_bundle_base_url_from_manifest_url() {
        assert_eq!(
            bundle_base_from_manifest_url(
                "https://lol.secure.dyn.riotcdn.net/channels/public/releases/9A.manifest"
            )
            .unwrap(),
            "https://lol.secure.dyn.riotcdn.net/channels/public/bundles"
        );
    }

    #[test]
    fn rejects_unsafe_manifest_paths() {
        let root = Path::new("D:/Clients/16.13");
        assert!(resolve_install_path(root, "DATA/FINAL/file.wad").is_ok());
        assert!(resolve_install_path(root, "../escape.exe").is_err());
        assert!(resolve_install_path(root, "C:/escape.exe").is_err());
        assert!(resolve_install_path(root, "DATA\\escape.exe").is_err());
    }

    #[test]
    fn selects_exact_release_before_latest_patch_release() {
        let releases = vec![
            release("16.13.7915903", "latest"),
            release("16.13.7906961", "exact"),
            release("16.12.7884269", "old"),
        ];

        let selected = select_release(&releases, "16.13", Some("16.13.7906961")).unwrap();

        assert_eq!(selected.manifest_url, "exact");
    }

    #[test]
    fn selects_latest_patch_release_without_exact_version() {
        let releases = vec![
            release("16.13.7906961", "first"),
            release("16.13.7915903", "latest"),
            release("16.12.7884269", "old"),
        ];

        let selected = select_release(&releases, "16.13", None).unwrap();

        assert_eq!(selected.manifest_url, "latest");
    }

    #[test]
    fn sums_install_plan_totals() {
        let files = vec![install_file(100, &[10, 15]), install_file(250, &[20])];

        assert_eq!(install_totals(&files), (45, 350));
    }

    #[test]
    fn parses_optional_real_manifest_sample() {
        let Ok(path) = std::env::var("CHRONORIFT_MANIFEST_SAMPLE") else {
            return;
        };

        let bytes = std::fs::read(path).expect("real manifest should read");
        let rman = read_rman_manifest(bytes).expect("real manifest should parse");
        let files = select_install_files(&rman, "es_MX").expect("locale files should plan");

        assert!(files.len() > 100);
        assert!(files.iter().any(|file| file.path == GAME_EXE));
        assert!(files.iter().any(|file| file.path.contains(".es_MX.")));
        assert!(!files.iter().any(|file| file.path.contains(".fr_FR.")));
    }

    fn release(version: &str, url: &str) -> SieveRelease {
        SieveRelease {
            release: Release {
                labels: HashMap::from([
                    (
                        "riot:artifact_type_id".to_string(),
                        LabelValues {
                            values: vec![GAME_ARTIFACT.to_string()],
                        },
                    ),
                    (
                        "riot:artifact_version_id".to_string(),
                        LabelValues {
                            values: vec![format!("{version}+branch.releases")],
                        },
                    ),
                ]),
            },
            download: Download {
                url: url.to_string(),
            },
        }
    }

    fn install_file(filesize: u32, chunk_sizes: &[u32]) -> InstallFile {
        InstallFile {
            path: format!("file-{filesize}"),
            filesize,
            chunks: chunk_sizes
                .iter()
                .map(|size| BundleChunk {
                    bundle_id: 1,
                    bundle_offset: 0,
                    bundle_size: *size,
                    target_size: *size,
                })
                .collect(),
        }
    }
}
