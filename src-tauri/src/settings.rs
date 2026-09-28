use crate::locale::normalize_locale;
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fmt::{self, Display},
    fs, io,
    path::{Path, PathBuf},
};

pub const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub replay_folder: Option<String>,
    pub clients_root: Option<String>,
    pub executable_path: Option<String>,
    pub locale: Option<String>,
}

#[derive(Debug)]
pub enum SettingsError {
    Io(io::Error),
    Serialize(serde_json::Error),
    InvalidPath,
}

impl Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "Could not write settings: {err}"),
            Self::Serialize(err) => write!(f, "Could not serialize settings: {err}"),
            Self::InvalidPath => write!(f, "Invalid settings path"),
        }
    }
}

impl Error for SettingsError {}

impl From<io::Error> for SettingsError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl Settings {
    /// Trims values, turns empty strings into `None`, and drops malformed locales.
    pub fn normalized(self) -> Self {
        Self {
            replay_folder: clean(self.replay_folder),
            clients_root: clean(self.clients_root),
            executable_path: clean(self.executable_path),
            locale: clean(self.locale).and_then(|locale| normalize_locale(&locale)),
        }
    }
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// Reads settings from `path`. A missing or corrupt file yields defaults.
pub fn load_from(path: &Path) -> Settings {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
        .map(Settings::normalized)
        .unwrap_or_default()
}

/// Normalizes and writes settings atomically (temp file + rename), returning what was stored.
pub fn save_to(path: &Path, settings: &Settings) -> Result<Settings, SettingsError> {
    let normalized = settings.clone().normalized();
    let parent = path.parent().ok_or(SettingsError::InvalidPath)?;
    let file_name = path.file_name().ok_or(SettingsError::InvalidPath)?;
    fs::create_dir_all(parent)?;

    let json = serde_json::to_vec_pretty(&normalized).map_err(SettingsError::Serialize)?;
    let mut temp_name = file_name.to_os_string();
    temp_name.push(".tmp");
    let temp_path: PathBuf = parent.join(temp_name);
    fs::write(&temp_path, json)?;
    if let Err(err) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(err.into());
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "chronorift-settings-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn roundtrips_settings_and_creates_parent_dir() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("nested").join(SETTINGS_FILE);
        let settings = Settings {
            replay_folder: Some("/replays".to_string()),
            clients_root: Some("/clients".to_string()),
            executable_path: None,
            locale: Some("es_MX".to_string()),
        };

        let saved = save_to(&path, &settings).expect("save should succeed");
        let loaded = load_from(&path);
        let raw = fs::read_to_string(&path).unwrap();
        let leftover_temp = dir.join("nested").join("settings.json.tmp").exists();
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(saved, settings);
        assert_eq!(loaded, settings);
        assert!(raw.contains("\"replayFolder\""));
        assert!(raw.contains("\"executablePath\": null"));
        assert!(!leftover_temp);
    }

    #[test]
    fn missing_or_corrupt_file_loads_defaults() {
        let dir = temp_dir("corrupt");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SETTINGS_FILE);
        fs::write(&path, b"{ not json").unwrap();

        let corrupt = load_from(&path);
        let missing = load_from(&dir.join("missing.json"));
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(corrupt, Settings::default());
        assert_eq!(missing, Settings::default());
    }

    #[test]
    fn normalizes_empty_strings_and_invalid_locale() {
        let dir = temp_dir("normalize");
        let path = dir.join(SETTINGS_FILE);
        let settings = Settings {
            replay_folder: Some("   ".to_string()),
            clients_root: Some("".to_string()),
            executable_path: Some("  /game/League of Legends.exe ".to_string()),
            locale: Some("english".to_string()),
        };

        let saved = save_to(&path, &settings).expect("save should succeed");
        let loaded = load_from(&path);
        fs::remove_dir_all(&dir).unwrap();

        let expected = Settings {
            executable_path: Some("/game/League of Legends.exe".to_string()),
            ..Settings::default()
        };
        assert_eq!(saved, expected);
        assert_eq!(loaded, expected);
        assert_eq!(
            Settings {
                locale: Some("ko_kr".to_string()),
                ..Settings::default()
            }
            .normalized()
            .locale,
            Some("ko_KR".to_string())
        );
    }

    #[test]
    fn partial_json_fills_missing_fields() {
        let dir = temp_dir("partial");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SETTINGS_FILE);
        fs::write(&path, br#"{"replayFolder":"/r"}"#).unwrap();

        let loaded = load_from(&path);
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(loaded.replay_folder.as_deref(), Some("/r"));
        assert_eq!(loaded.locale, None);
    }
}
