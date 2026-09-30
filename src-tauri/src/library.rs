use crate::rofl::{self, ReplayFormat, ReplayInspection, ReplayStatus};
use serde::Serialize;
use std::{
    collections::HashMap,
    error::Error,
    fmt::{self, Display},
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplayLibrary {
    pub folder_path: String,
    pub entries: Vec<ReplayLibraryEntry>,
    pub failures: Vec<ReplayLibraryFailure>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplayLibraryEntry {
    pub path: String,
    pub file_name: String,
    pub file_size: u64,
    pub modified_at_ms: Option<u64>,
    pub format: ReplayFormat,
    pub status: ReplayStatus,
    pub game_version: Option<String>,
    pub patch: Option<String>,
    pub region: Option<String>,
    pub match_id: Option<String>,
    pub game_duration_ms: Option<u64>,
    pub winning_team: Option<String>,
    pub blue_kills: u32,
    pub red_kills: u32,
    pub player_count: usize,
    /// Resumen por jugador: permite detectar al dueño de la biblioteca y mostrar su resultado.
    pub players: Vec<LibraryPlayer>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPlayer {
    pub name: String,
    pub champion: String,
    pub team: String,
    pub position: String,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub win: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReplayLibraryFailure {
    pub path: String,
    pub message: String,
}

#[derive(Debug)]
pub enum LibraryError {
    Io(io::Error),
    NotDirectory,
}

impl Display for LibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::NotDirectory => write!(f, "La ruta de repeticiones no es una carpeta"),
        }
    }
}

impl Error for LibraryError {}

impl From<io::Error> for LibraryError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

/// Tamaño y fecha de un archivo: si cambian, lo leído antes ya no vale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl FileStamp {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

/// Repetición completa leída durante el escaneo.
pub struct ScannedReplay {
    pub inspection: ReplayInspection,
    pub stamp: FileStamp,
}

/// Inspecciones ya leídas, por ruta. Elegir una repetición de la biblioteca no vuelve a leer el
/// archivo mientras conserve tamaño y fecha.
#[derive(Clone, Default)]
pub struct ReplayCache(Arc<Mutex<HashMap<PathBuf, ScannedReplay>>>);

impl ReplayCache {
    /// Reemplaza todo lo guardado por un escaneo nuevo. La interfaz muestra una sola biblioteca:
    /// lo borrado, movido o ya ilegible, y lo de otras carpetas, deja de ocupar memoria.
    pub fn replace(&self, scanned: Vec<ScannedReplay>) {
        let replays = scanned
            .into_iter()
            .map(|replay| (PathBuf::from(&replay.inspection.path), replay))
            .collect();
        *self.0.lock().unwrap_or_else(|poison| poison.into_inner()) = replays;
    }

    pub fn inspect(&self, path: &Path) -> Result<ReplayInspection, String> {
        let stamp = FileStamp::of(&fs::metadata(path).map_err(|error| error.to_string())?);
        {
            let replays = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
            if let Some(known) = replays.get(path).filter(|known| known.stamp == stamp) {
                return Ok(known.inspection.clone());
            }
        }
        let inspection = rofl::inspect_file(path).map_err(|error| error.to_string())?;
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(
                path.to_path_buf(),
                ScannedReplay {
                    inspection: inspection.clone(),
                    stamp,
                },
            );
        Ok(inspection)
    }
}

/// Escanea un nivel de la carpeta. Devuelve la biblioteca y, aparte, cada repetición completa para
/// no tener que leerla otra vez al elegirla.
pub fn scan_folder(
    path: impl Into<PathBuf>,
) -> Result<(ReplayLibrary, Vec<ScannedReplay>), LibraryError> {
    let path = path.into();
    if !path.is_dir() {
        return Err(LibraryError::NotDirectory);
    }

    let mut replay_paths = fs::read_dir(&path)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|entry_path| entry_path.is_file() && is_replay_path(entry_path))
        .collect::<Vec<_>>();
    replay_paths.sort_by(|left, right| left.file_name().cmp(&right.file_name()));

    let mut entries = Vec::with_capacity(replay_paths.len());
    let mut scanned = Vec::with_capacity(replay_paths.len());
    let mut failures = Vec::new();

    for replay_path in replay_paths {
        match inspect_entry(&replay_path) {
            Ok((entry, replay)) => {
                entries.push(entry);
                scanned.push(replay);
            }
            Err(error) => failures.push(ReplayLibraryFailure {
                path: replay_path.display().to_string(),
                message: error,
            }),
        }
    }

    entries.sort_by(|left, right| {
        right
            .modified_at_ms
            .cmp(&left.modified_at_ms)
            .then_with(|| left.file_name.cmp(&right.file_name))
    });

    Ok((
        ReplayLibrary {
            folder_path: path.display().to_string(),
            entries,
            failures,
        },
        scanned,
    ))
}

fn inspect_entry(path: &Path) -> Result<(ReplayLibraryEntry, ScannedReplay), String> {
    let file_metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let replay = rofl::inspect_file(path).map_err(|error| error.to_string())?;
    let modified_at_ms = file_metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());

    let entry = ReplayLibraryEntry {
        path: replay.path.clone(),
        file_name: replay.file_name.clone(),
        file_size: file_metadata.len(),
        modified_at_ms,
        format: replay.format.clone(),
        status: replay.status.clone(),
        game_version: replay.game_version.clone(),
        patch: replay.patch.clone(),
        region: replay.region.clone(),
        match_id: replay.match_id.clone(),
        game_duration_ms: replay.game_duration_ms,
        winning_team: replay.winning_team.clone(),
        blue_kills: replay.blue_kills,
        red_kills: replay.red_kills,
        player_count: replay.players.len(),
        players: replay
            .players
            .iter()
            .map(|player| LibraryPlayer {
                name: player.name.clone(),
                champion: player.champion.clone(),
                team: player.team.clone(),
                position: player.position.clone(),
                kills: player.kills,
                deaths: player.deaths,
                assists: player.assists,
                win: player.win,
            })
            .collect(),
    };
    let scanned = ScannedReplay {
        inspection: replay,
        stamp: FileStamp::of(&file_metadata),
    };
    Ok((entry, scanned))
}

fn is_replay_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("rofl"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rofl::test_support::{metadata, player, write_rofl2};
    use std::{
        fs::File,
        time::{Duration, SystemTime},
    };

    fn temp_directory(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("chronorift-{label}-{unique}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn set_modified(path: &Path, seconds: u64) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(seconds))
            .unwrap();
    }

    /// Carpeta mixta: una repetición válida, una demasiado pequeña, una ajena, otros archivos y
    /// una subcarpeta. Solo se lee un nivel y cada archivo queda como entrada o como fallo.
    #[test]
    fn scans_one_folder_level_and_reports_every_replay() {
        let root = temp_directory("library");
        let parsed_path = root.join("LA2-1625354615.rofl");
        let players = [
            player(("Blue", "1"), "Ahri", "100", "MIDDLE", [4, 1, 3], false),
            player(("Red", "2"), "Zed", "200", "MIDDLE", [6, 4, 2], true),
        ];
        write_rofl2(
            &parsed_path,
            "16.18.817.5716",
            &metadata(1_654_000, &players),
        );
        let invalid_path = root.join("second.ROFL");
        fs::write(&invalid_path, b"bad").unwrap();
        fs::write(root.join("broken.rofl"), vec![0x42; 400]).unwrap();
        fs::write(root.join("notes.txt"), b"not a replay").unwrap();
        let nested = root.join("nested");
        fs::create_dir_all(&nested).unwrap();
        write_rofl2(
            &nested.join("LA2-1.rofl"),
            "16.18.817.5716",
            &metadata(1, &players),
        );
        set_modified(&parsed_path, 1_000);
        set_modified(&invalid_path, 2_000);

        let (library, _) = scan_folder(&root).unwrap();

        // Más reciente primero; las subcarpetas no se recorren.
        let names = library
            .entries
            .iter()
            .map(|entry| entry.file_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["second.ROFL", "LA2-1625354615.rofl"]);
        assert_eq!(library.failures.len(), 1);
        assert!(library.failures[0].path.ends_with("broken.rofl"));
        assert!(library.failures[0].message.contains("RIOT"));
        assert_eq!(library.entries[0].status, ReplayStatus::Invalid);

        let parsed = &library.entries[1];
        assert_eq!(parsed.status, ReplayStatus::Parsed);
        assert_eq!(parsed.format, ReplayFormat::Rofl2);
        assert_eq!(parsed.game_version.as_deref(), Some("16.18.817.5716"));
        assert_eq!(parsed.patch.as_deref(), Some("16.18"));
        assert_eq!(parsed.region.as_deref(), Some("LA2"));
        assert_eq!(parsed.match_id.as_deref(), Some("1625354615"));
        assert_eq!(parsed.game_duration_ms, Some(1_654_000));
        assert_eq!(
            (parsed.blue_kills, parsed.red_kills, parsed.player_count),
            (4, 6, 2)
        );
        assert_eq!(parsed.winning_team.as_deref(), Some("Red"));
        assert_eq!(parsed.modified_at_ms, Some(1_000_000));
        assert_eq!(
            parsed.players[1],
            LibraryPlayer {
                name: "Red#2".to_string(),
                champion: "Zed".to_string(),
                team: "Red".to_string(),
                position: "MIDDLE".to_string(),
                kills: 6,
                deaths: 4,
                assists: 2,
                win: true,
            }
        );

        assert!(matches!(
            scan_folder(&parsed_path),
            Err(LibraryError::NotDirectory)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    /// El escaneo conserva cada repetición completa y elegirla no vuelve a leer el archivo
    /// mientras conserve tamaño y fecha; si cambia cualquiera de los dos, se lee otra vez. Un
    /// escaneo nuevo reemplaza todo lo guardado.
    #[test]
    fn reuses_scanned_replays_until_the_file_changes() {
        let root = temp_directory("replay-cache");
        let path = root.join("LA2-1625354615.rofl");
        let players = [
            player(("Blue", "1"), "Ahri", "100", "MIDDLE", [4, 1, 3], false),
            player(("Red", "2"), "Zed", "200", "MIDDLE", [6, 4, 2], true),
        ];
        write_rofl2(&path, "16.18.817.5716", &metadata(1_654_000, &players));
        set_modified(&path, 1_000);

        let (library, scanned) = scan_folder(&root).unwrap();
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].inspection, rofl::inspect_file(&path).unwrap());
        let cache = ReplayCache::default();
        cache.replace(scanned);
        let key = PathBuf::from(&library.entries[0].path);

        // Otro contenido con el mismo tamaño y fecha: se sirve lo leído, sin abrir el archivo.
        let len = fs::metadata(&path).unwrap().len();
        fs::write(&path, vec![0_u8; len as usize]).unwrap();
        set_modified(&path, 1_000);
        let cached = cache.inspect(&key).unwrap();
        assert_eq!(cached.status, ReplayStatus::Parsed);
        assert_eq!(cached.players.len(), 2);

        // Con otra fecha se lee de nuevo y el archivo ya no es una repetición válida.
        set_modified(&path, 2_000);
        let reread = cache.inspect(&key);
        assert!(!matches!(reread, Ok(ref replay) if replay.status == ReplayStatus::Parsed));

        // Un escaneo nuevo reemplaza todo lo guardado: la interfaz muestra una sola biblioteca, así
        // que lo ilegible, lo borrado y lo leído de otra carpeta dejan de ocupar memoria.
        let other = temp_directory("replay-cache-other");
        let elsewhere = other.join("LA2-1.rofl");
        write_rofl2(&elsewhere, "16.18.817.5716", &metadata(1, &players));
        cache.inspect(&elsewhere).unwrap();
        let second = root.join("LA2-1625390199.rofl");
        write_rofl2(&second, "16.18.817.5716", &metadata(1_000, &players));
        let (_, scanned) = scan_folder(&root).unwrap();
        cache.replace(scanned);
        assert_eq!(cached_paths(&cache), [second.as_path()]);

        fs::remove_file(&second).unwrap();
        let (_, scanned) = scan_folder(&root).unwrap();
        cache.replace(scanned);
        assert!(cached_paths(&cache).is_empty());
        fs::remove_dir_all(other).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    fn cached_paths(cache: &ReplayCache) -> Vec<PathBuf> {
        let mut paths = cache.0.lock().unwrap().keys().cloned().collect::<Vec<_>>();
        paths.sort();
        paths
    }
}
