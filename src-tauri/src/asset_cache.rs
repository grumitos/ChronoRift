//! Caché en disco de DataDragon y CommunityDragon. La interfaz pide imágenes y datos por el
//! esquema `lolasset`: cada archivo se descarga una vez y después se sirve desde disco.

use crate::client::{same_origin_redirects, USER_AGENT};
use std::{
    collections::HashMap,
    fmt::{self, Display},
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::http::{header, Response, StatusCode};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

/// En Windows la interfaz lo usa como `http://lolasset.localhost/<origen>/<ruta>`.
pub const SCHEME: &str = "lolasset";

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
const MAX_DOWNLOADS: usize = 6;
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
    /// DataDragon (S3) responde 403, no 404, cuando falta un archivo: en una ruta versionada de
    /// ese origen, un 403 cuenta como faltante y se recuerda igual.
    forbidden_is_missing: bool,
}

pub struct AssetCache {
    root: PathBuf,
    origins: HashMap<String, String>,
    client: reqwest::Client,
    downloads: Semaphore,
    /// Un candado por archivo: las peticiones simultáneas esperan a la primera descarga.
    locks: Mutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>,
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
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(same_origin_redirects())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            root,
            origins,
            client,
            downloads: Semaphore::new(MAX_DOWNLOADS),
            locks: Mutex::default(),
        }
    }

    /// `path` es la ruta de la petición (`/ddragon/cdn/…`) con sus `%XX` tal como llegan.
    pub async fn get(&self, path: &str) -> Result<CachedAsset, AssetError> {
        let request = self.resolve(path)?;
        let lock = self.lock_for(&request.file);
        let result = {
            let _guard = lock.lock().await;
            self.fetch(&request).await
        };
        self.release(&request.file, lock);
        result
    }

    async fn fetch(&self, request: &AssetRequest) -> Result<CachedAsset, AssetError> {
        let ttl = (!request.immutable).then_some(MUTABLE_TTL);
        if let Some(bytes) = read_if_fresh(&request.file, ttl)? {
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
            let _permit = self
                .downloads
                .acquire()
                .await
                .map_err(|error| AssetError::Upstream(error.to_string()))?;
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
        let upstream = |error: reqwest::Error| AssetError::Upstream(error.to_string());
        let mut response = self.client.get(url).send().await.map_err(upstream)?;
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
}

/// Respuesta del esquema. Lleva CORS abierto porque la interfaz (`tauri.localhost`) lee los JSON
/// con `fetch` desde otro origen.
pub fn http_response(result: Result<CachedAsset, AssetError>) -> Response<Vec<u8>> {
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
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, cache_control)
        // El tipo sale de la extensión; el visor no debe adivinar otro por el contenido.
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(body)
        .unwrap_or_else(|_| {
            // Con cabeceras fijas no ocurre; aun así, nunca un 200 vacío que parezca válido.
            let mut response = Response::new(Vec::new());
            *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            response
        })
}

fn asset(request: &AssetRequest, bytes: Vec<u8>) -> CachedAsset {
    CachedAsset {
        bytes,
        content_type: content_type(&request.file),
        immutable: request.immutable,
    }
}

fn read_if_fresh(file: &Path, ttl: Option<Duration>) -> io::Result<Option<Vec<u8>>> {
    let metadata = match fs::metadata(file) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if let Some(ttl) = ttl {
        let age = metadata
            .modified()
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok());
        if age.is_none_or(|age| age > ttl) {
            return Ok(None);
        }
    }
    fs::read(file).map(Some)
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
        sync::atomic::{AtomicBool, Ordering},
        thread,
    };

    /// Origen HTTP local: rutas fijas, peticiones contadas por ruta y un modo sin red.
    #[derive(Clone, Default)]
    struct Origin {
        routes: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        /// Ruta → `Location` de un 302.
        redirects: Arc<Mutex<HashMap<String, String>>>,
        /// Ruta → estado sin cuerpo, como el 403 de S3 para un archivo que falta.
        statuses: Arc<Mutex<HashMap<String, &'static str>>>,
        hits: Arc<Mutex<HashMap<String, usize>>>,
        offline: Arc<AtomicBool>,
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
    /// DataDragon avisa que falta un archivo) se recuerda solo para ese nombre exacto, lo que no lleva versión se revalida y sin red se sirve la copia
    /// vieja. Las redirecciones solo se siguen dentro del mismo origen, las rutas inválidas no
    /// tocan ni el disco ni la red y los temporales viejos se borran al iniciar.
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

        // Lo que recibe el visor: tipo por extensión y sin adivinar, caché larga solo con versión,
        // errores sin guardar y CORS abierto para `fetch`.
        let response = http_response(run(reopened.get(&format!("/ddragon{item}"))));
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body(), b"item-png");
        for (name, value) in [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ] {
            assert_eq!(response.headers()[&name], value, "{name}");
        }
        for (error, status) in [
            (AssetError::InvalidPath, StatusCode::BAD_REQUEST),
            (AssetError::NotFound, StatusCode::NOT_FOUND),
            (
                AssetError::Upstream("offline".into()),
                StatusCode::BAD_GATEWAY,
            ),
        ] {
            let response = http_response(Err(error));
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(
                response.headers()[header::X_CONTENT_TYPE_OPTIONS],
                "nosniff"
            );
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
}
