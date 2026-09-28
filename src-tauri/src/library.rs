use crate::rofl::{self, ReplayFormat, ReplayInspection, ReplayStatus};
use serde::Serialize;
use std::{
    cmp::Ordering,
    error::Error,
    fmt::{self, Display},
    fs, io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplayLibrary {
    pub folder: String,
    pub replays: Vec<ReplaySummary>,
    pub errors: Vec<ReplayScanError>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplaySummary {
    pub path: String,
    pub file_name: String,
    pub modified_ms: Option<u64>,
    pub size_bytes: u64,
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
    pub blue_champions: Vec<String>,
    pub red_champions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReplayScanError {
    pub path: String,
    pub message: String,
}

#[derive(Debug)]
pub enum LibraryError {
    Io(io::Error),
    NotADirectory(String),
}

impl Display for LibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::NotADirectory(path) => write!(f, "Replay folder is not a directory: {path}"),
        }
    }
}

impl Error for LibraryError {}

impl From<io::Error> for LibraryError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

/// Lists `.rofl` files directly inside `path` (non-recursive) and inspects each one.
/// Strictly read-only: never creates, modifies, or deletes anything.
pub fn scan_replay_folder(path: impl Into<PathBuf>) -> Result<ReplayLibrary, LibraryError> {
    let folder = path.into();
    if !folder.is_dir() {
        return Err(LibraryError::NotADirectory(folder.display().to_string()));
    }

    let mut replays = Vec::new();
    let mut errors = Vec::new();

    for entry in fs::read_dir(&folder)? {
        let Ok(entry) = entry else {
            continue;
        };
        let file_path = entry.path();
        if !is_rofl_file(&file_path) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }

        let modified_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .and_then(|duration| u64::try_from(duration.as_millis()).ok());

        match rofl::inspect_file(&file_path) {
            Ok(inspection) => {
                replays.push(summarize(inspection, modified_ms, metadata.len()));
            }
            Err(err) => errors.push(ReplayScanError {
                path: file_path.display().to_string(),
                message: err.to_string(),
            }),
        }
    }

    sort_summaries(&mut replays);
    errors.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(ReplayLibrary {
        folder: folder.display().to_string(),
        replays,
        errors,
    })
}

fn is_rofl_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("rofl"))
}

pub fn summarize(
    inspection: ReplayInspection,
    modified_ms: Option<u64>,
    size_bytes: u64,
) -> ReplaySummary {
    let team_champions = |team: &str| -> Vec<String> {
        inspection
            .players
            .iter()
            .filter(|player| player.team == team)
            .map(|player| player.champion.clone())
            .collect()
    };
    let blue_champions = team_champions("Blue");
    let red_champions = team_champions("Red");

    ReplaySummary {
        path: inspection.path,
        file_name: inspection.file_name,
        modified_ms,
        size_bytes,
        format: inspection.format,
        status: inspection.status,
        game_version: inspection.game_version,
        patch: inspection.patch,
        region: inspection.region,
        match_id: inspection.match_id,
        game_duration_ms: inspection.game_duration_ms,
        winning_team: inspection.winning_team,
        blue_kills: inspection.blue_kills,
        red_kills: inspection.red_kills,
        blue_champions,
        red_champions,
    }
}

/// Newest first by `modified_ms` (missing timestamps last), then by file name.
fn sort_summaries(replays: &mut [ReplaySummary]) {
    replays.sort_by(|a, b| {
        let by_time = match (a.modified_ms, b.modified_ms) {
            (Some(a), Some(b)) => b.cmp(&a),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        by_time.then_with(|| a.file_name.cmp(&b.file_name))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "chronorift-library-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_replay(dir: &Path, name: &str, modified_secs: u64) {
        let metadata = serde_json::json!({ "gameLength": 1000, "statsJson": "[]" });
        let path = dir.join(name);
        fs::write(&path, rofl::rofl2_fixture_bytes(&metadata, "14.10.1.1")).unwrap();
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(modified_secs))
            .unwrap();
    }

    fn summary(file_name: &str, modified_ms: Option<u64>) -> ReplaySummary {
        ReplaySummary {
            path: file_name.to_string(),
            file_name: file_name.to_string(),
            modified_ms,
            size_bytes: 0,
            format: ReplayFormat::Unknown,
            status: ReplayStatus::Invalid,
            game_version: None,
            patch: None,
            region: None,
            match_id: None,
            game_duration_ms: None,
            winning_team: None,
            blue_kills: 0,
            red_kills: 0,
            blue_champions: Vec::new(),
            red_champions: Vec::new(),
        }
    }

    #[test]
    fn scan_ignores_non_rofl_files_and_collects_invalid_replays() {
        let dir = temp_dir("filter");
        write_replay(&dir, "LA2-111.rofl", 1_700_000_000);
        write_replay(&dir, "NA1-222.ROFL", 1_700_000_100);
        fs::write(dir.join("notes.txt"), b"not a replay").unwrap();
        fs::write(dir.join("broken.rofl"), vec![0u8; 400]).unwrap();
        fs::create_dir_all(dir.join("nested")).unwrap();
        write_replay(&dir.join("nested"), "EUW1-333.rofl", 1_700_000_200);

        let library = scan_replay_folder(&dir).expect("scan should succeed");
        fs::remove_dir_all(&dir).unwrap();

        let names: Vec<_> = library
            .replays
            .iter()
            .map(|r| r.file_name.as_str())
            .collect();
        assert_eq!(names, vec!["NA1-222.ROFL", "LA2-111.rofl"]);
        assert_eq!(library.replays[0].modified_ms, Some(1_700_000_100_000));
        assert_eq!(library.replays[0].patch.as_deref(), Some("14.10"));
        assert_eq!(library.replays[0].region.as_deref(), Some("NA1"));
        assert_eq!(library.replays[0].match_id.as_deref(), Some("222"));
        assert!(library.replays[0].size_bytes > 0);
        assert_eq!(library.errors.len(), 1);
        assert!(library.errors[0].path.ends_with("broken.rofl"));
        assert!(!library.errors[0].message.is_empty());
    }

    #[test]
    fn scans_optional_real_replay_sample() {
        let Ok(sample) = std::env::var("CHRONORIFT_REPLAY_SAMPLE") else {
            return;
        };
        let sample = PathBuf::from(sample);
        let dir = temp_dir("sample");
        let file_name = sample.file_name().expect("sample file name");
        fs::copy(&sample, dir.join(file_name)).unwrap();
        fs::write(dir.join("junk.txt"), b"junk").unwrap();
        fs::write(dir.join("broken.rofl"), vec![0u8; 400]).unwrap();

        let library = scan_replay_folder(&dir).expect("scan should succeed");
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(library.replays.len(), 1);
        let replay = &library.replays[0];
        assert_eq!(replay.status, ReplayStatus::Parsed);
        assert!(replay.patch.is_some());
        assert!(replay.region.is_some());
        assert!(replay.game_duration_ms.unwrap_or_default() > 0);
        assert!(replay.modified_ms.is_some());
        assert!(!replay.blue_champions.is_empty());
        assert!(!replay.red_champions.is_empty());
        assert_eq!(library.errors.len(), 1);
        assert!(library.errors[0].path.ends_with("broken.rofl"));
    }

    #[test]
    fn scan_rejects_non_directory() {
        let dir = temp_dir("notdir");
        let file = dir.join("file.rofl");
        fs::write(&file, b"x").unwrap();

        let result = scan_replay_folder(&file);
        let missing = scan_replay_folder(dir.join("missing"));
        fs::remove_dir_all(&dir).unwrap();

        assert!(matches!(result, Err(LibraryError::NotADirectory(_))));
        assert!(matches!(missing, Err(LibraryError::NotADirectory(_))));
    }

    #[test]
    fn sorts_newest_first_with_missing_times_last_and_name_tiebreak() {
        let mut replays = vec![
            summary("c.rofl", None),
            summary("b.rofl", Some(10)),
            summary("a.rofl", None),
            summary("z.rofl", Some(20)),
            summary("a.rofl", Some(10)),
        ];

        sort_summaries(&mut replays);

        let order: Vec<_> = replays
            .iter()
            .map(|r| (r.file_name.as_str(), r.modified_ms))
            .collect();
        assert_eq!(
            order,
            vec![
                ("z.rofl", Some(20)),
                ("a.rofl", Some(10)),
                ("b.rofl", Some(10)),
                ("a.rofl", None),
                ("c.rofl", None),
            ]
        );
    }

    #[test]
    fn summary_serializes_contract_shape() {
        let value = serde_json::to_value(summary("x.rofl", Some(5))).unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "blueChampions",
                "blueKills",
                "fileName",
                "format",
                "gameDurationMs",
                "gameVersion",
                "matchId",
                "modifiedMs",
                "patch",
                "path",
                "redChampions",
                "redKills",
                "region",
                "sizeBytes",
                "status",
                "winningTeam",
            ]
        );
        assert_eq!(value["format"], "unknown");
        assert_eq!(value["status"], "invalid");
        assert_eq!(value["blueChampions"], serde_json::json!([]));
    }

    #[test]
    fn projects_inspection_into_summary_with_team_champions() {
        let dir = temp_dir("project");
        let stats = serde_json::json!([
            { "SKIN": "Nasus", "TEAM": "100", "CHAMPIONS_KILLED": "3", "WIN": "Win" },
            { "SKIN": "Ahri", "TEAM": "200", "CHAMPIONS_KILLED": "1" },
            { "SKIN": "Lux", "TEAM": "100", "CHAMPIONS_KILLED": "2", "WIN": "Win" },
            { "SKIN": "Zed", "TEAM": "200", "CHAMPIONS_KILLED": "4" }
        ]);
        let metadata = serde_json::json!({
            "gameLength": 1_800_000,
            "statsJson": stats.to_string(),
        });
        let path = dir.join("KR-42.rofl");
        fs::write(&path, rofl::rofl2_fixture_bytes(&metadata, "15.3.1.2")).unwrap();

        let inspection = rofl::inspect_file(&path).expect("fixture should parse");
        fs::remove_dir_all(&dir).unwrap();
        let summary = summarize(inspection, Some(7), 99);

        assert_eq!(summary.file_name, "KR-42.rofl");
        assert_eq!(summary.modified_ms, Some(7));
        assert_eq!(summary.size_bytes, 99);
        assert_eq!(summary.format, ReplayFormat::Rofl2);
        assert_eq!(summary.status, ReplayStatus::Parsed);
        assert_eq!(summary.game_version.as_deref(), Some("15.3.1.2"));
        assert_eq!(summary.patch.as_deref(), Some("15.3"));
        assert_eq!(summary.region.as_deref(), Some("KR"));
        assert_eq!(summary.match_id.as_deref(), Some("42"));
        assert_eq!(summary.game_duration_ms, Some(1_800_000));
        assert_eq!(summary.winning_team.as_deref(), Some("Blue"));
        assert_eq!((summary.blue_kills, summary.red_kills), (5, 5));
        assert_eq!(summary.blue_champions, vec!["Nasus", "Lux"]);
        assert_eq!(summary.red_champions, vec!["Ahri", "Zed"]);
    }
}
