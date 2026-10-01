//! Caché en disco de DataDragon y CommunityDragon. La interfaz pide imágenes y datos por el
//! esquema `lolasset`: cada archivo se descarga una vez y después se sirve desde disco. La
//! precarga baja una lista entera de una vez, con sus propios turnos de descarga.

use crate::client::{same_origin_redirects, USER_AGENT};
use serde::Serialize;
use std::{
    collections::HashMap,
    fmt::{self, Display},
    fs, io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard, PoisonError,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{
    async_runtime::JoinHandle,
    http::{header, HeaderValue, Response, StatusCode},
};
use tokio::sync::{mpsc, Mutex as AsyncMutex, Semaphore, SemaphorePermit};

/// En Windows la interfaz lo usa como `http://lolasset.localhost/<origen>/<ruta>`.
pub const SCHEME: &str = "lolasset";

/// Evento con el avance de la precarga (`PrefetchProgress`).
pub const PREFETCH_PROGRESS_EVENT: &str = "asset-prefetch-progress";

/// Política de cada respuesta del esquema. `lolasset.localhost` cuenta como origen local y sirve
/// SVG de terceros: abierto como documento, uno de ellos no ejecuta código y `sandbox` le da un
/// origen opaco. En `fetch` e `<img>` la cabecera no cambia nada.
const ASSET_CSP: &str =
    "default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; sandbox";

/// Orígenes de la interfaz, los únicos que pueden leer la caché con `fetch`: la app empaquetada en
/// Windows (`http`, o `https` con `useHttpsScheme`), en macOS y Linux, y Vite en desarrollo.
const APP_ORIGINS: [&str; 4] = [
    "http://tauri.localhost",
    "https://tauri.localhost",
    "tauri://localhost",
    "http://localhost:1420",
];

/// Origen de la propia interfaz, para una petición sin `Origin`: Vite en desarrollo y, si no, la
/// app empaquetada. Tauri elige entre `devUrl` y los archivos empaquetados con el mismo criterio.
const OWN_ORIGIN: &str = if tauri::is_dev() {
    "http://localhost:1420"
} else if cfg!(windows) {
    "http://tauri.localhost"
} else {
    "tauri://localhost"
};

/// Orígenes permitidos: el primer segmento de la ruta elige uno y nunca se acepta otro host.
const SOURCES: [(&str, &str); 2] = [
    ("ddragon", "https://ddragon.leagueoflegends.com"),
    ("cdragon", "https://raw.communitydragon.org"),
];

/// Lo que no lleva versión en la ruta (`api/versions.json`, `latest/…`) se vuelve a pedir
/// pasado este plazo; si la red falla, se sirve la copia vieja.
const MUTABLE_TTL: Duration = Duration::from_secs(12 * 60 * 60);
/// Un 404 de un recurso versionado se recuerda este plazo: un parche recién publicado puede
/// llegar más tarde al CDN.
const MISSING_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MISSING_SUFFIX: &str = ".missing";
const TEMP_SUFFIX: &str = ".tmp";
/// Un temporal más viejo que esto quedó de una escritura interrumpida; uno reciente puede ser de
/// otra instancia de la app que está escribiendo.
const STALE_TEMP_AGE: Duration = Duration::from_secs(10 * 60);
const MAX_BYTES: usize = 32 * 1024 * 1024;
/// Descargas a la vez de las peticiones de la interfaz (abrir una partida).
const MAX_DOWNLOADS: usize = 6;
/// Cada archivo tarda unos 300–400 ms en responder aunque el ancho de banda sobre, así que la
/// precarga necesita muchas peticiones en vuelo. Tiene turnos propios y nunca toma los de
/// `MAX_DOWNLOADS`: abrir una partida no espera a la precarga. CommunityDragon lo mantiene la
/// comunidad y se le piden menos a la vez.
const MAX_PREFETCH: usize = 24;
const MAX_PREFETCH_COMMUNITY: usize = 8;
/// Errores de red seguidos tras los cuales se da la red por caída y se corta la precarga.
const PREFETCH_OFFLINE_ERRORS: usize = 12;
/// Cada cuánto, como máximo, se avisa del avance de la precarga.
const PREFETCH_NOTICE_INTERVAL: Duration = Duration::from_millis(100);
const MAX_SEGMENTS: usize = 16;
const MAX_SEGMENT_LEN: usize = 128;

#[derive(Debug)]
pub enum AssetError {
    InvalidPath,
    NotFound,
    Upstream(String),
    Io(io::Error),
}

impl Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath => write!(f, "Invalid asset path"),
            Self::NotFound => write!(f, "Asset not found"),
            Self::Upstream(message) => write!(f, "Asset download failed: {message}"),
            Self::Io(error) => write!(f, "Asset cache error: {error}"),
        }
    }
}

impl From<io::Error> for AssetError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl AssetRequest {
    /// Plazo de lo que cambia; lo que lleva versión en la ruta no vence.
    fn ttl(&self) -> Option<Duration> {
        (!self.immutable).then_some(MUTABLE_TTL)
    }
}

/// Avance de una precarga, también la carga de `asset-prefetch-progress`. `done` cuenta lo ya
/// resuelto: `ok` (en disco, recién bajado o ya estaba), `missing` (el origen no lo tiene) y
/// `errors`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefetchProgress {
    pub done: usize,
    pub total: usize,
    pub ok: usize,
    pub missing: usize,
    pub errors: usize,
}

/// Resultado de `prefetch_assets`. `cancelled` si se detuvo o empezó otra precarga; `offline` si
/// se cortó por errores de red seguidos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetPrefetchSummary {
    #[serde(flatten)]
    pub progress: PrefetchProgress,
    pub cancelled: bool,
    pub offline: bool,
}

pub struct CachedAsset {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub immutable: bool,
}

/// Petición validada: de dónde se descarga y dónde queda en disco.
struct AssetRequest {
    upstream: String,
    file: PathBuf,
    immutable: bool,
    /// CommunityDragon: la precarga le pide menos archivos a la vez.
    community: bool,
    /// DataDragon (S3) responde 403, no 404, cuando falta un archivo: en una ruta versionada de
    /// ese origen, un 403 cuenta como faltante y se recuerda igual.
    forbidden_is_missing: bool,
}

pub struct AssetCache {
    root: PathBuf,
    origins: HashMap<String, String>,
    /// `None` si no se pudo armar el cliente con la política de la caché: entonces no se descarga
    /// nada y solo se sirve lo que ya está en disco.
    client: Option<reqwest::Client>,
    /// Turnos de las peticiones de la interfaz.
    downloads: Semaphore,
    /// Turnos de la precarga, aparte de `downloads`: uno para todo y otro para CommunityDragon.
    prefetch_downloads: Semaphore,
    prefetch_community: Semaphore,
    /// Un candado por archivo: las peticiones simultáneas esperan a la primera descarga, también
    /// la de la precarga y la de la interfaz por el mismo archivo.
    locks: Mutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>,
    /// La precarga en curso: una sola a la vez.
    prefetch_run: Mutex<Option<Arc<PrefetchRun>>>,
}

impl AssetCache {
    pub fn new(root: PathBuf) -> Self {
        let origins = SOURCES
            .iter()
            .map(|(source, origin)| (source.to_string(), origin.to_string()))
            .collect();
        Self::with_origins(root, origins)
    }

    fn with_origins(root: PathBuf, origins: HashMap<String, String>) -> Self {
        // Sin esta política no se descarga: `Client::new()` no tiene tiempos límite y sigue
        // redirecciones hacia cualquier host.
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(same_origin_redirects())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .ok();
        Self {
            root,
            origins,
            client,
            downloads: Semaphore::new(MAX_DOWNLOADS),
            prefetch_downloads: Semaphore::new(MAX_PREFETCH),
            prefetch_community: Semaphore::new(MAX_PREFETCH_COMMUNITY),
            locks: Mutex::default(),
            prefetch_run: Mutex::default(),
        }
    }

    /// `path` es la ruta de la petición (`/ddragon/cdn/…`) con sus `%XX` tal como llegan.
    pub async fn get(&self, path: &str) -> Result<CachedAsset, AssetError> {
        let request = self.resolve(path)?;
        self.locked(&request, Some(&self.downloads)).await
    }

    /// Resuelve el archivo con su candado. `downloads` limita la descarga; `None` si quien llama
    /// ya tomó su turno antes de pedir el candado.
    async fn locked(
        &self,
        request: &AssetRequest,
        downloads: Option<&Semaphore>,
    ) -> Result<CachedAsset, AssetError> {
        let lock = self.lock_for(&request.file);
        let result = {
            let _guard = lock.lock().await;
            self.fetch(request, downloads).await
        };
        self.release(&request.file, lock);
        result
    }

    async fn fetch(
        &self,
        request: &AssetRequest,
        downloads: Option<&Semaphore>,
    ) -> Result<CachedAsset, AssetError> {
        if let Some(bytes) = read_if_fresh(&request.file, request.ttl())? {
            return Ok(asset(request, bytes));
        }
        let missing = missing_marker(&request.file);
        if request.immutable
            && read_if_fresh(&missing, Some(MISSING_TTL))?.is_some()
            && has_exact_name(&missing)
        {
            return Err(AssetError::NotFound);
        }

        let downloaded = {
            let _permit = match downloads {
                Some(downloads) => Some(acquire(downloads).await?),
                None => None,
            };
            self.download(&request.upstream, request.forbidden_is_missing)
                .await
        };
        match downloaded {
            Ok(bytes) => {
                write_atomic(&request.file, &bytes)?;
                let _ = fs::remove_file(&missing);
                Ok(asset(request, bytes))
            }
            Err(AssetError::NotFound) => {
                if request.immutable {
                    write_atomic(&missing, b"")?;
                }
                Err(AssetError::NotFound)
            }
            // Sin red, la copia vieja de un recurso que cambia sigue sirviendo.
            Err(error) => fs::read(&request.file)
                .map(|bytes| asset(request, bytes))
                .map_err(|_| error),
        }
    }

    async fn download(&self, url: &str, forbidden_is_missing: bool) -> Result<Vec<u8>, AssetError> {
        // Sin cliente, lo que falta en disco es un error de red: no se recuerda como faltante.
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| AssetError::Upstream("HTTP client unavailable".to_string()))?;
        let upstream = |error: reqwest::Error| AssetError::Upstream(error.to_string());
        let mut response = client.get(url).send().await.map_err(upstream)?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND
            || (forbidden_is_missing && status == reqwest::StatusCode::FORBIDDEN)
        {
            return Err(AssetError::NotFound);
        }
        if !status.is_success() {
            return Err(AssetError::Upstream(format!("HTTP {status}")));
        }
        let too_large = || AssetError::Upstream(format!("larger than {MAX_BYTES} bytes"));
        let declared = response.content_length().unwrap_or(0);
        if declared > MAX_BYTES as u64 {
            return Err(too_large());
        }
        // Por partes: sin `Content-Length`, el tope también acota la memoria.
        let mut bytes = Vec::with_capacity(declared as usize);
        while let Some(chunk) = response.chunk().await.map_err(upstream)? {
            if bytes.len() + chunk.len() > MAX_BYTES {
                return Err(too_large());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    fn resolve(&self, path: &str) -> Result<AssetRequest, AssetError> {
        let mut raw = path.trim_start_matches('/').split('/');
        let source = raw.next().unwrap_or_default();
        let origin = self.origins.get(source).ok_or(AssetError::InvalidPath)?;
        let segments = raw.map(decode_segment).collect::<Result<Vec<_>, _>>()?;
        if segments.is_empty() || segments.len() > MAX_SEGMENTS {
            return Err(AssetError::InvalidPath);
        }
        // Con la versión en la ruta el contenido no cambia nunca: `cdn/16.18.1/…` y `16.18/…`.
        let immutable = if source == "ddragon" {
            segments.len() > 2 && segments[0] == "cdn" && is_version(&segments[1], 3)
        } else {
            segments.len() > 1 && is_version(&segments[0], 2)
        };
        let encoded = segments
            .iter()
            .map(|segment| encode_segment(segment))
            .collect::<Vec<_>>();
        let mut file = self.root.join(source);
        file.extend(&segments);
        Ok(AssetRequest {
            upstream: format!("{origin}/{}", encoded.join("/")),
            file,
            immutable,
            community: source == "cdragon",
            // Por origen y no por URL: las pruebas apuntan `ddragon` a un servidor local.
            forbidden_is_missing: source == "ddragon" && immutable,
        })
    }

    fn lock_for(&self, file: &Path) -> Arc<AsyncMutex<()>> {
        let mut locks = self
            .locks
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        Arc::clone(locks.entry(file.to_path_buf()).or_default())
    }

    fn release(&self, file: &Path, lock: Arc<AsyncMutex<()>>) {
        let mut locks = self
            .locks
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        // Solo el mapa y esta petición lo tienen: nadie más espera ese archivo.
        if Arc::strong_count(&lock) == 2 {
            locks.remove(file);
        }
    }

    /// Baja `paths` (las mismas rutas que recibe `get`) para que la interfaz las encuentre en
    /// disco, con hasta `MAX_PREFETCH` peticiones a la vez. Una sola precarga a la vez: empezar
    /// otra corta la anterior, que vuelve con `cancelled`. `on_progress` recibe el avance como
    /// máximo cada `PREFETCH_NOTICE_INTERVAL` y una vez al terminar, salvo si se cancela.
    pub async fn prefetch(
        self: &Arc<Self>,
        paths: Vec<String>,
        on_progress: impl Fn(PrefetchProgress) + Send + Sync + 'static,
    ) -> AssetPrefetchSummary {
        let workers = paths.len().min(MAX_PREFETCH);
        let run = Arc::new(PrefetchRun::new(paths, Box::new(on_progress)));
        let previous = self.current_prefetch().replace(Arc::clone(&run));
        if let Some(previous) = previous {
            previous.cancel();
        }
        // Cada tarea lleva un `Sender` que nunca envía: `recv` devuelve `None` cuando todas
        // terminaron, también las que se cancelaron en pleno vuelo.
        let (finished, mut all_finished) = mpsc::channel::<()>(1);
        for _ in 0..workers {
            let cache = Arc::clone(self);
            let job = Arc::clone(&run);
            let finished = finished.clone();
            run.add_worker(tauri::async_runtime::spawn(async move {
                let _finished = finished;
                cache.prefetch_worker(&job).await;
            }));
        }
        drop(finished);
        while all_finished.recv().await.is_some() {}
        {
            let mut current = self.current_prefetch();
            if current
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &run))
            {
                *current = None;
            }
        }
        run.finish()
    }

    /// Detiene la precarga en curso, si hay una. Lo que ya está en disco se queda.
    pub fn cancel_prefetch(&self) {
        let run = self.current_prefetch().clone();
        if let Some(run) = run {
            run.cancel();
        }
    }

    fn current_prefetch(&self) -> MutexGuard<'_, Option<Arc<PrefetchRun>>> {
        self.prefetch_run
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    async fn prefetch_worker(&self, run: &PrefetchRun) {
        while !run.stopped() {
            let Some(path) = run.next_path() else { break };
            let outcome = self.prefetch_one(path).await;
            run.record(outcome);
        }
    }

    /// Deja un archivo en disco sin leerlo ni devolverlo.
    async fn prefetch_one(&self, path: &str) -> Result<(), AssetError> {
        let request = self.resolve(path)?;
        // Lo que ya está en disco no gasta turno ni candado: basta mirar sus metadatos.
        if is_fresh(&request.file, request.ttl())? {
            return Ok(());
        }
        // El turno se toma antes que el candado: esperarlo con el candado en la mano haría que
        // la interfaz, al pedir ese mismo archivo, esperara detrás de la cola de la precarga.
        let limit = if request.community {
            &self.prefetch_community
        } else {
            &self.prefetch_downloads
        };
        let _turn = acquire(limit).await?;
        self.locked(&request, None).await.map(drop)
    }
}

/// Una precarga en curso. El estado, el aviso de avance y la cancelación comparten un mismo
/// candado: tras `cancel` ya no sale ningún aviso de esta precarga, así que no se mezcla con el
/// avance de la que la reemplaza.
struct PrefetchRun {
    paths: Vec<String>,
    next: AtomicUsize,
    state: Mutex<RunState>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    on_progress: Box<dyn Fn(PrefetchProgress) + Send + Sync>,
}

#[derive(Default)]
struct RunState {
    ok: usize,
    missing: usize,
    errors: usize,
    /// Errores de red desde el último archivo resuelto (o ausente).
    network_errors: usize,
    offline: bool,
    cancelled: bool,
    last_notice: Option<Instant>,
}

impl PrefetchRun {
    fn new(paths: Vec<String>, on_progress: Box<dyn Fn(PrefetchProgress) + Send + Sync>) -> Self {
        Self {
            paths,
            next: AtomicUsize::new(0),
            state: Mutex::default(),
            workers: Mutex::default(),
            on_progress,
        }
    }

    fn state(&self) -> MutexGuard<'_, RunState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn progress(&self, state: &RunState) -> PrefetchProgress {
        PrefetchProgress {
            done: state.ok + state.missing + state.errors,
            total: self.paths.len(),
            ok: state.ok,
            missing: state.missing,
            errors: state.errors,
        }
    }

    fn next_path(&self) -> Option<&str> {
        let index = self.next.fetch_add(1, Ordering::Relaxed);
        self.paths.get(index).map(String::as_str)
    }

    fn stopped(&self) -> bool {
        let state = self.state();
        state.cancelled || state.offline
    }

    /// Un archivo resuelto. Solo los errores de red cuentan para dar la red por caída: una ruta
    /// inválida o un fallo de disco no dicen nada de ella.
    fn record(&self, outcome: Result<(), AssetError>) {
        let mut state = self.state();
        if state.cancelled {
            return;
        }
        match outcome {
            Ok(()) => {
                state.ok += 1;
                state.network_errors = 0;
            }
            Err(AssetError::NotFound) => {
                state.missing += 1;
                state.network_errors = 0;
            }
            Err(AssetError::Upstream(_)) => {
                state.errors += 1;
                state.network_errors += 1;
                state.offline |= state.network_errors >= PREFETCH_OFFLINE_ERRORS;
            }
            Err(AssetError::InvalidPath | AssetError::Io(_)) => state.errors += 1,
        }
        let now = Instant::now();
        if state
            .last_notice
            .is_none_or(|last| now.duration_since(last) >= PREFETCH_NOTICE_INTERVAL)
        {
            state.last_notice = Some(now);
            (self.on_progress)(self.progress(&state));
        }
    }

    /// Registra una tarea para poder cortarla. Si la precarga ya se canceló, la corta al instante.
    fn add_worker(&self, worker: JoinHandle<()>) {
        let mut workers = self.workers.lock().unwrap_or_else(PoisonError::into_inner);
        if self.state().cancelled {
            worker.abort();
        }
        workers.push(worker);
    }

    /// Corta las descargas en vuelo. Es seguro: `write_atomic` no tiene puntos de espera, así que
    /// una tarea cortada nunca deja un archivo a medias.
    fn cancel(&self) {
        self.state().cancelled = true;
        let workers = self.workers.lock().unwrap_or_else(PoisonError::into_inner);
        for worker in workers.iter() {
            worker.abort();
        }
    }

    fn finish(&self) -> AssetPrefetchSummary {
        let state = self.state();
        let progress = self.progress(&state);
        if !state.cancelled {
            (self.on_progress)(progress);
        }
        AssetPrefetchSummary {
            progress,
            cancelled: state.cancelled,
            offline: state.offline,
        }
    }
}

/// Respuesta del esquema. La interfaz (`http://tauri.localhost`) lee los JSON con `fetch` desde
/// otro origen: `origin` (la cabecera `Origin` de la petición) se refleja en CORS solo si es uno de
/// `APP_ORIGINS`, también en los errores, para que la interfaz vea el 404 y pruebe el idioma
/// siguiente. Otro origen, incluido el opaco (`null`) de un documento aislado, no puede leerla.
///
/// Sin `Origin` el permiso es para `OWN_ORIGIN`. Un `<img>` no la manda, y en un `fetch` GET
/// Chromium la agrega recién en su servicio de red: no está confirmado que WebView2 la entregue al
/// esquema. El visor compara ese valor fijo con el origen de quien pide, así que ningún otro origen
/// gana acceso.
pub fn http_response(
    result: Result<CachedAsset, AssetError>,
    origin: Option<&str>,
) -> Response<Vec<u8>> {
    let (status, content_type, cache_control, body) = match result {
        Ok(asset) => (
            StatusCode::OK,
            asset.content_type,
            if asset.immutable {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            },
            asset.bytes,
        ),
        Err(error) => (
            match error {
                AssetError::InvalidPath => StatusCode::BAD_REQUEST,
                AssetError::NotFound => StatusCode::NOT_FOUND,
                AssetError::Upstream(_) | AssetError::Io(_) => StatusCode::BAD_GATEWAY,
            },
            "text/plain; charset=utf-8",
            "no-store",
            error.to_string().into_bytes(),
        ),
    };
    // Solo valores fijos: armar la respuesta no puede fallar.
    let mut response = Response::new(body);
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    // El tipo sale de la extensión; el visor no debe adivinar otro por el contenido.
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(ASSET_CSP),
    );
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    // Se responde con una constante, nunca con los bytes que mandó la petición.
    let allowed = match origin {
        None => Some(OWN_ORIGIN),
        Some(origin) => APP_ORIGINS.into_iter().find(|allowed| *allowed == origin),
    };
    if let Some(allowed) = allowed {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static(allowed),
        );
    }
    response
}

fn asset(request: &AssetRequest, bytes: Vec<u8>) -> CachedAsset {
    CachedAsset {
        bytes,
        content_type: content_type(&request.file),
        immutable: request.immutable,
    }
}

/// Si el archivo existe y no venció, mirando solo sus metadatos: no lo lee.
fn is_fresh(file: &Path, ttl: Option<Duration>) -> io::Result<bool> {
    let metadata = match fs::metadata(file) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if let Some(ttl) = ttl {
        let age = metadata
            .modified()
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok());
        if age.is_none_or(|age| age > ttl) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn read_if_fresh(file: &Path, ttl: Option<Duration>) -> io::Result<Option<Vec<u8>>> {
    if is_fresh(file, ttl)? {
        fs::read(file).map(Some)
    } else {
        Ok(None)
    }
}

async fn acquire(semaphore: &Semaphore) -> Result<SemaphorePermit<'_>, AssetError> {
    semaphore
        .acquire()
        .await
        .map_err(|error| AssetError::Upstream(error.to_string()))
}

/// Escribe en un temporal y lo renombra: una descarga cortada nunca deja un archivo a medias.
fn write_atomic(file: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = file
        .parent()
        .ok_or_else(|| io::Error::other("asset without folder"))?;
    fs::create_dir_all(parent)?;
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("asset");
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let temp = parent.join(temp_file_name(name, unique));
    fs::write(&temp, bytes)?;
    fs::rename(&temp, file).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

/// Temporal de `write_atomic`, junto al archivo final: `.<nombre>.<n>.tmp`.
fn temp_file_name(name: &str, unique: u128) -> String {
    format!(".{name}.{unique}{TEMP_SUFFIX}")
}

/// Reconoce solo los nombres que arma `temp_file_name`.
fn is_temp_file_name(name: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|rest| rest.strip_suffix(TEMP_SUFFIX))
        .and_then(|rest| rest.rsplit_once('.'))
        .is_some_and(|(base, unique)| {
            !base.is_empty()
                && !unique.is_empty()
                && unique.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// Quita los temporales que dejó una escritura cortada en una ejecución anterior (un cierre
/// forzado, un corte de luz). Solo recorre la carpeta de la caché, sin seguir enlaces, y solo
/// borra nombres de `temp_file_name` con más de `STALE_TEMP_AGE`.
pub fn remove_stale_temps(root: &Path) {
    let mut pending = vec![(root.to_path_buf(), 0)];
    while let Some((directory, depth)) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            // `file_type` no sigue enlaces ni uniones: nada fuera de la caché se recorre.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                // Las carpetas de la caché no pasan de `MAX_SEGMENTS` niveles; más hondo no se mira.
                if depth < MAX_SEGMENTS {
                    pending.push((entry.path(), depth + 1));
                }
            } else if kind.is_file()
                && entry.file_name().to_str().is_some_and(is_temp_file_name)
                && entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                    .is_some_and(|age| age > STALE_TEMP_AGE)
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn missing_marker(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(MISSING_SUFFIX);
    file.with_file_name(name)
}

/// NTFS no distingue mayúsculas, pero los CDN sí: el 404 de `Foo.png` no dice nada de
/// `foo.png`. Un marcador solo vale para el nombre exacto que lo creó.
fn has_exact_name(file: &Path) -> bool {
    fs::canonicalize(file).is_ok_and(|real| real.file_name() == file.file_name())
}

fn is_version(segment: &str, parts: usize) -> bool {
    let pieces = segment.split('.').collect::<Vec<_>>();
    pieces.len() == parts
        && pieces
            .iter()
            .all(|piece| !piece.is_empty() && piece.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Decodifica `%XX` y rechaza lo que no sirva como nombre de archivo en Windows.
fn decode_segment(raw: &str) -> Result<String, AssetError> {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes
                .get(index + 1..index + 3)
                .filter(|pair| pair.iter().all(u8::is_ascii_hexdigit))
                .ok_or(AssetError::InvalidPath)?;
            let hex = std::str::from_utf8(hex).map_err(|_| AssetError::InvalidPath)?;
            decoded.push(u8::from_str_radix(hex, 16).map_err(|_| AssetError::InvalidPath)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let segment = String::from_utf8(decoded).map_err(|_| AssetError::InvalidPath)?;
    let valid = !segment.is_empty()
        && segment.len() <= MAX_SEGMENT_LEN
        && segment != "."
        && segment != ".."
        && !segment.ends_with(['.', ' '])
        && !segment.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
        })
        && !is_reserved_name(&segment);
    if valid {
        Ok(segment)
    } else {
        Err(AssetError::InvalidPath)
    }
}

/// Nombres de dispositivo de Windows: valen con cualquier extensión y con espacios antes de ella
/// (`nul .png`), y los puertos también con los dígitos `¹²³`.
fn is_reserved_name(segment: &str) -> bool {
    let stem = segment
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let port = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || port.is_some_and(|number| {
        matches!(number, "¹" | "²" | "³")
            || (number.len() == 1 && number.as_bytes()[0].is_ascii_digit())
    })
}

fn encode_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=@".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn content_type(file: &Path) -> &'static str {
    let extension = file
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::File,
        io::{BufRead, BufReader, Write},
        net::{TcpListener, TcpStream},
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc,
        },
        thread,
    };

    /// Origen HTTP local: rutas fijas, peticiones contadas por ruta, un modo sin red y una
    /// compuerta que retiene las rutas con `/Stall` mientras esté cerrada.
    #[derive(Clone, Default)]
    struct Origin {
        routes: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        /// Ruta → `Location` de un 302.
        redirects: Arc<Mutex<HashMap<String, String>>>,
        /// Ruta → estado sin cuerpo, como el 403 de S3 para un archivo que falta.
        statuses: Arc<Mutex<HashMap<String, &'static str>>>,
        hits: Arc<Mutex<HashMap<String, usize>>>,
        offline: Arc<AtomicBool>,
        /// Mientras sea `true`, las rutas con `/Stall` no responden.
        stalled: Arc<AtomicBool>,
        /// Peticiones que el origen tiene abiertas ahora y el máximo que llegaron a ser a la vez.
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    impl Origin {
        fn start() -> (Self, String) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let origin = Self::default();
            let server = origin.clone();
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let server = server.clone();
                    thread::spawn(move || server.serve(stream));
                }
            });
            (origin, base)
        }

        fn route(&self, path: &str, body: &[u8]) {
            self.routes
                .lock()
                .unwrap()
                .insert(path.to_string(), body.to_vec());
        }

        fn redirect(&self, path: &str, location: String) {
            self.redirects
                .lock()
                .unwrap()
                .insert(path.to_string(), location);
        }

        fn status(&self, path: &str, status: &'static str) {
            self.statuses
                .lock()
                .unwrap()
                .insert(path.to_string(), status);
        }

        fn hits(&self, path: &str) -> usize {
            self.hits.lock().unwrap().get(path).copied().unwrap_or(0)
        }

        fn total_hits(&self) -> usize {
            self.hits.lock().unwrap().values().sum()
        }

        fn active(&self) -> usize {
            self.active.load(Ordering::SeqCst)
        }

        fn peak(&self) -> usize {
            self.peak.load(Ordering::SeqCst)
        }

        fn serve(&self, stream: TcpStream) {
            let Ok(reader_stream) = stream.try_clone() else {
                return;
            };
            let mut reader = BufReader::new(reader_stream);
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                return;
            }
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if line.trim().is_empty() => break,
                    Ok(_) => {}
                }
            }
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .to_string();
            *self.hits.lock().unwrap().entry(path.clone()).or_default() += 1;
            let redirect = self.redirects.lock().unwrap().get(&path).cloned();
            if let Some(location) = redirect {
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                return;
            }
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            while path.contains("/Stall") && self.stalled.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
            // Respuesta lenta a propósito para que las peticiones simultáneas se solapen.
            thread::sleep(Duration::from_millis(60));
            let fixed = self.statuses.lock().unwrap().get(&path).copied();
            let (status, body) = if self.offline.load(Ordering::SeqCst) {
                ("503 Service Unavailable", Vec::new())
            } else if let Some(status) = fixed {
                (status, Vec::new())
            } else {
                match self.routes.lock().unwrap().get(&path) {
                    Some(body) => ("200 OK", body.clone()),
                    None => ("404 Not Found", Vec::new()),
                }
            };
            // Antes de responder: así quien recibe la respuesta nunca ve abiertas más peticiones
            // de las que tiene.
            self.active.fetch_sub(1, Ordering::SeqCst);
            let mut stream = stream;
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&body);
        }
    }

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        tauri::async_runtime::block_on(future)
    }

    fn cache_over(root: &Path, base: &str) -> AssetCache {
        AssetCache::with_origins(
            root.to_path_buf(),
            HashMap::from([
                ("ddragon".to_string(), base.to_string()),
                ("cdragon".to_string(), base.to_string()),
            ]),
        )
    }

    fn age(file: &Path, by: Duration) {
        File::options()
            .write(true)
            .open(file)
            .unwrap()
            .set_modified(SystemTime::now() - by)
            .unwrap();
    }

    /// Cada archivo se descarga una vez: las peticiones simultáneas esperan a la primera y las
    /// siguientes, también en otra sesión, salen del disco. Un 404 versionado (o el 403 con que
    /// DataDragon avisa que falta un archivo) se recuerda solo para ese nombre exacto, lo que no
    /// lleva versión se revalida y sin red se sirve la copia vieja. Las respuestas llevan una CSP
    /// que aísla y CORS solo para la interfaz. Las redirecciones solo se siguen dentro del mismo
    /// origen, las rutas inválidas no tocan ni el disco ni la red y los temporales viejos se borran
    /// al iniciar.
    #[test]
    fn downloads_each_asset_once_and_serves_it_from_disk() {
        let (origin, base) = Origin::start();
        let item = "/cdn/16.18.1/img/item/3020.png";
        let versions = "/api/versions.json";
        let perks = "/16.18/plugins/rcp-be-lol-game-data/global/es_mx/v1/perks.json";
        origin.route(item, b"item-png");
        origin.route(versions, br#"["16.18.1"]"#);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("chronorift-assets-{unique}"));
        let cache = Arc::new(cache_over(&root, &base));

        // El mismo objeto en cuatro jugadores a la vez: una sola descarga.
        let requests = (0..4)
            .map(|_| {
                let cache = Arc::clone(&cache);
                tauri::async_runtime::spawn(
                    async move { cache.get(&format!("/ddragon{item}")).await },
                )
            })
            .collect::<Vec<_>>();
        for request in requests {
            let asset = run(request).unwrap().unwrap();
            assert_eq!(asset.bytes, b"item-png");
            assert_eq!(asset.content_type, "image/png");
            assert!(asset.immutable);
        }
        assert_eq!(origin.hits(item), 1);
        assert_eq!(
            fs::read(root.join("ddragon/cdn/16.18.1/img/item/3020.png")).unwrap(),
            b"item-png"
        );
        let reopened = cache_over(&root, &base);
        assert_eq!(
            run(reopened.get(&format!("/ddragon{item}"))).unwrap().bytes,
            b"item-png"
        );
        assert_eq!(origin.hits(item), 1);

        // Sin cliente HTTP (si no se pudo armar con la política de la caché) solo se sirve lo que
        // está en disco: lo demás es un error de red, sin pedirlo al origen ni recordarlo como
        // faltante, que ocultaría el archivo 24 horas.
        let offline = AssetCache {
            client: None,
            ..cache_over(&root, &base)
        };
        let before = origin.total_hits();
        assert_eq!(
            run(offline.get(&format!("/ddragon{item}"))).unwrap().bytes,
            b"item-png"
        );
        let absent = "/cdn/16.18.1/img/item/9999.png";
        assert!(matches!(
            run(offline.get(&format!("/ddragon{absent}"))),
            Err(AssetError::Upstream(_))
        ));
        assert_eq!(origin.total_hits(), before);
        assert!(!missing_marker(&root.join(format!("ddragon{absent}"))).exists());

        // Lo que recibe el visor: tipo por extensión y sin adivinar, caché larga solo con versión,
        // una CSP que aísla cualquier documento (un SVG de terceros abierto solo) y CORS solo para
        // la interfaz, que en la app empaquetada es `http://tauri.localhost`.
        let respond = |origin| http_response(run(reopened.get(&format!("/ddragon{item}"))), origin);
        let response = respond(Some("http://tauri.localhost"));
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body(), b"item-png");
        for (name, value) in [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; sandbox",
            ),
            (
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                "http://tauri.localhost",
            ),
            (header::VARY, "Origin"),
        ] {
            assert_eq!(response.headers()[&name], value, "{name}");
        }
        // Sin `Origin` (un `<img>`, o un GET si WebView2 no la entrega) el permiso es solo para el
        // origen propio de la interfaz, nunca `*`.
        assert_eq!(
            respond(None).headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            OWN_ORIGIN
        );
        // El origen opaco de un documento aislado, uno parecido o cualquier otro no reciben permiso.
        for origin in [
            "null",
            "http://tauri.localhost.evil.example",
            "https://evil.example",
        ] {
            let response = respond(Some(origin));
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                !response
                    .headers()
                    .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN),
                "{origin}"
            );
            assert_eq!(
                response.headers()[header::CONTENT_SECURITY_POLICY],
                ASSET_CSP
            );
        }
        // Los errores también llevan CORS para la interfaz: sin él `fetch` falla en lugar de dar
        // el 404, y la interfaz no prueba el idioma siguiente.
        for (error, status) in [
            (AssetError::InvalidPath, StatusCode::BAD_REQUEST),
            (AssetError::NotFound, StatusCode::NOT_FOUND),
            (
                AssetError::Upstream("offline".into()),
                StatusCode::BAD_GATEWAY,
            ),
        ] {
            let response = http_response(Err(error), Some("http://localhost:1420"));
            assert_eq!(response.status(), status);
            for (name, value) in [
                (header::CACHE_CONTROL, "no-store"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                (header::CONTENT_SECURITY_POLICY, ASSET_CSP),
                (header::ACCESS_CONTROL_ALLOW_ORIGIN, "http://localhost:1420"),
            ] {
                assert_eq!(response.headers()[&name], value, "{name}");
            }
        }

        // El respaldo de idioma (es_mx → default) no vuelve a la red en cada sesión.
        for _ in 0..2 {
            assert!(matches!(
                run(cache.get(&format!("/cdragon{perks}"))),
                Err(AssetError::NotFound)
            ));
        }
        assert_eq!(origin.hits(perks), 1);

        // DataDragon (S3) responde 403 cuando falta un archivo: en una ruta versionada cuenta como
        // faltante y se pide una sola vez, también en otra sesión. En CommunityDragon un 403 sigue
        // siendo un error y no se recuerda.
        let forbidden = "/cdn/16.18.1/img/champion/Missing.png";
        let community_forbidden = "/16.18/plugins/rcp-be-lol-game-data/global/default/v1/x.json";
        origin.status(forbidden, "403 Forbidden");
        origin.status(community_forbidden, "403 Forbidden");
        for _ in 0..2 {
            assert!(matches!(
                run(cache.get(&format!("/ddragon{forbidden}"))),
                Err(AssetError::NotFound)
            ));
        }
        assert!(matches!(
            run(cache_over(&root, &base).get(&format!("/ddragon{forbidden}"))),
            Err(AssetError::NotFound)
        ));
        assert_eq!(origin.hits(forbidden), 1);
        for _ in 0..2 {
            assert!(matches!(
                run(cache.get(&format!("/cdragon{community_forbidden}"))),
                Err(AssetError::Upstream(_))
            ));
        }
        assert_eq!(origin.hits(community_forbidden), 2);

        // En NTFS `FooBar.png` y `Foobar.png` son el mismo archivo, pero el CDN los distingue: el
        // 404 del primero no oculta al segundo.
        let wrong_case = "/cdn/16.18.1/img/champion/FooBar.png";
        let right_case = "/cdn/16.18.1/img/champion/Foobar.png";
        origin.route(right_case, b"champion-png");
        // Como en DataDragon, el nombre que no existe responde 403.
        origin.status(wrong_case, "403 Forbidden");
        assert!(matches!(
            run(cache.get(&format!("/ddragon{wrong_case}"))),
            Err(AssetError::NotFound)
        ));
        assert_eq!(
            run(cache.get(&format!("/ddragon{right_case}")))
                .unwrap()
                .bytes,
            b"champion-png"
        );
        assert_eq!((origin.hits(wrong_case), origin.hits(right_case)), (1, 1));

        // Una redirección dentro del mismo origen se sigue; una hacia otro host, nunca.
        let (elsewhere, elsewhere_base) = Origin::start();
        elsewhere.route(item, b"elsewhere-png");
        origin.redirect("/cdn/16.18.1/img/item/1001.png", format!("{base}{item}"));
        origin.redirect(
            "/cdn/16.18.1/img/item/1002.png",
            format!("{elsewhere_base}{item}"),
        );
        assert_eq!(
            run(cache.get("/ddragon/cdn/16.18.1/img/item/1001.png"))
                .unwrap()
                .bytes,
            b"item-png"
        );
        assert!(matches!(
            run(cache.get("/ddragon/cdn/16.18.1/img/item/1002.png")),
            Err(AssetError::Upstream(_))
        ));
        assert_eq!(elsewhere.total_hits(), 0);

        let path = format!("/ddragon{versions}");
        for _ in 0..2 {
            let fresh = run(cache.get(&path)).unwrap();
            assert_eq!(fresh.bytes, br#"["16.18.1"]"#);
            assert!(!fresh.immutable);
        }
        assert_eq!(origin.hits(versions), 1);
        age(
            &root.join("ddragon/api/versions.json"),
            MUTABLE_TTL + Duration::from_secs(60),
        );
        origin.offline.store(true, Ordering::SeqCst);
        assert_eq!(run(cache.get(&path)).unwrap().bytes, br#"["16.18.1"]"#);
        assert_eq!(origin.hits(versions), 2);
        origin.offline.store(false, Ordering::SeqCst);
        origin.route(versions, br#"["16.19.1","16.18.1"]"#);
        assert_eq!(
            run(cache.get(&path)).unwrap().bytes,
            br#"["16.19.1","16.18.1"]"#
        );
        assert_eq!(origin.hits(versions), 3);

        let before = origin.total_hits();
        for invalid in [
            "/other/cdn/16.18.1/x.png",
            "/ddragon/../secret.png",
            "/ddragon/cdn/%2e%2e/x.png",
            "/ddragon/a%5Cb.png",
            "/ddragon/a%3Ab.png",
            "/ddragon/CON.png",
            "/ddragon/nul%20.png",
            "/ddragon/COM%C2%B9.png",
            "/ddragon/conout$.json",
            "/ddragon/",
            "/ddragon/x.png.",
            "/ddragon/a/%zz.png",
        ] {
            assert!(
                matches!(run(cache.get(invalid)), Err(AssetError::InvalidPath)),
                "{invalid}"
            );
        }
        assert_eq!(origin.total_hits(), before);

        // Temporales de una escritura cortada: al iniciar se borra el viejo; uno reciente puede
        // ser de otra instancia y queda, igual que los archivos y los marcadores de 404.
        let folder = root.join("ddragon/cdn/16.18.1/img/item");
        let stale = folder.join(temp_file_name("3021.png", 1));
        let fresh = folder.join(temp_file_name("3022.png", 2));
        for temp in [&stale, &fresh] {
            fs::write(temp, b"partial").unwrap();
        }
        age(&stale, STALE_TEMP_AGE + Duration::from_secs(60));
        remove_stale_temps(&root);
        assert!(!stale.exists());
        assert!(fresh.exists());
        assert!(folder.join("3020.png").is_file());
        assert!(missing_marker(&root.join(format!("cdragon{perks}"))).is_file());
        fs::remove_dir_all(root).unwrap();
    }

    /// Espera a que se cumpla algo que ocurre en otras tareas, sin suponer cuánto tardan.
    fn wait_until(what: &str, condition: impl Fn() -> bool) {
        let limit = Instant::now() + Duration::from_secs(20);
        while !condition() {
            assert!(Instant::now() < limit, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// La precarga baja muchas imágenes a la vez, más que las que pide la interfaz, y sin tomar
    /// los turnos de esta: abrir una partida no espera a la precarga, y si pide un archivo que la
    /// precarga ya está bajando, comparten esa descarga. Lo ya cacheado no cuesta una petición, lo
    /// que el origen no tiene se cuenta como faltante, una nueva precarga corta la anterior y la
    /// red caída corta la precarga. El origen retiene las rutas `/Stall` para que nada dependa de
    /// cuánto tarde cada tarea.
    #[test]
    fn prefetches_in_parallel_without_blocking_on_demand_requests() {
        let (origin, base) = Origin::start();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("chronorift-prefetch-{unique}"));
        let cache = Arc::new(cache_over(&root, &base));
        let champion = |name: &str| format!("/cdn/16.18.1/img/champion/{name}.png");
        let item = "/cdn/16.18.1/img/item/3020.png";
        origin.route(item, b"item-png");
        let cached = "/cdn/16.18.1/img/item/1001.png";
        origin.route(cached, b"cached-png");
        let stalled = (0..40)
            .map(|number| champion(&format!("Stall{number}")))
            .collect::<Vec<_>>();
        for path in &stalled {
            origin.route(path, b"champion-png");
        }
        run(cache.get(&format!("/ddragon{cached}"))).unwrap();
        origin.stalled.store(true, Ordering::SeqCst);

        // Cuarenta archivos retenidos, uno que el origen no tiene y uno que ya está en disco.
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut paths = vec![
            format!("/ddragon{}", champion("Gone")),
            format!("/ddragon{cached}"),
        ];
        paths.extend(stalled.iter().map(|path| format!("/ddragon{path}")));
        let prefetch = {
            let (cache, events) = (Arc::clone(&cache), Arc::clone(&events));
            tauri::async_runtime::spawn(async move {
                cache
                    .prefetch(paths, move |progress| events.lock().unwrap().push(progress))
                    .await
            })
        };

        // Las 24 peticiones de la precarga se abren juntas (más que las 6 de la interfaz), no
        // pasan de 24 aunque quede cola, y la interfaz conserva todos sus turnos.
        wait_until("24 prefetch requests in flight", || {
            origin.active() == MAX_PREFETCH
        });
        thread::sleep(Duration::from_millis(100));
        assert_eq!(origin.peak(), MAX_PREFETCH);
        assert_eq!(cache.downloads.available_permits(), MAX_DOWNLOADS);
        assert_eq!(origin.hits(cached), 1);

        // Con la precarga atascada, abrir una partida se resuelve igual. Y pedir un archivo que la
        // precarga ya está bajando espera a esa descarga en lugar de repetirla.
        let (sender, receiver) = mpsc::channel();
        for path in [item.to_string(), stalled[0].clone()] {
            let (cache, sender) = (Arc::clone(&cache), sender.clone());
            tauri::async_runtime::spawn(async move {
                let asset = cache.get(&format!("/ddragon{path}")).await;
                let _ = sender.send((path, asset.map(|asset| asset.bytes)));
            });
        }
        let (first, bytes) = receiver
            .recv_timeout(Duration::from_secs(20))
            .expect("the on-demand request was queued behind the prefetch");
        assert_eq!(
            (first.as_str(), bytes.unwrap()),
            (item, b"item-png".to_vec())
        );
        assert!(receiver.try_recv().is_err());

        origin.stalled.store(false, Ordering::SeqCst);
        let summary = run(prefetch).unwrap();
        let (second, bytes) = receiver.recv_timeout(Duration::from_secs(20)).unwrap();
        assert_eq!(
            (second.as_str(), bytes.unwrap()),
            (stalled[0].as_str(), b"champion-png".to_vec())
        );
        assert_eq!(origin.hits(&stalled[0]), 1);
        assert_eq!(origin.hits(cached), 1);
        // La forma que recibe la interfaz.
        assert_eq!(
            serde_json::to_value(summary).unwrap(),
            serde_json::json!({
                "done": 42, "total": 42, "ok": 41, "missing": 1, "errors": 0,
                "cancelled": false, "offline": false
            })
        );
        // El último aviso es el resultado final y el avance nunca retrocede.
        let events = events.lock().unwrap();
        assert_eq!(events.last(), Some(&summary.progress));
        assert!(events.windows(2).all(|pair| pair[0].done <= pair[1].done));
        drop(events);

        // Detenerla vuelve enseguida, con las peticiones todavía retenidas. Una nueva precarga
        // corta la anterior y no espera a lo que esa dejó en el origen.
        origin.stalled.store(true, Ordering::SeqCst);
        let stalled_run = |name: &str| {
            let paths = (0..30)
                .map(|number| format!("/ddragon{}", champion(&format!("Stall{name}{number}"))))
                .collect::<Vec<_>>();
            let cache = Arc::clone(&cache);
            tauri::async_runtime::spawn(async move { cache.prefetch(paths, |_| {}).await })
        };
        let stopped = stalled_run("A");
        wait_until("the first run to start", || {
            origin.hits(&champion("StallA0")) == 1
        });
        cache.cancel_prefetch();
        let stopped = run(stopped).unwrap();
        assert!(stopped.cancelled && !stopped.offline);
        assert_eq!((stopped.progress.done, stopped.progress.total), (0, 30));

        let replaced = stalled_run("B");
        wait_until("the second run to start", || {
            origin.hits(&champion("StallB0")) == 1
        });
        let ordinary = (0..3)
            .map(|number| {
                let path = champion(&format!("Plain{number}"));
                origin.route(&path, b"champion-png");
                format!("/ddragon{path}")
            })
            .collect::<Vec<_>>();
        let replacement = run(cache.prefetch(ordinary, |_| {}));
        assert_eq!((replacement.progress.ok, replacement.cancelled), (3, false));
        let replaced = run(replaced).unwrap();
        assert!(replaced.cancelled && replaced.progress.done < 30);
        origin.stalled.store(false, Ordering::SeqCst);

        // Con la red caída, 12 errores seguidos cortan la precarga sin recorrer toda la lista.
        origin.offline.store(true, Ordering::SeqCst);
        let paths = (0..60)
            .map(|number| {
                let path = champion(&format!("Offline{number}"));
                origin.route(&path, b"champion-png");
                format!("/ddragon{path}")
            })
            .collect::<Vec<_>>();
        let offline = run(cache.prefetch(paths, |_| {}));
        assert!(offline.offline && !offline.cancelled);
        assert!(offline.progress.done < offline.progress.total);
        assert_eq!(offline.progress.errors, offline.progress.done);
        assert!(!missing_marker(&root.join(format!("ddragon{}", champion("Offline0")))).exists());
        fs::remove_dir_all(root).unwrap();
    }
}
