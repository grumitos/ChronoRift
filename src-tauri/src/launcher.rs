use serde::Deserialize;
use std::{
    error::Error,
    fmt::{self, Display},
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchReplayRequest {
    pub replay_path: String,
    pub executable_path: String,
    pub locale: Option<String>,
}

#[derive(Debug)]
pub enum LaunchError {
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    UnsupportedPlatform,
    MissingReplay(PathBuf),
    MissingExecutable(PathBuf),
    MissingGameFolder(PathBuf),
    InvalidLocale(String),
    Spawn(std::io::Error),
}

impl Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(f, "Replay launch is only supported on Windows"),
            Self::MissingReplay(path) => {
                write!(f, "Replay file does not exist: {}", path.display())
            }
            Self::MissingExecutable(path) => {
                write!(f, "League executable does not exist: {}", path.display())
            }
            Self::MissingGameFolder(path) => {
                write!(
                    f,
                    "League executable has no game folder: {}",
                    path.display()
                )
            }
            Self::InvalidLocale(locale) => write!(f, "Invalid Riot locale: {locale}"),
            Self::Spawn(err) => write!(f, "Could not launch League executable: {err}"),
        }
    }
}

impl Error for LaunchError {}

pub fn launch_replay(request: LaunchReplayRequest) -> Result<(), LaunchError> {
    launch_replay_impl(request)
}

#[cfg(target_os = "windows")]
fn launch_replay_impl(request: LaunchReplayRequest) -> Result<(), LaunchError> {
    use std::process::Command;

    let replay_path = PathBuf::from(request.replay_path);
    if !replay_path.is_file() {
        return Err(LaunchError::MissingReplay(replay_path));
    }

    let executable_path = PathBuf::from(request.executable_path);
    if !executable_path.is_file() {
        return Err(LaunchError::MissingExecutable(executable_path));
    }

    let game_folder = executable_path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| LaunchError::MissingGameFolder(executable_path.clone()))?;
    let game_base_dir = game_folder
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| LaunchError::MissingGameFolder(game_folder.clone()))?;

    let locale = request.locale.unwrap_or_else(|| "en_US".to_string());
    if !is_safe_riot_locale(&locale) {
        return Err(LaunchError::InvalidLocale(locale));
    }

    Command::new(executable_path)
        .current_dir(&game_folder)
        .arg(replay_path)
        .arg(format!("-GameBaseDir={}", game_base_dir.display()))
        .arg("-SkipRads")
        .arg("-SkipBuild")
        .arg("-EnableLNP")
        .arg("-UseNewX3D=1")
        .arg("-UseNewX3DFramebuffers=1")
        .arg(format!("-Locale={locale}"))
        .spawn()
        .map(|_| ())
        .map_err(LaunchError::Spawn)
}

#[cfg(not(target_os = "windows"))]
fn launch_replay_impl(_request: LaunchReplayRequest) -> Result<(), LaunchError> {
    Err(LaunchError::UnsupportedPlatform)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_safe_riot_locale(locale: &str) -> bool {
    let bytes = locale.as_bytes();
    bytes.len() == 5
        && bytes[2] == b'_'
        && bytes[..2].iter().all(u8::is_ascii_lowercase)
        && bytes[3..].iter().all(u8::is_ascii_uppercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_riot_locale_shape() {
        assert!(is_safe_riot_locale("en_US"));
        assert!(is_safe_riot_locale("es_MX"));
        assert!(!is_safe_riot_locale("english"));
        assert!(!is_safe_riot_locale("en-us"));
        assert!(!is_safe_riot_locale("en_US --bad"));
    }
}
