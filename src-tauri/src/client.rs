use crate::{settings, tr};
use cdragon_rman::{BundleChunk, FileFlagEntry, Rman};
use reqwest::{header, redirect, StatusCode, Url};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    error::Error,
    ffi::OsStr,
    fmt::{self, Display},
    fs::{self, File, OpenOptions},
    future::Future,
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
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
            attempt.error(tr!(
                format!("redirección rechazada hacia {refused}"),
                format!("redirect refused to {refused}")
            ))
        } else if attempt.previous().len() > MAX_REDIRECTS {
            attempt.error(tr!("demasiadas redirecciones", "too many redirects"))
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
/// Diario de los `.part` a medio armar: qué rangos de cada uno ya llegaron al disco.
const JOURNAL_FILE: &str = "journal.json";
const JOURNAL_VERSION: u32 = 1;
/// Carpeta, dentro de `METADATA_DIR`, del manifiesto de la instalación en curso.
const MANIFEST_CACHE_DIR: &str = "manifest";
/// Cada cuánto, como mínimo, se vuelcan los `.part` y se reemplaza el diario. Una parada o un
/// fallo hacen un último punto de control; un corte de luz pierde a lo sumo este lapso.
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(10);

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
            Self::Install => tr!("instalación nueva", "new install"),
            Self::Resume => tr!("reanudación", "resume"),
            Self::Repair => tr!("reparación", "repair"),
            Self::Ready => tr!("cliente listo", "ready client"),
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
    /// Bytes de los archivos del juego ya confirmados (sin metadatos) más los que el diario
    /// registra en los `.part` conservados de una descarga detenida.
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
    manifest_url: Option<String>,
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
            Self::InvalidPatch => tr!(write!(f, "Parche no válido"), write!(f, "Invalid patch")),
            Self::InvalidLocale(locale) => tr!(
                write!(f, "Idioma no válido: {locale}"),
                write!(f, "Invalid language: {locale}")
            ),
            Self::UnavailableLocale(locale) => tr!(
                write!(f, "Idioma no disponible en este cliente: {locale}"),
                write!(f, "Language not available in this client: {locale}")
            ),
            Self::InvalidClientsRoot(err) => write!(f, "{err}"),
            Self::ClientsRootUnavailable(path) => tr!(
                write!(f, "La carpeta de clientes no está disponible: {path}"),
                write!(f, "The clients folder is not available: {path}")
            ),
            Self::InvalidReleaseVersion(version) => tr!(
                write!(f, "Versión de cliente inesperada: {version}"),
                write!(f, "Unexpected client version: {version}")
            ),
            Self::InvalidManifestUrl(url) => tr!(
                write!(f, "URL de manifiesto no válida: {url}"),
                write!(f, "Invalid manifest URL: {url}")
            ),
            Self::UntrustedManifestUrl(url) => tr!(
                write!(f, "Manifiesto fuera del CDN de Riot: {url}"),
                write!(f, "Manifest outside Riot's CDN: {url}")
            ),
            Self::MissingBundleBase(url) => tr!(
                write!(f, "Sin URL de bundles para el manifiesto: {url}"),
                write!(f, "No bundle URL for the manifest: {url}")
            ),
            Self::HttpClient(err) => tr!(
                write!(
                    f,
                    "No se pudo preparar el cliente HTTP: {}",
                    network_detail(err)
                ),
                write!(f, "Could not set up the HTTP client: {}", network_detail(err))
            ),
            Self::Resolve(err) => tr!(
                write!(f, "No se pudo consultar Riot Sieve: {}", network_detail(err)),
                write!(f, "Could not query Riot Sieve: {}", network_detail(err))
            ),
            Self::InvalidResolveResponse(err) => tr!(
                write!(f, "Respuesta inesperada de Riot Sieve: {err}"),
                write!(f, "Unexpected response from Riot Sieve: {err}")
            ),
            Self::ManifestIndex(err) => tr!(
                write!(
                    f,
                    "No se pudo leer el índice riot-manifests: {}",
                    network_detail(err)
                ),
                write!(
                    f,
                    "Could not read the riot-manifests index: {}",
                    network_detail(err)
                )
            ),
            Self::ResolveFailed { sieve, index } => write!(f, "{sieve}. {index}"),
            Self::ResponseTooLarge { host, limit } => {
                let mib = limit / (1024 * 1024);
                tr!(
                    write!(f, "Respuesta demasiado grande de {host}: supera {mib} MiB"),
                    write!(f, "Response too large from {host}: over {mib} MiB")
                )
            }
            Self::Metadata(err) => tr!(
                write!(f, "No se pudieron guardar los datos de la instalación: {err}"),
                write!(f, "Could not save the install data: {err}")
            ),
            Self::ReleaseNotFound {
                region,
                patch,
                version: Some(version),
            } => tr!(
                write!(
                    f,
                    "Sin manifiesto de {GAME_ARTIFACT} para {region} {version} ({patch}) en Riot Sieve ni en riot-manifests"
                ),
                write!(
                    f,
                    "No {GAME_ARTIFACT} manifest for {region} {version} ({patch}) in Riot Sieve or riot-manifests"
                )
            ),
            Self::ReleaseNotFound {
                region,
                patch,
                version: None,
            } => tr!(
                write!(f, "Sin manifiesto de {GAME_ARTIFACT} para {region} {patch}"),
                write!(f, "No {GAME_ARTIFACT} manifest for {region} {patch}")
            ),
            Self::Download(err) => tr!(
                write!(f, "Falló la descarga: {}", network_detail(err)),
                write!(f, "Download failed: {}", network_detail(err))
            ),
            Self::InvalidChunkSize {
                expected,
                actual: Some(actual),
            } => tr!(
                write!(
                    f,
                    "Rango de bundle con tamaño inesperado: {actual} bytes en lugar de {expected}"
                ),
                write!(
                    f,
                    "Bundle range with an unexpected size: {actual} bytes instead of {expected}"
                )
            ),
            Self::InvalidChunkSize {
                expected,
                actual: None,
            } => tr!(
                write!(
                    f,
                    "Rango de bundle sin tamaño declarado; se esperaban {expected} bytes"
                ),
                write!(
                    f,
                    "Bundle range without a declared size; expected {expected} bytes"
                )
            ),
            Self::RangeIgnored(status) => tr!(
                write!(f, "El CDN no respetó el rango pedido: HTTP {status}"),
                write!(f, "The CDN ignored the requested range: HTTP {status}")
            ),
            Self::IncompleteRange { expected, actual } => tr!(
                write!(
                    f,
                    "Rango de bundle incompleto: {actual} de {expected} bytes"
                ),
                write!(f, "Incomplete bundle range: {actual} of {expected} bytes")
            ),
            Self::InvalidChunkData { expected, actual } => tr!(
                write!(
                    f,
                    "Fragmento descomprimido con tamaño inesperado: {actual} bytes en lugar de {expected}"
                ),
                write!(
                    f,
                    "Decompressed chunk with an unexpected size: {actual} bytes instead of {expected}"
                )
            ),
            Self::ChunkLayoutMismatch(path) => tr!(
                write!(
                    f,
                    "Los fragmentos del manifiesto no suman el tamaño del archivo: {path}"
                ),
                write!(
                    f,
                    "The manifest chunks do not add up to the file size: {path}"
                )
            ),
            Self::Io(err) => tr!(
                write!(f, "Falló la instalación: {err}"),
                write!(f, "The install failed: {err}")
            ),
            Self::Rman(err) => tr!(
                write!(f, "No se pudo leer el manifiesto de Riot: {err}"),
                write!(f, "Could not read the Riot manifest: {err}")
            ),
            Self::CorruptManifest => tr!(
                write!(f, "Manifiesto de Riot dañado o desconocido"),
                write!(f, "Damaged or unknown Riot manifest")
            ),
            Self::MissingChunk(id) => tr!(
                write!(f, "El manifiesto usa un fragmento inexistente: {id:016X}"),
                write!(f, "The manifest uses a chunk that does not exist: {id:016X}")
            ),
            Self::Zstd(err) => tr!(
                write!(f, "No se pudo descomprimir un fragmento: {err}"),
                write!(f, "Could not decompress a chunk: {err}")
            ),
            Self::UnsafeManifestPath(path) => tr!(
                write!(f, "Ruta insegura en el manifiesto: {path}"),
                write!(f, "Unsafe path in the manifest: {path}")
            ),
            Self::WouldOverwrite(path) => tr!(
                write!(f, "El archivo ya existe: {}", path.display()),
                write!(f, "The file already exists: {}", path.display())
            ),
            Self::InvalidExistingTarget(path) => tr!(
                write!(f, "El destino no es un archivo: {}", path.display()),
                write!(f, "The target is not a file: {}", path.display())
            ),
            Self::ConfirmationRequired {
                planned,
                version,
                destination,
            } => tr!(
                write!(f, "El plan cambió a {planned}: {version} en {destination}"),
                write!(f, "The plan changed to {planned}: {version} in {destination}")
            ),
            Self::LocaleChanged { confirmed, planned } => tr!(
                write!(
                    f,
                    "El plan cambió de idioma: {planned} en lugar de {confirmed}"
                ),
                write!(
                    f,
                    "The plan changed language: {planned} instead of {confirmed}"
                )
            ),
            Self::ManifestChanged(url) => tr!(
                write!(f, "El plan cambió de manifiesto: {url}"),
                write!(f, "The plan changed manifest: {url}")
            ),
            Self::MissingExecutable(path) => tr!(
                write!(f, "Falta el ejecutable: {}", path.display()),
                write!(f, "The executable is missing: {}", path.display())
            ),
            Self::Emit(err) => tr!(
                write!(f, "No se pudo publicar el progreso: {err}"),
                write!(f, "Could not publish the progress: {err}")
            ),
            Self::Worker(err) => tr!(
                write!(f, "Falló una tarea de la descarga: {err}"),
                write!(f, "A download task failed: {err}")
            ),
            Self::Cancelled => tr!(
                write!(f, "Descarga detenida"),
                write!(f, "Download stopped")
            ),
            Self::AlreadyRunning => tr!(
                write!(f, "Ya hay una descarga en curso."),
                write!(f, "A download is already running.")
            ),
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

/// Motivo de un fallo de red en el idioma de la interfaz, con el host cuando se conoce. Lo que no
/// se reconoce queda con el texto de reqwest.
fn network_detail(error: &reqwest::Error) -> String {
    // La política de redirecciones ya da su motivo traducido, con el origen rechazado.
    if error.is_redirect() {
        if let Some(reason) = error.source() {
            return reason.to_string();
        }
    }
    let reason = if error.is_timeout() {
        tr!("tiempo de espera agotado", "timed out").to_string()
    } else if error.is_connect() {
        tr!("no se pudo conectar", "could not connect").to_string()
    } else if let Some(status) = error.status() {
        format!("HTTP {}", status.as_u16())
    } else if error.is_body() || error.is_decode() {
        tr!("respuesta incompleta", "incomplete response").to_string()
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
    // Una descarga detenida suma lo que su diario registra en los `.part` conservados.
    let journaled = planned
        .as_ref()
        .and_then(|plan| plan.manifest_url.as_deref())
        .map_or(0, |manifest_url| {
            journaled_bytes(&destination, manifest_url)
        });
    let installed_bytes = game_file_bytes(&destination, &metadata_dir) + journaled;
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
    let mut planned = until_cancelled(
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
    // Otra instancia pudo cambiar el diario entre el plan y el candado: entonces el plan ya no
    // es el que se revisó.
    let identity = JournalIdentity::of(&planned.plan);
    let kept = KeptParts::load(&planned.destination, &identity, &planned.pending_files);
    if kept != planned.kept {
        return Err(ClientInstallError::ConfirmationRequired {
            planned: planned.plan.mode,
            version: planned.plan.game_build.clone(),
            destination: planned.plan.destination_path.clone(),
        });
    }
    // Recién con el plan confirmado, y también si ya está listo: un `.backup` de una reparación
    // cortada puede quedar aunque el archivo ya esté completo. Los `.part` que el diario
    // conserva para este plan se quedan.
    remove_stale_temp_files(&planned.destination, &kept)?;

    let mut total_bytes = 0;
    let started = Instant::now();
    if planned.plan.mode != ClientWorkMode::Ready {
        // El plan confirmado da el total si la descarga queda a medias. `install.json` sigue
        // hasta que se confirma el primer archivo y vuelve a escribirse con el cliente completo.
        write_metadata(&planned.destination, PLAN_METADATA, &planned.plan)?;
        // Reanudar o volver a planificar no vuelve a bajar el manifiesto.
        if let Some(manifest) = planned.manifest.take() {
            store_manifest(&planned.destination, &planned.plan.manifest_url, &manifest)?;
        }
        let work = plan_download(&planned.pending_files, &kept, tuning.max_window)?;
        total_bytes = work.stored_bytes + work.missing_bytes;
        let assembling = (0..planned.pending_files.len())
            .map(|file| kept.get(file).is_some())
            .collect::<Vec<_>>();
        let writer = FileWriter::new(
            &planned.destination,
            &planned.pending_files,
            &kept,
            Assembly::new(work.chunks, work.remaining, &assembling),
            identity,
            planned.plan.mode == ClientWorkMode::Repair,
        )?;
        let mut downloader = Downloader {
            client: &client,
            progress,
            destination: &planned.destination,
            bundle_base: &bundle_base,
            tuning,
            cancel,
            stored_bytes: work.stored_bytes,
            missing_bytes: work.missing_bytes,
            received: Arc::default(),
            reported_bytes: 0,
            last_report: None,
        };
        download_files(
            &mut downloader,
            &planned.pending_files,
            work.windows,
            writer,
        )
        .await?;
    }
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
    remove_manifest_cache(&planned.destination)?;
    publish(
        progress,
        &planned.destination,
        "complete",
        tr!("Cliente listo", "Client ready"),
        total_bytes,
        Some(total_bytes),
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
    /// Posición en la selección completa del manifiesto: estable entre ejecuciones del mismo
    /// plan, nombra su `.part`.
    index: usize,
    path: String,
    filesize: u32,
    chunks: Vec<BundleChunk>,
}

impl InstallFile {
    fn temp_name(&self) -> String {
        format!(
            "{:05}-{}.{TEMP_SUFFIX}",
            self.index,
            temp_name_for_manifest_path(&self.path)
        )
    }
}

#[derive(Debug)]
struct PlannedInstall {
    plan: ClientInstallPlan,
    destination: PathBuf,
    pending_files: Vec<InstallFile>,
    /// `.part` de una ejecución anterior que el plan aprovecha.
    kept: KeptParts,
    /// Manifiesto recién descargado, para guardarlo al confirmar; `None` si salió del guardado.
    manifest: Option<Vec<u8>>,
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
        tr!(
            "Buscando el manifiesto de Riot",
            "Looking up the Riot manifest"
        ),
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
    let (files, manifest) = load_install_files(
        client,
        progress,
        &destination,
        &release.manifest_url,
        &locale,
    )
    .await?;
    let classified = classify_install_files(&destination, &files)?;
    // Lo que el diario de una descarga detenida ya tiene en disco no se cuenta como pendiente.
    let identity = JournalIdentity {
        manifest_url: release.manifest_url.clone(),
        resolved_version: release.version.clone(),
        locale: locale.clone(),
        destination_path: destination.display().to_string(),
    };
    let kept = KeptParts::load(&destination, &identity, &classified.pending_files);
    let (needed, remaining, _) = needed_chunks(&classified.pending_files, &kept)?;
    let download_bytes = needed
        .iter()
        .map(|chunk| u64::from(chunk.bundle_size))
        .sum();
    let install_bytes = remaining.iter().sum();
    let total_install_bytes = files.iter().map(|file| u64::from(file.filesize)).sum();
    // Una descarga detenida antes de confirmar su primer archivo también se reanuda; una
    // reparación sigue siendo reparación.
    let mode = match classified.mode {
        ClientWorkMode::Install if kept.any() => ClientWorkMode::Resume,
        mode => mode,
    };

    Ok(PlannedInstall {
        plan: ClientInstallPlan {
            mode,
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
        kept,
        manifest,
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
            tr!(
                "Descargando el manifiesto de Riot",
                "Downloading the Riot manifest"
            ),
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

/// Archivos del manifiesto para el idioma. Reanudar o volver a planificar usa el manifiesto que
/// guardó la instalación confirmada en el destino, si es el de esa URL y se puede leer entero;
/// si no, lo descarga. Devuelve también los bytes recién descargados, para guardarlos al confirmar.
async fn load_install_files(
    client: &reqwest::Client,
    progress: &dyn ProgressSink,
    destination: &Path,
    manifest_url: &str,
    locale: &str,
) -> Result<(Vec<InstallFile>, Option<Vec<u8>>), ClientInstallError> {
    if let Some((cached, manifest_id)) = read_cached_manifest(destination, manifest_url) {
        match parse_install_files(&cached, locale, Some(manifest_id)) {
            Ok(files) => {
                let size = cached.len() as u64;
                publish(
                    progress,
                    destination,
                    "manifest",
                    tr!(
                        "Usando el manifiesto de Riot guardado",
                        "Using the saved Riot manifest"
                    ),
                    size,
                    Some(size),
                    false,
                )?;
                publish_planning(progress, destination)?;
                return Ok((files, None));
            }
            // El idioma no depende de la copia: descargarlo otra vez daría lo mismo.
            Err(error @ ClientInstallError::UnavailableLocale(_)) => return Err(error),
            // Una copia dañada o de otro manifiesto se ignora.
            Err(_) => {}
        }
    }
    let manifest = download_manifest(client, progress, destination, manifest_url).await?;
    publish_planning(progress, destination)?;
    let files = parse_install_files(&manifest, locale, None)?;
    Ok((files, Some(manifest)))
}

fn publish_planning(
    progress: &dyn ProgressSink,
    destination: &Path,
) -> Result<(), ClientInstallError> {
    publish(
        progress,
        destination,
        "plan",
        tr!("Planificando archivos", "Planning files"),
        0,
        None,
        false,
    )
}

/// Dónde se guarda el manifiesto de esa URL y el id que debe declarar. Las URL de Riot terminan en
/// `<id de 16 dígitos hexadecimales>.manifest`; otra forma no se guarda.
fn manifest_cache_path(destination: &Path, manifest_url: &str) -> Option<(PathBuf, u64)> {
    let url = Url::parse(manifest_url).ok()?;
    let name = url.path_segments()?.next_back()?;
    let stem = name.strip_suffix(".manifest")?;
    if stem.len() != 16 || !stem.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let manifest_id = u64::from_str_radix(stem, 16).ok()?;
    let path = destination
        .join(METADATA_DIR)
        .join(MANIFEST_CACHE_DIR)
        .join(format!("{stem}.manifest"));
    Some((path, manifest_id))
}

fn read_cached_manifest(destination: &Path, manifest_url: &str) -> Option<(Vec<u8>, u64)> {
    let (path, manifest_id) = manifest_cache_path(destination, manifest_url)?;
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_MANIFEST_BYTES).then_some((bytes, manifest_id))
}

/// Guarda el manifiesto confirmado (aparte y luego renombrado) y quita los de otras URL.
fn store_manifest(
    destination: &Path,
    manifest_url: &str,
    manifest: &[u8],
) -> Result<(), ClientInstallError> {
    let Some((path, _)) = manifest_cache_path(destination, manifest_url) else {
        return Ok(());
    };
    let directory = destination.join(METADATA_DIR).join(MANIFEST_CACHE_DIR);
    fs::create_dir_all(&directory)?;
    remove_cached_manifests(&directory, Some(&path))?;
    if path.is_file() {
        return Ok(());
    }
    let pending = path.with_extension("tmp");
    fs::write(&pending, manifest)?;
    fs::rename(&pending, &path)?;
    Ok(())
}

/// Quita el manifiesto guardado: el cliente quedó completo.
fn remove_manifest_cache(destination: &Path) -> Result<(), ClientInstallError> {
    let directory = destination.join(METADATA_DIR).join(MANIFEST_CACHE_DIR);
    remove_cached_manifests(&directory, None)?;
    // Solo si quedó vacía.
    let _ = fs::remove_dir(directory);
    Ok(())
}

/// Quita los manifiestos guardados (y sus copias a medio escribir) salvo `keep`; lo desconocido se
/// queda.
fn remove_cached_manifests(
    directory: &Path,
    keep: Option<&Path>,
) -> Result<(), ClientInstallError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let path = entry?.path();
        let ours = path
            .extension()
            .is_some_and(|extension| extension == "manifest" || extension == "tmp");
        if ours && Some(path.as_path()) != keep && path.is_file() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn read_rman_manifest(manifest: &[u8]) -> Result<Rman, ClientInstallError> {
    // Los manifiestos actuales de Riot declaran RMAN 2.1; cdragon-rman 0.2 solo acepta 2.0,
    // pero el diseño de tablas que se lee aquí es compatible. Las pruebas cubren este ajuste.
    let read = match manifest.split_at_checked(6) {
        Some((b"RMAN\x02\x01", body)) => Rman::read(Cursor::new(*b"RMAN\x02\x00").chain(body)),
        _ => Rman::read(manifest),
    };
    read.map_err(ClientInstallError::Rman)
}

/// cdragon-rman documenta que entra en pánico con datos corruptos; aquí se convierte en error.
/// Con `manifest_id`, el manifiesto debe declarar ese id (el de su URL).
fn parse_install_files(
    manifest: &[u8],
    locale: &str,
    manifest_id: Option<u64>,
) -> Result<Vec<InstallFile>, ClientInstallError> {
    panic::catch_unwind(AssertUnwindSafe(|| {
        let rman = read_rman_manifest(manifest)?;
        if manifest_id.is_some_and(|id| id != rman.manifest_id) {
            return Err(ClientInstallError::CorruptManifest);
        }
        select_install_files(&rman, locale)
    }))
    .unwrap_or(Err(ClientInstallError::CorruptManifest))
}

/// Cómo se decide qué archivos del manifiesto se instalan para un idioma.
enum FileSelection<'a> {
    /// Las banderas RMAN de cada archivo, que es lo que Riot usa para filtrarlos: el idioma pedido
    /// (tal cual, sin equivalencias) más la plataforma Windows.
    Flags(Vec<FileFlagEntry<'a>>),
    /// Manifiestos donde ningún archivo trae banderas: el idioma y la plataforma se deducen del
    /// nombre (`Aatrox.es_MX.wad.client`).
    PathMarkers(Vec<String>),
}

/// Las banderas del manifiesto, listas para leer la máscara de cada archivo. En RMAN 2.1 los ids
/// empiezan en 1 y la máscara usa el bit `id - 1`, pero cdragon-rman 0.2 prueba el bit `id`: se le
/// pasan ids desplazados. Un id fuera de 1..=64 no cabe en la máscara (y con 65 o más la
/// biblioteca desbordaría el desplazamiento), así que se omite.
fn mask_flags(rman: &Rman) -> Vec<FileFlagEntry<'_>> {
    rman.iter_flags()
        .filter(|flag| (1..=64).contains(&flag.id))
        .map(|flag| FileFlagEntry {
            id: flag.id - 1,
            flag: flag.flag,
        })
        .collect()
}

/// Rutas completas de los directorios, igual que `Rman::dir_paths` pero sin sus fallos con
/// manifiestos dañados: un ciclo de padres la haría girar para siempre (sin pánico que atrapar) y
/// un padre inexistente la haría entrar en pánico.
fn directory_paths(rman: &Rman) -> Result<HashMap<u64, String>, ClientInstallError> {
    // Con ids repetidos la biblioteca busca al padre en el primero y deja el último en el mapa.
    let mut known = HashMap::new();
    for directory in rman.iter_directories() {
        known
            .entry(directory.id)
            .or_insert((directory.parent_id, directory.name));
    }

    let mut paths = HashMap::with_capacity(known.len());
    for directory in rman.iter_directories() {
        // Un directorio que se tiene a sí mismo de padre es la raíz.
        let mut parent = directory.parent_id.filter(|id| *id != directory.id);
        let mut parts = vec![directory.name];
        while let Some(id) = parent {
            // Una cadena sin ciclos pasa por cada directorio una sola vez.
            if parts.len() >= known.len() {
                return Err(ClientInstallError::CorruptManifest);
            }
            let &(next, name) = known.get(&id).ok_or(ClientInstallError::CorruptManifest)?;
            parts.push(name);
            parent = next.filter(|next| *next != id);
        }
        parts.reverse();
        paths.insert(directory.id, parts.join("/"));
    }
    Ok(paths)
}

/// Un archivo con banderas de idioma se instala solo si trae el pedido, y uno con banderas de
/// plataforma solo si trae Windows. Las banderas de otra clase no restringen nada.
fn wanted_by_flags<'a>(file_flags: impl Iterator<Item = &'a str>, locale: &str) -> bool {
    let (mut has_locale, mut for_locale) = (false, false);
    let (mut has_platform, mut for_platform) = (false, false);
    for flag in file_flags {
        if PLATFORM_MARKERS.contains(&flag) {
            has_platform = true;
            for_platform |= flag == PLATFORM;
        } else if let Some(flag_locale) = normalize_locale(flag) {
            has_locale = true;
            for_locale |= flag_locale == locale;
        }
    }
    (!has_locale || for_locale) && (!has_platform || for_platform)
}

fn select_install_files(rman: &Rman, locale: &str) -> Result<Vec<InstallFile>, ClientInstallError> {
    let flags = mask_flags(rman);
    let uses_flags = rman.iter_files().any(|file| {
        file.flags
            .as_ref()
            .is_some_and(|set| set.iter(flags.iter()).next().is_some())
    });
    let selection = if uses_flags {
        let offered = flags
            .iter()
            .any(|flag| normalize_locale(flag.flag).is_some_and(|offered| offered == locale));
        if !offered {
            return Err(ClientInstallError::UnavailableLocale(locale.to_string()));
        }
        FileSelection::Flags(flags)
    } else {
        let locale_markers = rman
            .iter_flags()
            .filter_map(|flag| normalize_locale(flag.flag))
            .map(|flag| locale_asset_marker(&flag).to_string())
            .collect::<Vec<_>>();
        if !locale_markers
            .iter()
            .any(|flag| flag == locale_asset_marker(locale))
        {
            return Err(ClientInstallError::UnavailableLocale(locale.to_string()));
        }
        FileSelection::PathMarkers(locale_markers)
    };

    let directories = directory_paths(rman)?;
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
        let wanted = match &selection {
            FileSelection::Flags(flags) => match &file.flags {
                Some(set) => wanted_by_flags(set.iter(flags.iter()), locale),
                None => true,
            },
            FileSelection::PathMarkers(locale_markers) => {
                path_matches_locale(&path, locale_asset_marker(locale), locale_markers)
            }
        };
        if !wanted {
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
            index: files.len(),
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
    /// Bytes de bundle de lo que ya estaba en los `.part` conservados: el progreso arranca ahí.
    stored_bytes: u64,
    /// Bytes de bundle que faltan descargar.
    missing_bytes: u64,
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

/// Fragmento único que todavía falta en algún archivo: aunque varios lo usen, se descarga una sola
/// vez.
#[derive(Debug)]
struct NeededChunk {
    bundle_id: u64,
    bundle_offset: u32,
    bundle_size: u32,
    target_size: u32,
    /// Posiciones que faltan escribir, por orden de ruta del archivo.
    placements: Vec<Placement>,
}

/// Fragmento de una ventana: dónde está en la respuesta y qué fragmento único es.
#[derive(Debug)]
struct WindowChunk {
    bundle_offset: u32,
    bundle_size: u32,
    target_size: u32,
    chunk: usize,
}

/// Rango contiguo de un bundle que se pide en una sola petición HTTP.
#[derive(Debug)]
struct DownloadWindow {
    bundle_id: u64,
    start: u32,
    end: u32,
    chunks: Vec<WindowChunk>,
    /// Archivo que se estaba terminando cuando se ordenó la ventana: el detalle del progreso.
    file_index: usize,
}

/// Lo que falta descargar de los archivos pendientes.
#[derive(Debug)]
struct DownloadWork {
    chunks: Vec<NeededChunk>,
    /// En el orden en que se piden.
    windows: Vec<DownloadWindow>,
    /// Bytes que faltan escribir en cada archivo.
    remaining: Vec<u64>,
    /// Bytes de bundle de los fragmentos que ya están en los `.part` conservados.
    stored_bytes: u64,
    /// Bytes de bundle que faltan descargar.
    missing_bytes: u64,
}

/// Posición de cada archivo en orden de ruta: un orden estable que no depende del orden interno
/// del manifiesto. Decide dónde aterriza un fragmento compartido y qué archivo empieza después.
fn path_rank(files: &[InstallFile]) -> Vec<usize> {
    let mut by_path = (0..files.len()).collect::<Vec<_>>();
    by_path.sort_by(|a, b| files[*a].path.cmp(&files[*b].path));
    let mut rank = vec![0; files.len()];
    for (position, file) in by_path.into_iter().enumerate() {
        rank[file] = position;
    }
    rank
}

/// Fragmentos que faltan: los de cada archivo pendiente menos los que el diario ya registra en su
/// `.part`. Cada fragmento único se cuenta una vez. Devuelve también los bytes que faltan escribir
/// en cada archivo y los bytes de bundle que ya están en disco. Los fragmentos de cada archivo
/// deben sumar su tamaño.
fn needed_chunks(
    files: &[InstallFile],
    kept: &KeptParts,
) -> Result<(Vec<NeededChunk>, Vec<u64>, u64), ClientInstallError> {
    let rank = path_rank(files);
    let mut unique: HashMap<(u64, u32), usize> = HashMap::new();
    let mut chunks: Vec<NeededChunk> = Vec::new();
    let mut remaining = vec![0_u64; files.len()];
    for (file_index, file) in files.iter().enumerate() {
        let written = kept.ranges(file_index);
        let mut target_offset = 0_u64;
        for chunk in &file.chunks {
            if chunk.bundle_size == 0 {
                return Err(ClientInstallError::CorruptManifest);
            }
            let id = *unique
                .entry((chunk.bundle_id, chunk.bundle_offset))
                .or_insert_with(|| {
                    chunks.push(NeededChunk {
                        bundle_id: chunk.bundle_id,
                        bundle_offset: chunk.bundle_offset,
                        bundle_size: chunk.bundle_size,
                        target_size: chunk.target_size,
                        placements: Vec::new(),
                    });
                    chunks.len() - 1
                });
            let size = u64::from(chunk.target_size);
            if !covers(written, target_offset, size) {
                chunks[id].placements.push(Placement {
                    file_index,
                    target_offset,
                });
                remaining[file_index] += size;
            }
            target_offset += size;
        }
        if target_offset != u64::from(file.filesize) {
            return Err(ClientInstallError::ChunkLayoutMismatch(file.path.clone()));
        }
    }
    let stored_bytes = chunks
        .iter()
        .filter(|chunk| chunk.placements.is_empty())
        .map(|chunk| u64::from(chunk.bundle_size))
        .sum();
    chunks.retain(|chunk| !chunk.placements.is_empty());
    for chunk in &mut chunks {
        chunk
            .placements
            .sort_by_key(|placement| (rank[placement.file_index], placement.target_offset));
    }
    Ok((chunks, remaining, stored_bytes))
}

/// Agrupa los fragmentos que faltan por bundle, fusiona los contiguos en ventanas de hasta
/// `max_window` bytes y las ordena con [`order_windows`].
fn plan_download(
    files: &[InstallFile],
    kept: &KeptParts,
    max_window: u32,
) -> Result<DownloadWork, ClientInstallError> {
    let (chunks, remaining, stored_bytes) = needed_chunks(files, kept)?;
    let missing_bytes = chunks
        .iter()
        .map(|chunk| u64::from(chunk.bundle_size))
        .sum();
    let mut by_bundle: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (id, chunk) in chunks.iter().enumerate() {
        by_bundle.entry(chunk.bundle_id).or_default().push(id);
    }

    let mut windows = Vec::new();
    for (bundle_id, mut ids) in by_bundle {
        ids.sort_by_key(|id| chunks[*id].bundle_offset);
        let mut current: Option<DownloadWindow> = None;
        for id in ids {
            let chunk = &chunks[id];
            let end = chunk
                .bundle_offset
                .checked_add(chunk.bundle_size)
                .ok_or(ClientInstallError::CorruptManifest)?;
            let piece = WindowChunk {
                bundle_offset: chunk.bundle_offset,
                bundle_size: chunk.bundle_size,
                target_size: chunk.target_size,
                chunk: id,
            };
            let extends = current.as_ref().is_some_and(|window| {
                window.end == chunk.bundle_offset && end - window.start <= max_window
            });
            if extends {
                let window = current.as_mut().expect("window in progress");
                window.end = end;
                window.chunks.push(piece);
            } else {
                windows.extend(current.take());
                current = Some(DownloadWindow {
                    bundle_id,
                    start: chunk.bundle_offset,
                    end,
                    chunks: vec![piece],
                    file_index: 0,
                });
            }
        }
        windows.extend(current);
    }
    let windows = order_windows(windows, &chunks, &path_rank(files));
    Ok(DownloadWork {
        chunks,
        windows,
        remaining,
        stored_bytes,
        missing_bytes,
    })
}

/// Ordena las ventanas para que los archivos se terminen uno tras otro. Un bundle guarda fragmentos
/// de muchos archivos, así que pedir las ventanas por bundle deja cientos de `.part` a medias, y
/// en NTFS cada escritura suelta en un `.part` disperso reserva una unidad entera de 64 KiB.
///
/// Cada fragmento cuenta para el archivo donde aterriza (el primero por ruta). Se termina el
/// archivo empezado al que le quedan menos ventanas, y al terminarlo se piden todas las suyas; si
/// no hay ninguno empezado, sigue el próximo por ruta. Cada ventana recuerda el archivo por el que
/// se pidió: es el detalle del progreso.
fn order_windows(
    windows: Vec<DownloadWindow>,
    chunks: &[NeededChunk],
    rank: &[usize],
) -> Vec<DownloadWindow> {
    let file_count = rank.len();
    // Archivos donde aterriza cada ventana y, por archivo, sus ventanas desde la primera posición
    // que escriben en él.
    let mut touches = Vec::with_capacity(windows.len());
    let mut of_file: Vec<Vec<(u64, usize)>> = vec![Vec::new(); file_count];
    for (index, window) in windows.iter().enumerate() {
        let mut landings = window
            .chunks
            .iter()
            .filter_map(|piece| chunks[piece.chunk].placements.first())
            .map(|placement| (placement.file_index, placement.target_offset))
            .collect::<Vec<_>>();
        landings.sort_unstable();
        landings.dedup_by_key(|(file, _)| *file);
        for (file, offset) in &landings {
            of_file[*file].push((*offset, index));
        }
        touches.push(
            landings
                .into_iter()
                .map(|(file, _)| file)
                .collect::<Vec<_>>(),
        );
    }
    for list in &mut of_file {
        list.sort_unstable();
    }

    let mut by_rank = (0..file_count).collect::<Vec<_>>();
    by_rank.sort_by_key(|file| rank[*file]);
    let mut remaining = of_file.iter().map(Vec::len).collect::<Vec<_>>();
    // Archivos empezados: (ventanas que les quedan, rango, archivo).
    let mut started: BTreeSet<(usize, usize, usize)> = BTreeSet::new();
    let mut emitted = vec![false; windows.len()];
    let mut order = Vec::with_capacity(windows.len());
    let mut cursor = 0;
    loop {
        let file = match started.first() {
            Some(&(_, _, file)) => file,
            None => {
                while cursor < file_count && remaining[by_rank[cursor]] == 0 {
                    cursor += 1;
                }
                let Some(&file) = by_rank.get(cursor) else {
                    break;
                };
                file
            }
        };
        for &(_, window) in &of_file[file] {
            if std::mem::replace(&mut emitted[window], true) {
                continue;
            }
            order.push((window, file));
            for &touched in &touches[window] {
                started.remove(&(remaining[touched], rank[touched], touched));
                remaining[touched] -= 1;
                if remaining[touched] > 0 {
                    started.insert((remaining[touched], rank[touched], touched));
                }
            }
        }
    }

    let mut slots = windows.into_iter().map(Some).collect::<Vec<_>>();
    order
        .into_iter()
        .filter_map(|(window, file)| {
            let mut window = slots[window].take()?;
            window.file_index = file;
            Some(window)
        })
        .collect()
}

/// `true` si `[offset, offset + len)` está dentro de uno de los rangos (ordenados y disjuntos).
fn covers(ranges: &[(u64, u64)], offset: u64, len: u64) -> bool {
    let after = ranges.partition_point(|(start, _)| *start <= offset);
    after > 0 && ranges[after - 1].1 >= offset + len
}

/// Ordena y une rangos que se tocan o se solapan.
fn merge_ranges(mut ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Sin datos en disco.
    Waiting,
    /// Con un `.part` a medio armar.
    Assembling,
    Done,
}

/// Lo que hay que hacer con los bytes de un fragmento: lo decide [`Assembly`] y lo ejecuta
/// [`FileWriter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Escribe el fragmento recibido en esa posición.
    Write(Placement),
    /// Copia `len` bytes de un fragmento compartido que ya está escrito en `from`.
    Copy {
        to: Placement,
        from: Placement,
        len: u32,
    },
    /// El archivo quedó completo.
    Commit(usize),
}

/// Cuentas de los archivos y fragmentos pendientes, sin E/S: decide dónde va cada fragmento. Se
/// escribe en los archivos que ya están a medio armar; si ninguno lo está, aterriza en el primero
/// por ruta. En los demás archivos que lo usan se copia desde ahí cuando empiezan, así un fragmento
/// compartido por cientos de archivos no los deja a todos a medias. Un archivo cuyos fragmentos ya
/// aterrizaron todos en otros se arma entero con copias, sin red.
#[derive(Debug)]
struct Assembly {
    stages: Vec<Stage>,
    remaining: Vec<u64>,
    /// Posiciones pendientes de cada archivo cuyo fragmento todavía no llegó.
    unavailable: Vec<usize>,
    /// Copias que esperan a que su archivo empiece: fragmento y posición.
    deferred: Vec<Vec<(usize, u64)>>,
    placements: Vec<Vec<Placement>>,
    sizes: Vec<u32>,
    /// Dónde quedó escrito cada fragmento: la fuente de sus copias.
    sources: Vec<Option<Placement>>,
}

impl Assembly {
    /// `assembling` marca los archivos que ya tienen un `.part` conservado.
    fn new(chunks: Vec<NeededChunk>, remaining: Vec<u64>, assembling: &[bool]) -> Self {
        let mut unavailable = vec![0; remaining.len()];
        let mut placements = Vec::with_capacity(chunks.len());
        let mut sizes = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            for placement in &chunk.placements {
                unavailable[placement.file_index] += 1;
            }
            placements.push(chunk.placements);
            sizes.push(chunk.target_size);
        }
        Self {
            stages: assembling
                .iter()
                .map(|kept| {
                    if *kept {
                        Stage::Assembling
                    } else {
                        Stage::Waiting
                    }
                })
                .collect(),
            deferred: vec![Vec::new(); remaining.len()],
            remaining,
            unavailable,
            sources: vec![None; placements.len()],
            placements,
            sizes,
        }
    }

    /// Archivos que el diario ya tiene completos: se confirman al empezar.
    fn prepare(&mut self, steps: &mut Vec<Step>) {
        for (file, stage) in self.stages.iter_mut().enumerate() {
            if *stage == Stage::Assembling && self.remaining[file] == 0 {
                *stage = Stage::Done;
                steps.push(Step::Commit(file));
            }
        }
    }

    /// Decide dónde se escribe un fragmento recién llegado.
    fn land(&mut self, chunk: usize, steps: &mut Vec<Step>) {
        let placements = std::mem::take(&mut self.placements[chunk]);
        let len = self.sizes[chunk];
        let landing = placements
            .iter()
            .find(|placement| self.stages[placement.file_index] == Stage::Assembling)
            .or_else(|| placements.first())
            .map(|placement| placement.file_index);
        let mut deferred = Vec::new();
        for &placement in &placements {
            let file = placement.file_index;
            self.unavailable[file] -= 1;
            match self.stages[file] {
                Stage::Assembling => {}
                Stage::Waiting if Some(file) == landing => self.start(file, steps),
                _ => {
                    deferred.push(placement);
                    continue;
                }
            }
            steps.push(Step::Write(placement));
            self.sources[chunk].get_or_insert(placement);
            self.wrote(file, len, steps);
        }
        for placement in &deferred {
            self.deferred[placement.file_index].push((chunk, placement.target_offset));
        }
        for placement in deferred {
            let file = placement.file_index;
            if self.stages[file] == Stage::Waiting && self.unavailable[file] == 0 {
                self.start(file, steps);
            }
        }
    }

    /// El archivo empieza: primero las copias de los fragmentos que ya aterrizaron en otros.
    fn start(&mut self, file: usize, steps: &mut Vec<Step>) {
        self.stages[file] = Stage::Assembling;
        for (chunk, target_offset) in std::mem::take(&mut self.deferred[file]) {
            let from = self.sources[chunk].expect("a deferred chunk already landed");
            let len = self.sizes[chunk];
            steps.push(Step::Copy {
                to: Placement {
                    file_index: file,
                    target_offset,
                },
                from,
                len,
            });
            self.wrote(file, len, steps);
        }
    }

    fn wrote(&mut self, file: usize, len: u32, steps: &mut Vec<Step>) {
        // El plan comprobó que los fragmentos suman el tamaño del archivo.
        self.remaining[file] = self.remaining[file].saturating_sub(u64::from(len));
        if self.remaining[file] == 0 {
            self.stages[file] = Stage::Done;
            steps.push(Step::Commit(file));
        }
    }
}

/// Marca el `.part` como disperso antes de su primera escritura y dice si lo logró. Los fragmentos
/// no llegan en el orden del archivo y, en un archivo normal, NTFS rellena con ceros de forma
/// síncrona todo lo que queda antes de una escritura lejana. Medido en NTFS, 1 MiB escrito a
/// 2 GiB tarda unos 2 s y reserva 2 GiB; en un archivo disperso es inmediato. Ese relleno frenaba
/// al único escritor y, con él, a todas las ventanas en vuelo.
///
/// A cambio, NTFS reserva un archivo disperso en unidades de 16 clústeres (64 KiB con clústeres
/// de 4 KiB), en el momento de escribir: medido el 2026-09-30, 2.000 escrituras sueltas de 28 KiB
/// reservaron 4,57 veces lo escrito (dos unidades cuando cruzan un borde) y de 4 KiB, 16 veces;
/// las contiguas, aun desordenadas, lo justo. Por eso [`order_windows`] termina los archivos uno
/// tras otro. La marca queda al confirmar el archivo: quitarla obliga a volcar en el acto los
/// tramos que se llenaron tarde. Si el sistema de archivos no la admite, se sigue sin ella.
#[cfg(windows)]
fn mark_sparse(file: &File) -> bool {
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
    let marked = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            (&request as *const FILE_SET_SPARSE_BUFFER).cast(),
            std::mem::size_of::<FILE_SET_SPARSE_BUFFER>() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    marked != 0
}

#[cfg(not(windows))]
fn mark_sparse(_file: &File) -> bool {
    false
}

/// Plan al que pertenece un diario: solo se aprovecha con el mismo manifiesto, versión, idioma y
/// destino.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JournalIdentity {
    manifest_url: String,
    resolved_version: String,
    locale: String,
    destination_path: String,
}

impl JournalIdentity {
    fn of(plan: &ClientInstallPlan) -> Self {
        Self {
            manifest_url: plan.manifest_url.clone(),
            resolved_version: plan.resolved_version.clone(),
            locale: plan.locale.clone(),
            destination_path: plan.destination_path.clone(),
        }
    }
}

/// `.chronorift/journal.json`: los `.part` a medio armar y los rangos de cada uno que ya llegaron
/// al disco. Se reemplaza entero en cada punto de control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadJournal {
    version: u32,
    #[serde(flatten)]
    identity: JournalIdentity,
    files: Vec<JournalFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JournalFile {
    /// Ruta del manifiesto.
    path: String,
    /// Nombre del `.part` en la carpeta temporal.
    temp_name: String,
    size: u64,
    /// Disperso y creado con su tamaño final; si no, solo crece hasta lo escrito.
    sparse: bool,
    /// Fecha de creación del `.part` (ns desde 1970): otro archivo con el mismo nombre no vale.
    created: Option<u64>,
    /// Rangos escritos y volcados al disco, ordenados y disjuntos.
    ranges: Vec<(u64, u64)>,
}

impl JournalFile {
    fn bytes(&self) -> u64 {
        self.ranges.iter().map(|(start, end)| end - start).sum()
    }

    /// El `.part` sigue en su lugar, es el mismo archivo y los rangos tienen sentido.
    fn part_matches(&self, temp_dir: &Path) -> bool {
        let mut end = 0;
        for &(start, stop) in &self.ranges {
            if start < end || start >= stop || stop > self.size {
                return false;
            }
            end = stop;
        }
        // Solo un nombre, nunca una ruta: el diario no puede apuntar fuera de la carpeta temporal.
        if Path::new(&self.temp_name).file_name() != Some(OsStr::new(&self.temp_name)) {
            return false;
        }
        let Ok(metadata) = fs::symlink_metadata(temp_dir.join(&self.temp_name)) else {
            return false;
        };
        let length_matches = if self.sparse {
            metadata.len() == self.size
        } else {
            (end..=self.size).contains(&metadata.len())
        };
        metadata.is_file() && length_matches && created_nanos(&metadata) == self.created
    }
}

fn created_nanos(metadata: &fs::Metadata) -> Option<u64> {
    let created = metadata.created().ok()?;
    let since = created.duration_since(std::time::UNIX_EPOCH).ok()?;
    u64::try_from(since.as_nanos()).ok()
}

fn read_journal(metadata_dir: &Path) -> Option<DownloadJournal> {
    let bytes = fs::read(metadata_dir.join(JOURNAL_FILE)).ok()?;
    serde_json::from_slice::<DownloadJournal>(&bytes)
        .ok()
        .filter(|journal| journal.version == JOURNAL_VERSION)
}

/// `.part` conservados por una ejecución anterior que se pueden aprovechar, por archivo pendiente.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct KeptParts(Vec<Option<JournalFile>>);

impl KeptParts {
    /// Las entradas del diario del mismo plan cuyo `.part` sigue intacto. Una entrada de otro
    /// archivo, de otro tamaño o cuyo `.part` cambió no se usa (y su `.part` se quita al confirmar).
    fn load(destination: &Path, identity: &JournalIdentity, files: &[InstallFile]) -> Self {
        let metadata_dir = destination.join(METADATA_DIR);
        let Some(journal) =
            read_journal(&metadata_dir).filter(|journal| journal.identity == *identity)
        else {
            return Self(vec![None; files.len()]);
        };
        let temp_dir = metadata_dir.join(TEMP_DIR);
        let mut entries = journal
            .files
            .into_iter()
            .map(|entry| (entry.path.clone(), entry))
            .collect::<HashMap<_, _>>();
        Self(
            files
                .iter()
                .map(|file| {
                    entries.remove(&file.path).filter(|entry| {
                        entry.size == u64::from(file.filesize)
                            && entry.temp_name == file.temp_name()
                            && entry.part_matches(&temp_dir)
                    })
                })
                .collect(),
        )
    }

    fn ranges(&self, file: usize) -> &[(u64, u64)] {
        self.0
            .get(file)
            .and_then(Option::as_ref)
            .map_or(&[], |entry| entry.ranges.as_slice())
    }

    fn get(&self, file: usize) -> Option<&JournalFile> {
        self.0.get(file).and_then(Option::as_ref)
    }

    fn any(&self) -> bool {
        self.0
            .iter()
            .flatten()
            .any(|entry| !entry.ranges.is_empty())
    }

    fn temp_names(&self) -> HashSet<&str> {
        self.0
            .iter()
            .flatten()
            .map(|entry| entry.temp_name.as_str())
            .collect()
    }
}

/// Bytes que el diario del plan confirmado registra en `.part` que siguen en su lugar y cuyo
/// archivo todavía no existe. Para el estado local: no lee el manifiesto.
fn journaled_bytes(destination: &Path, manifest_url: &str) -> u64 {
    let metadata_dir = destination.join(METADATA_DIR);
    let Some(journal) = read_journal(&metadata_dir) else {
        return 0;
    };
    if journal.identity.manifest_url != manifest_url
        || journal.identity.destination_path != destination.display().to_string()
    {
        return 0;
    }
    let temp_dir = metadata_dir.join(TEMP_DIR);
    journal
        .files
        .iter()
        .filter(|entry| {
            entry.part_matches(&temp_dir)
                && resolve_install_path(destination, &entry.path)
                    .is_ok_and(|target| !target.exists())
        })
        .map(JournalFile::bytes)
        .sum()
}

/// Reemplaza el diario de forma atómica: se escribe aparte, se vuelca y se renombra encima. Sin
/// `.part` que conservar, se quita.
fn write_journal(metadata_dir: &Path, journal: &DownloadJournal) -> Result<(), ClientInstallError> {
    if journal.files.is_empty() {
        return remove_metadata_file(metadata_dir, JOURNAL_FILE);
    }
    let path = metadata_dir.join(JOURNAL_FILE);
    let pending = metadata_dir.join(format!("{JOURNAL_FILE}.tmp"));
    let mut file = File::create(&pending)?;
    serde_json::to_writer(&mut file, journal).map_err(ClientInstallError::Metadata)?;
    file.sync_data()?;
    drop(file);
    fs::rename(&pending, &path)?;
    Ok(())
}

/// Un archivo pendiente y su `.part` en la carpeta temporal.
struct PartFile {
    manifest_path: String,
    target: PathBuf,
    temp_name: String,
    temp_path: PathBuf,
    size: u64,
    handle: Option<File>,
    /// El `.part` existe: lo creó esta ejecución o lo conservó el diario.
    exists: bool,
    sparse: bool,
    created: Option<u64>,
    /// Rangos escritos y volcados al disco: los que registra el diario.
    synced: Vec<(u64, u64)>,
    /// Rangos escritos desde el último punto de control.
    unsynced: Vec<(u64, u64)>,
    committed: bool,
}

impl PartFile {
    /// Abre el `.part`: el conservado por el diario, o uno nuevo, disperso y, si la marca se
    /// aplicó, con su tamaño final (así el diario puede comprobarlo).
    fn open(&mut self) -> Result<&mut File, ClientInstallError> {
        if self.handle.is_none() {
            let mut options = OpenOptions::new();
            options.read(true).write(true);
            let handle = if self.exists {
                options.open(&self.temp_path)?
            } else {
                let handle = options.create_new(true).open(&self.temp_path)?;
                self.sparse = mark_sparse(&handle);
                if self.sparse {
                    handle.set_len(self.size)?;
                }
                self.created = handle.metadata().ok().and_then(|info| created_nanos(&info));
                self.exists = true;
                handle
            };
            self.handle = Some(handle);
        }
        Ok(self.handle.as_mut().expect("temp file is open"))
    }

    fn journal_entry(&self) -> JournalFile {
        JournalFile {
            path: self.manifest_path.clone(),
            temp_name: self.temp_name.clone(),
            size: self.size,
            sparse: self.sparse,
            created: self.created,
            ranges: self.synced.clone(),
        }
    }
}

/// Descomprime y escribe los fragmentos donde decide [`Assembly`], y lleva el diario. Es dueño de
/// los `.part` para poder trabajar en un hilo bloqueante sin frenar las descargas en curso.
struct FileWriter {
    files: Vec<PartFile>,
    assembly: Assembly,
    metadata_dir: PathBuf,
    temp_dir: PathBuf,
    allow_replace: bool,
    /// `install.json` del cliente anterior: deja de describirlo con el primer archivo que se
    /// agrega o reemplaza, así que se quita justo antes.
    stale_install_metadata: Option<PathBuf>,
    identity: JournalIdentity,
    last_checkpoint: Instant,
    /// El diario en disco ya no describe los `.part`: un archivo se confirmó, o es el comienzo.
    journal_stale: bool,
    steps: Vec<Step>,
}

impl FileWriter {
    fn new(
        destination: &Path,
        files: &[InstallFile],
        kept: &KeptParts,
        assembly: Assembly,
        identity: JournalIdentity,
        allow_replace: bool,
    ) -> Result<Self, ClientInstallError> {
        let metadata_dir = destination.join(METADATA_DIR);
        let temp_dir = metadata_dir.join(TEMP_DIR);
        fs::create_dir_all(&temp_dir)?;
        let mut parts = Vec::with_capacity(files.len());
        for (index, file) in files.iter().enumerate() {
            let entry = kept.get(index);
            let temp_name = file.temp_name();
            parts.push(PartFile {
                manifest_path: file.path.clone(),
                target: resolve_install_path(destination, &file.path)?,
                temp_path: temp_dir.join(&temp_name),
                temp_name,
                size: u64::from(file.filesize),
                handle: None,
                exists: entry.is_some(),
                sparse: entry.is_some_and(|entry| entry.sparse),
                created: entry.and_then(|entry| entry.created),
                synced: entry.map(|entry| entry.ranges.clone()).unwrap_or_default(),
                unsynced: Vec::new(),
                committed: false,
            });
        }
        Ok(Self {
            files: parts,
            assembly,
            stale_install_metadata: Some(metadata_dir.join(INSTALL_METADATA)),
            metadata_dir,
            temp_dir,
            allow_replace,
            identity,
            last_checkpoint: Instant::now(),
            journal_stale: true,
            steps: Vec::new(),
        })
    }

    /// Confirma los archivos que el diario ya tiene completos y deja el diario solo con los
    /// `.part` que se conservaron.
    fn prepare(&mut self) -> Result<(), ClientInstallError> {
        let mut steps = std::mem::take(&mut self.steps);
        steps.clear();
        self.assembly.prepare(&mut steps);
        let outcome = self.execute(&steps, &[]);
        self.steps = steps;
        outcome?;
        self.checkpoint()
    }

    /// Escribe cada fragmento de la ventana donde corresponde y confirma los archivos que quedan
    /// completos, así una interrupción conserva lo terminado.
    fn apply(&mut self, window: &DownloadWindow, data: &[u8]) -> Result<(), ClientInstallError> {
        for piece in &window.chunks {
            let start = (piece.bundle_offset - window.start) as usize;
            let compressed = &data[start..start + piece.bundle_size as usize];
            let decompressed = zstd::bulk::decompress(compressed, piece.target_size as usize)
                .map_err(ClientInstallError::Zstd)?;
            if decompressed.len() != piece.target_size as usize {
                return Err(ClientInstallError::InvalidChunkData {
                    expected: piece.target_size,
                    actual: decompressed.len(),
                });
            }
            let mut steps = std::mem::take(&mut self.steps);
            steps.clear();
            self.assembly.land(piece.chunk, &mut steps);
            let outcome = self.execute(&steps, &decompressed);
            self.steps = steps;
            outcome?;
        }
        if self.last_checkpoint.elapsed() >= CHECKPOINT_INTERVAL {
            self.checkpoint()?;
        }
        Ok(())
    }

    fn execute(&mut self, steps: &[Step], data: &[u8]) -> Result<(), ClientInstallError> {
        for step in steps {
            match *step {
                Step::Write(placement) => self.write_at(placement, data)?,
                Step::Copy { to, from, len } => {
                    let copied = self.read_at(from, len)?;
                    self.write_at(to, &copied)?;
                }
                Step::Commit(file) => self.commit(file)?,
            }
        }
        Ok(())
    }

    fn write_at(&mut self, placement: Placement, data: &[u8]) -> Result<(), ClientInstallError> {
        let file = &mut self.files[placement.file_index];
        let handle = file.open()?;
        handle.seek(SeekFrom::Start(placement.target_offset))?;
        handle.write_all(data)?;
        file.unsynced.push((
            placement.target_offset,
            placement.target_offset + data.len() as u64,
        ));
        Ok(())
    }

    /// Lee un fragmento ya escrito: del `.part`, o del archivo si ya se confirmó.
    fn read_at(&mut self, placement: Placement, len: u32) -> Result<Vec<u8>, ClientInstallError> {
        let file = &mut self.files[placement.file_index];
        let mut data = vec![0; len as usize];
        if file.committed {
            let mut source = File::open(&file.target)?;
            source.seek(SeekFrom::Start(placement.target_offset))?;
            source.read_exact(&mut data)?;
        } else {
            let handle = file.open()?;
            handle.seek(SeekFrom::Start(placement.target_offset))?;
            handle.read_exact(&mut data)?;
        }
        Ok(data)
    }

    fn commit(&mut self, index: usize) -> Result<(), ClientInstallError> {
        if let Some(stale) = self.stale_install_metadata.take() {
            match fs::remove_file(stale) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
                _ => {}
            }
        }
        let file = &mut self.files[index];
        // Un archivo vacío no tuvo escrituras: su `.part` se crea aquí.
        file.open()?;
        // Lo escrito llega al disco antes del renombre: tras un corte de luz, un archivo con el
        // tamaño correcto nunca queda con huecos.
        if let Some(handle) = file.handle.take() {
            handle.sync_data()?;
        }
        commit_downloaded_file(
            &file.temp_path,
            &file.target,
            &self.temp_dir,
            self.allow_replace,
        )?;
        file.committed = true;
        file.synced.clear();
        file.unsynced.clear();
        self.journal_stale = true;
        Ok(())
    }

    /// Punto de control: vuelca los `.part` escritos desde el anterior y recién entonces
    /// reemplaza el diario, que así nunca registra bytes que no llegaron al disco. Sin cambios
    /// (la red quieta), no toca el disco.
    fn checkpoint(&mut self) -> Result<(), ClientInstallError> {
        self.last_checkpoint = Instant::now();
        for file in &mut self.files {
            if file.unsynced.is_empty() {
                continue;
            }
            self.journal_stale = true;
            if let Some(handle) = &file.handle {
                handle.sync_data()?;
            }
            let mut ranges = std::mem::take(&mut file.synced);
            ranges.append(&mut file.unsynced);
            file.synced = merge_ranges(ranges);
        }
        if !self.journal_stale {
            return Ok(());
        }
        let journal = DownloadJournal {
            version: JOURNAL_VERSION,
            identity: self.identity.clone(),
            files: self
                .files
                .iter()
                .filter(|file| !file.committed && !file.synced.is_empty())
                .map(PartFile::journal_entry)
                .collect(),
        };
        // Si no se puede reemplazar (un antivirus que lo tiene abierto, por ejemplo), sigue
        // valiendo el anterior, que solo registra bytes ya volcados: se reintenta en el próximo.
        if write_journal(&self.metadata_dir, &journal).is_ok() {
            self.journal_stale = false;
        }
        Ok(())
    }

    /// Confirma los archivos vacíos (sin fragmentos), comprueba que ninguno quedó incompleto y
    /// quita el diario.
    fn finish(&mut self) -> Result<(), ClientInstallError> {
        for index in 0..self.files.len() {
            if self.files[index].committed {
                continue;
            }
            if self.assembly.remaining[index] != 0 {
                return Err(ClientInstallError::ChunkLayoutMismatch(
                    self.files[index].manifest_path.clone(),
                ));
            }
            self.commit(index)?;
        }
        remove_metadata_file(&self.metadata_dir, JOURNAL_FILE)
    }

    /// Tras una parada o un fallo: último punto de control, y los `.part` quedan para reanudar.
    /// Si el volcado falla, sigue valiendo el diario anterior.
    fn suspend(&mut self) {
        let _ = self.checkpoint();
        for file in &mut self.files {
            file.handle = None;
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

type FetchTask = tauri::async_runtime::JoinHandle<Result<Vec<u8>, ClientInstallError>>;
/// El escritor va en una caja: viaja entre el bucle y el hilo de escritura en cada ventana.
type WriterTask =
    tauri::async_runtime::JoinHandle<(Box<FileWriter>, Result<(), ClientInstallError>)>;

/// Lo que despierta al bucle de la descarga.
enum Event {
    /// Terminó la petición de esa posición de `in_flight`.
    Fetched(
        usize,
        Result<Result<Vec<u8>, ClientInstallError>, tauri::Error>,
    ),
    /// El hilo de escritura terminó su trabajo y devuelve el escritor.
    Written(Result<(Box<FileWriter>, Result<(), ClientInstallError>), tauri::Error>),
    Tick,
}

/// Espera la primera petición que termine, el hilo de escritura o el próximo tic.
async fn next_event(
    in_flight: &mut [(usize, FetchTask)],
    writing: &mut Option<WriterTask>,
    ticker: &mut Interval,
) -> Event {
    std::future::poll_fn(|cx| {
        if let Some(task) = writing.as_mut() {
            if let Poll::Ready(outcome) = Pin::new(task).poll(cx) {
                return Poll::Ready(Event::Written(outcome));
            }
        }
        for (slot, (_, task)) in in_flight.iter_mut().enumerate() {
            if let Poll::Ready(fetched) = Pin::new(task).poll(cx) {
                return Poll::Ready(Event::Fetched(slot, fetched));
            }
        }
        ticker.poll_tick(cx).map(|_| Event::Tick)
    })
    .await
}

impl Downloader<'_> {
    /// Mantiene hasta `connections` peticiones en curso como tareas independientes, cada una con
    /// sus reintentos, y escribe cada ventana en cuanto llega, en un hilo bloqueante: una ventana
    /// que espera un reintento no frena a las demás. La memoria queda acotada a unas
    /// `connections + 2` ventanas: las que están en vuelo o esperan al disco, más la que se
    /// escribe. Mientras espera revisa la parada y publica los bytes recibidos.
    async fn run(
        &mut self,
        windows: Arc<Vec<DownloadWindow>>,
        files: &[InstallFile],
        writer: FileWriter,
    ) -> (Option<FileWriter>, Result<(), ClientInstallError>) {
        // El detalle del progreso es el archivo de la primera ventana que falta escribir.
        let detail = |index: usize| {
            windows
                .get(index.min(windows.len().saturating_sub(1)))
                .and_then(|window| files.get(window.file_index))
                .map_or("", |file| file.path.as_str())
        };
        let spawn_writer =
            |mut writer: Box<FileWriter>, job: Option<(usize, Vec<u8>)>| -> WriterTask {
                let windows = Arc::clone(&windows);
                tauri::async_runtime::spawn_blocking(move || {
                    let outcome = match job {
                        Some((index, data)) => writer.apply(&windows[index], &data),
                        None => writer.prepare(),
                    };
                    (writer, outcome)
                })
            };
        let mut idle = None;
        // Lo primero es confirmar lo que el diario ya tiene completo.
        let mut writing = Some(spawn_writer(Box::new(writer), None));
        let mut writing_index = None;
        let mut in_flight: Vec<(usize, FetchTask)> = Vec::new();
        let mut arrived: VecDeque<(usize, Vec<u8>)> = VecDeque::new();
        let mut written = vec![false; windows.len()];
        let mut oldest = 0;
        let mut next = 0;
        let mut ticker = ticker();
        let failure = 'windows: {
            if let Err(error) = self.report(detail(0), Report::Always) {
                break 'windows Some(error);
            }
            loop {
                while next < windows.len()
                    && in_flight.len() < self.tuning.connections
                    && in_flight.len() + arrived.len() <= self.tuning.connections
                {
                    in_flight.push((next, self.spawn_fetch(&windows[next])));
                    next += 1;
                }
                if writing.is_none() {
                    if let Some((index, data)) = arrived.pop_front() {
                        let owned = idle.take().expect("writer is available between jobs");
                        writing = Some(spawn_writer(owned, Some((index, data))));
                        writing_index = Some(index);
                    } else if in_flight.is_empty() && next == windows.len() {
                        break;
                    }
                }
                match next_event(&mut in_flight, &mut writing, &mut ticker).await {
                    Event::Fetched(slot, fetched) => {
                        let (index, _) = in_flight.swap_remove(slot);
                        match fetched {
                            Ok(Ok(data)) => arrived.push_back((index, data)),
                            Ok(Err(error)) => break 'windows Some(error),
                            Err(error) => break 'windows Some(ClientInstallError::Worker(error)),
                        }
                    }
                    Event::Written(outcome) => {
                        writing = None;
                        let (returned, result) = match outcome {
                            Ok(done) => done,
                            Err(error) => break 'windows Some(ClientInstallError::Worker(error)),
                        };
                        idle = Some(returned);
                        if let Err(error) = result {
                            break 'windows Some(error);
                        }
                        if let Some(index) = writing_index.take() {
                            written[index] = true;
                            while written.get(oldest) == Some(&true) {
                                oldest += 1;
                            }
                        }
                        if self.cancel.is_cancelled() {
                            break 'windows Some(ClientInstallError::Cancelled);
                        }
                        if let Err(error) = self.report(detail(oldest), Report::AfterApply) {
                            break 'windows Some(error);
                        }
                    }
                    // Detener no espera a la red: las peticiones se abortan en el próximo tic.
                    Event::Tick if self.cancel.is_cancelled() => {
                        break 'windows Some(ClientInstallError::Cancelled)
                    }
                    Event::Tick => {
                        if let Err(error) = self.report(detail(oldest), Report::IfChanged) {
                            break 'windows Some(error);
                        }
                    }
                }
            }
            // Todo escrito: el último evento de la descarga muestra el total.
            self.report(detail(windows.len()), Report::Always).err()
        };

        for (_, fetch) in &in_flight {
            fetch.abort();
        }
        // El disco no se interrumpe a mitad de ventana: la parada espera a que termine esta.
        if let Some(task) = writing.take() {
            if let Ok((returned, _)) = task.await {
                idle = Some(returned);
            }
        }
        let idle = idle.map(|writer| *writer);
        match failure {
            Some(error) => (idle, Err(error)),
            None => (idle, Ok(())),
        }
    }

    fn spawn_fetch(&self, window: &DownloadWindow) -> FetchTask {
        tauri::async_runtime::spawn(fetch_window(
            self.client.clone(),
            format!("{}/{:016X}.bundle", self.bundle_base, window.bundle_id),
            (window.start, window.end),
            self.tuning.retry_delays,
            Arc::clone(&self.received),
        ))
    }

    /// Publica los bytes que ya estaban en disco más los recibidos hasta ahora. El valor nunca
    /// retrocede: si un reintento descarta bytes, el progreso espera a que el nuevo intento lo
    /// alcance.
    fn report(&mut self, detail: &str, when: Report) -> Result<(), ClientInstallError> {
        let received = self.received.load(Ordering::SeqCst).min(self.missing_bytes);
        let bytes = self.reported_bytes.max(self.stored_bytes + received);
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
            Some(self.stored_bytes + self.missing_bytes),
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
    windows: Vec<DownloadWindow>,
    writer: FileWriter,
) -> Result<(), ClientInstallError> {
    let temp_dir = writer.temp_dir.clone();
    let (writer, outcome) = downloader.run(Arc::new(windows), files, writer).await;
    let Some(mut writer) = writer else {
        // Solo ocurre si falló el hilo de escritura: vale el diario del último punto de control.
        return outcome;
    };
    // Volcar y renombrar es E/S bloqueante: fuera del hilo asíncrono.
    tauri::async_runtime::spawn_blocking(move || {
        let finished = outcome.and_then(|()| writer.finish());
        if finished.is_err() {
            writer.suspend();
        }
        finished
    })
    .await
    .map_err(ClientInstallError::Worker)??;

    // Solo quita la carpeta temporal si quedó vacía; nunca borra archivos del usuario.
    let _ = fs::remove_dir(&temp_dir);
    Ok(())
}

/// Quita lo que dejó una ejecución anterior en la carpeta temporal de ChronoRift y ya no sirve:
/// los `.part` que el diario no conserva para este plan y los `.backup` de una reparación cortada
/// entre sus dos renombres (el archivo reemplazado, del tamaño equivocado). Sin `.part`
/// conservados, también el diario. Solo corre al confirmar un plan; lo desconocido se queda.
fn remove_stale_temp_files(destination: &Path, kept: &KeptParts) -> Result<(), ClientInstallError> {
    let metadata_dir = destination.join(METADATA_DIR);
    let keep = kept.temp_names();
    if keep.is_empty() {
        remove_metadata_file(&metadata_dir, JOURNAL_FILE)?;
    }
    let temp_dir = metadata_dir.join(TEMP_DIR);
    let entries = match fs::read_dir(&temp_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let path = entry?.path();
        let stale = path
            .extension()
            .is_some_and(|extension| extension == TEMP_SUFFIX || extension == BACKUP_SUFFIX)
            && !path
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| keep.contains(name));
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
    remove_metadata_file(&destination.join(METADATA_DIR), name)
}

fn remove_metadata_file(metadata_dir: &Path, name: &str) -> Result<(), ClientInstallError> {
    match fs::remove_file(metadata_dir.join(name)) {
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
    use crate::i18n;
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

    /// Manifiesto de un solo bundle con un fragmento por archivo. Cada archivo trae
    /// `(directorio, nombre, ids de bandera)`; la máscara sigue la regla de Riot (bit `id - 1`),
    /// escrita aquí aparte para que una lectura desplazada en producción no pase inadvertida.
    fn flagged_manifest(
        flags: &[(u8, &str)],
        directories: &[(u64, Option<u64>, &str)],
        files: &[(Option<u64>, &str, &[u8])],
    ) -> Vec<u8> {
        let chunks = (0..files.len())
            .map(|index| (0x100 + index as u64, 5, 10))
            .collect::<Vec<_>>();
        let manifest_files = files
            .iter()
            .enumerate()
            .map(|(index, &(directory, name, ids))| {
                let mask = ids.iter().fold(0_u64, |mask, id| mask | 1 << (id - 1));
                (
                    index as u64 + 1,
                    directory,
                    name,
                    10,
                    vec![0x100 + index as u64],
                    (!ids.is_empty()).then_some(mask),
                )
            })
            .collect::<Vec<_>>();
        encode_test_manifest(flags, directories, &[(1, chunks)], &manifest_files)
    }

    fn selected_paths(manifest: &[u8], locale: &str) -> Vec<String> {
        parse_install_files(manifest, locale, None)
            .expect("manifest should plan")
            .into_iter()
            .map(|file| file.path)
            .collect()
    }

    /// La selección sale de las banderas RMAN de cada archivo, no de su nombre, y respeta el
    /// idioma pedido tal cual: es_AR trae sus archivos propios además de los compartidos con es_MX.
    #[test]
    fn selects_files_by_rman_flags_not_by_name() {
        // El id 70 no cabe en una máscara de 64 bits: se omite en vez de desbordar la lectura.
        let flags = [
            (1, "windows"),
            (2, "macos"),
            (3, "es_AR"),
            (4, "es_MX"),
            (5, "fr_FR"),
            (6, "ko_KR"),
            (70, "en_GB"),
        ];
        let manifest = flagged_manifest(
            &flags,
            &[(1, None, "DATA")],
            &[
                (None, GAME_EXE, &[]),
                (Some(1), "Bootstrap.windows.wad.client", &[1]),
                (Some(1), "Bootstrap.macos.wad.client", &[2]),
                (Some(1), "Bootstrap.both.wad.client", &[1, 2]),
                // Los nombres engañan a propósito: manda la bandera.
                (Some(1), "Aatrox.fr_FR.wad.client", &[4]),
                (Some(1), "Aatrox.es_MX.wad.client", &[5]),
                (Some(1), "Shared.es_MX.wad.client", &[3, 4]),
                (Some(1), "UI.es_AR.wad.client", &[3]),
                (Some(1), "Voice.ko_KR.windows.wad.client", &[6, 1]),
                (Some(1), "Voice.ko_KR.macos.wad.client", &[6, 2]),
            ],
        );

        assert_eq!(
            selected_paths(&manifest, "es_MX"),
            [
                GAME_EXE,
                "DATA/Bootstrap.windows.wad.client",
                "DATA/Bootstrap.both.wad.client",
                "DATA/Aatrox.fr_FR.wad.client",
                "DATA/Shared.es_MX.wad.client",
            ]
        );
        assert_eq!(
            selected_paths(&manifest, "es_AR"),
            [
                GAME_EXE,
                "DATA/Bootstrap.windows.wad.client",
                "DATA/Bootstrap.both.wad.client",
                "DATA/Shared.es_MX.wad.client",
                "DATA/UI.es_AR.wad.client",
            ]
        );
        assert_eq!(
            selected_paths(&manifest, "ko_KR"),
            [
                GAME_EXE,
                "DATA/Bootstrap.windows.wad.client",
                "DATA/Bootstrap.both.wad.client",
                "DATA/Voice.ko_KR.windows.wad.client",
            ]
        );
        // Con banderas, un idioma que el manifiesto no ofrece no se sustituye por otro.
        for locale in ["ja_JP", "en_GB"] {
            assert!(
                matches!(
                    parse_install_files(&manifest, locale, None),
                    Err(ClientInstallError::UnavailableLocale(_))
                ),
                "{locale}"
            );
        }
    }

    /// Las rutas de directorio coinciden con las de la biblioteca (incluida una raíz sin nombre),
    /// y un árbol dañado es un manifiesto corrupto: antes un ciclo dejaba el proceso girando.
    #[test]
    fn directory_paths_match_the_library_and_reject_broken_trees() {
        let flags = [(1, "windows"), (2, "es_MX")];
        let file: [(Option<u64>, &str, &[u8]); 1] = [(Some(1), "a.wad.client", &[2])];

        let healthy = flagged_manifest(
            &flags,
            &[(3, Some(2), "FINAL"), (2, Some(1), "DATA"), (1, None, "")],
            &file,
        );
        let rman = read_rman_manifest(&healthy).unwrap();
        assert_eq!(directory_paths(&rman).unwrap(), rman.dir_paths());
        assert_eq!(directory_paths(&rman).unwrap()[&3], "/DATA/FINAL");

        for (label, directories) in [
            ("ciclo", vec![(1, Some(2), "A"), (2, Some(1), "B")]),
            ("padre inexistente", vec![(1, Some(9), "A")]),
        ] {
            let broken = flagged_manifest(&flags, &directories, &file);
            assert!(
                matches!(
                    parse_install_files(&broken, "es_MX", None),
                    Err(ClientInstallError::CorruptManifest)
                ),
                "{label}"
            );
        }

        // Un directorio que es su propio padre es la raíz.
        let selfish = flagged_manifest(&flags, &[(1, Some(1), "DATA")], &file);
        assert_eq!(selected_paths(&selfish, "es_MX"), ["DATA/a.wad.client"]);
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

    /// Un fragmento compartido se pide una sola vez y los contiguos se fusionan hasta el tope de la
    /// ventana. En un cliente con la forma de uno real, el orden nuevo termina los archivos uno
    /// tras otro: pocos `.part` a medias y poca reserva de más en NTFS, con las mismas peticiones
    /// que el orden por bundle de antes.
    #[test]
    fn plans_windows_so_files_finish_one_after_another() {
        let chunk = |bundle_id: u64, bundle_offset: u32, bundle_size: u32| BundleChunk {
            bundle_id,
            bundle_offset,
            bundle_size,
            target_size: bundle_size * 2,
        };
        let file =
            |index: usize, path: &str, filesize: u32, chunks: Vec<BundleChunk>| InstallFile {
                index,
                path: path.to_string(),
                filesize,
                chunks,
            };
        let files = vec![
            file(
                0,
                "a.bin",
                120,
                vec![
                    chunk(2, 0, 10),
                    chunk(2, 10, 10),
                    chunk(2, 20, 10),
                    chunk(1, 0, 30),
                ],
            ),
            file(1, "b.bin", 40, vec![chunk(2, 0, 10), chunk(2, 40, 10)]),
        ];

        let work = plan_download(&files, &KeptParts::default(), 25).unwrap();

        let summary = work
            .windows
            .iter()
            .map(|window| {
                (
                    window.bundle_id,
                    window.start,
                    window.end,
                    window.chunks.len(),
                    window.file_index,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            summary,
            [
                (2, 0, 20, 2, 0),
                (2, 20, 30, 1, 0),
                (1, 0, 30, 1, 0),
                (2, 40, 50, 1, 1)
            ]
        );
        let shared = work.windows[0].chunks[0].chunk;
        assert_eq!(
            work.chunks[shared].placements,
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
        assert_eq!(
            (
                work.missing_bytes,
                work.stored_bytes,
                work.remaining.clone()
            ),
            (70, 0, vec![120, 40])
        );
        // Con el primer fragmento de `a.bin` ya en su `.part`, solo falta el de `b.bin` que no
        // comparte: el compartido se vuelve a pedir para `b.bin`.
        let kept = KeptParts(vec![
            Some(JournalFile {
                path: "a.bin".to_string(),
                temp_name: files[0].temp_name(),
                size: 120,
                sparse: true,
                created: None,
                ranges: vec![(0, 60)],
            }),
            None,
        ]);
        let resumed = plan_download(&files, &kept, 25).unwrap();
        assert_eq!(
            (
                resumed.missing_bytes,
                resumed.stored_bytes,
                resumed.remaining
            ),
            (50, 20, vec![60, 40])
        );
        assert_eq!(DownloadTuning::with_connections(None).connections, 8);
        assert_eq!(DownloadTuning::with_connections(Some(0)).connections, 1);
        assert_eq!(DownloadTuning::with_connections(Some(200)).connections, 32);

        let broken = [file(0, "broken.bin", 0, vec![chunk(1, 0, 0)])];
        assert!(matches!(
            plan_download(&broken, &KeptParts::default(), 25),
            Err(ClientInstallError::CorruptManifest)
        ));
        let short = [file(0, "short.bin", 99, vec![chunk(1, 0, 10)])];
        assert!(matches!(
            plan_download(&short, &KeptParts::default(), 25),
            Err(ClientInstallError::ChunkLayoutMismatch(_))
        ));

        let client = synthetic_client();
        let (before, after) = reservation_peaks(&client, 256 * 1024);
        eprintln!("synthetic client: by bundle {before:?}, file by file {after:?}");
        assert_eq!(before.requests, after.requests);
        assert!(after.files <= 4, "{after:?}");
        assert!(
            after.overhead * 4 < before.overhead,
            "{before:?} vs {after:?}"
        );
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
        //    antes de rendirse. Con una conexión el punto de corte es conocido: el plan empieza
        //    por `Bootstrap` (A1), que queda confirmado, y sigue con `Aatrox.es_MX` (B2), que
        //    agota los reintentos. Nada quedó a medias: ni `.part` ni diario.
        cdn.fail_paths_containing(Some(bundle_b2));
        let failed = run(install_client_with(
            &progress,
            &endpoints,
            request(Some(&first)),
            test_tuning(1, 1),
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
        let bootstrap = "DATA/FINAL/Bootstrap.windows.wad.client";
        assert_eq!(
            fs::read(destination.join(bootstrap)).unwrap(),
            fixture.content(bootstrap)
        );
        for path in [
            GAME_EXE,
            "DATA/FINAL/Shared.wad.client",
            "DATA/FINAL/Champions/Aatrox.es_MX.wad.client",
        ] {
            assert!(!destination.join(path).exists(), "{path}");
        }
        assert!(eventually_empty(&temp_dir));
        assert!(!metadata_dir.join(JOURNAL_FILE).exists());
        // El estado local ve la instalación a medias, con el total del plan confirmado.
        let partial = status_of(&root_text, "16.18.817.5716");
        assert_eq!(partial.state, LocalClientState::Partial);
        assert_eq!(partial.total_install_bytes, Some(first.total_install_bytes));
        assert_eq!(
            partial.installed_bytes,
            fixture.content(bootstrap).len() as u64
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
            (1, 3)
        );
        let merged = DownloadTuning {
            retry_delays: &FAST_RETRIES,
            read_timeout: Duration::from_secs(10),
            progress_interval: Duration::ZERO,
            ..DownloadTuning::with_connections(None)
        };
        let windows = plan_download(
            &resume_planned.pending_files,
            &resume_planned.kept,
            merged.max_window,
        )
        .unwrap()
        .windows
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

    /// Una ventana retenida no frena a las demás: se escriben a medida que llegan. Detener vuelve
    /// enseguida y conserva los archivos confirmados, los `.part` a medio armar y su diario; el
    /// estado local cuenta esos bytes. Reanudar pide solo los fragmentos que faltan, sin volver a
    /// bajar el manifiesto, y el progreso arranca desde lo que ya estaba en disco. El estado
    /// distingue una carpeta que todavía no existe de una unidad que no está.
    #[test]
    fn e2e_stopping_keeps_partial_files_and_resuming_fetches_only_what_is_missing() {
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
        let destination = root.join("16.18.817.5716").join(GAME_FOLDER);
        let metadata_dir = destination.join(METADATA_DIR);
        let temp_dir = metadata_dir.join(TEMP_DIR);
        let journal_path = metadata_dir.join(JOURNAL_FILE);
        let aatrox = "DATA/FINAL/Champions/Aatrox.es_MX.wad.client";
        let committed_bytes = || {
            fixture
                .selected()
                .filter(|(path, _, _)| destination.join(path).is_file())
                .map(|(_, content, _)| content.len() as u64)
                .sum::<u64>()
        };
        // Fragmentos del ejecutable: el primero lo comparte `Shared.wad.client`.
        let exe_chunks = parse_install_files(&fixture.manifest, "es_MX", None)
            .unwrap()
            .into_iter()
            .find(|file| file.path == GAME_EXE)
            .unwrap()
            .chunks;
        let installs = |tuning: DownloadTuning, cancel: &CancelSignal, confirmed| {
            run(install_client_with(
                &ProgressLog::default(),
                &endpoints,
                request(Some(confirmed)),
                tuning,
                cancel,
            ))
        };
        // Detiene la instalación en cuanto `ready` se cumple, y comprueba que vuelve enseguida.
        let stop_when =
            |held: &str, tuning: DownloadTuning, confirmed, ready: &dyn Fn() -> bool| {
                let cancel = CancelSignal::default();
                thread::scope(|scope| {
                    let _held = cdn.hold(held);
                    let running = scope.spawn(|| installs(tuning, &cancel, confirmed));
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while !ready() {
                        assert!(
                            Instant::now() < deadline,
                            "the install never reached the stop point"
                        );
                        thread::sleep(Duration::from_millis(5));
                    }
                    let asked = Instant::now();
                    cancel.cancel();
                    let stopped = running.join().unwrap();
                    // Detener no espera a las ventanas retenidas.
                    assert!(
                        asked.elapsed() < Duration::from_secs(1),
                        "stopping took {:?}",
                        asked.elapsed()
                    );
                    stopped.unwrap_err()
                })
            };

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

        // 1. El CDN retiene el bundle A1, que tiene la primera ventana del plan. Con 3 conexiones,
        //    las dos ventanas de B2 llegan, se escriben y confirman `Aatrox.es_MX` mientras tanto,
        //    y sus lugares los ocupan otras dos de A1: cinco peticiones, tres retenidas.
        let first = plan(request(None)).unwrap();
        assert_eq!(cdn.hits(manifest_path), 1);
        let stopped = stop_when(
            "00000000000000A1.bundle",
            test_tuning(3, 1),
            &first,
            &|| destination.join(aatrox).is_file() && cdn.held() == 3,
        );
        assert_eq!(cdn.hits("/bundles/"), 5);
        assert_eq!(stopped.to_string(), "Descarga detenida");
        assert_eq!(
            i18n::with_language(i18n::Language::En, || stopped.to_string()),
            "Download stopped"
        );
        assert_eq!(
            fs::read(destination.join(aatrox)).unwrap(),
            fixture.content(aatrox)
        );
        assert_eq!(committed_bytes(), fixture.content(aatrox).len() as u64);
        // Nada quedó a medias: sin `.part`, sin diario y sin candado.
        assert!(!temp_dir.exists() || eventually_empty(&temp_dir));
        assert!(!journal_path.exists());
        assert!(!metadata_dir.join(LOCK_FILE).exists());
        let partial = status_of(&root_text, "16.18.817.5716");
        assert_eq!(partial.state, LocalClientState::Partial);
        assert_eq!(partial.total_install_bytes, Some(first.total_install_bytes));
        assert_eq!(partial.installed_bytes, committed_bytes());
        assert_eq!(list_local_clients(&root_text).unwrap(), vec![partial]);

        // 2. Reanudar con una conexión, reteniendo solo la ventana del último fragmento del
        //    ejecutable. `Bootstrap` y `Shared` se confirman; el ejecutable recibe el fragmento que
        //    comparte con `Shared` (copiado del disco) y el segundo, y al detener queda su `.part`
        //    disperso con su tamaño final y el diario con esos 8 KiB.
        let resume = plan(request(None)).unwrap();
        assert_eq!(resume.mode, ClientWorkMode::Resume);
        assert_eq!(
            (resume.reused_file_count, resume.affected_file_count),
            (1, 3)
        );
        let last_chunk = format!("00000000000000A1.bundle#{}#", exe_chunks[2].bundle_offset);
        let exe = fixture.content(GAME_EXE);
        let journaled = (exe.len() - exe_chunks[2].target_size as usize) as u64;
        let exe_part = temp_dir.join(format!(
            "00000-{}.part",
            temp_name_for_manifest_path(GAME_EXE)
        ));
        stop_when(&last_chunk, test_tuning(1, 1), &resume, &|| {
            fs::read(&exe_part)
                .is_ok_and(|part| part.get(..journaled as usize) == exe.get(..journaled as usize))
        });
        assert!(destination.join("DATA/FINAL/Shared.wad.client").is_file());
        assert!(!destination.join(GAME_EXE).exists());
        assert_eq!(fs::metadata(&exe_part).unwrap().len(), exe.len() as u64);
        #[cfg(windows)]
        assert!(is_sparse(&exe_part));
        let journal = read_journal(&metadata_dir).unwrap();
        assert_eq!(journal.identity, JournalIdentity::of(&resume));
        assert_eq!(
            journal
                .files
                .iter()
                .map(|entry| (entry.path.as_str(), entry.ranges.clone()))
                .collect::<Vec<_>>(),
            [(GAME_EXE, vec![(0, journaled)])]
        );
        let partial = status_of(&root_text, "16.18.817.5716");
        assert_eq!(partial.state, LocalClientState::Partial);
        assert_eq!(partial.installed_bytes, committed_bytes() + journaled);
        assert_eq!(partial.total_install_bytes, Some(first.total_install_bytes));

        // 3. El plan cuenta solo lo que falta y la reanudación pide solo eso. El progreso arranca
        //    desde los bytes de bundle que ya estaban en disco, con el total de los archivos
        //    pendientes, y al terminar no quedan diario, `.part` ni manifiesto guardado. Ni el plan
        //    ni las instalaciones de los pasos 2 y 3 volvieron a bajar el manifiesto.
        let last = plan(request(None)).unwrap();
        assert_eq!(last.mode, ClientWorkMode::Resume);
        assert_eq!((last.reused_file_count, last.affected_file_count), (3, 1));
        assert_eq!(last.download_bytes, u64::from(exe_chunks[2].bundle_size));
        assert_eq!(last.install_bytes, exe.len() as u64 - journaled);
        let stored = u64::from(exe_chunks[0].bundle_size + exe_chunks[1].bundle_size);
        let progress = ProgressLog::default();
        // Las peticiones retenidas del paso 2 se atienden al soltarlas: no son de este paso.
        cdn.wait_until_idle();
        let (requests_before, bytes_before) = cdn.bundle_traffic();
        run(install_client_with(
            &progress,
            &endpoints,
            request(Some(&last)),
            test_tuning(1, 1),
            &CancelSignal::default(),
        ))
        .unwrap();
        let (requests_after, bytes_after) = cdn.bundle_traffic();
        assert_eq!(
            (
                requests_after - requests_before,
                (bytes_after - bytes_before) as u64
            ),
            (1, last.download_bytes)
        );
        let events = progress.take();
        let first_download = events
            .iter()
            .find(|event| event.stage == "download")
            .unwrap();
        assert_eq!(
            (first_download.downloaded_bytes, first_download.total_bytes),
            (stored, Some(stored + last.download_bytes))
        );
        let complete = events.last().unwrap();
        assert!(complete.done);
        assert_eq!(complete.total_bytes, Some(stored + last.download_bytes));
        assert_eq!(cdn.hits(manifest_path), 2);
        assert!(!journal_path.exists());
        assert!(!metadata_dir.join(MANIFEST_CACHE_DIR).exists());
        assert!(!temp_dir.exists() || eventually_empty(&temp_dir));
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

    /// Con un manifiesto real (CHRONORIFT_MANIFEST_SAMPLE), comprueba que la selección por banderas
    /// instala todo archivo que lleva el idioma pedido en el nombre, la contrasta con la regla por
    /// rutas de antes e informa los totales, las peticiones del plan y la reserva de más estimada
    /// en NTFS con el orden por bundle de antes y con el de ahora.
    #[test]
    fn real_manifest_flag_selection_keeps_named_locale_files() {
        let Ok(path) = std::env::var("CHRONORIFT_MANIFEST_SAMPLE") else {
            eprintln!("skipped: set CHRONORIFT_MANIFEST_SAMPLE to a Riot lol-game-client manifest");
            return;
        };
        let locale =
            std::env::var("CHRONORIFT_MANIFEST_LOCALE").unwrap_or_else(|_| "es_MX".to_string());
        let bytes = fs::read(path).expect("real manifest should read");
        let files = parse_install_files(&bytes, &locale, None).expect("real manifest should plan");
        assert!(files.iter().any(|file| file.path == GAME_EXE));
        assert!(files.len() > 100);

        let rman = read_rman_manifest(&bytes).unwrap();
        let directories = directory_paths(&rman).unwrap();
        let locale_markers = rman
            .iter_flags()
            .filter_map(|flag| normalize_locale(flag.flag))
            .map(|flag| locale_asset_marker(&flag).to_string())
            .collect::<Vec<_>>();
        let marker = locale_asset_marker(&locale);
        let selected = files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<HashSet<_>>();
        // Archivos que solo eligen las banderas, que solo elige la regla por rutas (de más) y
        // archivos de Windows con el idioma pedido en el nombre que las banderas dejan fuera.
        let mut only_by_flags = Vec::new();
        let mut only_by_path = Vec::new();
        let mut named_but_skipped = Vec::new();
        for file in rman.iter_files() {
            let path = file.path(&directories);
            let chosen = selected.contains(path.as_str());
            match (chosen, path_matches_locale(&path, marker, &locale_markers)) {
                (true, false) => only_by_flags.push(path.clone()),
                (false, true) => only_by_path.push(path.clone()),
                _ => {}
            }
            if !chosen && has_marker(&path, &locale) && !has_marker(&path, "macos") {
                named_but_skipped.push(path);
            }
        }
        let work = plan_download(&files, &KeptParts::default(), MAX_WINDOW_BYTES).unwrap();
        let install_bytes = work.remaining.iter().sum::<u64>();
        let chunk_references = files.iter().map(|file| file.chunks.len()).sum::<usize>();
        eprintln!(
            "{locale}: {} files, {chunk_references} chunk references, {} range requests, download {} B, install {install_bytes} B, {} only by flags, {} only by path rule",
            files.len(),
            work.windows.len(),
            work.missing_bytes,
            only_by_flags.len(),
            only_by_path.len()
        );
        eprintln!(
            "first only by flags: {:?}; first only by path rule: {:?}",
            &only_by_flags[..only_by_flags.len().min(10)],
            &only_by_path[..only_by_path.len().min(10)]
        );
        let (before, after) = reservation_peaks(&files, MAX_WINDOW_BYTES);
        let mib = |bytes: u64| bytes as f64 / 1_048_576.0;
        eprintln!(
            "reserved beyond written, one window at a time: by bundle {} files at once, {:.0} MiB; file by file {} files at once, {:.0} MiB",
            before.files,
            mib(before.overhead),
            after.files,
            mib(after.overhead)
        );
        assert!(
            named_but_skipped.is_empty(),
            "files named for {locale} but skipped by the flags: {named_but_skipped:?}"
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

    /// Unidad que NTFS reserva entera en un archivo disperso con clústeres de 4 KiB: medida el
    /// 2026-09-30, cada escritura suelta reserva una (o dos si cruza un borde) aunque escriba 4 KiB.
    const SPARSE_UNIT: u64 = 64 * 1024;

    /// Pico de archivos a medias y de bytes reservados de más al escribir las ventanas en orden,
    /// una a la vez, y cantidad de peticiones.
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    struct ReservationPeak {
        files: usize,
        overhead: u64,
        requests: usize,
    }

    /// Modelo de la reserva: un `.part` a medio armar reserva cada unidad que tocó; al confirmarlo
    /// queda justo.
    struct ReservationModel {
        units: Vec<HashSet<u64>>,
        written: Vec<u64>,
        sizes: Vec<u64>,
        peak: ReservationPeak,
    }

    impl ReservationModel {
        fn new(files: &[InstallFile]) -> Self {
            Self {
                units: vec![HashSet::new(); files.len()],
                written: vec![0; files.len()],
                sizes: files.iter().map(|file| u64::from(file.filesize)).collect(),
                peak: ReservationPeak::default(),
            }
        }

        /// Devuelve `true` si el archivo quedó completo.
        fn write(&mut self, placement: Placement, len: u32) -> bool {
            let file = placement.file_index;
            let end = placement.target_offset + u64::from(len);
            self.units[file]
                .extend(placement.target_offset / SPARSE_UNIT..end.div_ceil(SPARSE_UNIT));
            self.written[file] += u64::from(len);
            self.written[file] == self.sizes[file]
        }

        fn commit(&mut self, file: usize) {
            self.units[file].clear();
        }

        fn sample(&mut self) {
            let open = (0..self.units.len())
                .filter(|file| !self.units[*file].is_empty())
                .collect::<Vec<_>>();
            let overhead = open
                .iter()
                .map(|file| self.units[*file].len() as u64 * SPARSE_UNIT - self.written[*file])
                .sum::<u64>();
            self.peak.files = self.peak.files.max(open.len());
            self.peak.overhead = self.peak.overhead.max(overhead);
        }
    }

    /// Reserva de más con el orden de antes (por la primera escritura de cada ventana, todas las
    /// posiciones de cada fragmento al llegar) y con el de ahora (`order_windows` y `Assembly`).
    fn reservation_peaks(
        files: &[InstallFile],
        max_window: u32,
    ) -> (ReservationPeak, ReservationPeak) {
        let work = plan_download(files, &KeptParts::default(), max_window).unwrap();

        let mut by_bundle = work.windows.iter().collect::<Vec<_>>();
        by_bundle.sort_by_cached_key(|window| {
            window
                .chunks
                .iter()
                .flat_map(|piece| work.chunks[piece.chunk].placements.iter().copied())
                .min()
        });
        let mut before = ReservationModel::new(files);
        for window in &by_bundle {
            for piece in &window.chunks {
                for placement in &work.chunks[piece.chunk].placements {
                    if before.write(*placement, piece.target_size) {
                        before.commit(placement.file_index);
                    }
                }
            }
            before.sample();
        }
        before.peak.requests = by_bundle.len();

        let mut after = ReservationModel::new(files);
        let requests = work.windows.len();
        let mut assembly = Assembly::new(work.chunks, work.remaining, &vec![false; files.len()]);
        let mut steps = Vec::new();
        for window in &work.windows {
            for piece in &window.chunks {
                steps.clear();
                assembly.land(piece.chunk, &mut steps);
                for step in &steps {
                    match *step {
                        Step::Write(to) => {
                            after.write(to, piece.target_size);
                        }
                        Step::Copy { to, len, .. } => {
                            after.write(to, len);
                        }
                        Step::Commit(file) => after.commit(file),
                    }
                }
            }
            after.sample();
        }
        after.peak.requests = requests;
        (before.peak, after.peak)
    }

    /// Cliente sintético con la forma de uno real: fragmentos de 8 a 60 KiB (unos 28 KiB de
    /// promedio, como el manifiesto real), los fragmentos nuevos de cada versión guardados en sus
    /// propios bundles en orden de ruta, un tercio de los fragmentos cambiados en cada versión,
    /// un índice del manifiesto que no sigue la ruta y un bloque compartido por todos los archivos.
    fn synthetic_client() -> Vec<InstallFile> {
        const FILES: usize = 48;
        const CHUNKS: usize = 40;
        const VERSIONS: usize = 4;
        const BUNDLE_LIMIT: u32 = 1024 * 1024;
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let mut random = move |below: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % below
        };
        // Tamaño de cada fragmento y versión en la que cambió por última vez.
        let sizes = (0..FILES * CHUNKS)
            .map(|_| (8 + random(53) as u32) * 1024)
            .collect::<Vec<_>>();
        // Versiones en las que cambió cada fragmento: en la 0 son todos nuevos.
        let mut changed_at = vec![vec![0]; FILES * CHUNKS];
        for version in 1..VERSIONS {
            for versions in &mut changed_at {
                if random(3) == 0 {
                    versions.push(version);
                }
            }
        }
        let shared = BundleChunk {
            bundle_id: 0xFFFF,
            bundle_offset: 0,
            bundle_size: 2 * 1024,
            target_size: 4 * 1024,
        };
        // Cada versión empaca sus fragmentos nuevos en orden de ruta; los que una versión
        // posterior reemplazó quedan como huecos en sus bundles.
        let mut chunks = vec![None; FILES * CHUNKS];
        let mut bundle_id = 0x1000;
        for version in 0..VERSIONS {
            let mut offset = 0;
            for (slot, versions) in changed_at.iter().enumerate() {
                if !versions.contains(&version) {
                    continue;
                }
                let bundle_size = sizes[slot] / 2;
                if offset + bundle_size > BUNDLE_LIMIT {
                    bundle_id += 1;
                    offset = 0;
                }
                if versions.last() == Some(&version) {
                    chunks[slot] = Some(BundleChunk {
                        bundle_id,
                        bundle_offset: offset,
                        bundle_size,
                        target_size: sizes[slot],
                    });
                }
                offset += bundle_size;
            }
            bundle_id += 1;
        }
        // El índice del manifiesto no sigue la ruta.
        let mut order = (0..FILES).collect::<Vec<_>>();
        for index in (1..FILES).rev() {
            order.swap(index, random(index as u64 + 1) as usize);
        }
        order
            .into_iter()
            .enumerate()
            .map(|(index, by_path)| {
                let mut file_chunks = (0..CHUNKS)
                    .map(|position| chunks[by_path * CHUNKS + position].clone().unwrap())
                    .collect::<Vec<_>>();
                file_chunks.push(shared.clone());
                InstallFile {
                    index,
                    path: format!("DATA/FINAL/{by_path:03}.wad.client"),
                    filesize: file_chunks.iter().map(|chunk| chunk.target_size).sum(),
                    chunks: file_chunks,
                }
            })
            .collect()
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
        /// Ruta de cada petición recibida, antes de retenerla.
        paths: Mutex<Vec<String>>,
        /// Fallos inducidos: vale el primero cuyo fragmento aparece en la ruta y aún tiene usos.
        faults: Mutex<Vec<Fault>>,
        /// Mientras tiene un fragmento, las peticiones que lo contienen esperan. Se compara con
        /// `<ruta>#<inicio del rango>#`, así que puede retener un solo rango.
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

        /// Peticiones recibidas (retenidas incluidas) cuya ruta contiene `fragment`.
        fn hits(&self, fragment: &str) -> usize {
            self.state
                .paths
                .lock()
                .unwrap()
                .iter()
                .filter(|path| path.contains(fragment))
                .count()
        }

        /// Peticiones retenidas ahora mismo.
        fn held(&self) -> usize {
            self.state.held_requests.load(Ordering::SeqCst)
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

        /// Retiene las peticiones cuya ruta (o `<ruta>#<inicio>#`) contiene `fragment` hasta
        /// soltar el guardia.
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
        state.paths.lock().unwrap().push(path.clone());
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
            let key = format!("{path}#{}#", range.map_or(0, |(from, _)| from));
            let retained = |hold: &Option<String>| {
                hold.as_deref()
                    .is_some_and(|fragment| key.contains(fragment))
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

    /// Directorio, nombre, tamaño, bundle, si se instala con es_MX en Windows y máscara de banderas.
    type FixtureFile<'a> = (Option<u64>, &'a str, usize, u64, bool, Option<u64>);

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
        // La última columna es la máscara de banderas con el bit `id - 1` de las banderas de abajo
        // (windows 1, en_US 2, es_MX 3, fr_FR 4, macos 5): así los flujos completos pasan por la
        // selección por banderas, que es la de los manifiestos reales.
        let specs: [FixtureFile; 5] = [
            (None, GAME_EXE, 9_000, 0xA1, true, None),
            (
                Some(3),
                "Aatrox.es_MX.wad.client",
                7_000,
                0xB2,
                true,
                Some(1 << 2),
            ),
            (
                Some(3),
                "Aatrox.fr_FR.wad.client",
                5_000,
                0xB2,
                false,
                Some(1 << 3),
            ),
            (
                Some(2),
                "Bootstrap.windows.wad.client",
                3_000,
                0xA1,
                true,
                Some(1),
            ),
            (
                Some(2),
                "Bootstrap.macos.wad.client",
                2_000,
                0xB2,
                false,
                Some(1 << 4),
            ),
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
        for (index, (directory, name, size, bundle_id, selected, mask)) in
            specs.into_iter().enumerate()
        {
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
            manifest_files.push((
                index as u64 + 1,
                directory,
                name,
                size as u32,
                chunk_ids,
                mask,
            ));
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
            None,
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
    /// Id, directorio, nombre, tamaño, fragmentos y máscara de banderas (el bit `id - 1` de cada una).
    type ManifestFile<'a> = (u64, Option<u64>, &'a str, u32, Vec<u64>, Option<u64>);

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
        for ((file_id, directory, name, size, chunk_ids, mask), slot) in
            files.iter().zip(file_slots)
        {
            let fields = [
                TestField::U64(*file_id),
                directory.map_or(TestField::Absent, TestField::U64),
                TestField::U32(*size),
                TestField::Offset,
                mask.map_or(TestField::Absent, TestField::U64),
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
