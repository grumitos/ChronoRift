use crate::settings;
use cdragon_rman::{BundleChunk, Rman};
use reqwest::{header, redirect, StatusCode, Url};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    error::Error,
    fmt::{self, Display},
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Seek, SeekFrom, Write},
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Runtime};

const INSTALL_PROGRESS_EVENT: &str = "download-progress";
const PRODUCT: &str = "lol";
const PLATFORM: &str = "windows";
const GAME_ARTIFACT: &str = "lol-game-client";
pub(crate) const GAME_EXE: &str = "League of Legends.exe";
/// Replica el diseño de Riot (`<base>\Game\League of Legends.exe`) para que
/// `-GameBaseDir` apunte a una carpeta propia de cada compilación.
const GAME_FOLDER: &str = "Game";
const PLATFORM_MARKERS: [&str; 2] = ["windows", "macos"];
const DEFAULT_REGION: &str = "NA1";
const SIEVE_BASE: &str = "https://sieve.services.riotcdn.net";
const MANIFEST_INDEX_BASE: &str = "https://raw.githubusercontent.com/Morilli/riot-manifests/master";
/// Los manifiestos de `lol-game-client` son iguales en todas las regiones; si la región de la
/// repetición no figura en el índice, se prueban estas.
const INDEX_FALLBACK_REGIONS: [&str; 2] = ["NA1", "EUW1"];
const RIOT_CDN_HOST_SUFFIX: &str = ".riotcdn.net";
pub(crate) const USER_AGENT: &str = concat!("ChronoRift/", env!("CARGO_PKG_VERSION"));
/// Mismo tope que la política por defecto de reqwest.
const MAX_REDIRECTS: usize = 10;

/// Solo se siguen redirecciones dentro del origen (esquema, host y puerto) de la petición: un
/// manifiesto, un bundle o un recurso del juego nunca llega de otro host. Ninguno de los endpoints
/// revisados el 2026-09-29 redirigía a otro host.
pub(crate) fn same_origin_redirects() -> redirect::Policy {
    redirect::Policy::custom(|attempt| {
        let same_origin = attempt
            .previous()
            .first()
            .is_some_and(|first| first.origin() == attempt.url().origin());
        if !same_origin {
            let refused = attempt.url().origin().ascii_serialization();
            attempt.error(format!("redirección rechazada hacia {refused}"))
        } else if attempt.previous().len() > MAX_REDIRECTS {
            attempt.error("demasiadas redirecciones")
        } else {
            attempt.follow()
        }
    })
}

/// Tamaño máximo de cada petición de rango: acota la memoria y lo que se repite si falla.
const MAX_WINDOW_BYTES: u32 = 8 * 1024 * 1024;
/// Peticiones de rango en paralelo; `settings.json` puede cambiarlo (`downloadConnections`).
const DEFAULT_CONNECTIONS: usize = 8;
const MAX_CONNECTIONS: usize = 32;
const TEMP_SUFFIX: &str = "part";
/// Copia del archivo reemplazado mientras dura el intercambio de una reparación.
const BACKUP_SUFFIX: &str = "backup";
/// Carpeta de temporales dentro de `METADATA_DIR`.
const TEMP_DIR: &str = "tmp";
/// Carpeta de ChronoRift dentro de cada cliente: metadatos y temporales.
const METADATA_DIR: &str = ".chronorift";
/// Se escribe al terminar una instalación; marca el cliente como listo.
const INSTALL_METADATA: &str = "install.json";
/// Plan confirmado de la instalación en curso; da el total si la descarga queda a medias.
const PLAN_METADATA: &str = "plan.json";

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ClientWorkMode {
    Install,
    Resume,
    Repair,
    Ready,
}

/// Nombre para mensajes: el aviso de un plan que cambió antes de confirmarlo.
impl Display for ClientWorkMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Install => "instalación nueva",
            Self::Resume => "reanudación",
            Self::Repair => "reparación",
            Self::Ready => "cliente listo",
        };
        f.write_str(value)
    }
}

/// De dónde salió la URL del manifiesto; se muestra en el plan antes de confirmar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReleaseSource {
    /// API oficial de Riot (solo lista las compilaciones vigentes).
    Sieve,
    /// Índice público Morilli/riot-manifests; el manifiesto y los bundles siguen viniendo del CDN de Riot.
    ManifestIndex,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInstallRequest {
    pub patch: String,
    pub game_version: Option<String>,
    pub region: Option<String>,
    pub locale: String,
    pub clients_root: String,
    pub confirmed: Option<ConfirmedPlan>,
}

/// Lo que el usuario vio y confirmó; si el plan recalculado difiere, no se toca el disco.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmedPlan {
    pub mode: ClientWorkMode,
    pub resolved_version: String,
    pub destination_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInstallPlan {
    pub mode: ClientWorkMode,
    pub patch: String,
    /// Versión del artefacto de Riot, por ejemplo `16.18.8175716`.
    pub resolved_version: String,
    /// Compilación en el formato de la repetición y del ejecutable, por ejemplo `16.18.817.5716`.
    pub game_build: String,
    /// `true` si la compilación coincide exactamente con la de la repetición.
    pub exact_build: bool,
    pub source: ReleaseSource,
    pub region: String,
    pub locale: String,
    pub clients_root: String,
    pub destination_path: String,
    pub manifest_url: String,
    pub file_count: usize,
    pub affected_file_count: usize,
    pub reused_file_count: usize,
    pub repaired_file_count: usize,
    pub download_bytes: u64,
    pub install_bytes: u64,
    pub total_install_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInstallResult {
    #[serde(flatten)]
    pub plan: ClientInstallPlan,
    pub executable_path: String,
    /// Duración de la fase de descarga y escritura, para medir el rendimiento real.
    pub download_seconds: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInstallProgress {
    pub destination_path: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub percent: Option<f64>,
    pub done: bool,
    pub stage: String,
    pub detail: String,
}

/// Destino de los eventos de progreso: la ventana de Tauri o un colector en las pruebas.
pub trait ProgressSink: Sync {
    fn publish(&self, progress: ClientInstallProgress) -> Result<(), ClientInstallError>;
}

impl<R: Runtime> ProgressSink for AppHandle<R> {
    fn publish(&self, progress: ClientInstallProgress) -> Result<(), ClientInstallError> {
        self.emit(INSTALL_PROGRESS_EVENT, progress)
            .map_err(ClientInstallError::Emit)
    }
}

/// Señal para detener una instalación en curso. Lo ya confirmado queda en disco para reanudar.
#[derive(Debug, Clone, Default)]
pub struct CancelSignal(Arc<AtomicBool>);

impl CancelSignal {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn reset(&self) {
        self.0.store(false, Ordering::SeqCst);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalClientState {
    Ready,
    Partial,
    Missing,
}

/// Estado de una compilación en la carpeta de clientes, leído sin red y sin crear nada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalClientStatus {
    pub game_build: String,
    pub destination_path: String,
    pub state: LocalClientState,
    pub executable_path: Option<String>,
    pub locale: Option<String>,
    /// Bytes de los archivos del juego ya confirmados (sin metadatos ni temporales).
    pub installed_bytes: u64,
    /// Total previsto, de `install.json` o del plan confirmado.
    pub total_install_bytes: Option<u64>,
}

/// Campos que el estado local lee de `install.json` o `plan.json`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StoredInstall {
    locale: Option<String>,
    total_install_bytes: Option<u64>,
}

#[derive(Debug)]
pub enum ClientInstallError {
    InvalidPatch,
    InvalidLocale(String),
    /// Bien escrito, pero el manifiesto no trae archivos para ese idioma.
    UnavailableLocale(String),
    InvalidClientsRoot(settings::SettingsError),
    InvalidReleaseVersion(String),
    InvalidManifestUrl(String),
    UntrustedManifestUrl(String),
    MissingBundleBase(String),
    HttpClient(reqwest::Error),
    Resolve(reqwest::Error),
    InvalidResolveResponse(serde_json::Error),
    ManifestIndex(reqwest::Error),
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
    InvalidChunkData {
        expected: u32,
        actual: usize,
    },
    ChunkLayoutMismatch(String),
    Io(std::io::Error),
    Rman(cdragon_rman::RmanError),
    CorruptManifest,
    MissingChunk(u64),
    Zstd(std::io::Error),
    UnsafeManifestPath(String),
    WouldOverwrite(PathBuf),
    InvalidExistingTarget(PathBuf),
    ConfirmationRequired {
        planned: ClientWorkMode,
        version: String,
        destination: String,
    },
    MissingExecutable(PathBuf),
    Emit(tauri::Error),
    Worker(tauri::Error),
    Cancelled,
}

impl Display for ClientInstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPatch => write!(f, "Parche no válido"),
            Self::InvalidLocale(locale) => write!(f, "Idioma no válido: {locale}"),
            Self::UnavailableLocale(locale) => {
                write!(f, "Idioma no disponible en este cliente: {locale}")
            }
            Self::InvalidClientsRoot(err) => write!(f, "{err}"),
            Self::InvalidReleaseVersion(version) => {
                write!(f, "Versión de cliente inesperada: {version}")
            }
            Self::InvalidManifestUrl(url) => write!(f, "URL de manifiesto no válida: {url}"),
            Self::UntrustedManifestUrl(url) => {
                write!(f, "Manifiesto fuera del CDN de Riot: {url}")
            }
            Self::MissingBundleBase(url) => {
                write!(f, "Sin URL de bundles para el manifiesto: {url}")
            }
            Self::HttpClient(err) => write!(
                f,
                "No se pudo preparar el cliente HTTP: {}",
                network_detail(err)
            ),
            Self::Resolve(err) => {
                write!(f, "No se pudo consultar Riot Sieve: {}", network_detail(err))
            }
            Self::InvalidResolveResponse(err) => {
                write!(f, "Respuesta inesperada de Riot Sieve: {err}")
            }
            Self::ManifestIndex(err) => write!(
                f,
                "No se pudo leer el índice riot-manifests: {}",
                network_detail(err)
            ),
            Self::Metadata(err) => {
                write!(f, "No se pudieron guardar los datos de la instalación: {err}")
            }
            Self::ReleaseNotFound {
                region,
                patch,
                version,
            } => {
                if let Some(version) = version {
                    write!(
                        f,
                        "Sin manifiesto de {GAME_ARTIFACT} para {region} {version} ({patch}) en Riot Sieve ni en riot-manifests"
                    )
                } else {
                    write!(f, "Sin manifiesto de {GAME_ARTIFACT} para {region} {patch}")
                }
            }
            Self::Download(err) => write!(f, "Falló la descarga: {}", network_detail(err)),
            Self::InvalidChunkSize { expected, actual } => write!(
                f,
                "Rango de bundle con tamaño inesperado: {actual} bytes en lugar de {expected}"
            ),
            Self::InvalidChunkData { expected, actual } => write!(
                f,
                "Fragmento descomprimido con tamaño inesperado: {actual} bytes en lugar de {expected}"
            ),
            Self::ChunkLayoutMismatch(path) => write!(
                f,
                "Los fragmentos del manifiesto no suman el tamaño del archivo: {path}"
            ),
            Self::Io(err) => write!(f, "Falló la instalación: {err}"),
            Self::Rman(err) => write!(f, "No se pudo leer el manifiesto de Riot: {err}"),
            Self::CorruptManifest => write!(f, "Manifiesto de Riot dañado o desconocido"),
            Self::MissingChunk(id) => {
                write!(f, "El manifiesto usa un fragmento inexistente: {id:016X}")
            }
            Self::Zstd(err) => write!(f, "No se pudo descomprimir un fragmento: {err}"),
            Self::UnsafeManifestPath(path) => {
                write!(f, "Ruta insegura en el manifiesto: {path}")
            }
            Self::WouldOverwrite(path) => {
                write!(f, "El archivo ya existe: {}", path.display())
            }
            Self::InvalidExistingTarget(path) => {
                write!(f, "El destino no es un archivo: {}", path.display())
            }
            Self::ConfirmationRequired {
                planned,
                version,
                destination,
            } => write!(f, "El plan cambió a {planned}: {version} en {destination}"),
            Self::MissingExecutable(path) => {
                write!(f, "Falta el ejecutable: {}", path.display())
            }
            Self::Emit(err) => write!(f, "No se pudo publicar el progreso: {err}"),
            Self::Worker(err) => write!(f, "Falló una tarea de la descarga: {err}"),
            Self::Cancelled => write!(f, "Descarga detenida"),
        }
    }
}

impl Error for ClientInstallError {}

/// Motivo de un fallo de red en español, con el host cuando se conoce. Lo que no se reconoce
/// queda con el texto de reqwest.
fn network_detail(error: &reqwest::Error) -> String {
    // La política de redirecciones ya da su motivo en español, con el origen rechazado.
    if error.is_redirect() {
        if let Some(reason) = error.source() {
            return reason.to_string();
        }
    }
    let reason = if error.is_timeout() {
        "tiempo de espera agotado".to_string()
    } else if error.is_connect() {
        "no se pudo conectar".to_string()
    } else if let Some(status) = error.status() {
        format!("HTTP {}", status.as_u16())
    } else if error.is_body() || error.is_decode() {
        "respuesta incompleta".to_string()
    } else {
        return error.to_string();
    };
    match error.url().and_then(Url::host_str) {
        Some(host) => format!("{reason} ({host})"),
        None => reason,
    }
}

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

/// Orígenes de red para resolver compilaciones. Las pruebas los apuntan a un servidor local.
#[derive(Debug, Clone)]
struct ReleaseEndpoints {
    sieve_base: String,
    index_base: String,
    manifest_hosts: ManifestHosts,
}

#[derive(Debug, Clone)]
enum ManifestHosts {
    /// Solo HTTPS bajo `*.riotcdn.net`.
    RiotCdn,
    /// Un origen exacto; lo usan las pruebas con un CDN local.
    #[cfg_attr(not(test), allow(dead_code))]
    Origin(String),
}

impl ReleaseEndpoints {
    fn riot() -> Self {
        Self {
            sieve_base: SIEVE_BASE.to_string(),
            index_base: MANIFEST_INDEX_BASE.to_string(),
            manifest_hosts: ManifestHosts::RiotCdn,
        }
    }
}

pub async fn install_client(
    progress: &dyn ProgressSink,
    request: ClientInstallRequest,
    tuning: DownloadTuning,
    cancel: &CancelSignal,
) -> Result<ClientInstallResult, ClientInstallError> {
    install_client_with(progress, &ReleaseEndpoints::riot(), request, tuning, cancel).await
}

/// Estado local de la compilación de una repetición en la carpeta de clientes.
pub fn local_client_status(
    clients_root: &str,
    game_version: &str,
) -> Result<LocalClientStatus, ClientInstallError> {
    let root = settings::normalize_clients_root(clients_root)
        .map_err(ClientInstallError::InvalidClientsRoot)?;
    let game_build = normalize_game_version(game_version)
        .and_then(|version| client_folder_name(&version))
        .ok_or_else(|| ClientInstallError::InvalidReleaseVersion(game_version.to_string()))?;
    let destination = root.join(&game_build).join(GAME_FOLDER);
    Ok(status_for_destination(destination, game_build))
}

/// Compilaciones con archivos en la carpeta de clientes, de la más nueva a la más vieja.
pub fn list_local_clients(
    clients_root: &str,
) -> Result<Vec<LocalClientStatus>, ClientInstallError> {
    let root = settings::normalize_clients_root(clients_root)
        .map_err(ClientInstallError::InvalidClientsRoot)?;
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut clients = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|name| name.split('.').count() == 4 && is_release_version(name))
        .map(|build| status_for_destination(root.join(&build).join(GAME_FOLDER), build))
        .filter(|status| status.state != LocalClientState::Missing)
        .collect::<Vec<_>>();
    clients.sort_by_cached_key(|status| std::cmp::Reverse(version_key(&status.game_build)));
    Ok(clients)
}

fn status_for_destination(destination: PathBuf, game_build: String) -> LocalClientStatus {
    let metadata_dir = destination.join(METADATA_DIR);
    let read = |name: &str| -> Option<StoredInstall> {
        let bytes = fs::read(metadata_dir.join(name)).ok()?;
        serde_json::from_slice(&bytes).ok()
    };
    let installed = read(INSTALL_METADATA);
    let planned = read(PLAN_METADATA);
    let executable = destination.join(GAME_EXE);
    let installed_bytes = game_file_bytes(&destination, &metadata_dir);
    let state = if installed.is_some() && executable.is_file() {
        LocalClientState::Ready
    } else if planned.is_some() || installed_bytes > 0 {
        LocalClientState::Partial
    } else {
        LocalClientState::Missing
    };
    let stored = installed.or(planned).unwrap_or_default();
    LocalClientStatus {
        game_build,
        destination_path: destination.display().to_string(),
        state,
        executable_path: (state == LocalClientState::Ready)
            .then(|| executable.display().to_string()),
        locale: stored.locale,
        installed_bytes,
        total_install_bytes: stored.total_install_bytes,
    }
}

/// Suma los archivos de la compilación sin contar la carpeta de ChronoRift ni enlaces.
fn game_file_bytes(directory: &Path, skip: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() && path != skip => game_file_bytes(&path, skip),
                Ok(kind) if kind.is_file() => entry.metadata().map_or(0, |info| info.len()),
                _ => 0,
            }
        })
        .sum()
}

fn version_key(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

pub async fn plan_client_install(
    progress: &dyn ProgressSink,
    request: ClientInstallRequest,
) -> Result<ClientInstallPlan, ClientInstallError> {
    let client = http_client()?;
    build_install_plan(&client, progress, &ReleaseEndpoints::riot(), &request)
        .await
        .map(|planned| planned.plan)
}

async fn install_client_with(
    progress: &dyn ProgressSink,
    endpoints: &ReleaseEndpoints,
    request: ClientInstallRequest,
    tuning: DownloadTuning,
    cancel: &CancelSignal,
) -> Result<ClientInstallResult, ClientInstallError> {
    let client = http_client()?;
    let planned = build_install_plan(&client, progress, endpoints, &request).await?;
    validate_confirmation(&planned.plan, request.confirmed.as_ref())?;
    if cancel.is_cancelled() {
        return Err(ClientInstallError::Cancelled);
    }
    // Recién con el plan confirmado, y también si ya está listo: un `.backup` de una reparación
    // cortada puede quedar aunque el archivo ya esté completo.
    remove_stale_temp_files(&planned.destination.join(METADATA_DIR).join(TEMP_DIR))?;
    let bundle_base = bundle_base_from_manifest_url(&planned.plan.manifest_url)?;
    fs::create_dir_all(&planned.destination)?;
    if planned.plan.mode != ClientWorkMode::Ready {
        // El plan confirmado da el total si la descarga queda a medias; `install.json` vuelve
        // a escribirse solo cuando el cliente está completo.
        write_metadata(&planned.destination, PLAN_METADATA, &planned.plan)?;
        remove_metadata(&planned.destination, INSTALL_METADATA)?;
    }

    let mut downloader = Downloader {
        client: &client,
        progress,
        destination: &planned.destination,
        bundle_base: &bundle_base,
        tuning,
        cancel,
        total_bytes: planned.plan.download_bytes,
        downloaded_bytes: 0,
    };
    let started = Instant::now();
    download_files(
        &mut downloader,
        &planned.pending_files,
        planned.plan.mode == ClientWorkMode::Repair,
    )
    .await?;
    let download_seconds = started.elapsed().as_secs_f64();

    let executable = planned.destination.join(GAME_EXE);
    if !executable.is_file() {
        return Err(ClientInstallError::MissingExecutable(executable));
    }

    let result = ClientInstallResult {
        executable_path: executable.display().to_string(),
        plan: planned.plan,
        download_seconds,
    };
    write_metadata(&planned.destination, INSTALL_METADATA, &result)?;
    remove_metadata(&planned.destination, PLAN_METADATA)?;
    publish(
        progress,
        &planned.destination,
        "complete",
        "Cliente listo",
        result.plan.download_bytes,
        Some(result.plan.download_bytes),
        true,
    )?;
    Ok(result)
}

fn http_client() -> Result<reqwest::Client, ClientInstallError> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(same_origin_redirects())
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(ClientInstallError::HttpClient)
}

#[derive(Debug, Clone)]
struct SelectedRelease {
    version: String,
    manifest_url: String,
    source: ReleaseSource,
    exact_build: bool,
}

#[derive(Debug, Clone)]
struct InstallFile {
    path: String,
    filesize: u32,
    chunks: Vec<BundleChunk>,
}

#[derive(Debug)]
struct PlannedInstall {
    plan: ClientInstallPlan,
    destination: PathBuf,
    pending_files: Vec<InstallFile>,
}

#[derive(Debug)]
struct ClassifiedInstall {
    mode: ClientWorkMode,
    pending_files: Vec<InstallFile>,
    reused_file_count: usize,
    repaired_file_count: usize,
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
    client: &reqwest::Client,
    progress: &dyn ProgressSink,
    endpoints: &ReleaseEndpoints,
    request: &ClientInstallRequest,
) -> Result<PlannedInstall, ClientInstallError> {
    let patch = normalize_patch(&request.patch).ok_or(ClientInstallError::InvalidPatch)?;
    let locale = normalize_locale(&request.locale)
        .ok_or_else(|| ClientInstallError::InvalidLocale(request.locale.clone()))?;
    let region = normalize_region(request.region.as_deref());
    let clients_root = settings::normalize_clients_root(&request.clients_root)
        .map_err(ClientInstallError::InvalidClientsRoot)?;
    let target_version = request
        .game_version
        .as_deref()
        .and_then(normalize_game_version);

    publish(
        progress,
        &clients_root,
        "resolve",
        "Buscando el manifiesto de Riot",
        0,
        None,
        false,
    )?;
    let release = resolve_game_release(
        client,
        endpoints,
        &region,
        &patch,
        target_version.as_deref(),
    )
    .await?;
    let game_build = client_folder_name(&release.version)
        .ok_or_else(|| ClientInstallError::InvalidReleaseVersion(release.version.clone()))?;
    let destination = clients_root.join(&game_build).join(GAME_FOLDER);
    let manifest = download_manifest(client, progress, &destination, &release.manifest_url).await?;
    publish(
        progress,
        &destination,
        "plan",
        "Planificando archivos",
        0,
        None,
        false,
    )?;

    let files = parse_install_files(manifest, &locale)?;
    let classified = classify_install_files(&destination, &files)?;
    let (download_bytes, install_bytes) = install_totals(&classified.pending_files);
    let (_, total_install_bytes) = install_totals(&files);

    Ok(PlannedInstall {
        plan: ClientInstallPlan {
            mode: classified.mode,
            patch,
            resolved_version: release.version,
            game_build,
            exact_build: release.exact_build,
            source: release.source,
            region,
            locale,
            clients_root: clients_root.display().to_string(),
            destination_path: destination.display().to_string(),
            manifest_url: release.manifest_url,
            file_count: files.len(),
            affected_file_count: classified.pending_files.len(),
            reused_file_count: classified.reused_file_count,
            repaired_file_count: classified.repaired_file_count,
            download_bytes,
            install_bytes,
            total_install_bytes,
        },
        destination,
        pending_files: classified.pending_files,
    })
}

fn validate_confirmation(
    plan: &ClientInstallPlan,
    confirmed: Option<&ConfirmedPlan>,
) -> Result<(), ClientInstallError> {
    let matches = confirmed.is_some_and(|confirmed| {
        confirmed.mode == plan.mode
            && confirmed.resolved_version == plan.resolved_version
            && confirmed.destination_path == plan.destination_path
    });
    if matches {
        return Ok(());
    }
    Err(ClientInstallError::ConfirmationRequired {
        planned: plan.mode,
        version: plan.game_build.clone(),
        destination: plan.destination_path.clone(),
    })
}

/// Bytes a descargar (cada fragmento de bundle una sola vez, aunque lo usen varios archivos)
/// y bytes que ocuparán los archivos en disco.
fn install_totals(files: &[InstallFile]) -> (u64, u64) {
    let mut seen = HashSet::new();
    let download_bytes = files
        .iter()
        .flat_map(|file| file.chunks.iter())
        .filter(|chunk| seen.insert((chunk.bundle_id, chunk.bundle_offset)))
        .map(|chunk| u64::from(chunk.bundle_size))
        .sum::<u64>();
    let install_bytes = files
        .iter()
        .map(|file| u64::from(file.filesize))
        .sum::<u64>();
    (download_bytes, install_bytes)
}

/// Sieve primero; si no tiene la compilación exacta, el índice público; si tampoco, la
/// compilación más reciente del mismo parche que liste Sieve, marcada como no exacta.
async fn resolve_game_release(
    client: &reqwest::Client,
    endpoints: &ReleaseEndpoints,
    region: &str,
    patch: &str,
    target_version: Option<&str>,
) -> Result<SelectedRelease, ClientInstallError> {
    let (sieve_releases, sieve_error) = match fetch_sieve_releases(client, endpoints, region).await
    {
        Ok(releases) => (releases, None),
        Err(error) => (Vec::new(), Some(error)),
    };

    let mut index_error = None;
    if let Some(target) = target_version {
        if let Some(release) = select_sieve_release(&sieve_releases, patch, Some(target))
            .filter(|release| release.exact_build)
        {
            return with_trusted_manifest(release, endpoints);
        }
        match lookup_manifest_index(client, endpoints, region, target).await {
            Ok(Some(manifest_url)) => {
                return Ok(SelectedRelease {
                    version: target.to_string(),
                    manifest_url,
                    source: ReleaseSource::ManifestIndex,
                    exact_build: true,
                })
            }
            Ok(None) => {}
            Err(error) => index_error = Some(error),
        }
    }

    if let Some(release) = select_sieve_release(&sieve_releases, patch, target_version) {
        return with_trusted_manifest(release, endpoints);
    }

    Err(sieve_error
        .or(index_error)
        .unwrap_or_else(|| ClientInstallError::ReleaseNotFound {
            region: region.to_string(),
            patch: patch.to_string(),
            version: target_version.map(ToOwned::to_owned),
        }))
}

async fn fetch_sieve_releases(
    client: &reqwest::Client,
    endpoints: &ReleaseEndpoints,
    region: &str,
) -> Result<Vec<SieveRelease>, ClientInstallError> {
    let url = format!(
        "{}/api/v1/products/{PRODUCT}/version-sets/{region}?q[platform]={PLATFORM}",
        endpoints.sieve_base
    );
    let text = client
        .get(&url)
        .send()
        .await
        .map_err(ClientInstallError::Resolve)?
        .error_for_status()
        .map_err(ClientInstallError::Resolve)?
        .text()
        .await
        .map_err(ClientInstallError::Resolve)?;
    let response: SieveResponse =
        serde_json::from_str(&text).map_err(ClientInstallError::InvalidResolveResponse)?;
    Ok(response.releases)
}

fn select_sieve_release(
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
            (version_patch(&version).as_deref() == Some(patch))
                .then(|| (version, release.download.url.clone()))
        })
        .collect::<Vec<_>>();

    let exact = target_version.and_then(|target| {
        candidates
            .iter()
            .find(|(version, _)| version == target)
            .cloned()
    });
    let (version, manifest_url) = exact.or_else(|| {
        candidates
            .into_iter()
            .max_by_key(|(version, _)| version_revision(version).unwrap_or(0))
    })?;

    Some(SelectedRelease {
        exact_build: target_version == Some(version.as_str()),
        version,
        manifest_url,
        source: ReleaseSource::Sieve,
    })
}

fn with_trusted_manifest(
    release: SelectedRelease,
    endpoints: &ReleaseEndpoints,
) -> Result<SelectedRelease, ClientInstallError> {
    let manifest_url = validate_manifest_url(&release.manifest_url, endpoints)?;
    Ok(SelectedRelease {
        manifest_url,
        ..release
    })
}

/// Consulta `LoL/<región>/windows/lol-game-client/<versión>.txt`, que contiene la URL del
/// manifiesto en el CDN de Riot. Devuelve `None` si ninguna región la tiene.
async fn lookup_manifest_index(
    client: &reqwest::Client,
    endpoints: &ReleaseEndpoints,
    region: &str,
    version: &str,
) -> Result<Option<String>, ClientInstallError> {
    let mut regions = vec![region];
    for fallback in INDEX_FALLBACK_REGIONS {
        if !regions.contains(&fallback) {
            regions.push(fallback);
        }
    }

    for candidate in regions {
        let url = format!(
            "{}/LoL/{candidate}/{PLATFORM}/{GAME_ARTIFACT}/{version}.txt",
            endpoints.index_base
        );
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(ClientInstallError::ManifestIndex)?;
        if response.status() == StatusCode::NOT_FOUND {
            continue;
        }
        let body = response
            .error_for_status()
            .map_err(ClientInstallError::ManifestIndex)?
            .text()
            .await
            .map_err(ClientInstallError::ManifestIndex)?;
        return validate_manifest_url(body.trim(), endpoints).map(Some);
    }
    Ok(None)
}

fn validate_manifest_url(
    raw: &str,
    endpoints: &ReleaseEndpoints,
) -> Result<String, ClientInstallError> {
    let url = Url::parse(raw.trim())
        .map_err(|_| ClientInstallError::InvalidManifestUrl(raw.to_string()))?;
    let trusted_origin = match &endpoints.manifest_hosts {
        ManifestHosts::RiotCdn => {
            url.scheme() == "https"
                && url
                    .host_str()
                    .is_some_and(|host| host.ends_with(RIOT_CDN_HOST_SUFFIX))
        }
        ManifestHosts::Origin(origin) => url.as_str().starts_with(&format!("{origin}/")),
    };
    if !trusted_origin || !url.path().ends_with(".manifest") {
        return Err(ClientInstallError::UntrustedManifestUrl(raw.to_string()));
    }
    Ok(url.to_string())
}

fn first_label<'a>(labels: &'a HashMap<String, LabelValues>, key: &str) -> Option<&'a str> {
    labels.get(key)?.values.first().map(String::as_str)
}

async fn download_manifest(
    client: &reqwest::Client,
    progress: &dyn ProgressSink,
    destination: &Path,
    manifest_url: &str,
) -> Result<Vec<u8>, ClientInstallError> {
    let mut response = client
        .get(manifest_url)
        .send()
        .await
        .map_err(ClientInstallError::Download)?
        .error_for_status()
        .map_err(ClientInstallError::Download)?;
    let total = response.content_length();
    let mut bytes = Vec::new();

    publish(
        progress,
        destination,
        "manifest",
        "Descargando el manifiesto de Riot",
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
        publish(
            progress,
            destination,
            "manifest",
            "Descargando el manifiesto de Riot",
            bytes.len() as u64,
            total,
            false,
        )?;
    }

    Ok(bytes)
}

fn read_rman_manifest(mut manifest: Vec<u8>) -> Result<Rman, ClientInstallError> {
    // Los manifiestos actuales de Riot declaran RMAN 2.1; cdragon-rman 0.2 solo acepta 2.0,
    // pero el diseño de tablas que se lee aquí es compatible. Las pruebas cubren este ajuste.
    if manifest.get(..6) == Some(b"RMAN\x02\x01") {
        manifest[5] = 0;
    }
    Rman::read(Cursor::new(manifest)).map_err(ClientInstallError::Rman)
}

/// cdragon-rman documenta que entra en pánico con datos corruptos; aquí se convierte en error.
fn parse_install_files(
    manifest: Vec<u8>,
    locale: &str,
) -> Result<Vec<InstallFile>, ClientInstallError> {
    panic::catch_unwind(AssertUnwindSafe(|| {
        let rman = read_rman_manifest(manifest)?;
        select_install_files(&rman, locale)
    }))
    .unwrap_or(Err(ClientInstallError::CorruptManifest))
}

fn select_install_files(rman: &Rman, locale: &str) -> Result<Vec<InstallFile>, ClientInstallError> {
    let locale_markers = rman
        .iter_flags()
        .filter_map(|flag| normalize_locale(flag.flag))
        .map(|flag| locale_asset_marker(&flag).to_string())
        .collect::<Vec<_>>();
    let marker = locale_asset_marker(locale);
    if !locale_markers.iter().any(|flag| flag == marker) {
        return Err(ClientInstallError::UnavailableLocale(locale.to_string()));
    }

    let directories = rman.dir_paths();
    let chunks = rman.bundle_chunks();
    let mut files = Vec::new();
    for file in rman.iter_files() {
        let path = match file.directory_id {
            Some(id) => {
                let directory = directories
                    .get(&id)
                    .ok_or(ClientInstallError::CorruptManifest)?;
                format!("{directory}/{}", file.name)
            }
            None => file.name.to_string(),
        };
        if !path_matches_locale(&path, marker, &locale_markers) {
            continue;
        }
        let file_chunks = file
            .iter_chunks()
            .map(|chunk_id| {
                chunks
                    .get(&chunk_id)
                    .cloned()
                    .ok_or(ClientInstallError::MissingChunk(chunk_id))
            })
            .collect::<Result<Vec<_>, _>>()?;
        files.push(InstallFile {
            path,
            filesize: file.filesize,
            chunks: file_chunks,
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

fn classify_install_files(
    destination: &Path,
    files: &[InstallFile],
) -> Result<ClassifiedInstall, ClientInstallError> {
    let mut pending_files = Vec::new();
    let mut reused_file_count = 0;
    let mut repaired_file_count = 0;

    for file in files {
        let target = resolve_install_path(destination, &file.path)?;
        match fs::symlink_metadata(&target) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(ClientInstallError::InvalidExistingTarget(target));
            }
            Ok(metadata) if metadata.len() == u64::from(file.filesize) => {
                reused_file_count += 1;
            }
            Ok(_) => {
                repaired_file_count += 1;
                pending_files.push(file.clone());
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                pending_files.push(file.clone());
            }
            Err(err) => return Err(ClientInstallError::Io(err)),
        }
    }

    let mode = if repaired_file_count > 0 {
        ClientWorkMode::Repair
    } else if pending_files.is_empty() {
        ClientWorkMode::Ready
    } else if reused_file_count > 0 {
        ClientWorkMode::Resume
    } else {
        ClientWorkMode::Install
    };

    Ok(ClassifiedInstall {
        mode,
        pending_files,
        reused_file_count,
        repaired_file_count,
    })
}

/// Cuántas peticiones de rango van en paralelo y el tamaño máximo de cada una.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadTuning {
    pub connections: usize,
    pub max_window: u32,
}

impl DownloadTuning {
    /// `connections` viene de `settings.json`; sin valor se usa el predeterminado.
    pub fn with_connections(connections: Option<u8>) -> Self {
        Self {
            connections: connections
                .map_or(DEFAULT_CONNECTIONS, usize::from)
                .clamp(1, MAX_CONNECTIONS),
            max_window: MAX_WINDOW_BYTES,
        }
    }
}

/// Estado compartido de una descarga: cliente HTTP, progreso acumulado y destino.
struct Downloader<'a> {
    client: &'a reqwest::Client,
    progress: &'a dyn ProgressSink,
    destination: &'a Path,
    bundle_base: &'a str,
    tuning: DownloadTuning,
    cancel: &'a CancelSignal,
    total_bytes: u64,
    downloaded_bytes: u64,
}

/// Posición de un archivo pendiente donde se escribe un fragmento descomprimido.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Placement {
    file_index: usize,
    target_offset: u64,
}

/// Fragmento único de un bundle: aunque varios archivos lo usen, se descarga una sola vez.
#[derive(Debug)]
struct WindowChunk {
    bundle_offset: u32,
    bundle_size: u32,
    target_size: u32,
    placements: Vec<Placement>,
}

/// Rango contiguo de un bundle que se pide en una sola petición HTTP.
#[derive(Debug)]
struct DownloadWindow {
    bundle_id: u64,
    start: u32,
    end: u32,
    chunks: Vec<WindowChunk>,
}

impl DownloadWindow {
    fn first_placement(&self) -> Option<Placement> {
        self.chunks
            .iter()
            .flat_map(|chunk| chunk.placements.iter().copied())
            .min()
    }
}

/// Agrupa los fragmentos pendientes por bundle, quita duplicados, fusiona los contiguos en
/// ventanas de hasta `max_window` bytes y las ordena por la primera escritura que producen,
/// para que cada archivo se escriba casi en orden.
fn plan_download_windows(
    files: &[InstallFile],
    max_window: u32,
) -> Result<Vec<DownloadWindow>, ClientInstallError> {
    let mut unique: HashMap<(u64, u32), WindowChunk> = HashMap::new();
    for (file_index, file) in files.iter().enumerate() {
        let mut target_offset = 0_u64;
        for chunk in &file.chunks {
            if chunk.bundle_size == 0 {
                return Err(ClientInstallError::CorruptManifest);
            }
            unique
                .entry((chunk.bundle_id, chunk.bundle_offset))
                .or_insert_with(|| WindowChunk {
                    bundle_offset: chunk.bundle_offset,
                    bundle_size: chunk.bundle_size,
                    target_size: chunk.target_size,
                    placements: Vec::new(),
                })
                .placements
                .push(Placement {
                    file_index,
                    target_offset,
                });
            target_offset += u64::from(chunk.target_size);
        }
    }

    let mut by_bundle: HashMap<u64, Vec<WindowChunk>> = HashMap::new();
    for ((bundle_id, _), chunk) in unique {
        by_bundle.entry(bundle_id).or_default().push(chunk);
    }

    let mut windows = Vec::new();
    for (bundle_id, mut chunks) in by_bundle {
        chunks.sort_by_key(|chunk| chunk.bundle_offset);
        let mut current: Option<DownloadWindow> = None;
        for chunk in chunks {
            let end = chunk
                .bundle_offset
                .checked_add(chunk.bundle_size)
                .ok_or(ClientInstallError::CorruptManifest)?;
            let extends = current.as_ref().is_some_and(|window| {
                window.end == chunk.bundle_offset && end - window.start <= max_window
            });
            if extends {
                let window = current.as_mut().expect("window in progress");
                window.end = end;
                window.chunks.push(chunk);
            } else {
                windows.extend(current.take());
                current = Some(DownloadWindow {
                    bundle_id,
                    start: chunk.bundle_offset,
                    end,
                    chunks: vec![chunk],
                });
            }
        }
        windows.extend(current);
    }
    windows.sort_by_cached_key(DownloadWindow::first_placement);
    Ok(windows)
}

/// Archivo que se arma en la carpeta temporal hasta que tiene todos sus bytes.
struct PendingFile {
    manifest_path: String,
    target: PathBuf,
    temp_path: PathBuf,
    handle: Option<File>,
    remaining: u64,
    committed: bool,
}

impl PendingFile {
    /// Escribe un fragmento en su posición; devuelve `true` cuando el archivo queda completo.
    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<bool, ClientInstallError> {
        if self.handle.is_none() {
            let handle = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.temp_path)?;
            self.handle = Some(handle);
        }
        let handle = self.handle.as_mut().expect("temp file is open");
        handle.seek(SeekFrom::Start(offset))?;
        handle.write_all(data)?;
        self.remaining = self
            .remaining
            .checked_sub(data.len() as u64)
            .ok_or_else(|| ClientInstallError::ChunkLayoutMismatch(self.manifest_path.clone()))?;
        if self.remaining == 0 {
            // Se cierra antes de renombrar: Windows no mueve archivos con identificadores abiertos.
            self.handle = None;
        }
        Ok(self.remaining == 0)
    }
}

/// Descomprime y escribe las ventanas en orden. Es dueño de los archivos temporales para
/// poder trabajar en un hilo bloqueante sin frenar las descargas en curso.
struct FileWriter {
    files: Vec<PendingFile>,
    temp_dir: PathBuf,
    allow_replace: bool,
}

impl FileWriter {
    fn new(
        destination: &Path,
        temp_dir: &Path,
        files: &[InstallFile],
        allow_replace: bool,
    ) -> Result<Self, ClientInstallError> {
        let mut pending = Vec::with_capacity(files.len());
        for (index, file) in files.iter().enumerate() {
            let temp_name = temp_name_for_manifest_path(&file.path);
            pending.push(PendingFile {
                manifest_path: file.path.clone(),
                target: resolve_install_path(destination, &file.path)?,
                temp_path: temp_dir.join(format!("{index:05}-{temp_name}.{TEMP_SUFFIX}")),
                handle: None,
                remaining: u64::from(file.filesize),
                committed: false,
            });
        }
        Ok(Self {
            files: pending,
            temp_dir: temp_dir.to_path_buf(),
            allow_replace,
        })
    }

    /// Escribe cada fragmento de la ventana en todos los archivos que lo usan y confirma los
    /// archivos que quedan completos, así una interrupción conserva lo terminado.
    fn apply(&mut self, window: &DownloadWindow, data: &[u8]) -> Result<(), ClientInstallError> {
        for chunk in &window.chunks {
            let start = (chunk.bundle_offset - window.start) as usize;
            let compressed = &data[start..start + chunk.bundle_size as usize];
            let decompressed = zstd::bulk::decompress(compressed, chunk.target_size as usize)
                .map_err(ClientInstallError::Zstd)?;
            if decompressed.len() != chunk.target_size as usize {
                return Err(ClientInstallError::InvalidChunkData {
                    expected: chunk.target_size,
                    actual: decompressed.len(),
                });
            }
            for placement in &chunk.placements {
                let file = &mut self.files[placement.file_index];
                if file.write_at(placement.target_offset, &decompressed)? {
                    commit_downloaded_file(
                        &file.temp_path,
                        &file.target,
                        &self.temp_dir,
                        self.allow_replace,
                    )?;
                    file.committed = true;
                }
            }
        }
        Ok(())
    }

    /// Confirma los archivos vacíos (sin fragmentos) y comprueba que ninguno quedó incompleto.
    fn finish(&mut self) -> Result<(), ClientInstallError> {
        for file in self.files.iter_mut().filter(|file| !file.committed) {
            if file.remaining != 0 {
                return Err(ClientInstallError::ChunkLayoutMismatch(
                    file.manifest_path.clone(),
                ));
            }
            file.handle = None;
            if !file.temp_path.exists() {
                File::create(&file.temp_path)?;
            }
            commit_downloaded_file(
                &file.temp_path,
                &file.target,
                &self.temp_dir,
                self.allow_replace,
            )?;
            file.committed = true;
        }
        Ok(())
    }

    /// Quita los temporales de archivos sin confirmar; los confirmados quedan para reanudar.
    fn discard_uncommitted(&mut self) {
        for file in self.files.iter_mut().filter(|file| !file.committed) {
            file.handle = None;
            let _ = fs::remove_file(&file.temp_path);
        }
    }
}

impl Downloader<'_> {
    /// Mantiene hasta `connections` peticiones en curso como tareas independientes y aplica
    /// cada ventana en orden en un hilo bloqueante. La memoria queda acotada a unas
    /// `connections + 1` ventanas.
    async fn run(
        &mut self,
        windows: Arc<Vec<DownloadWindow>>,
        writer: FileWriter,
    ) -> (Option<FileWriter>, Result<(), ClientInstallError>) {
        let mut writer = Some(writer);
        let mut in_flight = VecDeque::new();
        let mut next = 0;
        let failure = 'windows: {
            for index in 0..windows.len() {
                // Se revisa entre ventanas: lo ya aplicado queda y lo pendiente se descarta.
                if self.cancel.is_cancelled() {
                    break 'windows Some(ClientInstallError::Cancelled);
                }
                while next < windows.len() && in_flight.len() < self.tuning.connections {
                    let window = &windows[next];
                    let expected = window.end - window.start;
                    let request = self
                        .client
                        .get(format!(
                            "{}/{:016X}.bundle",
                            self.bundle_base, window.bundle_id
                        ))
                        .header(
                            header::RANGE,
                            format!("bytes={}-{}", window.start, window.end - 1),
                        );
                    in_flight.push_back(tauri::async_runtime::spawn(async move {
                        let data = request
                            .send()
                            .await
                            .map_err(ClientInstallError::Download)?
                            .error_for_status()
                            .map_err(ClientInstallError::Download)?
                            .bytes()
                            .await
                            .map_err(ClientInstallError::Download)?;
                        if data.len() != expected as usize {
                            return Err(ClientInstallError::InvalidChunkSize {
                                expected,
                                actual: data.len(),
                            });
                        }
                        Ok::<_, ClientInstallError>(data)
                    }));
                    next += 1;
                }

                let Some(fetch) = in_flight.pop_front() else {
                    break;
                };
                let data = match fetch.await {
                    Ok(Ok(data)) => data,
                    Ok(Err(error)) => break 'windows Some(error),
                    Err(error) => break 'windows Some(ClientInstallError::Worker(error)),
                };

                let mut owned = writer.take().expect("writer is available between windows");
                let shared = Arc::clone(&windows);
                let applied = tauri::async_runtime::spawn_blocking(move || {
                    let outcome = owned.apply(&shared[index], &data);
                    (owned, outcome)
                })
                .await;
                match applied {
                    Ok((returned, outcome)) => {
                        writer = Some(returned);
                        if let Err(error) = outcome {
                            break 'windows Some(error);
                        }
                    }
                    Err(error) => break 'windows Some(ClientInstallError::Worker(error)),
                }

                let window = &windows[index];
                self.downloaded_bytes += u64::from(window.end - window.start);
                let detail = window
                    .first_placement()
                    .and_then(|placement| writer.as_ref()?.files.get(placement.file_index))
                    .map(|file| file.manifest_path.clone())
                    .unwrap_or_default();
                if let Err(error) = publish(
                    self.progress,
                    self.destination,
                    "download",
                    &detail,
                    self.downloaded_bytes,
                    Some(self.total_bytes),
                    false,
                ) {
                    break 'windows Some(error);
                }
            }
            None
        };

        for fetch in &in_flight {
            fetch.abort();
        }
        match failure {
            Some(error) => (writer, Err(error)),
            None => (writer, Ok(())),
        }
    }
}

async fn download_files(
    downloader: &mut Downloader<'_>,
    files: &[InstallFile],
    allow_replace: bool,
) -> Result<(), ClientInstallError> {
    if files.is_empty() {
        return Ok(());
    }
    let windows = Arc::new(plan_download_windows(files, downloader.tuning.max_window)?);
    // Los temporales de una ejecución interrumpida ya se quitaron al confirmar el plan.
    let temp_dir = downloader.destination.join(METADATA_DIR).join(TEMP_DIR);
    fs::create_dir_all(&temp_dir)?;
    let writer = FileWriter::new(downloader.destination, &temp_dir, files, allow_replace)?;

    let (writer, outcome) = downloader.run(windows, writer).await;
    let Some(mut writer) = writer else {
        // Solo ocurre si falló el hilo de escritura; los `.part` se limpian en el próximo intento.
        return outcome;
    };
    let finished = outcome.and_then(|()| writer.finish());
    if finished.is_err() {
        writer.discard_uncommitted();
        return finished;
    }

    // Solo quita la carpeta temporal si quedó vacía; nunca borra archivos del usuario.
    let _ = fs::remove_dir(&temp_dir);
    Ok(())
}

/// Quita lo que dejó una ejecución interrumpida en la carpeta temporal de ChronoRift: los `.part`
/// a medio armar y los `.backup` de una reparación cortada entre sus dos renombres (el archivo
/// reemplazado, del tamaño equivocado). Solo corre al confirmar un plan; lo desconocido se queda.
fn remove_stale_temp_files(temp_dir: &Path) -> Result<(), ClientInstallError> {
    let entries = match fs::read_dir(temp_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let path = entry?.path();
        let stale = path
            .extension()
            .is_some_and(|extension| extension == TEMP_SUFFIX || extension == BACKUP_SUFFIX);
        if stale && path.is_file() {
            fs::remove_file(path)?;
        }
    }
    // Solo si quedó vacía.
    let _ = fs::remove_dir(temp_dir);
    Ok(())
}

fn commit_downloaded_file(
    temp_path: &Path,
    target: &Path,
    temp_dir: &Path,
    allow_replace: bool,
) -> Result<(), ClientInstallError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    if !target.exists() {
        fs::rename(temp_path, target)?;
        return Ok(());
    }
    if !allow_replace {
        return Err(ClientInstallError::WouldOverwrite(target.to_path_buf()));
    }

    let file_name = temp_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("client-file");
    let backup_path = temp_dir.join(format!("{file_name}.{BACKUP_SUFFIX}"));
    if backup_path.exists() {
        fs::remove_file(&backup_path)?;
    }
    fs::rename(target, &backup_path)?;
    if let Err(err) = fs::rename(temp_path, target) {
        let _ = fs::rename(&backup_path, target);
        return Err(ClientInstallError::Io(err));
    }
    fs::remove_file(backup_path)?;
    Ok(())
}

fn write_metadata(
    destination: &Path,
    name: &str,
    value: &impl Serialize,
) -> Result<(), ClientInstallError> {
    let metadata_dir = destination.join(METADATA_DIR);
    fs::create_dir_all(&metadata_dir)?;
    let mut file = File::create(metadata_dir.join(name))?;
    serde_json::to_writer_pretty(&mut file, value).map_err(ClientInstallError::Metadata)?;
    file.write_all(b"\n")?;
    Ok(())
}

/// Solo borra metadatos propios de ChronoRift dentro de la carpeta del cliente.
fn remove_metadata(destination: &Path, name: &str) -> Result<(), ClientInstallError> {
    match fs::remove_file(destination.join(METADATA_DIR).join(name)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

fn publish(
    progress: &dyn ProgressSink,
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
    progress.publish(ClientInstallProgress {
        destination_path: destination.display().to_string(),
        downloaded_bytes,
        total_bytes,
        percent,
        done,
        stage: stage.to_string(),
        detail: detail.to_string(),
    })
}

fn normalize_patch(patch: &str) -> Option<String> {
    let mut parts = patch.trim().split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    (!major.is_empty()
        && !minor.is_empty()
        && major.chars().all(|ch| ch.is_ascii_digit())
        && minor.chars().all(|ch| ch.is_ascii_digit()))
    .then(|| format!("{major}.{minor}"))
}

fn normalize_region(region: Option<&str>) -> String {
    region
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 8
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .map(str::to_ascii_uppercase)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Convierte la versión de la repetición (`16.18.817.5716`) en la del artefacto de Riot
/// (`16.18.8175716`). La revisión se rellena a 4 dígitos, como en todos los artefactos conocidos.
fn normalize_game_version(version: &str) -> Option<String> {
    let version = version.trim().split('+').next()?.trim();
    let parts = version.split('.').collect::<Vec<_>>();
    let normalized = if parts.len() >= 4 {
        format!("{}.{}.{}{:0>4}", parts[0], parts[1], parts[2], parts[3])
    } else {
        version.to_string()
    };
    is_release_version(&normalized).then_some(normalized)
}

/// Nombre de carpeta por compilación en el formato de la repetición (`16.18.817.5716`).
/// Solo admite dígitos y puntos, así que nunca puede salir de la carpeta de clientes.
fn client_folder_name(artifact_version: &str) -> Option<String> {
    if !is_release_version(artifact_version) {
        return None;
    }
    let parts = artifact_version.split('.').collect::<Vec<_>>();
    if parts.len() == 3 && parts[2].len() > 4 {
        let (build, revision) = parts[2].split_at(parts[2].len() - 4);
        return Some(format!("{}.{}.{build}.{revision}", parts[0], parts[1]));
    }
    Some(artifact_version.to_string())
}

fn is_release_version(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() >= 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
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
    use std::{
        future::Future,
        io::{BufRead, BufReader},
        net::{TcpListener, TcpStream},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn normalizes_versions_regions_and_client_folders() {
        assert_eq!(
            normalize_game_version("16.13.790.6961").as_deref(),
            Some("16.13.7906961")
        );
        assert_eq!(
            normalize_game_version("16.13.7906961+branch.releases-16-13").as_deref(),
            Some("16.13.7906961")
        );
        assert_eq!(
            normalize_game_version("16.18.817.716").as_deref(),
            Some("16.18.8170716")
        );
        assert_eq!(normalize_game_version("16.18.817/../x"), None);
        assert_eq!(normalize_game_version("english"), None);

        assert_eq!(
            client_folder_name("16.18.8175716").as_deref(),
            Some("16.18.817.5716")
        );
        assert_eq!(client_folder_name("16.18.8175716/../../escape"), None);
        assert_eq!(client_folder_name("16..8175716"), None);

        assert_eq!(normalize_region(Some(" la2 ")), "LA2");
        assert_eq!(normalize_region(Some("LA2/../x?q=1")), DEFAULT_REGION);
        assert_eq!(normalize_region(None), DEFAULT_REGION);
        assert_eq!(normalize_patch("16.18.817.5716").as_deref(), Some("16.18"));
        assert_eq!(normalize_patch("16."), None);
    }

    #[test]
    fn filters_files_for_locale_and_windows() {
        let markers = ["windows", "en_US", "es_MX", "fr_FR", "macos"]
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();

        assert!(path_matches_locale(GAME_EXE, "es_MX", &markers));
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
    fn rejects_unsafe_paths_and_manifests_outside_riot_cdn() {
        let root = Path::new("D:/Clients/16.13");
        assert!(resolve_install_path(root, "DATA/FINAL/file.wad").is_ok());
        assert!(resolve_install_path(root, "../escape.exe").is_err());
        assert!(resolve_install_path(root, "C:/escape.exe").is_err());
        assert!(resolve_install_path(root, "DATA\\escape.exe").is_err());

        let riot = ReleaseEndpoints::riot();
        let manifest =
            "https://lol.secure.dyn.riotcdn.net/channels/public/releases/344E097FEB027691.manifest";
        assert_eq!(validate_manifest_url(manifest, &riot).unwrap(), manifest);
        for untrusted in [
            "http://lol.secure.dyn.riotcdn.net/channels/public/releases/A.manifest",
            "https://riotcdn.net.evil.example/channels/public/releases/A.manifest",
            "https://lol.secure.dyn.riotcdn.net@evil.example/releases/A.manifest",
            "https://lol.secure.dyn.riotcdn.net/channels/public/releases/A.exe",
        ] {
            assert!(
                matches!(
                    validate_manifest_url(untrusted, &riot),
                    Err(ClientInstallError::UntrustedManifestUrl(_))
                ),
                "{untrusted}"
            );
        }
        assert_eq!(
            bundle_base_from_manifest_url(manifest).unwrap(),
            "https://lol.secure.dyn.riotcdn.net/channels/public/bundles"
        );
    }

    #[test]
    fn commits_a_replacement_only_when_repair_was_confirmed() {
        let root = test_directory("commit-repair");
        let temp_dir = root.join(".chronorift").join("tmp");
        fs::create_dir_all(&temp_dir).unwrap();
        let target = root.join("file.bin");
        let temp = temp_dir.join("file.bin");
        fs::write(&target, b"old").unwrap();
        fs::write(&temp, b"repaired").unwrap();

        let rejected = commit_downloaded_file(&temp, &target, &temp_dir, false);
        assert!(matches!(
            rejected,
            Err(ClientInstallError::WouldOverwrite(_))
        ));
        assert_eq!(fs::read(&target).unwrap(), b"old");

        commit_downloaded_file(&temp, &target, &temp_dir, true).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"repaired");
        assert!(!temp.exists());
        assert!(eventually_empty(&temp_dir));
        fs::remove_dir_all(root).unwrap();
    }

    /// Un fragmento compartido se pide una sola vez, los contiguos se fusionan hasta el tope de
    /// la ventana y el orden sigue a la primera escritura de cada archivo.
    #[test]
    fn plans_deduplicated_contiguous_windows_in_file_order() {
        let chunk = |bundle_id: u64, bundle_offset: u32, bundle_size: u32| BundleChunk {
            bundle_id,
            bundle_offset,
            bundle_size,
            target_size: bundle_size * 2,
        };
        let files = vec![
            InstallFile {
                path: "a.bin".to_string(),
                filesize: 120,
                chunks: vec![
                    chunk(2, 0, 10),
                    chunk(2, 10, 10),
                    chunk(2, 20, 10),
                    chunk(1, 0, 30),
                ],
            },
            InstallFile {
                path: "b.bin".to_string(),
                filesize: 40,
                chunks: vec![chunk(2, 0, 10), chunk(2, 40, 10)],
            },
        ];

        let windows = plan_download_windows(&files, 25).unwrap();

        let summary = windows
            .iter()
            .map(|window| {
                (
                    window.bundle_id,
                    window.start,
                    window.end,
                    window.chunks.len(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            summary,
            [(2, 0, 20, 2), (2, 20, 30, 1), (1, 0, 30, 1), (2, 40, 50, 1)]
        );
        assert_eq!(
            windows[0].chunks[0].placements,
            [
                Placement {
                    file_index: 0,
                    target_offset: 0
                },
                Placement {
                    file_index: 1,
                    target_offset: 0
                }
            ]
        );
        assert_eq!(install_totals(&files), (70, 160));
        assert_eq!(DownloadTuning::with_connections(None).connections, 8);
        assert_eq!(DownloadTuning::with_connections(Some(0)).connections, 1);
        assert_eq!(DownloadTuning::with_connections(Some(200)).connections, 32);

        let empty_chunk = [InstallFile {
            path: "broken.bin".to_string(),
            filesize: 0,
            chunks: vec![chunk(1, 0, 0)],
        }];
        assert!(matches!(
            plan_download_windows(&empty_chunk, 25),
            Err(ClientInstallError::CorruptManifest)
        ));
    }

    /// Flujo completo contra un CDN local: plan → instalación interrumpida → reanudación →
    /// reparación confirmada → listo, sin tocar archivos ajenos al manifiesto y quitando al
    /// confirmar los temporales que dejó un corte.
    #[test]
    fn e2e_installs_resumes_repairs_and_reuses_a_client_from_a_local_cdn() {
        let fixture = build_fixture();
        let cdn = TestCdn::start();
        let manifest_path = "/channels/public/releases/0000000000005EED.manifest";
        cdn.route(manifest_path, fixture.manifest.clone());
        for (bundle_id, bytes) in &fixture.bundles {
            cdn.route(
                &format!("/channels/public/bundles/{bundle_id:016X}.bundle"),
                bytes.clone(),
            );
        }
        // Sieve solo conoce el parche vigente; la compilación de la repetición sale del índice.
        cdn.route(
            "/api/v1/products/lol/version-sets/LA2",
            sieve_json(&[(
                "16.19.8217343",
                format!(
                    "{}/channels/public/releases/000000000000AAAA.manifest",
                    cdn.base
                ),
            )]),
        );
        cdn.route(
            "/index/LoL/LA2/windows/lol-game-client/16.18.8175716.txt",
            format!("{}{manifest_path}\n", cdn.base),
        );

        let root = test_directory("e2e-client");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("keep-me.txt"), b"unrelated").unwrap();
        let endpoints = cdn.endpoints();
        let progress = ProgressLog::default();
        let request = |confirmed: Option<&ClientInstallPlan>| ClientInstallRequest {
            patch: "16.18".to_string(),
            game_version: Some("16.18.817.5716".to_string()),
            region: Some("la2".to_string()),
            locale: "es_MX".to_string(),
            clients_root: root.display().to_string(),
            confirmed: confirmed.map(|plan| ConfirmedPlan {
                mode: plan.mode,
                resolved_version: plan.resolved_version.clone(),
                destination_path: plan.destination_path.clone(),
            }),
        };
        let plan = |request: ClientInstallRequest| {
            run(async {
                let client = http_client().unwrap();
                build_install_plan(&client, &progress, &endpoints, &request)
                    .await
                    .map(|planned| planned.plan)
            })
        };
        // Ventanas de un solo fragmento y 3 conexiones: varias peticiones en vuelo a la vez.
        let tuning = DownloadTuning {
            connections: 3,
            max_window: 1,
        };
        let never = CancelSignal::default();
        let install = |request: ClientInstallRequest| {
            run(install_client_with(
                &progress, &endpoints, request, tuning, &never,
            ))
        };
        let root_text = root.display().to_string();
        let destination = root.join("16.18.817.5716").join(GAME_FOLDER);
        let temp_dir = destination.join(METADATA_DIR).join("tmp");

        // 1. Plan inicial: fuente visible, destino por compilación y nada escrito en disco.
        let first = plan(request(None)).unwrap();
        assert_eq!(first.mode, ClientWorkMode::Install);
        assert_eq!(first.source, ReleaseSource::ManifestIndex);
        assert!(first.exact_build);
        assert_eq!(first.resolved_version, "16.18.8175716");
        assert_eq!(first.game_build, "16.18.817.5716");
        assert_eq!(first.destination_path, destination.display().to_string());
        assert_eq!(first.locale, "es_MX");
        assert_eq!((first.file_count, first.affected_file_count), (4, 4));
        assert_eq!(first.total_install_bytes, fixture.selected_bytes());
        // El fragmento compartido por dos archivos cuenta una sola vez.
        assert_eq!(first.download_bytes, fixture.download_bytes);
        assert!(!root.join("16.18.817.5716").exists());

        // 2. Sin confirmación no se escribe nada.
        assert!(matches!(
            install(request(None)),
            Err(ClientInstallError::ConfirmationRequired { .. })
        ));
        assert!(!destination.exists());

        // 3. Interrupción: el bundle B2 falla. Las ventanas se aplican en orden, así que el
        //    ejecutable y el archivo que comparte su primer fragmento quedan confirmados y el
        //    resto no, aunque otras peticiones estuvieran en vuelo.
        cdn.fail_paths_containing(Some("00000000000000B2.bundle"));
        let failed = install(request(Some(&first))).unwrap_err();
        assert!(matches!(failed, ClientInstallError::Download(_)));
        // El motivo de red llega en español, con el host.
        assert_eq!(
            failed.to_string(),
            "Falló la descarga: HTTP 500 (127.0.0.1)"
        );
        for path in [GAME_EXE, "DATA/FINAL/Shared.wad.client"] {
            assert_eq!(
                fs::read(destination.join(path)).unwrap(),
                fixture.content(path),
                "{path}"
            );
        }
        for path in [
            "DATA/FINAL/Champions/Aatrox.es_MX.wad.client",
            "DATA/FINAL/Bootstrap.windows.wad.client",
        ] {
            assert!(!destination.join(path).exists(), "{path}");
        }
        assert!(eventually_empty(&temp_dir));
        // El estado local ve la instalación a medias, con el total del plan confirmado.
        let partial = local_client_status(&root_text, "16.18.817.5716").unwrap();
        assert_eq!(partial.state, LocalClientState::Partial);
        assert_eq!(partial.total_install_bytes, Some(first.total_install_bytes));
        assert_eq!(
            partial.installed_bytes,
            (fixture.content(GAME_EXE).len()
                + fixture.content("DATA/FINAL/Shared.wad.client").len()) as u64
        );
        assert!(partial.executable_path.is_none());
        fs::write(destination.join("user-notes.txt"), b"mine").unwrap();

        // 4. Reanudación: reutiliza lo confirmado y pide solo los rangos que faltan, en paralelo.
        cdn.fail_paths_containing(None);
        let resume = plan(request(None)).unwrap();
        assert_eq!(resume.mode, ClientWorkMode::Resume);
        assert_eq!(
            (resume.reused_file_count, resume.affected_file_count),
            (2, 2)
        );
        progress.take();
        // Lo que el paso 3 dejó en vuelo no debe contarse como tráfico de la reanudación.
        cdn.wait_until_idle();
        cdn.reset_peak();
        let (requests_before, bytes_before) = cdn.bundle_traffic();
        let installed = install(request(Some(&resume))).unwrap();
        let (requests_after, bytes_after) = cdn.bundle_traffic();
        assert_eq!(requests_after - requests_before, 3);
        assert_eq!((bytes_after - bytes_before) as u64, resume.download_bytes);
        let peak = cdn.peak_bundle_requests();
        assert!(
            (2..=3).contains(&peak),
            "concurrent bundle requests: {peak}"
        );
        assert_eq!(
            installed.executable_path,
            destination.join(GAME_EXE).display().to_string()
        );
        for (path, content, _) in fixture.selected() {
            assert_eq!(
                &fs::read(destination.join(path)).unwrap(),
                content,
                "{path}"
            );
        }
        assert!(!destination
            .join("DATA/FINAL/Champions/Aatrox.fr_FR.wad.client")
            .exists());
        assert!(!destination
            .join("DATA/FINAL/Bootstrap.macos.wad.client")
            .exists());
        assert!(!temp_dir.exists() || eventually_empty(&temp_dir));
        let metadata: serde_json::Value = serde_json::from_slice(
            &fs::read(destination.join(METADATA_DIR).join(INSTALL_METADATA)).unwrap(),
        )
        .unwrap();
        assert_eq!(metadata["gameBuild"], "16.18.817.5716");
        assert_eq!(metadata["source"], "manifestIndex");
        assert!(!destination.join(METADATA_DIR).join(PLAN_METADATA).exists());
        let ready = local_client_status(&root_text, "16.18.817.5716").unwrap();
        assert_eq!(ready.state, LocalClientState::Ready);
        assert_eq!(
            ready.executable_path.as_deref(),
            Some(installed.executable_path.as_str())
        );
        assert_eq!(ready.locale.as_deref(), Some("es_MX"));
        assert_eq!(ready.total_install_bytes, Some(fixture.selected_bytes()));
        assert_eq!(list_local_clients(&root_text).unwrap(), vec![ready]);

        let events = progress.take();
        assert_eq!(
            events.first().map(|event| event.stage.as_str()),
            Some("resolve")
        );
        assert!(events.iter().any(|event| event.stage == "manifest"));
        let downloads = events
            .iter()
            .filter(|event| event.stage == "download")
            .collect::<Vec<_>>();
        assert_eq!(downloads.len(), 3);
        assert_eq!(
            downloads[0].detail,
            "DATA/FINAL/Champions/Aatrox.es_MX.wad.client"
        );
        assert!(downloads
            .windows(2)
            .all(|pair| pair[0].downloaded_bytes < pair[1].downloaded_bytes));
        assert!(installed.download_seconds >= 0.0);
        let last = events.last().unwrap();
        assert!(last.done && last.stage == "complete");
        assert_eq!(last.downloaded_bytes, resume.download_bytes);
        assert_eq!(last.total_bytes, Some(resume.download_bytes));
        assert!(events.iter().all(|event| event
            .destination_path
            .starts_with(&root.display().to_string())));

        // 5. Reparación: un archivo con otro tamaño exige confirmar exactamente la reparación.
        fs::write(destination.join(GAME_EXE), b"truncated").unwrap();
        let repair = plan(request(None)).unwrap();
        assert_eq!(repair.mode, ClientWorkMode::Repair);
        assert_eq!(repair.repaired_file_count, 1);
        assert!(matches!(
            install(request(Some(&resume))),
            Err(ClientInstallError::ConfirmationRequired { .. })
        ));
        assert_eq!(fs::read(destination.join(GAME_EXE)).unwrap(), b"truncated");
        let (_, bytes_before) = cdn.bundle_traffic();
        install(request(Some(&repair))).unwrap();
        let (_, bytes_after) = cdn.bundle_traffic();
        assert_eq!((bytes_after - bytes_before) as u64, repair.download_bytes);
        assert_eq!(
            fs::read(destination.join(GAME_EXE)).unwrap(),
            fixture.content(GAME_EXE)
        );

        // 6. Listo: nada que descargar ni peticiones de bundle. Un corte entre los dos renombres
        //    de una reparación deja un `.backup` huérfano: planificar no lo toca y confirmar el
        //    plan lo quita junto con los `.part` viejos.
        fs::create_dir_all(&temp_dir).unwrap();
        let orphans = [
            temp_dir.join("00000-League_of_Legends.exe.part.backup"),
            temp_dir.join("00003-DATA_FINAL_Bootstrap.windows.wad.client.part"),
        ];
        for orphan in &orphans {
            fs::write(orphan, b"interrupted").unwrap();
        }
        let ready = plan(request(None)).unwrap();
        assert_eq!(ready.mode, ClientWorkMode::Ready);
        assert_eq!(ready.download_bytes, 0);
        assert!(orphans.iter().all(|orphan| orphan.is_file()));
        let (requests_before, _) = cdn.bundle_traffic();
        install(request(Some(&ready))).unwrap();
        assert_eq!(cdn.bundle_traffic().0, requests_before);
        assert!(!temp_dir.exists() || eventually_empty(&temp_dir));

        // Los archivos ajenos al manifiesto siguen intactos.
        assert_eq!(fs::read(root.join("keep-me.txt")).unwrap(), b"unrelated");
        assert_eq!(
            fs::read(destination.join("user-notes.txt")).unwrap(),
            b"mine"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Detener a mitad de la descarga conserva lo confirmado, no deja temporales, deja el estado
    /// local en "incompleto" con el total del plan y la reanudación termina el cliente.
    #[test]
    fn e2e_stopping_keeps_committed_files_and_resuming_finishes_the_client() {
        let fixture = build_fixture();
        let cdn = TestCdn::start();
        let manifest_path = "/channels/public/releases/0000000000005EED.manifest";
        cdn.route(manifest_path, fixture.manifest.clone());
        for (bundle_id, bytes) in &fixture.bundles {
            cdn.route(
                &format!("/channels/public/bundles/{bundle_id:016X}.bundle"),
                bytes.clone(),
            );
        }
        cdn.route(
            "/api/v1/products/lol/version-sets/LA2",
            sieve_json(&[(
                "16.19.8217343",
                format!(
                    "{}/channels/public/releases/000000000000AAAA.manifest",
                    cdn.base
                ),
            )]),
        );
        cdn.route(
            "/index/LoL/LA2/windows/lol-game-client/16.18.8175716.txt",
            format!("{}{manifest_path}\n", cdn.base),
        );

        let root = test_directory("e2e-stop");
        fs::create_dir_all(&root).unwrap();
        let root_text = root.display().to_string();
        let endpoints = cdn.endpoints();
        let request = |confirmed: Option<&ClientInstallPlan>| ClientInstallRequest {
            patch: "16.18".to_string(),
            game_version: Some("16.18.817.5716".to_string()),
            region: Some("LA2".to_string()),
            locale: "es_MX".to_string(),
            clients_root: root_text.clone(),
            confirmed: confirmed.map(|plan| ConfirmedPlan {
                mode: plan.mode,
                resolved_version: plan.resolved_version.clone(),
                destination_path: plan.destination_path.clone(),
            }),
        };
        let plan = |request: ClientInstallRequest| {
            run(async {
                let client = http_client().unwrap();
                build_install_plan(&client, &ProgressLog::default(), &endpoints, &request)
                    .await
                    .map(|planned| planned.plan)
            })
        };
        // Una conexión y ventanas de un fragmento: la parada ocurre en un punto conocido.
        let tuning = DownloadTuning {
            connections: 1,
            max_window: 1,
        };
        let destination = root.join("16.18.817.5716").join(GAME_FOLDER);

        // Consultar el estado de una compilación ausente no crea carpetas.
        let missing = local_client_status(&root_text, "16.18.817.5716").unwrap();
        assert_eq!(missing.state, LocalClientState::Missing);
        assert!(!root.join("16.18.817.5716").exists());
        assert!(list_local_clients(&root_text).unwrap().is_empty());

        let first = plan(request(None)).unwrap();
        let cancel = CancelSignal::default();
        let stopped = run(install_client_with(
            &CancelOnDownload(cancel.clone()),
            &endpoints,
            request(Some(&first)),
            tuning,
            &cancel,
        ));
        assert!(matches!(stopped, Err(ClientInstallError::Cancelled)));
        let temp_dir = destination.join(METADATA_DIR).join("tmp");
        assert!(!temp_dir.exists() || eventually_empty(&temp_dir));

        let committed = fixture
            .selected()
            .filter(|(path, _, _)| destination.join(path).is_file())
            .map(|(_, content, _)| content.len() as u64)
            .sum::<u64>();
        let partial = local_client_status(&root_text, "16.18.817.5716").unwrap();
        assert_eq!(partial.state, LocalClientState::Partial);
        assert_eq!(partial.total_install_bytes, Some(first.total_install_bytes));
        assert_eq!(partial.installed_bytes, committed);
        assert!(committed < first.total_install_bytes);
        assert_eq!(list_local_clients(&root_text).unwrap(), vec![partial]);

        cancel.reset();
        let resume = plan(request(None)).unwrap();
        let expected = if committed > 0 {
            ClientWorkMode::Resume
        } else {
            ClientWorkMode::Install
        };
        assert_eq!(resume.mode, expected);
        run(install_client_with(
            &ProgressLog::default(),
            &endpoints,
            request(Some(&resume)),
            tuning,
            &cancel,
        ))
        .unwrap();
        let ready = local_client_status(&root_text, "16.18.817.5716").unwrap();
        assert_eq!(ready.state, LocalClientState::Ready);
        assert_eq!(ready.installed_bytes, fixture.selected_bytes());
        assert_eq!(ready.total_install_bytes, Some(fixture.selected_bytes()));
        for (path, content, _) in fixture.selected() {
            assert_eq!(
                &fs::read(destination.join(path)).unwrap(),
                content,
                "{path}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// Detiene la instalación en el primer evento de descarga, como el botón Detener.
    struct CancelOnDownload(CancelSignal);

    impl ProgressSink for CancelOnDownload {
        fn publish(&self, progress: ClientInstallProgress) -> Result<(), ClientInstallError> {
            if progress.stage == "download" {
                self.0.cancel();
            }
            Ok(())
        }
    }

    /// Resolución de compilaciones: Sieve exacto, índice (con región de respaldo), compilación
    /// cercana marcada como no exacta, URL fuera del CDN rechazada, redirecciones solo dentro del
    /// mismo origen (con el motivo en español), idioma mal escrito o ausente y versión inexistente.
    #[test]
    fn e2e_resolves_sieve_then_index_and_rejects_untrusted_manifests() {
        let fixture = build_fixture();
        let cdn = TestCdn::start();
        let sieve_manifest = "/channels/public/releases/00000000000016A9.manifest";
        let index_manifest = "/channels/public/releases/0000000000005EED.manifest";
        cdn.route(sieve_manifest, fixture.manifest.clone());
        cdn.route(index_manifest, fixture.manifest.clone());
        cdn.route(
            "/api/v1/products/lol/version-sets/LA2",
            sieve_json(&[
                (
                    "16.19.8207193",
                    format!(
                        "{}/channels/public/releases/0000000000000001.manifest",
                        cdn.base
                    ),
                ),
                ("16.19.8217343", format!("{}{sieve_manifest}", cdn.base)),
            ]),
        );
        cdn.route(
            "/index/LoL/NA1/windows/lol-game-client/16.18.8175716.txt",
            format!("{}{index_manifest}", cdn.base),
        );
        cdn.route(
            "/index/LoL/LA2/windows/lol-game-client/16.17.8104348.txt",
            "https://evil.example/channels/public/releases/X.manifest",
        );

        let root = test_directory("e2e-resolve");
        fs::create_dir_all(&root).unwrap();
        let endpoints = cdn.endpoints();
        let progress = ProgressLog::default();
        let plan_in = |patch: &str, version: &str, locale: &str| {
            run(async {
                let client = http_client().unwrap();
                let request = ClientInstallRequest {
                    patch: patch.to_string(),
                    game_version: Some(version.to_string()),
                    region: Some("LA2".to_string()),
                    locale: locale.to_string(),
                    clients_root: root.display().to_string(),
                    confirmed: None,
                };
                build_install_plan(&client, &progress, &endpoints, &request)
                    .await
                    .map(|planned| planned.plan)
            })
        };
        let plan_for = |patch: &str, version: &str| plan_in(patch, version, "es_MX");

        let sieve = plan_for("16.19", "16.19.821.7343").unwrap();
        assert_eq!(
            (sieve.source, sieve.exact_build),
            (ReleaseSource::Sieve, true)
        );
        assert_eq!(sieve.manifest_url, format!("{}{sieve_manifest}", cdn.base));
        assert_eq!(sieve.game_build, "16.19.821.7343");

        let indexed = plan_for("16.18", "16.18.817.5716").unwrap();
        assert_eq!(
            (indexed.source, indexed.exact_build),
            (ReleaseSource::ManifestIndex, true)
        );
        assert_eq!(
            indexed.manifest_url,
            format!("{}{index_manifest}", cdn.base)
        );

        let closest = plan_for("16.19", "16.19.999.9999").unwrap();
        assert_eq!(
            (
                closest.source,
                closest.exact_build,
                closest.resolved_version.as_str()
            ),
            (ReleaseSource::Sieve, false, "16.19.8217343")
        );

        assert!(matches!(
            plan_for("16.17", "16.17.810.4348"),
            Err(ClientInstallError::UntrustedManifestUrl(_))
        ));

        // Una redirección dentro del mismo origen se sigue; una hacia otro host, nunca: la URL
        // validada es la única de la que llegan bytes.
        let elsewhere = TestCdn::start();
        elsewhere.route(index_manifest, fixture.manifest.clone());
        let same_origin = "/channels/public/releases/000000000000CAFE.manifest";
        let other_origin = "/channels/public/releases/00000000000000FF.manifest";
        cdn.redirect(same_origin, format!("{}{index_manifest}", cdn.base));
        cdn.redirect(other_origin, format!("{}{index_manifest}", elsewhere.base));
        for (version, manifest) in [
            ("16.16.8001000", same_origin),
            ("16.15.7901000", other_origin),
        ] {
            cdn.route(
                &format!("/index/LoL/LA2/windows/lol-game-client/{version}.txt"),
                format!("{}{manifest}", cdn.base),
            );
        }
        let redirected = plan_for("16.16", "16.16.800.1000").unwrap();
        assert_eq!(redirected.file_count, indexed.file_count);
        let refused = plan_for("16.15", "16.15.790.1000").unwrap_err();
        assert!(matches!(refused, ClientInstallError::Download(_)));
        assert_eq!(
            refused.to_string(),
            format!(
                "Falló la descarga: redirección rechazada hacia {}",
                elsewhere.base
            )
        );
        assert_eq!(elsewhere.requests(), 0);

        // Un idioma mal escrito no es lo mismo que uno que este cliente no trae.
        assert!(matches!(
            plan_in("16.18", "16.18.817.5716", "japanese"),
            Err(ClientInstallError::InvalidLocale(_))
        ));
        let unavailable = plan_in("16.18", "16.18.817.5716", "ja_JP").unwrap_err();
        assert_eq!(
            unavailable.to_string(),
            "Idioma no disponible en este cliente: ja_JP"
        );

        assert!(matches!(
            plan_for("15.1", "15.1.600.1234"),
            Err(ClientInstallError::ReleaseNotFound { .. })
        ));
        assert!(
            fs::read_dir(&root).unwrap().next().is_none(),
            "planning must not write to disk"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Con un manifiesto real (CHRONORIFT_MANIFEST_SAMPLE), contrasta la selección por rutas con
    /// las banderas RMAN de cada archivo e informa los totales y las peticiones del plan.
    #[test]
    fn real_manifest_selection_matches_rman_flags() {
        let Ok(path) = std::env::var("CHRONORIFT_MANIFEST_SAMPLE") else {
            eprintln!("skipped: set CHRONORIFT_MANIFEST_SAMPLE to a Riot lol-game-client manifest");
            return;
        };
        let locale =
            std::env::var("CHRONORIFT_MANIFEST_LOCALE").unwrap_or_else(|_| "es_MX".to_string());
        let bytes = fs::read(path).expect("real manifest should read");
        let files = parse_install_files(bytes.clone(), &locale).expect("real manifest should plan");
        assert!(files.iter().any(|file| file.path == GAME_EXE));
        assert!(files.len() > 100);

        let rman = read_rman_manifest(bytes).unwrap();
        // En RMAN 2.1 los ids de bandera empiezan en 1 y la máscara usa el bit `id - 1`;
        // cdragon-rman 0.2 prueba el bit `id`, así que se le pasan ids desplazados.
        let flags = rman
            .iter_flags()
            .map(|flag| cdragon_rman::FileFlagEntry {
                id: flag.id.saturating_sub(1),
                flag: flag.flag,
            })
            .collect::<Vec<_>>();
        let directories = rman.dir_paths();
        let selected = files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<HashSet<_>>();
        let marker = locale_asset_marker(&locale);
        let mut missing = Vec::new();
        let mut extra = Vec::new();
        for file in rman.iter_files() {
            let path = file.path(&directories);
            let file_flags = file
                .flags
                .as_ref()
                .map(|set| set.iter(flags.iter()).collect::<Vec<_>>())
                .unwrap_or_default();
            let locales = file_flags
                .iter()
                .filter_map(|flag| normalize_locale(flag))
                .collect::<Vec<_>>();
            let platforms = file_flags
                .iter()
                .filter(|flag| PLATFORM_MARKERS.contains(flag))
                .collect::<Vec<_>>();
            let wanted = (locales.is_empty()
                || locales
                    .iter()
                    .any(|flag| locale_asset_marker(flag) == marker))
                && (platforms.is_empty() || platforms.iter().any(|flag| **flag == PLATFORM));
            match (wanted, selected.contains(path.as_str())) {
                (true, false) => missing.push(path),
                (false, true) => extra.push(path),
                _ => {}
            }
        }
        let (download_bytes, install_bytes) = install_totals(&files);
        let chunk_references = files.iter().map(|file| file.chunks.len()).sum::<usize>();
        let windows = plan_download_windows(&files, MAX_WINDOW_BYTES).unwrap();
        eprintln!(
            "{locale}: {} files, {chunk_references} chunk references, {} range requests, download {download_bytes} B, install {install_bytes} B, {} extra by path rule",
            files.len(),
            windows.len(),
            extra.len()
        );
        assert!(
            missing.is_empty(),
            "files flagged for {locale} but skipped: {missing:?}"
        );
    }

    /// Instalación real contra Riot, solo con CHRONORIFT_REAL_CLIENTS_ROOT y
    /// CHRONORIFT_REAL_GAME_VERSION. Muestra el plan, comprueba que es la compilación exacta dentro
    /// de la carpeta pedida y lo confirma tal cual. Si se interrumpe, la siguiente ejecución reanuda.
    #[test]
    #[ignore = "downloads a full client (~23 GiB); run with --ignored and CHRONORIFT_ALLOW_REAL_DOWNLOAD=1"]
    fn real_install_downloads_the_replay_build() {
        // Doble compuerta: `--ignored` y una autorización explícita, además del destino.
        if std::env::var("CHRONORIFT_ALLOW_REAL_DOWNLOAD").as_deref() != Ok("1") {
            eprintln!(
                "skipped: set CHRONORIFT_ALLOW_REAL_DOWNLOAD=1 to allow a real client download"
            );
            return;
        }
        let (Ok(root), Ok(version)) = (
            std::env::var("CHRONORIFT_REAL_CLIENTS_ROOT"),
            std::env::var("CHRONORIFT_REAL_GAME_VERSION"),
        ) else {
            eprintln!("skipped: set CHRONORIFT_REAL_CLIENTS_ROOT and CHRONORIFT_REAL_GAME_VERSION");
            return;
        };
        let region =
            std::env::var("CHRONORIFT_REAL_REGION").unwrap_or_else(|_| DEFAULT_REGION.to_string());
        let locale =
            std::env::var("CHRONORIFT_REAL_LOCALE").unwrap_or_else(|_| "en_US".to_string());
        let connections = std::env::var("CHRONORIFT_REAL_CONNECTIONS")
            .ok()
            .and_then(|value| value.parse().ok());
        let patch = version.split('.').take(2).collect::<Vec<_>>().join(".");
        let request = |confirmed: Option<ConfirmedPlan>| ClientInstallRequest {
            patch: patch.clone(),
            game_version: Some(version.clone()),
            region: Some(region.clone()),
            locale: locale.clone(),
            clients_root: root.clone(),
            confirmed,
        };
        let sink = ProgressPrinter::default();
        let endpoints = ReleaseEndpoints::riot();

        let plan = run(async {
            let client = http_client().unwrap();
            build_install_plan(&client, &sink, &endpoints, &request(None))
                .await
                .map(|planned| planned.plan)
        })
        .expect("the plan should resolve");
        eprintln!("plan: {}", serde_json::to_string_pretty(&plan).unwrap());
        // Límite de seguridad: solo la compilación exacta y solo dentro de la carpeta pedida.
        assert_eq!(
            Some(plan.resolved_version.as_str()),
            normalize_game_version(&version).as_deref()
        );
        assert!(plan.exact_build);
        assert!(Path::new(&plan.destination_path)
            .starts_with(settings::normalize_clients_root(&root).unwrap()));

        let confirmed = ConfirmedPlan {
            mode: plan.mode,
            resolved_version: plan.resolved_version.clone(),
            destination_path: plan.destination_path.clone(),
        };
        let started = Instant::now();
        let result = run(install_client_with(
            &sink,
            &endpoints,
            request(Some(confirmed)),
            DownloadTuning::with_connections(connections),
            &CancelSignal::default(),
        ))
        .expect("the install should finish");
        let seconds = result.download_seconds.max(0.001);
        eprintln!(
            "result: mode={} downloaded={} B, total {:.1} s, download phase {:.1} s, {:.1} MiB/s ({:.0} Mbit/s), executable={}",
            result.plan.mode,
            result.plan.download_bytes,
            started.elapsed().as_secs_f64(),
            result.download_seconds,
            result.plan.download_bytes as f64 / 1_048_576.0 / seconds,
            result.plan.download_bytes as f64 * 8.0 / 1_000_000.0 / seconds,
            result.executable_path
        );
        assert!(Path::new(&result.executable_path).is_file());
    }

    /// Imprime una línea por cambio de etapa y por cada punto porcentual, con el tiempo transcurrido.
    struct ProgressPrinter {
        started: Instant,
        last: Mutex<(String, i64)>,
    }

    impl Default for ProgressPrinter {
        fn default() -> Self {
            Self {
                started: Instant::now(),
                last: Mutex::new((String::new(), -1)),
            }
        }
    }

    impl ProgressSink for ProgressPrinter {
        fn publish(&self, progress: ClientInstallProgress) -> Result<(), ClientInstallError> {
            let percent = progress.percent.map_or(-1, |value| value.floor() as i64);
            let mut last = self.last.lock().unwrap();
            if last.0 != progress.stage || percent > last.1 || progress.done {
                eprintln!(
                    "[{:>8.1}s] {:<8} {:>4}% {} / {} B {}",
                    self.started.elapsed().as_secs_f64(),
                    progress.stage,
                    percent,
                    progress.downloaded_bytes,
                    progress.total_bytes.unwrap_or(0),
                    progress.detail
                );
                *last = (progress.stage, percent);
            }
            Ok(())
        }
    }

    fn run<F: Future>(future: F) -> F::Output {
        tauri::async_runtime::block_on(future)
    }

    /// En Windows un antivirus puede retener un archivo recién borrado: queda "pendiente de
    /// borrado" y sigue apareciendo en el listado un instante. Espera a que la carpeta se vacíe.
    fn eventually_empty(directory: &Path) -> bool {
        (0..50).any(|_| {
            let empty = fs::read_dir(directory)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(true);
            if !empty {
                thread::sleep(std::time::Duration::from_millis(40));
            }
            empty
        })
    }

    fn test_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("chronorift-{label}-{nonce}"))
    }

    #[derive(Default)]
    struct ProgressLog(Mutex<Vec<ClientInstallProgress>>);

    impl ProgressSink for ProgressLog {
        fn publish(&self, progress: ClientInstallProgress) -> Result<(), ClientInstallError> {
            self.0.lock().unwrap().push(progress);
            Ok(())
        }
    }

    impl ProgressLog {
        fn take(&self) -> Vec<ClientInstallProgress> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    fn sieve_json(releases: &[(&str, String)]) -> String {
        let releases = releases
            .iter()
            .map(|(version, url)| {
                serde_json::json!({
                    "release": { "labels": {
                        "riot:artifact_type_id": { "values": [GAME_ARTIFACT] },
                        "riot:artifact_version_id": { "values": [format!("{version}+branch.releases")] }
                    }},
                    "download": { "url": url }
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({ "releases": releases }).to_string()
    }

    #[derive(Default)]
    struct CdnState {
        routes: Mutex<HashMap<String, Vec<u8>>>,
        /// Ruta → `Location` de un 302.
        redirects: Mutex<HashMap<String, String>>,
        requests: AtomicUsize,
        failing_path: Mutex<Option<String>>,
        bundle_requests: AtomicUsize,
        bundle_bytes: AtomicUsize,
        active_bundle_requests: AtomicUsize,
        peak_bundle_requests: AtomicUsize,
        open_connections: AtomicUsize,
    }

    /// CDN HTTP mínimo sobre `TcpListener`: rutas fijas, peticiones Range y fallos inducidos.
    struct TestCdn {
        base: String,
        state: Arc<CdnState>,
    }

    impl TestCdn {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let state = Arc::new(CdnState::default());
            let cdn = Self {
                base: format!("http://{}", listener.local_addr().unwrap()),
                state: Arc::clone(&state),
            };
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let state = Arc::clone(&state);
                    state.open_connections.fetch_add(1, Ordering::SeqCst);
                    thread::spawn(move || {
                        serve_request(stream, &state);
                        state.open_connections.fetch_sub(1, Ordering::SeqCst);
                    });
                }
            });
            cdn
        }

        fn route(&self, path: &str, body: impl Into<Vec<u8>>) {
            self.state
                .routes
                .lock()
                .unwrap()
                .insert(path.to_string(), body.into());
        }

        fn redirect(&self, path: &str, location: String) {
            self.state
                .redirects
                .lock()
                .unwrap()
                .insert(path.to_string(), location);
        }

        /// Peticiones recibidas de cualquier tipo.
        fn requests(&self) -> usize {
            self.state.requests.load(Ordering::SeqCst)
        }

        fn fail_paths_containing(&self, fragment: Option<&str>) {
            *self.state.failing_path.lock().unwrap() = fragment.map(str::to_string);
        }

        fn reset_peak(&self) {
            self.state.peak_bundle_requests.store(0, Ordering::SeqCst);
        }

        /// Espera a que el servidor termine lo que dejó una instalación abortada: las peticiones
        /// ya enviadas se atienden y se cuentan aunque el cliente las haya cancelado.
        fn wait_until_idle(&self) {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut quiet_polls = 0;
            while quiet_polls < 5 {
                assert!(
                    Instant::now() < deadline,
                    "the test CDN did not become idle"
                );
                if self.state.open_connections.load(Ordering::SeqCst) == 0 {
                    quiet_polls += 1;
                } else {
                    quiet_polls = 0;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }

        /// Máximo de peticiones de bundle atendidas a la vez desde `reset_peak`.
        fn peak_bundle_requests(&self) -> usize {
            self.state.peak_bundle_requests.load(Ordering::SeqCst)
        }

        /// Peticiones de bundle recibidas y bytes de bundle servidos.
        fn bundle_traffic(&self) -> (usize, usize) {
            (
                self.state.bundle_requests.load(Ordering::SeqCst),
                self.state.bundle_bytes.load(Ordering::SeqCst),
            )
        }

        fn endpoints(&self) -> ReleaseEndpoints {
            ReleaseEndpoints {
                sieve_base: self.base.clone(),
                index_base: format!("{}/index", self.base),
                manifest_hosts: ManifestHosts::Origin(self.base.clone()),
            }
        }
    }

    fn serve_request(stream: TcpStream, state: &CdnState) {
        let Ok(reader_stream) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(reader_stream);
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() {
            return;
        }
        let target = request_line.split_whitespace().nth(1).unwrap_or("/");
        let path = target.split('?').next().unwrap_or("/").to_string();
        let mut range = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("range") {
                    range = value
                        .trim()
                        .strip_prefix("bytes=")
                        .and_then(|value| value.split_once('-'))
                        .and_then(|(from, to)| {
                            Some((from.parse::<usize>().ok()?, to.parse::<usize>().ok()?))
                        });
                }
            }
        }

        state.requests.fetch_add(1, Ordering::SeqCst);
        let redirect = state.redirects.lock().unwrap().get(&path).cloned();
        if let Some(location) = redirect {
            let mut stream = stream;
            let _ = write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.flush();
            return;
        }

        let is_bundle = path.contains("/bundles/");
        if is_bundle {
            // Respuesta lenta a propósito para que las peticiones concurrentes se solapen.
            let active = state.active_bundle_requests.fetch_add(1, Ordering::SeqCst) + 1;
            state
                .peak_bundle_requests
                .fetch_max(active, Ordering::SeqCst);
            thread::sleep(std::time::Duration::from_millis(40));
        }
        let body = state.routes.lock().unwrap().get(&path).cloned();
        let failing = state
            .failing_path
            .lock()
            .unwrap()
            .as_deref()
            .is_some_and(|fragment| path.contains(fragment));
        let (status, payload) = match body {
            Some(_) if failing => ("500 Internal Server Error", Vec::new()),
            Some(bytes) => match range {
                Some((from, to)) if from <= to && to < bytes.len() => {
                    ("206 Partial Content", bytes[from..=to].to_vec())
                }
                Some(_) => ("416 Range Not Satisfiable", Vec::new()),
                None => ("200 OK", bytes),
            },
            None => ("404 Not Found", Vec::new()),
        };
        if is_bundle {
            state.bundle_requests.fetch_add(1, Ordering::SeqCst);
            state
                .bundle_bytes
                .fetch_add(payload.len(), Ordering::SeqCst);
            state.active_bundle_requests.fetch_sub(1, Ordering::SeqCst);
        }
        let mut stream = stream;
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        let _ = stream.write_all(&payload);
        let _ = stream.flush();
    }

    struct Fixture {
        manifest: Vec<u8>,
        bundles: HashMap<u64, Vec<u8>>,
        /// Ruta del manifiesto, contenido y si se instala con es_MX en Windows.
        files: Vec<(String, Vec<u8>, bool)>,
        /// Bytes comprimidos únicos que necesita la selección es_MX + Windows.
        download_bytes: u64,
    }

    impl Fixture {
        fn content(&self, path: &str) -> Vec<u8> {
            self.files
                .iter()
                .find(|(candidate, _, _)| candidate == path)
                .map(|(_, content, _)| content.clone())
                .unwrap()
        }

        fn selected(&self) -> impl Iterator<Item = &(String, Vec<u8>, bool)> {
            self.files.iter().filter(|(_, _, selected)| *selected)
        }

        fn selected_bytes(&self) -> u64 {
            self.selected()
                .map(|(_, content, _)| content.len() as u64)
                .sum()
        }
    }

    /// Cliente sintético: ejecutable en 3 fragmentos, archivos por idioma y por plataforma en
    /// dos bundles comprimidos con zstd, y un archivo que reutiliza un fragmento del ejecutable.
    fn build_fixture() -> Fixture {
        const CHUNK: usize = 4_096;
        let specs: [(Option<u64>, &str, usize, u64, bool); 5] = [
            (None, GAME_EXE, 9_000, 0xA1, true),
            (Some(3), "Aatrox.es_MX.wad.client", 7_000, 0xB2, true),
            (Some(3), "Aatrox.fr_FR.wad.client", 5_000, 0xB2, false),
            (Some(2), "Bootstrap.windows.wad.client", 3_000, 0xA1, true),
            (Some(2), "Bootstrap.macos.wad.client", 2_000, 0xB2, false),
        ];
        let directories = [
            (1, None, "DATA"),
            (2, Some(1), "FINAL"),
            (3, Some(2), "Champions"),
        ];
        let mut bundles: HashMap<u64, Vec<u8>> = HashMap::new();
        let mut bundle_chunks: HashMap<u64, Vec<(u64, u32, u32)>> = HashMap::new();
        let mut compressed_sizes: HashMap<u64, u32> = HashMap::new();
        let mut selected_chunks: HashSet<u64> = HashSet::new();
        let mut manifest_files = Vec::new();
        let mut files = Vec::new();
        let mut next_chunk_id = 0x1000_u64;
        for (index, (directory, name, size, bundle_id, selected)) in specs.into_iter().enumerate() {
            let content = (0..size)
                .map(|offset| ((offset * 31 + index * 97) % 251) as u8)
                .collect::<Vec<_>>();
            let mut chunk_ids = Vec::new();
            for piece in content.chunks(CHUNK) {
                let compressed = zstd::bulk::compress(piece, 3).unwrap();
                bundles
                    .entry(bundle_id)
                    .or_default()
                    .extend_from_slice(&compressed);
                bundle_chunks.entry(bundle_id).or_default().push((
                    next_chunk_id,
                    compressed.len() as u32,
                    piece.len() as u32,
                ));
                compressed_sizes.insert(next_chunk_id, compressed.len() as u32);
                if selected {
                    selected_chunks.insert(next_chunk_id);
                }
                chunk_ids.push(next_chunk_id);
                next_chunk_id += 1;
            }
            let path = match directory {
                None => name.to_string(),
                Some(2) => format!("DATA/FINAL/{name}"),
                Some(_) => format!("DATA/FINAL/Champions/{name}"),
            };
            manifest_files.push((index as u64 + 1, directory, name, size as u32, chunk_ids));
            files.push((path, content, selected));
        }

        // Mismo contenido que el primer fragmento del ejecutable: Riot lo referencia por id.
        let shared_chunk = manifest_files[0].4[0];
        let shared_content = files[0].1[..CHUNK].to_vec();
        manifest_files.push((
            6,
            Some(2),
            "Shared.wad.client",
            CHUNK as u32,
            vec![shared_chunk],
        ));
        files.push((
            "DATA/FINAL/Shared.wad.client".to_string(),
            shared_content,
            true,
        ));

        let download_bytes = selected_chunks
            .iter()
            .map(|chunk_id| u64::from(compressed_sizes[chunk_id]))
            .sum();
        let mut bundle_list = bundle_chunks.into_iter().collect::<Vec<_>>();
        bundle_list.sort_by_key(|(bundle_id, _)| *bundle_id);
        let flags = [
            (1, "windows"),
            (2, "en_US"),
            (3, "es_MX"),
            (4, "fr_FR"),
            (5, "macos"),
        ];
        let manifest = encode_test_manifest(&flags, &directories, &bundle_list, &manifest_files);
        Fixture {
            manifest,
            bundles,
            files,
            download_bytes,
        }
    }

    enum TestField {
        Absent,
        U32(u32),
        U64(u64),
        Offset,
    }

    /// Escritor del cuerpo RMAN: tablas con vtable previa y desplazamientos relativos, que es
    /// exactamente lo que lee cdragon-rman.
    #[derive(Default)]
    struct BodyWriter {
        bytes: Vec<u8>,
    }

    impl BodyWriter {
        fn pos(&self) -> i32 {
            self.bytes.len() as i32
        }

        fn put(&mut self, data: &[u8]) {
            self.bytes.extend_from_slice(data);
        }

        fn put_u16(&mut self, value: u16) {
            self.put(&value.to_le_bytes());
        }

        fn put_i32(&mut self, value: i32) {
            self.put(&value.to_le_bytes());
        }

        fn put_u32(&mut self, value: u32) {
            self.put(&value.to_le_bytes());
        }

        fn put_u64(&mut self, value: u64) {
            self.put(&value.to_le_bytes());
        }

        fn reserve_offset(&mut self) -> i32 {
            let at = self.pos();
            self.put_i32(0);
            at
        }

        /// Enlaza el desplazamiento reservado en `at` (relativo a su posición) con `target`.
        fn link(&mut self, at: i32, target: i32) {
            let start = at as usize;
            self.bytes[start..start + 4].copy_from_slice(&(target - at).to_le_bytes());
        }

        fn put_string(&mut self, value: &str) -> i32 {
            let at = self.pos();
            self.put_i32(value.len() as i32);
            self.put(value.as_bytes());
            at
        }

        fn put_vector(&mut self, count: usize) -> Vec<i32> {
            self.put_u32(count as u32);
            (0..count).map(|_| self.reserve_offset()).collect()
        }

        /// Devuelve la posición de la tabla y la de cada campo.
        fn put_table(&mut self, fields: &[TestField]) -> (i32, Vec<i32>) {
            let mut layout = Vec::with_capacity(fields.len());
            let mut size = 4_u16;
            for field in fields {
                let width = match field {
                    TestField::Absent => 0,
                    TestField::U32(_) | TestField::Offset => 4,
                    TestField::U64(_) => 8,
                };
                layout.push(if width == 0 { 0 } else { size });
                size += width;
            }
            let vtable = self.pos();
            self.put_u16(4 + 2 * fields.len() as u16);
            self.put_u16(size);
            for offset in &layout {
                self.put_u16(*offset);
            }
            let table = self.pos();
            self.put_i32(table - vtable);
            let mut positions = Vec::with_capacity(fields.len());
            for field in fields {
                positions.push(self.pos());
                match field {
                    TestField::Absent => {}
                    TestField::U32(value) => self.put_u32(*value),
                    TestField::U64(value) => self.put_u64(*value),
                    TestField::Offset => self.put_i32(0),
                }
            }
            (table, positions)
        }
    }

    type ManifestBundle = (u64, Vec<(u64, u32, u32)>);
    type ManifestFile<'a> = (u64, Option<u64>, &'a str, u32, Vec<u64>);

    /// Codifica un RMAN 2.1 (la versión que publica Riot hoy) con cuerpo comprimido en zstd.
    fn encode_test_manifest(
        flags: &[(u8, &str)],
        directories: &[(u64, Option<u64>, &str)],
        bundles: &[ManifestBundle],
        files: &[ManifestFile<'_>],
    ) -> Vec<u8> {
        let mut body = BodyWriter::default();
        // Cabecera del cuerpo: la tabla raíz empieza en 4 y sus cuatro desplazamientos en 8.
        body.put_i32(4);
        body.put_i32(0);
        let root = (0..4).map(|_| body.reserve_offset()).collect::<Vec<_>>();

        let bundles_at = body.pos();
        body.link(root[0], bundles_at);
        let bundle_slots = body.put_vector(bundles.len());
        for ((bundle_id, chunks), slot) in bundles.iter().zip(bundle_slots) {
            let (table, fields) = body.put_table(&[TestField::U64(*bundle_id), TestField::Offset]);
            body.link(slot, table);
            let chunks_at = body.pos();
            body.link(fields[1], chunks_at);
            let chunk_slots = body.put_vector(chunks.len());
            for ((chunk_id, compressed, size), chunk_slot) in chunks.iter().zip(chunk_slots) {
                let (chunk_table, _) = body.put_table(&[
                    TestField::U64(*chunk_id),
                    TestField::U32(*compressed),
                    TestField::U32(*size),
                ]);
                body.link(chunk_slot, chunk_table);
            }
        }

        let flags_at = body.pos();
        body.link(root[1], flags_at);
        let flag_slots = body.put_vector(flags.len());
        for ((flag_id, name), slot) in flags.iter().zip(flag_slots) {
            let entry = body.pos();
            body.link(slot, entry);
            body.put_i32(0);
            body.put(&[0, 0, 0, *flag_id]);
            let name_slot = body.reserve_offset();
            let name_at = body.put_string(name);
            body.link(name_slot, name_at);
        }

        let files_at = body.pos();
        body.link(root[2], files_at);
        let file_slots = body.put_vector(files.len());
        for ((file_id, directory, name, size, chunk_ids), slot) in files.iter().zip(file_slots) {
            let fields = [
                TestField::U64(*file_id),
                directory.map_or(TestField::Absent, TestField::U64),
                TestField::U32(*size),
                TestField::Offset,
                TestField::Absent,
                TestField::Absent,
                TestField::Absent,
                TestField::Offset,
                TestField::Absent,
                TestField::Absent,
                TestField::Absent,
                TestField::Absent,
                TestField::Absent,
            ];
            let (table, positions) = body.put_table(&fields);
            body.link(slot, table);
            let name_at = body.put_string(name);
            body.link(positions[3], name_at);
            let chunks_at = body.pos();
            body.put_u32(chunk_ids.len() as u32);
            for chunk_id in chunk_ids {
                body.put_u64(*chunk_id);
            }
            body.link(positions[7], chunks_at);
        }

        let directories_at = body.pos();
        body.link(root[3], directories_at);
        let directory_slots = body.put_vector(directories.len());
        for ((directory_id, parent, name), slot) in directories.iter().zip(directory_slots) {
            let (table, positions) = body.put_table(&[
                TestField::U64(*directory_id),
                parent.map_or(TestField::Absent, TestField::U64),
                TestField::Offset,
            ]);
            body.link(slot, table);
            let name_at = body.put_string(name);
            body.link(positions[2], name_at);
        }

        let compressed = zstd::bulk::compress(&body.bytes, 3).unwrap();
        let mut manifest = b"RMAN\x02\x01".to_vec();
        manifest.extend_from_slice(&(1_u16 << 9).to_le_bytes());
        manifest.extend_from_slice(&28_u32.to_le_bytes());
        manifest.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        manifest.extend_from_slice(&0x5EED_u64.to_le_bytes());
        manifest.extend_from_slice(&(body.bytes.len() as u32).to_le_bytes());
        manifest.extend_from_slice(&compressed);
        manifest
    }
}
