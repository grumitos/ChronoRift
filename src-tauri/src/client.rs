use crate::settings;
use cdragon_rman::{BundleChunk, Rman};
use reqwest::{header, redirect, StatusCode, Url};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    error::Error,
    fmt::{self, Display},
    fs::{self, File, OpenOptions},
    future::Future,
    io::{self, Cursor, Seek, SeekFrom, Write},
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    pin::{pin, Pin},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    task::Poll,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Runtime};
// `tokio::time` llega con la feature `time` que reqwest y hyper-util ya activan en el árbol.
use tokio::time::{Interval, MissedTickBehavior};

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
/// Espera antes de cada reintento de una ventana con un fallo pasajero: un intento y cinco
/// reintentos, cerca de un minuto de tolerancia en total.
const RETRY_DELAYS: [Duration; 5] = [
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(32),
];
/// Sin tope total: una transferencia lenta pero viva sigue, y una que pasa este lapso sin recibir
/// bytes se da por colgada (la ventana se reintenta).
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Intervalo mínimo entre eventos de progreso de la descarga: unos cinco por segundo.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);
/// Cada cuánto se revisa la parada (y el progreso) mientras se espera a la red o al disco.
const TICK: Duration = Duration::from_millis(100);
/// Topes de lectura: un manifiesto RMAN real ronda las decenas de MiB; Sieve y el índice, pocos KiB.
const MAX_MANIFEST_BYTES: u64 = 128 * 1024 * 1024;
const MAX_LISTING_BYTES: u64 = 4 * 1024 * 1024;
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
/// Candado entre procesos mientras dura una instalación en ese destino.
const LOCK_FILE: &str = "install.lock";

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
    pub locale: String,
    pub manifest_url: String,
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

/// Estado compartido de las instalaciones del proceso: la señal para detener la que está en curso
/// y la marca de que hay una. Lo ya confirmado queda en disco para reanudar.
#[derive(Debug, Clone, Default)]
pub struct CancelSignal(Arc<InstallControl>);

#[derive(Debug, Default)]
struct InstallControl {
    cancelled: AtomicBool,
    running: AtomicBool,
}

impl CancelSignal {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
    }

    fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// Marca el comienzo de una instalación y limpia una parada vieja. Si ya hay otra en este
    /// proceso, la rechaza sin tocar su señal.
    fn begin(&self) -> Result<RunningInstall, ClientInstallError> {
        self.0
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| ClientInstallError::AlreadyRunning)?;
        self.0.cancelled.store(false, Ordering::SeqCst);
        Ok(RunningInstall(Arc::clone(&self.0)))
    }
}

/// Libera la marca de instalación en curso al terminar, también si falla o se detiene.
struct RunningInstall(Arc<InstallControl>);

impl Drop for RunningInstall {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::SeqCst);
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
    /// `false` si, a falta de la compilación exacta, es la más nueva lista del mismo parche.
    pub exact_build: bool,
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
    /// La unidad o el recurso compartido de la carpeta de clientes no existe (un disco USB
    /// desconectado, por ejemplo).
    ClientsRootUnavailable(String),
    InvalidReleaseVersion(String),
    InvalidManifestUrl(String),
    UntrustedManifestUrl(String),
    MissingBundleBase(String),
    HttpClient(reqwest::Error),
    Resolve(reqwest::Error),
    InvalidResolveResponse(serde_json::Error),
    ManifestIndex(reqwest::Error),
    /// Fallaron Riot Sieve y el índice: se informan los dos motivos.
    ResolveFailed {
        sieve: Box<ClientInstallError>,
        index: Box<ClientInstallError>,
    },
    /// La respuesta pasó el tope de lectura (manifiesto, Sieve o índice).
    ResponseTooLarge {
        host: String,
        limit: u64,
    },
    Metadata(serde_json::Error),
    ReleaseNotFound {
        region: String,
        patch: String,
        version: Option<String>,
    },
    Download(reqwest::Error),
    /// El CDN declaró otro tamaño para el rango pedido, o ninguno.
    InvalidChunkSize {
        expected: u32,
        actual: Option<u64>,
    },
    /// El CDN no respondió 206 al rango (por ejemplo, 200 con el bundle entero). Se reintenta.
    RangeIgnored(u16),
    /// La respuesta terminó antes de completar el rango. Se reintenta.
    IncompleteRange {
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
    /// La confirmación trae otro idioma que el plan recalculado.
    LocaleChanged {
        confirmed: String,
        planned: String,
    },
    /// La confirmación trae otro manifiesto que el plan recalculado.
    ManifestChanged(String),
    MissingExecutable(PathBuf),
    Emit(tauri::Error),
    Worker(tauri::Error),
    Cancelled,
    /// Ya hay una instalación en este proceso, o en otro sobre el mismo destino.
    AlreadyRunning,
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
            Self::ClientsRootUnavailable(path) => {
                write!(f, "La carpeta de clientes no está disponible: {path}")
            }
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
            Self::ResolveFailed { sieve, index } => write!(f, "{sieve}. {index}"),
            Self::ResponseTooLarge { host, limit } => write!(
                f,
                "Respuesta demasiado grande de {host}: supera {} MiB",
                limit / (1024 * 1024)
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
            Self::InvalidChunkSize {
                expected,
                actual: Some(actual),
            } => write!(
                f,
                "Rango de bundle con tamaño inesperado: {actual} bytes en lugar de {expected}"
            ),
            Self::InvalidChunkSize {
                expected,
                actual: None,
            } => write!(
                f,
                "Rango de bundle sin tamaño declarado; se esperaban {expected} bytes"
            ),
            Self::RangeIgnored(status) => {
                write!(f, "El CDN no respetó el rango pedido: HTTP {status}")
            }
            Self::IncompleteRange { expected, actual } => write!(
                f,
                "Rango de bundle incompleto: {actual} de {expected} bytes"
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
            Self::LocaleChanged { confirmed, planned } => write!(
                f,
                "El plan cambió de idioma: {planned} en lugar de {confirmed}"
            ),
            Self::ManifestChanged(url) => write!(f, "El plan cambió de manifiesto: {url}"),
            Self::MissingExecutable(path) => {
                write!(f, "Falta el ejecutable: {}", path.display())
            }
            Self::Emit(err) => write!(f, "No se pudo publicar el progreso: {err}"),
            Self::Worker(err) => write!(f, "Falló una tarea de la descarga: {err}"),
            Self::Cancelled => write!(f, "Descarga detenida"),
            Self::AlreadyRunning => write!(f, "Ya hay una descarga en curso."),
        }
    }
}

impl Error for ClientInstallError {}

impl ClientInstallError {
    /// Fallos pasajeros que vale la pena reintentar: tiempo agotado, conexión caída o rechazada,
    /// 5xx, 429 y cuerpos incompletos. Nunca los de validación (tamaño, contenido, URL no
    /// confiable) ni los demás 4xx.
    fn is_transient(&self) -> bool {
        match self {
            Self::Download(error) => is_transient_network(error),
            Self::RangeIgnored(_) | Self::IncompleteRange { .. } => true,
            _ => false,
        }
    }
}

fn is_transient_network(error: &reqwest::Error) -> bool {
    if error.is_redirect() || error.is_builder() {
        return false;
    }
    if let Some(status) = error.status() {
        return status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS;
    }
    // `is_request` cubre una conexión cortada o reiniciada antes de la respuesta.
    error.is_timeout()
        || error.is_connect()
        || error.is_request()
        || error.is_body()
        || error.is_decode()
}

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

/// Estado local de las compilaciones de las repeticiones, por versión, leído sin red y sin crear
/// nada. Si la compilación exacta no tiene un cliente listo, vale el más nuevo listo del mismo
/// parche (`exact_build: false`), que es lo que instala un plan no exacto. Una versión mal escrita
/// se omite; solo una unidad inexistente rechaza la consulta entera.
pub fn local_client_statuses(
    clients_root: &str,
    versions: &[String],
) -> Result<HashMap<String, LocalClientStatus>, ClientInstallError> {
    let root = match clients_root_path(clients_root) {
        Ok(root) => root,
        Err(error @ ClientInstallError::ClientsRootUnavailable(_)) => return Err(error),
        // Sin carpeta válida no hay clientes que mostrar.
        Err(_) => return Ok(HashMap::new()),
    };
    // Los clientes listos de la carpeta se leen una sola vez, y solo si hace falta un respaldo.
    let mut ready: Option<Vec<LocalClientStatus>> = None;
    let mut statuses = HashMap::new();
    for version in versions {
        let Some(game_build) =
            normalize_game_version(version).and_then(|artifact| client_folder_name(&artifact))
        else {
            continue;
        };
        let exact = status_for_destination(root.join(&game_build).join(GAME_FOLDER), game_build);
        let status = if exact.state == LocalClientState::Ready {
            exact
        } else {
            let patch = version_patch(&exact.game_build);
            ready
                .get_or_insert_with(|| {
                    scan_clients(&root)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|status| status.state == LocalClientState::Ready)
                        .collect()
                })
                .iter()
                // Ya vienen de la más nueva a la más vieja.
                .find(|status| version_patch(&status.game_build) == patch)
                .map(|status| LocalClientStatus {
                    exact_build: false,
                    ..status.clone()
                })
                .unwrap_or(exact)
        };
        statuses.insert(version.clone(), status);
    }
    Ok(statuses)
}

/// Compilaciones con archivos en la carpeta de clientes, de la más nueva a la más vieja.
pub fn list_local_clients(
    clients_root: &str,
) -> Result<Vec<LocalClientStatus>, ClientInstallError> {
    let root = clients_root_path(clients_root)?;
    scan_clients(&root).map_err(ClientInstallError::Io)
}

/// Carpeta de clientes validada; una unidad o un recurso compartido que no existe tiene su propio
/// error. Una carpeta que falta en una unidad existente es válida: todavía no hay nada instalado.
fn clients_root_path(clients_root: &str) -> Result<PathBuf, ClientInstallError> {
    settings::normalize_clients_root(clients_root).map_err(|error| match error {
        settings::SettingsError::MissingVolume(path) => {
            ClientInstallError::ClientsRootUnavailable(path)
        }
        other => ClientInstallError::InvalidClientsRoot(other),
    })
}

fn scan_clients(root: &Path) -> io::Result<Vec<LocalClientStatus>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
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

fn read_stored(metadata_dir: &Path, name: &str) -> Option<StoredInstall> {
    let bytes = fs::read(metadata_dir.join(name)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn status_for_destination(destination: PathBuf, game_build: String) -> LocalClientStatus {
    let metadata_dir = destination.join(METADATA_DIR);
    let installed = read_stored(&metadata_dir, INSTALL_METADATA);
    let planned = read_stored(&metadata_dir, PLAN_METADATA);
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
        exact_build: true,
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
    // Una instalación por proceso; también limpia una parada que haya quedado de la anterior.
    let _running = cancel.begin()?;
    let client = http_client_with(tuning.read_timeout)?;
    // Detener también corta el plan recalculado (Sieve, índice y manifiesto).
    let planned = until_cancelled(
        cancel,
        build_install_plan(&client, progress, endpoints, &request),
    )
    .await?;
    validate_confirmation(&planned.plan, request.confirmed.as_ref())?;
    if cancel.is_cancelled() {
        return Err(ClientInstallError::Cancelled);
    }
    let bundle_base = bundle_base_from_manifest_url(&planned.plan.manifest_url)?;
    // Candado entre procesos antes de tocar el destino: la limpieza de temporales nunca alcanza
    // los archivos de otra instalación en curso.
    let _lock = InstallLock::acquire(&planned.destination)?;
    // Recién con el plan confirmado, y también si ya está listo: un `.backup` de una reparación
    // cortada puede quedar aunque el archivo ya esté completo.
    remove_stale_temp_files(&planned.destination.join(METADATA_DIR).join(TEMP_DIR))?;
    if planned.plan.mode != ClientWorkMode::Ready {
        // El plan confirmado da el total si la descarga queda a medias. `install.json` sigue
        // hasta que se confirma el primer archivo y vuelve a escribirse con el cliente completo.
        write_metadata(&planned.destination, PLAN_METADATA, &planned.plan)?;
    }

    let mut downloader = Downloader {
        client: &client,
        progress,
        destination: &planned.destination,
        bundle_base: &bundle_base,
        tuning,
        cancel,
        total_bytes: planned.plan.download_bytes,
        received: Arc::default(),
        reported_bytes: 0,
        last_report: None,
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
    // Un plan listo no descargó nada: el `install.json` que ya había conserva los datos de la
    // descarga que armó el cliente.
    let keep_previous = result.plan.mode == ClientWorkMode::Ready
        && read_stored(&planned.destination.join(METADATA_DIR), INSTALL_METADATA).is_some();
    if !keep_previous {
        write_metadata(&planned.destination, INSTALL_METADATA, &result)?;
    }
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
    http_client_with(READ_TIMEOUT)
}

/// Sin tope total: una descarga lenta pero viva nunca se corta. `read_timeout` limita la espera
/// de los encabezados y cada pausa entre bytes del cuerpo.
fn http_client_with(read_timeout: Duration) -> Result<reqwest::Client, ClientInstallError> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(same_origin_redirects())
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(read_timeout)
        .build()
        .map_err(ClientInstallError::HttpClient)
}

/// Candado entre procesos (otra instancia de la app): `install.lock` abierto sin compartir mientras
/// dura la instalación, y borrado al terminar. Recargar la ventana no cambia de proceso: a esa
/// segunda llamada la frena la guardia compartida de `CancelSignal`.
struct InstallLock {
    path: PathBuf,
    file: Option<File>,
}

impl InstallLock {
    fn acquire(destination: &Path) -> Result<Self, ClientInstallError> {
        let metadata_dir = destination.join(METADATA_DIR);
        fs::create_dir_all(&metadata_dir)?;
        let path = metadata_dir.join(LOCK_FILE);
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        match options.open(&path) {
            Ok(file) => Ok(Self {
                path,
                file: Some(file),
            }),
            // ERROR_SHARING_VIOLATION: otro proceso lo tiene abierto.
            Err(error) if cfg!(windows) && error.raw_os_error() == Some(32) => {
                Err(ClientInstallError::AlreadyRunning)
            }
            Err(error) => Err(error.into()),
        }
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        // Se cierra antes de borrarlo. Si otra instalación ya lo abrió, el borrado falla y su
        // candado sigue en pie.
        self.file = None;
        let _ = fs::remove_file(&self.path);
    }
}

enum Waited<T> {
    Ready(T),
    Tick,
}

/// Espera `future` hasta el próximo tic de `ticker`, lo que llegue primero; así el que espera
/// puede revisar la parada y publicar progreso sin `tokio::select!` (sin la feature `macros`).
async fn wait_or_tick<F: Future + Unpin>(
    future: &mut F,
    ticker: &mut Interval,
) -> Waited<F::Output> {
    std::future::poll_fn(|cx| {
        if let Poll::Ready(output) = Pin::new(&mut *future).poll(cx) {
            return Poll::Ready(Waited::Ready(output));
        }
        ticker.poll_tick(cx).map(|_| Waited::Tick)
    })
    .await
}

fn ticker() -> Interval {
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker
}

/// Corta `future` (y con él sus peticiones) si se pide detener la instalación.
async fn until_cancelled<T>(
    cancel: &CancelSignal,
    future: impl Future<Output = Result<T, ClientInstallError>>,
) -> Result<T, ClientInstallError> {
    let mut future = pin!(future);
    let mut ticker = ticker();
    loop {
        match wait_or_tick(&mut future, &mut ticker).await {
            Waited::Ready(result) => return result,
            Waited::Tick if cancel.is_cancelled() => return Err(ClientInstallError::Cancelled),
            Waited::Tick => {}
        }
    }
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
    let clients_root = clients_root_path(&request.clients_root)?;
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

/// El plan recalculado debe ser el que el usuario revisó: modo, versión, destino, idioma y
/// manifiesto.
fn validate_confirmation(
    plan: &ClientInstallPlan,
    confirmed: Option<&ConfirmedPlan>,
) -> Result<(), ClientInstallError> {
    let Some(confirmed) = confirmed.filter(|confirmed| {
        confirmed.mode == plan.mode
            && confirmed.resolved_version == plan.resolved_version
            && confirmed.destination_path == plan.destination_path
    }) else {
        return Err(ClientInstallError::ConfirmationRequired {
            planned: plan.mode,
            version: plan.game_build.clone(),
            destination: plan.destination_path.clone(),
        });
    };
    if normalize_locale(&confirmed.locale).as_deref() != Some(plan.locale.as_str()) {
        return Err(ClientInstallError::LocaleChanged {
            confirmed: confirmed.locale.clone(),
            planned: plan.locale.clone(),
        });
    }
    if confirmed.manifest_url != plan.manifest_url {
        return Err(ClientInstallError::ManifestChanged(
            plan.manifest_url.clone(),
        ));
    }
    Ok(())
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
/// compilación más reciente del mismo parche que liste Sieve, marcada como no exacta. Solo un
/// "no está" del índice lleva a esa compilación no exacta: cualquier otro fallo se informa.
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
            Err(index) => {
                return Err(match sieve_error {
                    Some(sieve) => ClientInstallError::ResolveFailed {
                        sieve: Box::new(sieve),
                        index: Box::new(index),
                    },
                    None => index,
                })
            }
        }
    }

    if let Some(release) = select_sieve_release(&sieve_releases, patch, target_version) {
        return with_trusted_manifest(release, endpoints);
    }

    Err(
        sieve_error.unwrap_or_else(|| ClientInstallError::ReleaseNotFound {
            region: region.to_string(),
            patch: patch.to_string(),
            version: target_version.map(ToOwned::to_owned),
        }),
    )
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
    let mut response = client
        .get(&url)
        .send()
        .await
        .map_err(ClientInstallError::Resolve)?
        .error_for_status()
        .map_err(ClientInstallError::Resolve)?;
    let body = read_capped(
        &mut response,
        MAX_LISTING_BYTES,
        ClientInstallError::Resolve,
        |_| Ok(()),
    )
    .await?;
    let response: SieveResponse =
        serde_json::from_slice(&body).map_err(ClientInstallError::InvalidResolveResponse)?;
    Ok(response.releases)
}

/// Lee el cuerpo por partes y corta en cuanto pasa de `limit`, con o sin `Content-Length`.
/// `on_chunk` recibe los bytes leídos hasta el momento.
async fn read_capped(
    response: &mut reqwest::Response,
    limit: u64,
    network: fn(reqwest::Error) -> ClientInstallError,
    mut on_chunk: impl FnMut(u64) -> Result<(), ClientInstallError>,
) -> Result<Vec<u8>, ClientInstallError> {
    let too_large = |response: &reqwest::Response| ClientInstallError::ResponseTooLarge {
        host: response.url().host_str().unwrap_or_default().to_string(),
        limit,
    };
    let declared = response.content_length();
    if declared.is_some_and(|length| length > limit) {
        return Err(too_large(response));
    }
    let mut bytes = Vec::with_capacity(declared.unwrap_or(0) as usize);
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        if (bytes.len() + chunk.len()) as u64 > limit {
            return Err(too_large(response));
        }
        bytes.extend_from_slice(&chunk);
        on_chunk(bytes.len() as u64)?;
    }
    Ok(bytes)
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
/// manifiesto en el CDN de Riot, en la región de la repetición y luego en las de respaldo; un fallo
/// (red, 5xx, 429, URL no confiable) no detiene la búsqueda en las demás. Devuelve `None` si
/// ninguna la tiene (404) y el primer fallo si ninguna respondió con una URL.
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

    let mut first_error = None;
    for candidate in regions {
        match index_entry(client, endpoints, candidate, version).await {
            Ok(Some(manifest_url)) => return Ok(Some(manifest_url)),
            Ok(None) => {}
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    first_error.map_or(Ok(None), Err)
}

/// Entrada del índice para una región; `None` si no existe (404).
async fn index_entry(
    client: &reqwest::Client,
    endpoints: &ReleaseEndpoints,
    region: &str,
    version: &str,
) -> Result<Option<String>, ClientInstallError> {
    let url = format!(
        "{}/LoL/{region}/{PLATFORM}/{GAME_ARTIFACT}/{version}.txt",
        endpoints.index_base
    );
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(ClientInstallError::ManifestIndex)?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let mut response = response
        .error_for_status()
        .map_err(ClientInstallError::ManifestIndex)?;
    let body = read_capped(
        &mut response,
        MAX_LISTING_BYTES,
        ClientInstallError::ManifestIndex,
        |_| Ok(()),
    )
    .await?;
    validate_manifest_url(String::from_utf8_lossy(&body).trim(), endpoints).map(Some)
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
    let report = |read: u64| {
        publish(
            progress,
            destination,
            "manifest",
            "Descargando el manifiesto de Riot",
            read,
            total,
            false,
        )
    };

    report(0)?;
    let mut last_report = Instant::now();
    read_capped(
        &mut response,
        MAX_MANIFEST_BYTES,
        ClientInstallError::Download,
        |read| {
            if last_report.elapsed() < PROGRESS_INTERVAL {
                return Ok(());
            }
            last_report = Instant::now();
            report(read)
        },
    )
    .await
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

/// Cuántas peticiones de rango van en paralelo, el tamaño máximo de cada una, cómo se reintentan y
/// cada cuánto se publica el progreso. Las pruebas usan esperas de milisegundos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadTuning {
    pub connections: usize,
    pub max_window: u32,
    /// Espera antes de cada reintento de una ventana; su largo es la cantidad de reintentos.
    pub retry_delays: &'static [Duration],
    /// Sin bytes durante este lapso, la petición se da por colgada.
    pub read_timeout: Duration,
    /// Intervalo mínimo entre eventos de progreso de la descarga.
    pub progress_interval: Duration,
}

impl DownloadTuning {
    /// `connections` viene de `settings.json`; sin valor se usa el predeterminado.
    pub fn with_connections(connections: Option<u8>) -> Self {
        Self {
            connections: connections
                .map_or(DEFAULT_CONNECTIONS, usize::from)
                .clamp(1, MAX_CONNECTIONS),
            max_window: MAX_WINDOW_BYTES,
            retry_delays: &RETRY_DELAYS,
            read_timeout: READ_TIMEOUT,
            progress_interval: PROGRESS_INTERVAL,
        }
    }
}

/// Estado compartido de una descarga: cliente HTTP, progreso y destino.
struct Downloader<'a> {
    client: &'a reqwest::Client,
    progress: &'a dyn ProgressSink,
    destination: &'a Path,
    bundle_base: &'a str,
    tuning: DownloadTuning,
    cancel: &'a CancelSignal,
    total_bytes: u64,
    /// Bytes de bundle recibidos por las peticiones en curso o terminadas; un intento fallido
    /// descuenta los suyos.
    received: Arc<AtomicU64>,
    /// Último valor publicado: nunca retrocede, aunque un reintento descarte bytes.
    reported_bytes: u64,
    last_report: Option<Instant>,
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
            mark_sparse(&handle);
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

/// Marca el `.part` como disperso antes de su primera escritura. Los fragmentos llegan en orden de
/// bundle y, en un archivo normal, NTFS rellena con ceros de forma síncrona todo lo que queda antes
/// de una escritura lejana. Medido en NTFS, 1 MiB escrito a 2 GiB tarda unos 2 s y reserva 2 GiB;
/// en un archivo disperso es inmediato y reserva 1 MiB. Ese relleno frenaba al único escritor y,
/// con él, a todas las ventanas en vuelo. La marca queda al confirmar el archivo: quitarla obliga a
/// volcar en el acto los tramos que se llenaron tarde. Si el sistema de archivos no la admite, se
/// sigue sin ella.
#[cfg(windows)]
fn mark_sparse(file: &File) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::{
        Ioctl::{FILE_SET_SPARSE_BUFFER, FSCTL_SET_SPARSE},
        IO::DeviceIoControl,
    };

    let request = FILE_SET_SPARSE_BUFFER { SetSparse: true };
    let mut returned = 0;
    // SAFETY: el identificador es válido mientras `file` viva, el búfer de entrada es un
    // `FILE_SET_SPARSE_BUFFER` que vive durante la llamada, no hay búfer de salida y la E/S no es
    // superpuesta.
    unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            (&request as *const FILE_SET_SPARSE_BUFFER).cast(),
            std::mem::size_of::<FILE_SET_SPARSE_BUFFER>() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        );
    }
}

#[cfg(not(windows))]
fn mark_sparse(_file: &File) {}

/// Descomprime y escribe las ventanas en orden. Es dueño de los archivos temporales para
/// poder trabajar en un hilo bloqueante sin frenar las descargas en curso.
struct FileWriter {
    files: Vec<PendingFile>,
    temp_dir: PathBuf,
    allow_replace: bool,
    /// `install.json` del cliente anterior: deja de describirlo con el primer archivo que se
    /// agrega o reemplaza, así que se quita justo antes.
    stale_install_metadata: Option<PathBuf>,
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
            stale_install_metadata: Some(destination.join(METADATA_DIR).join(INSTALL_METADATA)),
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
                    self.commit(placement.file_index)?;
                }
            }
        }
        Ok(())
    }

    /// Confirma los archivos vacíos (sin fragmentos) y comprueba que ninguno quedó incompleto.
    fn finish(&mut self) -> Result<(), ClientInstallError> {
        for index in 0..self.files.len() {
            let file = &mut self.files[index];
            if file.committed {
                continue;
            }
            if file.remaining != 0 {
                return Err(ClientInstallError::ChunkLayoutMismatch(
                    file.manifest_path.clone(),
                ));
            }
            file.handle = None;
            if !file.temp_path.exists() {
                File::create(&file.temp_path)?;
            }
            self.commit(index)?;
        }
        Ok(())
    }

    fn commit(&mut self, index: usize) -> Result<(), ClientInstallError> {
        if let Some(stale) = self.stale_install_metadata.take() {
            match fs::remove_file(stale) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
                _ => {}
            }
        }
        let file = &mut self.files[index];
        commit_downloaded_file(
            &file.temp_path,
            &file.target,
            &self.temp_dir,
            self.allow_replace,
        )?;
        file.committed = true;
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

/// Cuándo se publica el progreso de la descarga.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Report {
    /// Al empezar y al terminar: siempre.
    Always,
    /// Tras aplicar una ventana, si pasó el intervalo.
    AfterApply,
    /// En un tic de espera, si pasó el intervalo y llegaron bytes nuevos.
    IfChanged,
}

impl Downloader<'_> {
    /// Mantiene hasta `connections` peticiones en curso como tareas independientes, cada una con
    /// sus reintentos, y aplica cada ventana en orden en un hilo bloqueante. Mientras espera a la
    /// red o al disco revisa la parada y publica los bytes recibidos. La memoria queda acotada a
    /// unas `connections + 1` ventanas.
    async fn run(
        &mut self,
        windows: Arc<Vec<DownloadWindow>>,
        files: &[InstallFile],
        writer: FileWriter,
    ) -> (Option<FileWriter>, Result<(), ClientInstallError>) {
        // El detalle del progreso es el archivo que arma la ventana que se espera.
        let detail = |index: usize| {
            windows
                .get(index.min(windows.len().saturating_sub(1)))
                .and_then(DownloadWindow::first_placement)
                .and_then(|placement| files.get(placement.file_index))
                .map_or("", |file| file.path.as_str())
        };
        let mut writer = Some(writer);
        let mut in_flight = VecDeque::new();
        let mut next = 0;
        let mut ticker = ticker();
        let failure = 'windows: {
            if let Err(error) = self.report(detail(0), Report::Always) {
                break 'windows Some(error);
            }
            for index in 0..windows.len() {
                while next < windows.len() && in_flight.len() < self.tuning.connections {
                    in_flight.push_back(self.spawn_fetch(&windows[next]));
                    next += 1;
                }
                let Some(mut fetch) = in_flight.pop_front() else {
                    break;
                };
                // Detener no espera a la red: la petición se aborta en el próximo tic.
                let fetched = loop {
                    match wait_or_tick(&mut fetch, &mut ticker).await {
                        Waited::Ready(fetched) => break fetched,
                        Waited::Tick => {
                            let checked = if self.cancel.is_cancelled() {
                                Err(ClientInstallError::Cancelled)
                            } else {
                                self.report(detail(index), Report::IfChanged)
                            };
                            if let Err(error) = checked {
                                fetch.abort();
                                break 'windows Some(error);
                            }
                        }
                    }
                };
                let data = match fetched {
                    Ok(Ok(data)) => data,
                    Ok(Err(error)) => break 'windows Some(error),
                    Err(error) => break 'windows Some(ClientInstallError::Worker(error)),
                };

                let mut owned = writer.take().expect("writer is available between windows");
                let shared = Arc::clone(&windows);
                let mut applying = tauri::async_runtime::spawn_blocking(move || {
                    let outcome = owned.apply(&shared[index], &data);
                    (owned, outcome)
                });
                // El disco no se interrumpe a mitad de ventana: la parada espera a que termine
                // esta, y mientras tanto el progreso de la red se sigue publicando.
                let mut report_error = None;
                let applied = loop {
                    match wait_or_tick(&mut applying, &mut ticker).await {
                        Waited::Ready(applied) => break applied,
                        Waited::Tick if report_error.is_none() => {
                            report_error = self.report(detail(index), Report::IfChanged).err();
                        }
                        Waited::Tick => {}
                    }
                };
                match applied {
                    Ok((returned, outcome)) => {
                        writer = Some(returned);
                        if let Err(error) = outcome {
                            break 'windows Some(error);
                        }
                    }
                    Err(error) => break 'windows Some(ClientInstallError::Worker(error)),
                }
                if let Some(error) = report_error {
                    break 'windows Some(error);
                }
                if self.cancel.is_cancelled() {
                    break 'windows Some(ClientInstallError::Cancelled);
                }
                if let Err(error) = self.report(detail(index + 1), Report::AfterApply) {
                    break 'windows Some(error);
                }
            }
            // Todo aplicado: el último evento de la descarga muestra el total.
            self.report(detail(windows.len()), Report::Always).err()
        };

        for fetch in &in_flight {
            fetch.abort();
        }
        match failure {
            Some(error) => (writer, Err(error)),
            None => (writer, Ok(())),
        }
    }

    fn spawn_fetch(
        &self,
        window: &DownloadWindow,
    ) -> tauri::async_runtime::JoinHandle<Result<Vec<u8>, ClientInstallError>> {
        tauri::async_runtime::spawn(fetch_window(
            self.client.clone(),
            format!("{}/{:016X}.bundle", self.bundle_base, window.bundle_id),
            (window.start, window.end),
            self.tuning.retry_delays,
            Arc::clone(&self.received),
        ))
    }

    /// Publica los bytes recibidos hasta ahora. El valor nunca retrocede: si un reintento
    /// descarta bytes, el progreso espera a que el nuevo intento lo alcance.
    fn report(&mut self, detail: &str, when: Report) -> Result<(), ClientInstallError> {
        let received = self.received.load(Ordering::SeqCst).min(self.total_bytes);
        let bytes = self.reported_bytes.max(received);
        let due = self
            .last_report
            .is_none_or(|last| last.elapsed() >= self.tuning.progress_interval);
        let now = match when {
            Report::Always => true,
            Report::AfterApply => due,
            Report::IfChanged => due && bytes != self.reported_bytes,
        };
        if !now {
            return Ok(());
        }
        self.reported_bytes = bytes;
        self.last_report = Some(Instant::now());
        publish(
            self.progress,
            self.destination,
            "download",
            detail,
            bytes,
            Some(self.total_bytes),
            false,
        )
    }
}

/// Pide un rango de bundle y reintenta en silencio los fallos pasajeros, con las esperas de
/// `retry_delays`; solo el error del último intento llega a la instalación.
async fn fetch_window(
    client: reqwest::Client,
    url: String,
    range: (u32, u32),
    retry_delays: &'static [Duration],
    received: Arc<AtomicU64>,
) -> Result<Vec<u8>, ClientInstallError> {
    let mut delays = retry_delays.iter();
    loop {
        let mut counted = 0;
        let error = match fetch_range(&client, &url, range, &received, &mut counted).await {
            Ok(data) => return Ok(data),
            Err(error) => error,
        };
        // Lo que llegó en un intento fallido se vuelve a pedir: no cuenta como progreso.
        received.fetch_sub(counted, Ordering::SeqCst);
        match delays.next() {
            Some(delay) if error.is_transient() => tokio::time::sleep(*delay).await,
            _ => return Err(error),
        }
    }
}

/// Un intento: exige 206 con el `Content-Length` del rango antes de leer el cuerpo, y cuenta los
/// bytes a medida que llegan.
async fn fetch_range(
    client: &reqwest::Client,
    url: &str,
    (start, end): (u32, u32),
    received: &AtomicU64,
    counted: &mut u64,
) -> Result<Vec<u8>, ClientInstallError> {
    let expected = end - start;
    let mut response = client
        .get(url)
        .header(header::RANGE, format!("bytes={start}-{}", end - 1))
        .send()
        .await
        .map_err(ClientInstallError::Download)?
        .error_for_status()
        .map_err(ClientInstallError::Download)?;
    if response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(ClientInstallError::RangeIgnored(response.status().as_u16()));
    }
    let declared = response.content_length();
    if declared != Some(u64::from(expected)) {
        return Err(ClientInstallError::InvalidChunkSize {
            expected,
            actual: declared,
        });
    }
    let mut data = Vec::with_capacity(expected as usize);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(ClientInstallError::Download)?
    {
        if data.len() + chunk.len() > expected as usize {
            return Err(ClientInstallError::InvalidChunkSize {
                expected,
                actual: Some((data.len() + chunk.len()) as u64),
            });
        }
        data.extend_from_slice(&chunk);
        received.fetch_add(chunk.len() as u64, Ordering::SeqCst);
        *counted += chunk.len() as u64;
    }
    if data.len() != expected as usize {
        return Err(ClientInstallError::IncompleteRange {
            expected,
            actual: data.len(),
        });
    }
    Ok(data)
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

    let (writer, outcome) = downloader.run(windows, files, writer).await;
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
        io::{BufRead, BufReader, Read},
        net::{TcpListener, TcpStream},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Condvar, Mutex,
        },
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    /// Reintentos de milisegundos: el mismo camino que en la app, sin esperar un minuto.
    const FAST_RETRIES: [Duration; 5] = [
        Duration::from_millis(1),
        Duration::from_millis(2),
        Duration::from_millis(4),
        Duration::from_millis(8),
        Duration::from_millis(16),
    ];

    /// Ajustes de las pruebas: reintentos rápidos, sin estancamientos previstos (el tiempo de
    /// espera no se agota mientras el CDN retiene una petición) y un evento de progreso tras cada
    /// ventana aplicada.
    fn test_tuning(connections: usize, max_window: u32) -> DownloadTuning {
        DownloadTuning {
            connections,
            max_window,
            retry_delays: &FAST_RETRIES,
            read_timeout: Duration::from_secs(10),
            progress_interval: Duration::ZERO,
        }
    }

    /// Estado local de una sola versión.
    fn status_of(root: &str, version: &str) -> LocalClientStatus {
        local_client_statuses(root, &[version.to_string()]).unwrap()[version].clone()
    }

    fn confirm(plan: &ClientInstallPlan) -> ConfirmedPlan {
        ConfirmedPlan {
            mode: plan.mode,
            resolved_version: plan.resolved_version.clone(),
            destination_path: plan.destination_path.clone(),
            locale: plan.locale.clone(),
            manifest_url: plan.manifest_url.clone(),
        }
    }

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

    /// Flujo completo contra un CDN local: plan → confirmaciones que no coinciden → instalación
    /// interrumpida tras agotar los reintentos → reanudación por el camino predeterminado mientras
    /// una segunda instalación se rechaza → reparación que sobrevive a fallos pasajeros → listo,
    /// sin tocar archivos ajenos al manifiesto y quitando al confirmar los temporales de un corte.
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
        let live_manifest = format!(
            "{}/channels/public/releases/000000000000AAAA.manifest",
            cdn.base
        );
        cdn.route(
            "/api/v1/products/lol/version-sets/LA2",
            sieve_json(&[("16.19.8217343", live_manifest.clone())]),
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
            confirmed: confirmed.map(confirm),
        };
        let planned_with = |sink: &dyn ProgressSink, request: ClientInstallRequest| {
            run(async {
                let client = http_client().unwrap();
                build_install_plan(&client, sink, &endpoints, &request).await
            })
        };
        let plan = |request: ClientInstallRequest| {
            planned_with(&progress, request).map(|planned| planned.plan)
        };
        // Ventanas de un solo fragmento y 3 conexiones: varias peticiones en vuelo a la vez.
        let tuning = test_tuning(3, 1);
        let never = CancelSignal::default();
        let install = |request: ClientInstallRequest| {
            run(install_client_with(
                &progress, &endpoints, request, tuning, &never,
            ))
        };
        let root_text = root.display().to_string();
        let destination = root.join("16.18.817.5716").join(GAME_FOLDER);
        let metadata_dir = destination.join(METADATA_DIR);
        let temp_dir = metadata_dir.join("tmp");
        let (bundle_a1, bundle_b2) = ("00000000000000A1.bundle", "00000000000000B2.bundle");
        let attempts = 1 + FAST_RETRIES.len();

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

        // 2. Sin confirmación, o con otro idioma u otro manifiesto que los del plan revisado, no
        //    se escribe nada.
        assert!(matches!(
            install(request(None)),
            Err(ClientInstallError::ConfirmationRequired { .. })
        ));
        let other_locale = ClientInstallRequest {
            confirmed: Some(ConfirmedPlan {
                locale: "fr_FR".to_string(),
                ..confirm(&first)
            }),
            ..request(None)
        };
        assert_eq!(
            install(other_locale).unwrap_err().to_string(),
            "El plan cambió de idioma: es_MX en lugar de fr_FR"
        );
        let other_manifest = ClientInstallRequest {
            confirmed: Some(ConfirmedPlan {
                manifest_url: live_manifest.clone(),
                ..confirm(&first)
            }),
            ..request(None)
        };
        assert!(matches!(
            install(other_manifest),
            Err(ClientInstallError::ManifestChanged(_))
        ));
        assert!(!destination.exists());

        // 3. Interrupción: el bundle B2 falla siempre. Cada ventana hace 1 intento y 5 reintentos
        //    antes de rendirse. Las ventanas se aplican en orden, así que el ejecutable y el
        //    archivo que comparte su primer fragmento quedan confirmados y el resto no, aunque
        //    otras peticiones estuvieran en vuelo. Mientras el ejecutable está a medio armar, su
        //    `.part` es disperso.
        cdn.fail_paths_containing(Some(bundle_b2));
        let probe = PartProbe {
            temp_dir: temp_dir.clone(),
            seen: Mutex::default(),
        };
        let failed = run(install_client_with(
            &probe,
            &endpoints,
            request(Some(&first)),
            tuning,
            &never,
        ))
        .unwrap_err();
        assert!(matches!(failed, ClientInstallError::Download(_)));
        // Tras los reintentos llega el motivo de red en español, con el host.
        assert_eq!(
            failed.to_string(),
            "Falló la descarga: HTTP 500 (127.0.0.1)"
        );
        cdn.wait_until_idle();
        let log = cdn.take_bundle_log();
        // La ventana que se esperaba es la primera de B2 (desde el byte 0).
        assert_eq!(range_requests(&log, bundle_b2, 0), attempts);
        assert!(log
            .iter()
            .all(|(path, start)| range_requests(&log, path, *start) <= attempts));
        #[cfg(windows)]
        {
            let seen = probe.seen.lock().unwrap();
            assert!(
                seen.iter()
                    .any(|(name, _)| name.ends_with("League_of_Legends.exe.part")),
                "{seen:?}"
            );
            assert!(seen.iter().all(|(_, sparse)| *sparse), "{seen:?}");
        }
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
        let partial = status_of(&root_text, "16.18.817.5716");
        assert_eq!(partial.state, LocalClientState::Partial);
        assert_eq!(partial.total_install_bytes, Some(first.total_install_bytes));
        assert_eq!(
            partial.installed_bytes,
            (fixture.content(GAME_EXE).len()
                + fixture.content("DATA/FINAL/Shared.wad.client").len()) as u64
        );
        assert!(partial.executable_path.is_none());
        fs::write(destination.join("user-notes.txt"), b"mine").unwrap();

        // 4. Reanudación por el camino predeterminado (8 conexiones, ventanas de hasta 8 MiB):
        //    reutiliza lo confirmado y pide solo lo que falta, con los fragmentos contiguos
        //    fusionados y en paralelo. Mientras el CDN retiene esas peticiones, una segunda
        //    instalación se rechaza en este proceso y también desde otro (el candado del destino),
        //    sin que la limpieza de temporales llegue a correr.
        cdn.fail_paths_containing(None);
        let resume_planned = planned_with(&progress, request(None)).unwrap();
        let resume = resume_planned.plan.clone();
        assert_eq!(resume.mode, ClientWorkMode::Resume);
        assert_eq!(
            (resume.reused_file_count, resume.affected_file_count),
            (2, 2)
        );
        let merged = DownloadTuning {
            retry_delays: &FAST_RETRIES,
            read_timeout: Duration::from_secs(10),
            progress_interval: Duration::ZERO,
            ..DownloadTuning::with_connections(None)
        };
        let windows = plan_download_windows(&resume_planned.pending_files, merged.max_window)
            .unwrap()
            .len();
        let chunks = resume_planned
            .pending_files
            .iter()
            .map(|file| file.chunks.len())
            .sum::<usize>();
        assert!(windows < chunks, "{windows} windows for {chunks} chunks");
        progress.take();
        // Lo que el paso 3 dejó en vuelo no debe contarse como tráfico de la reanudación.
        cdn.wait_until_idle();
        cdn.reset_peak();
        let (requests_before, bytes_before) = cdn.bundle_traffic();
        let lock = metadata_dir.join(LOCK_FILE);
        let installed = thread::scope(|scope| {
            let held = cdn.hold("/bundles/");
            let running = scope.spawn(|| {
                run(install_client_with(
                    &progress,
                    &endpoints,
                    request(Some(&resume)),
                    merged,
                    &never,
                ))
            });
            // Todas las ventanas de la primera quedan retenidas: no hace más peticiones.
            cdn.wait_for_held(windows);
            // En el mismo proceso se rechaza al instante, sin consultar la red ni tocar la señal
            // de parada de la primera.
            let requests = cdn.requests();
            let same_process = install(request(Some(&resume))).unwrap_err();
            assert!(matches!(same_process, ClientInstallError::AlreadyRunning));
            assert_eq!(same_process.to_string(), "Ya hay una descarga en curso.");
            assert_eq!(cdn.requests(), requests);
            // Otra instancia confirma un plan recién revisado: el candado la frena antes de
            // limpiar temporales, así que un `.part` ajeno sigue en su lugar.
            assert!(lock.is_file());
            let foreign = temp_dir.join("99999-other-install.part");
            fs::create_dir_all(&temp_dir).unwrap();
            fs::write(&foreign, b"in use").unwrap();
            let quiet = ProgressLog::default();
            let fresh = planned_with(&quiet, request(None)).unwrap().plan;
            let other_process = run(install_client_with(
                &quiet,
                &endpoints,
                request(Some(&fresh)),
                merged,
                &CancelSignal::default(),
            ))
            .unwrap_err();
            assert_eq!(other_process.to_string(), "Ya hay una descarga en curso.");
            assert!(foreign.is_file());
            fs::remove_file(&foreign).unwrap();
            drop(held);
            running.join().unwrap()
        })
        .unwrap();
        assert!(!lock.exists());
        let (requests_after, bytes_after) = cdn.bundle_traffic();
        assert_eq!(requests_after - requests_before, windows);
        assert_eq!((bytes_after - bytes_before) as u64, resume.download_bytes);
        assert_eq!(cdn.peak_bundle_requests(), windows);
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
        let metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(metadata_dir.join(INSTALL_METADATA)).unwrap())
                .unwrap();
        assert_eq!(metadata["gameBuild"], "16.18.817.5716");
        assert_eq!(metadata["source"], "manifestIndex");
        assert!(!metadata_dir.join(PLAN_METADATA).exists());
        // Estado local: el cliente listo; otra compilación del mismo parche sin cliente propio usa
        // este (no exacto); otro parche sigue ausente y una versión ilegible se omite.
        let statuses = local_client_statuses(
            &root_text,
            &[
                "16.18.817.5716".to_string(),
                "16.18.820.1234".to_string(),
                "16.17.810.4348".to_string(),
                "not-a-version".to_string(),
            ],
        )
        .unwrap();
        let ready = statuses["16.18.817.5716"].clone();
        assert_eq!(
            (ready.state, ready.exact_build),
            (LocalClientState::Ready, true)
        );
        assert_eq!(
            ready.executable_path.as_deref(),
            Some(installed.executable_path.as_str())
        );
        assert_eq!(ready.locale.as_deref(), Some("es_MX"));
        assert_eq!(ready.total_install_bytes, Some(fixture.selected_bytes()));
        assert_eq!(
            statuses["16.18.820.1234"],
            LocalClientStatus {
                exact_build: false,
                ..ready.clone()
            }
        );
        let other_patch = &statuses["16.17.810.4348"];
        assert_eq!(
            (other_patch.state, other_patch.exact_build),
            (LocalClientState::Missing, true)
        );
        assert!(!statuses.contains_key("not-a-version"));
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
        assert_eq!(
            (downloads[0].downloaded_bytes, downloads[0].detail.as_str()),
            (0, "DATA/FINAL/Champions/Aatrox.es_MX.wad.client")
        );
        assert!(downloads
            .windows(2)
            .all(|pair| pair[0].downloaded_bytes <= pair[1].downloaded_bytes));
        assert_eq!(
            downloads.last().unwrap().downloaded_bytes,
            resume.download_bytes
        );
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
        // Si la red se cae antes de confirmar un archivo, `install.json` sigue: el cliente no
        // cambió y se sigue viendo listo.
        let impatient = DownloadTuning {
            read_timeout: Duration::from_millis(300),
            ..merged
        };
        let repair_with = |sink: &dyn ProgressSink| {
            run(install_client_with(
                sink,
                &endpoints,
                request(Some(&repair)),
                impatient,
                &never,
            ))
        };
        cdn.fail_paths_containing(Some(bundle_a1));
        assert!(repair_with(&progress).is_err());
        assert!(metadata_dir.join(INSTALL_METADATA).is_file());
        assert_eq!(
            status_of(&root_text, "16.18.817.5716").state,
            LocalClientState::Ready
        );
        // Un 503 y una respuesta que se cuelga a mitad del cuerpo se reintentan en silencio: el
        // cuelgue se detecta por inactividad (300 ms aquí), sin esperar a que el CDN cierre. El
        // progreso avanza con los bytes que llegan, antes de aplicar la ventana.
        cdn.fail_paths_containing(None);
        cdn.fail_next(bundle_a1, 1, 503);
        cdn.stall_next(bundle_a1, 1);
        cdn.wait_until_idle();
        cdn.take_bundle_log();
        progress.take();
        let (_, bytes_before) = cdn.bundle_traffic();
        let started = Instant::now();
        repair_with(&progress).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the stalled window took {:?}",
            started.elapsed()
        );
        let (_, bytes_after) = cdn.bundle_traffic();
        assert_eq!((bytes_after - bytes_before) as u64, repair.download_bytes);
        assert_eq!(range_requests(&cdn.take_bundle_log(), bundle_a1, 0), 3);
        assert!(progress.take().iter().any(|event| event.stage == "download"
            && event.downloaded_bytes > 0
            && event.downloaded_bytes < repair.download_bytes));
        assert_eq!(
            fs::read(destination.join(GAME_EXE)).unwrap(),
            fixture.content(GAME_EXE)
        );
        let repaired: serde_json::Value =
            serde_json::from_slice(&fs::read(metadata_dir.join(INSTALL_METADATA)).unwrap())
                .unwrap();
        assert_eq!(repaired["mode"], "repair");

        // 6. Listo: nada que descargar ni peticiones de bundle. Un corte entre los dos renombres
        //    de una reparación deja un `.backup` huérfano: planificar no lo toca y confirmar el
        //    plan lo quita junto con los `.part` viejos. `install.json` conserva los datos de la
        //    descarga que armó el cliente.
        fs::create_dir_all(&temp_dir).unwrap();
        let orphans = [
            temp_dir.join("00000-League_of_Legends.exe.part.backup"),
            temp_dir.join("00003-DATA_FINAL_Bootstrap.windows.wad.client.part"),
        ];
        for orphan in &orphans {
            fs::write(orphan, b"interrupted").unwrap();
        }
        let ready_plan = plan(request(None)).unwrap();
        assert_eq!(ready_plan.mode, ClientWorkMode::Ready);
        assert_eq!(ready_plan.download_bytes, 0);
        assert!(orphans.iter().all(|orphan| orphan.is_file()));
        let stored = fs::read(metadata_dir.join(INSTALL_METADATA)).unwrap();
        let (requests_before, _) = cdn.bundle_traffic();
        install(request(Some(&ready_plan))).unwrap();
        assert_eq!(cdn.bundle_traffic().0, requests_before);
        assert!(!temp_dir.exists() || eventually_empty(&temp_dir));
        assert_eq!(
            fs::read(metadata_dir.join(INSTALL_METADATA)).unwrap(),
            stored
        );

        // Los archivos ajenos al manifiesto siguen intactos.
        assert_eq!(fs::read(root.join("keep-me.txt")).unwrap(), b"unrelated");
        assert_eq!(
            fs::read(destination.join("user-notes.txt")).unwrap(),
            b"mine"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Detener mientras el CDN retiene una ventana vuelve enseguida, conserva lo confirmado, no
    /// deja temporales ni candado, deja el estado local en "incompleto" con el total del plan y la
    /// reanudación termina el cliente. El estado distingue una carpeta que todavía no existe de una
    /// unidad que no está.
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
            confirmed: confirmed.map(confirm),
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
        let tuning = test_tuning(1, 1);
        let destination = root.join("16.18.817.5716").join(GAME_FOLDER);

        // Consultar el estado de una compilación ausente no crea carpetas, y una carpeta de
        // clientes que todavía no existe no es un error.
        let missing = status_of(&root_text, "16.18.817.5716");
        assert_eq!(
            (missing.state, missing.exact_build),
            (LocalClientState::Missing, true)
        );
        assert!(!root.join("16.18.817.5716").exists());
        assert!(list_local_clients(&root_text).unwrap().is_empty());
        let not_created = root.join("not-created");
        let not_created_text = not_created.display().to_string();
        assert_eq!(
            status_of(&not_created_text, "16.18.817.5716").state,
            LocalClientState::Missing
        );
        assert!(list_local_clients(&not_created_text).unwrap().is_empty());
        assert!(!not_created.exists());
        // Una unidad que no está (un disco USB desconectado) sí lo es, con la ruta.
        #[cfg(windows)]
        if let Some(drive) = ('F'..='Z')
            .rev()
            .map(|letter| format!("{letter}:\\"))
            .find(|drive| !Path::new(drive).exists())
        {
            let unplugged = format!("{drive}ChronoRift");
            assert_eq!(
                local_client_statuses(&unplugged, &["16.18.817.5716".to_string()])
                    .unwrap_err()
                    .to_string(),
                format!("La carpeta de clientes no está disponible: {unplugged}")
            );
            assert!(matches!(
                list_local_clients(&unplugged),
                Err(ClientInstallError::ClientsRootUnavailable(_))
            ));
        }

        // El CDN retiene el bundle B2: cuando la instalación espera esa ventana, el ejecutable y
        // el archivo que comparte su primer fragmento ya están confirmados.
        let first = plan(request(None)).unwrap();
        let cancel = CancelSignal::default();
        let stopped = thread::scope(|scope| {
            let _held = cdn.hold("00000000000000B2.bundle");
            let running = scope.spawn(|| {
                run(install_client_with(
                    &ProgressLog::default(),
                    &endpoints,
                    request(Some(&first)),
                    tuning,
                    &cancel,
                ))
            });
            cdn.wait_for_held(1);
            let asked = Instant::now();
            cancel.cancel();
            let stopped = running.join().unwrap();
            // Detener no espera a la ventana retenida.
            assert!(
                asked.elapsed() < Duration::from_secs(1),
                "stopping took {:?}",
                asked.elapsed()
            );
            stopped
        });
        assert_eq!(stopped.unwrap_err().to_string(), "Descarga detenida");
        let temp_dir = destination.join(METADATA_DIR).join("tmp");
        assert!(!temp_dir.exists() || eventually_empty(&temp_dir));
        assert!(!destination.join(METADATA_DIR).join(LOCK_FILE).exists());

        let committed = fixture
            .selected()
            .filter(|(path, _, _)| destination.join(path).is_file())
            .map(|(_, content, _)| content.len() as u64)
            .sum::<u64>();
        assert_eq!(
            committed,
            (fixture.content(GAME_EXE).len()
                + fixture.content("DATA/FINAL/Shared.wad.client").len()) as u64
        );
        let partial = status_of(&root_text, "16.18.817.5716");
        assert_eq!(partial.state, LocalClientState::Partial);
        assert_eq!(partial.total_install_bytes, Some(first.total_install_bytes));
        assert_eq!(partial.installed_bytes, committed);
        assert_eq!(list_local_clients(&root_text).unwrap(), vec![partial]);

        // La señal quedó en "detenida": la próxima instalación la limpia al empezar.
        let resume = plan(request(None)).unwrap();
        assert_eq!(resume.mode, ClientWorkMode::Resume);
        run(install_client_with(
            &ProgressLog::default(),
            &endpoints,
            request(Some(&resume)),
            tuning,
            &cancel,
        ))
        .unwrap();
        let ready = status_of(&root_text, "16.18.817.5716");
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

    /// Anota, en cada evento de descarga, los `.part` de la carpeta temporal y si son dispersos.
    struct PartProbe {
        temp_dir: PathBuf,
        seen: Mutex<Vec<(String, bool)>>,
    }

    impl ProgressSink for PartProbe {
        fn publish(&self, progress: ClientInstallProgress) -> Result<(), ClientInstallError> {
            if progress.stage != "download" {
                return Ok(());
            }
            let parts = fs::read_dir(&self.temp_dir)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "part")
                })
                .map(|path| {
                    let name = path.file_name().unwrap().to_string_lossy().into_owned();
                    (name, is_sparse(&path))
                })
                .collect::<Vec<_>>();
            self.seen.lock().unwrap().extend(parts);
            Ok(())
        }
    }

    #[cfg(windows)]
    fn is_sparse(path: &Path) -> bool {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_SPARSE_FILE
        fs::metadata(path).is_ok_and(|metadata| metadata.file_attributes() & 0x200 != 0)
    }

    #[cfg(not(windows))]
    fn is_sparse(_path: &Path) -> bool {
        false
    }

    /// Resolución de compilaciones: Sieve exacto, índice (con región de respaldo), compilación
    /// cercana marcada como no exacta solo si el índice no la tiene, fallos del índice y de Sieve
    /// informados, URL fuera del CDN rechazada, redirecciones solo dentro del mismo origen (con el
    /// motivo en español), idioma mal escrito o ausente y versión inexistente.
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

        // Solo un "no está" del índice lleva a esa compilación no exacta. Un fallo en la región
        // de la repetición no impide probar las de respaldo…
        cdn.fail_paths_containing(Some("/LA2/windows/lol-game-client/16.19.8201111.txt"));
        cdn.route(
            "/index/LoL/NA1/windows/lol-game-client/16.19.8201111.txt",
            format!("{}{index_manifest}", cdn.base),
        );
        let other_region = plan_for("16.19", "16.19.820.1111").unwrap();
        assert_eq!(
            (other_region.source, other_region.exact_build),
            (ReleaseSource::ManifestIndex, true)
        );
        // …y si ninguna da la URL, ese fallo se informa en lugar de ofrecer otra compilación.
        cdn.fault(
            "/LA2/windows/lol-game-client/16.19.8222222.txt",
            None,
            FaultKind::Status(429),
        );
        assert_eq!(
            plan_for("16.19", "16.19.822.2222").unwrap_err().to_string(),
            "No se pudo leer el índice riot-manifests: HTTP 429 (127.0.0.1)"
        );
        // Si Sieve también falla, se informan los dos motivos.
        cdn.fail_next("/api/v1/products/lol/version-sets/LA2", 1, 503);
        assert_eq!(
            plan_for("16.19", "16.19.822.2222")
                .unwrap_err()
                .to_string(),
            "No se pudo consultar Riot Sieve: HTTP 503 (127.0.0.1). No se pudo leer el índice riot-manifests: HTTP 429 (127.0.0.1)"
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

        let confirmed = confirm(&plan);
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
        /// Fallos inducidos: vale el primero cuyo fragmento aparece en la ruta y aún tiene usos.
        faults: Mutex<Vec<Fault>>,
        /// Mientras tiene un fragmento, las peticiones cuya ruta lo contiene esperan.
        hold: Mutex<Option<String>>,
        hold_changed: Condvar,
        held_requests: AtomicUsize,
        /// Peticiones de bundle recibidas: ruta e inicio del rango.
        bundle_log: Mutex<Vec<(String, usize)>>,
        bundle_requests: AtomicUsize,
        /// Bytes de las respuestas 206 servidas completas.
        bundle_bytes: AtomicUsize,
        active_bundle_requests: AtomicUsize,
        peak_bundle_requests: AtomicUsize,
        open_connections: AtomicUsize,
    }

    struct Fault {
        fragment: String,
        /// Usos que quedan; `None`, siempre.
        remaining: Option<usize>,
        kind: FaultKind,
    }

    #[derive(Clone, Copy)]
    enum FaultKind {
        /// Responde este estado sin cuerpo.
        Status(u16),
        /// Envía los encabezados y la mitad del cuerpo, y deja de enviar sin cerrar.
        Stall,
    }

    /// CDN HTTP mínimo sobre `TcpListener`: rutas fijas, peticiones Range, fallos inducidos y
    /// peticiones retenidas.
    struct TestCdn {
        base: String,
        state: Arc<CdnState>,
    }

    /// Suelta las peticiones retenidas al salir, también si una aserción falla: así ningún hilo
    /// queda esperando al servidor.
    struct Held<'a>(&'a CdnState);

    impl Drop for Held<'_> {
        fn drop(&mut self) {
            *self.0.hold.lock().unwrap() = None;
            self.0.hold_changed.notify_all();
        }
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

        fn fault(&self, fragment: &str, remaining: Option<usize>, kind: FaultKind) {
            self.state.faults.lock().unwrap().push(Fault {
                fragment: fragment.to_string(),
                remaining,
                kind,
            });
        }

        /// Con un fragmento, esas rutas responden 500 siempre; con `None`, se quitan todos los
        /// fallos inducidos.
        fn fail_paths_containing(&self, fragment: Option<&str>) {
            match fragment {
                Some(fragment) => self.fault(fragment, None, FaultKind::Status(500)),
                None => self.state.faults.lock().unwrap().clear(),
            }
        }

        /// Las próximas `times` peticiones a esas rutas responden `status`.
        fn fail_next(&self, fragment: &str, times: usize, status: u16) {
            self.fault(fragment, Some(times), FaultKind::Status(status));
        }

        /// Las próximas `times` peticiones a esas rutas se cuelgan a mitad del cuerpo.
        fn stall_next(&self, fragment: &str, times: usize) {
            self.fault(fragment, Some(times), FaultKind::Stall);
        }

        /// Retiene las peticiones cuya ruta contiene `fragment` hasta soltar el guardia.
        fn hold(&self, fragment: &str) -> Held<'_> {
            *self.state.hold.lock().unwrap() = Some(fragment.to_string());
            Held(&self.state)
        }

        /// Espera a que el servidor retenga `count` peticiones.
        fn wait_for_held(&self, count: usize) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.state.held_requests.load(Ordering::SeqCst) < count {
                assert!(Instant::now() < deadline, "no request reached the hold");
                thread::sleep(Duration::from_millis(5));
            }
        }

        /// Peticiones de bundle desde la última llamada: ruta e inicio del rango.
        fn take_bundle_log(&self) -> Vec<(String, usize)> {
            std::mem::take(&mut *self.state.bundle_log.lock().unwrap())
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

    /// Veces que se pidió el rango que empieza en `start` de un bundle.
    fn range_requests(log: &[(String, usize)], bundle: &str, start: usize) -> usize {
        log.iter()
            .filter(|(path, from)| path.contains(bundle) && *from == start)
            .count()
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

        {
            let retained = |hold: &Option<String>| {
                hold.as_deref()
                    .is_some_and(|fragment| path.contains(fragment))
            };
            let mut hold = state.hold.lock().unwrap();
            if retained(&hold) {
                state.held_requests.fetch_add(1, Ordering::SeqCst);
                while retained(&hold) {
                    hold = state.hold_changed.wait(hold).unwrap();
                }
                state.held_requests.fetch_sub(1, Ordering::SeqCst);
            }
        }

        let is_bundle = path.contains("/bundles/");
        if is_bundle {
            state
                .bundle_log
                .lock()
                .unwrap()
                .push((path.clone(), range.map_or(0, |(from, _)| from)));
            // Respuesta lenta a propósito para que las peticiones concurrentes se solapen.
            let active = state.active_bundle_requests.fetch_add(1, Ordering::SeqCst) + 1;
            state
                .peak_bundle_requests
                .fetch_max(active, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(40));
        }
        let fault = {
            let mut faults = state.faults.lock().unwrap();
            faults
                .iter_mut()
                .find(|fault| path.contains(&fault.fragment) && fault.remaining != Some(0))
                .map(|fault| {
                    if let Some(remaining) = &mut fault.remaining {
                        *remaining -= 1;
                    }
                    fault.kind
                })
        };
        let body = state.routes.lock().unwrap().get(&path).cloned();
        let (status, payload) = match (fault, body) {
            (Some(FaultKind::Status(code)), _) => (format!("{code} Fault"), Vec::new()),
            (_, Some(bytes)) => match range {
                Some((from, to)) if from <= to && to < bytes.len() => {
                    ("206 Partial Content".to_string(), bytes[from..=to].to_vec())
                }
                Some(_) => ("416 Range Not Satisfiable".to_string(), Vec::new()),
                None => ("200 OK".to_string(), bytes),
            },
            (_, None) => ("404 Not Found".to_string(), Vec::new()),
        };
        let stall = matches!(fault, Some(FaultKind::Stall));
        if is_bundle {
            state.bundle_requests.fetch_add(1, Ordering::SeqCst);
            if !stall && status.starts_with("206") {
                state
                    .bundle_bytes
                    .fetch_add(payload.len(), Ordering::SeqCst);
            }
            state.active_bundle_requests.fetch_sub(1, Ordering::SeqCst);
        }
        let mut stream = stream;
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        if stall {
            // Media respuesta y silencio: la conexión sigue abierta hasta que el cliente se
            // rinde y la cierra (o 30 s).
            let _ = stream.write_all(&payload[..payload.len() / 2]);
            let _ = stream.flush();
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            let _ = stream.read(&mut [0_u8; 1]);
            return;
        }
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
