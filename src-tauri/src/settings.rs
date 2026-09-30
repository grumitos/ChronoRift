use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fmt::{self, Display},
    fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

pub const SETTINGS_FILE: &str = "settings.json";
/// Los comandos corren en paralelo fuera del hilo principal: leer, cambiar y guardar va de a uno,
/// también porque `save` usa un único temporal.
static UPDATE_LOCK: Mutex<()> = Mutex::new(());
/// Carpeta dedicada que ChronoRift usa cuando el usuario elige la raíz de una unidad.
pub const DEDICATED_FOLDER: &str = "ChronoRift";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    /// Carpeta raíz donde se instalan los clientes, uno por compilación.
    pub clients_root: Option<String>,
    /// Peticiones de rango en paralelo al descargar un cliente (1–32; sin valor, 8).
    pub download_connections: Option<u8>,
    /// Si las imágenes de los parches de la biblioteca se descargan de una vez (`true`) o al abrir
    /// cada partida (`false`). Sin valor, todavía no se preguntó.
    pub asset_prefetch: Option<bool>,
}

#[derive(Debug)]
pub enum SettingsError {
    Io(io::Error),
    Json(serde_json::Error),
    EmptyPath,
    RelativePath(String),
    MissingVolume(String),
}

impl Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "No se pudo acceder a la configuración: {error}"),
            Self::Json(error) => write!(f, "La configuración no es un JSON válido: {error}"),
            Self::EmptyPath => write!(f, "Falta la carpeta de clientes"),
            Self::RelativePath(path) => {
                write!(f, "La carpeta de clientes no es una ruta absoluta: {path}")
            }
            Self::MissingVolume(path) => {
                write!(f, "La unidad de la carpeta de clientes no existe: {path}")
            }
        }
    }
}

impl Error for SettingsError {}

/// Lee la configuración; si el archivo no existe devuelve los valores por defecto.
pub fn load(path: &Path) -> Result<AppSettings, SettingsError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(SettingsError::Json),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(AppSettings::default()),
        Err(error) => Err(SettingsError::Io(error)),
    }
}

/// Lee la configuración, aplica `change` y la guarda sin que otro cambio se intercale. Un JSON
/// dañado se reemplaza; si el archivo no se puede leer, se informa el error sin escribir nada.
pub fn update(
    path: &Path,
    change: impl FnOnce(&mut AppSettings),
) -> Result<AppSettings, SettingsError> {
    let _guard = UPDATE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let mut settings = match load(path) {
        Err(SettingsError::Json(_)) => AppSettings::default(),
        loaded => loaded?,
    };
    change(&mut settings);
    save(path, &settings)?;
    Ok(settings)
}

/// Escribe la configuración de forma atómica (archivo temporal y renombrado).
pub fn save(path: &Path, settings: &AppSettings) -> Result<(), SettingsError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(SettingsError::Io)?;
    }
    let mut bytes = serde_json::to_vec_pretty(settings).map_err(SettingsError::Json)?;
    bytes.push(b'\n');
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, bytes).map_err(SettingsError::Io)?;
    fs::rename(&temp, path).map_err(SettingsError::Io)
}

/// Valida la carpeta de clientes elegida por el usuario sin crear nada en disco.
///
/// Si el usuario elige la raíz de una unidad (por ejemplo `E:\`), se usa `E:\ChronoRift`
/// para no mezclar las carpetas de versiones con otros datos de la unidad.
pub fn normalize_clients_root(input: &str) -> Result<PathBuf, SettingsError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(SettingsError::EmptyPath);
    }
    let path = Path::new(trimmed).components().collect::<PathBuf>();
    if !path.is_absolute() {
        return Err(SettingsError::RelativePath(trimmed.to_string()));
    }
    let path = if path.parent().is_none() {
        path.join(DEDICATED_FOLDER)
    } else {
        path
    };
    if !path.ancestors().any(Path::is_dir) {
        return Err(SettingsError::MissingVolume(path.display().to_string()));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_directory(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("chronorift-{label}-{unique}"))
    }

    #[test]
    fn persists_the_clients_root_and_validates_it_without_creating_folders() {
        let root = temp_directory("settings");
        let settings_file = root.join("config").join(SETTINGS_FILE);

        // Sin archivo: valores por defecto y ninguna escritura.
        assert_eq!(load(&settings_file).unwrap(), AppSettings::default());
        assert!(!root.exists());

        // Una carpeta que todavía no existe es válida si su unidad existe, y no se crea.
        let clients = root.join("clients");
        let normalized = normalize_clients_root(&format!("  {}  ", clients.display())).unwrap();
        assert_eq!(normalized, clients);
        assert!(!clients.exists());

        // Un archivo de una versión anterior no tiene la elección de imágenes: queda sin preguntar.
        fs::create_dir_all(settings_file.parent().unwrap()).unwrap();
        fs::write(&settings_file, br#"{ "clientsRoot": "E:\\ChronoRift" }"#).unwrap();
        assert_eq!(load(&settings_file).unwrap().asset_prefetch, None);

        let saved = AppSettings {
            clients_root: Some(normalized.display().to_string()),
            download_connections: Some(12),
            asset_prefetch: None,
        };
        save(&settings_file, &saved).unwrap();
        assert_eq!(load(&settings_file).unwrap(), saved);
        assert!(!settings_file.with_extension("json.tmp").exists());

        // Cambiar un ajuste conserva los demás.
        let updated = update(&settings_file, |settings| {
            settings.download_connections = Some(4);
            settings.asset_prefetch = Some(true);
        })
        .unwrap();
        assert_eq!(updated.clients_root, saved.clients_root);
        assert_eq!(updated.asset_prefetch, Some(true));
        assert_eq!(load(&settings_file).unwrap(), updated);

        // Un JSON dañado se informa en lugar de ignorarse en silencio, y guardar lo reemplaza.
        fs::write(&settings_file, b"{not json").unwrap();
        assert!(matches!(load(&settings_file), Err(SettingsError::Json(_))));
        let replaced = update(&settings_file, |settings| {
            settings.clients_root = saved.clients_root.clone();
        })
        .unwrap();
        assert_eq!(replaced.download_connections, None);
        assert_eq!(load(&settings_file).unwrap(), replaced);

        // Un archivo que no se puede leer (aquí, una carpeta con ese nombre) no se pisa: el error
        // llega antes de escribir el temporal.
        let unreadable = root.join("unreadable").join(SETTINGS_FILE);
        fs::create_dir_all(&unreadable).unwrap();
        assert!(matches!(
            update(&unreadable, |_| {}),
            Err(SettingsError::Io(_))
        ));
        assert!(unreadable.is_dir());
        assert!(!unreadable.with_extension("json.tmp").exists());

        assert!(matches!(
            normalize_clients_root("   "),
            Err(SettingsError::EmptyPath)
        ));
        assert!(matches!(
            normalize_clients_root("clients/relative"),
            Err(SettingsError::RelativePath(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_root_gets_a_dedicated_chronorift_folder() {
        let temp = std::env::temp_dir();
        let drive_root = temp.ancestors().last().unwrap().to_path_buf();

        let normalized = normalize_clients_root(&drive_root.display().to_string()).unwrap();

        assert_eq!(normalized, drive_root.join(DEDICATED_FOLDER));
        assert!(!normalized.exists() || normalized.is_dir());

        // Una unidad inexistente se rechaza antes de planificar descargas.
        let missing_drive = ('F'..='Z')
            .rev()
            .map(|letter| format!("{letter}:\\"))
            .find(|root| !Path::new(root).exists());
        if let Some(missing_drive) = missing_drive {
            assert!(matches!(
                normalize_clients_root(&format!("{missing_drive}ChronoRift")),
                Err(SettingsError::MissingVolume(_))
            ));
        }
    }
}
