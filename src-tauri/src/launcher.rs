use crate::{install::CommandError, locale::normalize_locale};
use serde::Deserialize;
use std::{
    error::Error,
    ffi::OsString,
    fmt::{self, Display},
    path::{Path, PathBuf},
};

const EXECUTABLE_NAME: &str = "League of Legends.exe";
const DEFAULT_LOCALE: &str = "en_US";

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
    ReplayNotAbsolute(PathBuf),
    NotARofl(PathBuf),
    MissingReplay(PathBuf),
    MissingExecutable(PathBuf),
    WrongExecutable(PathBuf),
    NotInGameFolder(PathBuf),
    InvalidLocale(String),
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    Spawn(std::io::Error),
}

impl Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(f, "Replay launch is only supported on Windows"),
            Self::ReplayNotAbsolute(path) => {
                write!(f, "Replay path must be absolute: {}", path.display())
            }
            Self::NotARofl(path) => write!(f, "Replay must be a .rofl file: {}", path.display()),
            Self::MissingReplay(path) => {
                write!(f, "Replay file does not exist: {}", path.display())
            }
            Self::MissingExecutable(path) => {
                write!(f, "League executable does not exist: {}", path.display())
            }
            Self::WrongExecutable(path) => write!(
                f,
                "Select {EXECUTABLE_NAME}, not {}",
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string())
            ),
            Self::NotInGameFolder(path) => write!(
                f,
                "Select {EXECUTABLE_NAME} inside a Game folder (got {})",
                path.display()
            ),
            Self::InvalidLocale(locale) => write!(f, "Invalid Riot locale: {locale}"),
            Self::Spawn(err) => write!(f, "Could not launch League executable: {err}"),
        }
    }
}

impl Error for LaunchError {}

impl LaunchError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedPlatform => "unsupportedPlatform",
            Self::ReplayNotAbsolute(_) | Self::NotARofl(_) | Self::MissingReplay(_) => {
                "invalidReplay"
            }
            Self::MissingExecutable(_) | Self::WrongExecutable(_) | Self::NotInGameFolder(_) => {
                "invalidExecutable"
            }
            Self::InvalidLocale(_) => "invalidLocale",
            Self::Spawn(_) => "spawn",
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::ReplayNotAbsolute(path)
            | Self::NotARofl(path)
            | Self::MissingReplay(path)
            | Self::MissingExecutable(path)
            | Self::WrongExecutable(path)
            | Self::NotInGameFolder(path) => Some(path),
            _ => None,
        }
    }
}

impl From<LaunchError> for CommandError {
    fn from(err: LaunchError) -> Self {
        Self {
            code: err.code().to_string(),
            message: err.to_string(),
            path: err.path().map(|path| path.display().to_string()),
            available_locales: None,
        }
    }
}

/// A request whose paths and locale passed every check in [`validate_request`].
#[derive(Debug, PartialEq, Eq)]
pub struct ValidatedLaunch {
    pub replay: PathBuf,
    pub executable: PathBuf,
    pub locale: String,
}

pub fn launch_replay(request: LaunchReplayRequest) -> Result<(), LaunchError> {
    let validated = validate_request(request)?;
    spawn(&validated)
}

/// Checks the replay (absolute, existing `.rofl`), the executable (canonical,
/// existing `League of Legends.exe` inside a `Game` folder) and the locale.
/// Read-only: nothing is created, modified, or launched here.
pub fn validate_request(request: LaunchReplayRequest) -> Result<ValidatedLaunch, LaunchError> {
    let replay = PathBuf::from(request.replay_path.trim());
    if !replay.is_absolute() {
        return Err(LaunchError::ReplayNotAbsolute(replay));
    }
    if !has_rofl_extension(&replay) {
        return Err(LaunchError::NotARofl(replay));
    }
    if !replay.is_file() {
        return Err(LaunchError::MissingReplay(replay));
    }
    let replay = replay
        .canonicalize()
        .map(|path| simplify_verbatim(&path))
        .map_err(|_| LaunchError::MissingReplay(replay))?;

    let executable = PathBuf::from(request.executable_path.trim());
    let executable = executable
        .canonicalize()
        .map(|path| simplify_verbatim(&path))
        .map_err(|_| LaunchError::MissingExecutable(executable.clone()))?;
    if !executable.is_file() {
        return Err(LaunchError::MissingExecutable(executable));
    }
    check_executable_layout(&executable)?;

    let raw_locale = request
        .locale
        .filter(|locale| !locale.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_LOCALE.to_string());
    let locale = normalize_locale(&raw_locale).ok_or(LaunchError::InvalidLocale(raw_locale))?;

    Ok(ValidatedLaunch {
        replay,
        executable,
        locale,
    })
}

/// Pure path check: the file must be `League of Legends.exe` and sit directly
/// in a folder named `Game` (`<LoL>/Game/...` or a ChronoRift `<dest>/Game/...`).
fn check_executable_layout(executable: &Path) -> Result<(), LaunchError> {
    let is_exe = executable
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(EXECUTABLE_NAME));
    if !is_exe {
        return Err(LaunchError::WrongExecutable(executable.to_path_buf()));
    }

    let game_folder = executable.parent();
    let in_game_folder = game_folder
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("Game"));
    let has_base = game_folder.and_then(Path::parent).is_some();
    if !(in_game_folder && has_base) {
        return Err(LaunchError::NotInGameFolder(executable.to_path_buf()));
    }
    Ok(())
}

fn has_rofl_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("rofl"))
}

/// Turns a Windows verbatim path from `canonicalize` (`\\?\C:\x`, `\\?\UNC\s\x`)
/// back into the plain form the game expects. Other paths pass through.
fn simplify_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// Working directory and arguments for launching `executable` on `replay`.
///
/// Expects paths that passed [`validate_request`]: the executable's folder is
/// the working directory and its parent is `-GameBaseDir`. Only the spawn that
/// uses it is Windows-only; the function itself is tested on every platform.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn build_launch_args(
    replay: &Path,
    executable: &Path,
    locale: &str,
) -> (PathBuf, Vec<OsString>) {
    let game_folder = executable.parent().unwrap_or(Path::new("")).to_path_buf();
    let game_base_dir = game_folder.parent().unwrap_or(Path::new(""));

    let mut base_arg = OsString::from("-GameBaseDir=");
    base_arg.push(game_base_dir.as_os_str());
    let args = vec![
        replay.as_os_str().to_os_string(),
        base_arg,
        OsString::from("-SkipBuild"),
        OsString::from("-EnableLNP"),
        OsString::from("-UseNewX3D=1"),
        OsString::from("-UseNewX3DFramebuffers=1"),
        OsString::from(format!("-Locale={locale}")),
    ];
    (game_folder, args)
}

#[cfg(target_os = "windows")]
fn spawn(launch: &ValidatedLaunch) -> Result<(), LaunchError> {
    let (cwd, args) = build_launch_args(&launch.replay, &launch.executable, &launch.locale);
    std::process::Command::new(&launch.executable)
        .current_dir(cwd)
        .args(args)
        .spawn()
        .map(|_| ())
        .map_err(LaunchError::Spawn)
}

#[cfg(not(target_os = "windows"))]
fn spawn(_launch: &ValidatedLaunch) -> Result<(), LaunchError> {
    Err(LaunchError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "chronorift-launcher-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn request(replay: &Path, exe: &Path, locale: Option<&str>) -> LaunchReplayRequest {
        LaunchReplayRequest {
            replay_path: replay.display().to_string(),
            executable_path: exe.display().to_string(),
            locale: locale.map(str::to_string),
        }
    }

    #[test]
    fn builds_launch_args_from_game_folder() {
        let base = Path::new("/lol/15.3");
        let exe = base.join("Game").join(EXECUTABLE_NAME);
        let replay = Path::new("/replays/LA2-1.rofl");

        let (cwd, args) = build_launch_args(replay, &exe, "es_AR");

        assert_eq!(cwd, base.join("Game"));
        let mut expected_base = OsString::from("-GameBaseDir=");
        expected_base.push(base.as_os_str());
        assert_eq!(
            args,
            vec![
                OsString::from("/replays/LA2-1.rofl"),
                expected_base,
                OsString::from("-SkipBuild"),
                OsString::from("-EnableLNP"),
                OsString::from("-UseNewX3D=1"),
                OsString::from("-UseNewX3DFramebuffers=1"),
                OsString::from("-Locale=es_AR"),
            ]
        );
        assert!(!args.iter().any(|arg| arg == "-SkipRads"));
    }

    #[test]
    fn executable_layout_requires_name_and_game_folder() {
        assert!(check_executable_layout(Path::new("/lol/Game/League of Legends.exe")).is_ok());
        assert!(check_executable_layout(Path::new("/lol/GAME/league of legends.EXE")).is_ok());
        assert!(matches!(
            check_executable_layout(Path::new("/lol/Game/evil.exe")),
            Err(LaunchError::WrongExecutable(_))
        ));
        let err = check_executable_layout(Path::new("/lol/League of Legends.exe")).unwrap_err();
        assert!(matches!(err, LaunchError::NotInGameFolder(_)));
        assert!(err.to_string().contains("inside a Game folder"));
    }

    #[test]
    fn launch_errors_serialize_as_command_errors() {
        let err = CommandError::from(LaunchError::NotInGameFolder(PathBuf::from("/x/a.exe")));
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["code"], "invalidExecutable");
        assert_eq!(json["path"], "/x/a.exe");
        assert!(json["message"].as_str().unwrap().contains("Game folder"));

        let json = serde_json::to_value(CommandError::from(LaunchError::InvalidLocale(
            "en_US -x".into(),
        )))
        .unwrap();
        assert_eq!(json["code"], "invalidLocale");
        assert!(json.get("path").is_none());
        for (err, code) in [
            (
                LaunchError::MissingReplay(PathBuf::from("/r.rofl")),
                "invalidReplay",
            ),
            (LaunchError::UnsupportedPlatform, "unsupportedPlatform"),
            (LaunchError::Spawn(std::io::Error::other("no")), "spawn"),
        ] {
            assert_eq!(CommandError::from(err).code, code);
        }
    }

    #[test]
    fn simplifies_windows_verbatim_paths() {
        assert_eq!(
            simplify_verbatim(Path::new(r"\\?\C:\Riot\Game\League of Legends.exe")),
            PathBuf::from(r"C:\Riot\Game\League of Legends.exe")
        );
        assert_eq!(
            simplify_verbatim(Path::new(r"\\?\UNC\server\share\x.rofl")),
            PathBuf::from(r"\\server\share\x.rofl")
        );
        assert_eq!(
            simplify_verbatim(Path::new("/home/a/x.rofl")),
            PathBuf::from("/home/a/x.rofl")
        );
    }

    #[test]
    fn validates_request_paths_and_locale() {
        let dir = temp_dir("validate");
        let game = dir.join("15.3").join("Game");
        fs::create_dir_all(&game).unwrap();
        let exe = game.join(EXECUTABLE_NAME);
        fs::write(&exe, b"exe").unwrap();
        let replay = dir.join("NA1-1.ROFL");
        fs::write(&replay, b"replay").unwrap();
        let text = dir.join("notes.txt");
        fs::write(&text, b"x").unwrap();
        let loose_exe = dir.join(EXECUTABLE_NAME);
        fs::write(&loose_exe, b"exe").unwrap();

        let ok = validate_request(request(&replay, &exe, Some("es_ar")));
        let default_locale = validate_request(request(&replay, &exe, None));
        let relative = validate_request(request(Path::new("NA1-1.rofl"), &exe, None));
        let not_rofl = validate_request(request(&text, &exe, None));
        let missing = validate_request(request(&dir.join("gone.rofl"), &exe, None));
        let missing_exe = validate_request(request(&replay, &game.join("nope.exe"), None));
        let loose = validate_request(request(&replay, &loose_exe, None));
        let bad_locale = validate_request(request(&replay, &exe, Some("en_US -x")));
        fs::remove_dir_all(&dir).unwrap();

        let ok = ok.expect("valid request");
        assert_eq!(ok.locale, "es_AR");
        assert!(ok.executable.is_absolute());
        assert!(ok.replay.ends_with("NA1-1.ROFL"));
        assert_eq!(default_locale.unwrap().locale, "en_US");
        assert!(matches!(relative, Err(LaunchError::ReplayNotAbsolute(_))));
        assert!(matches!(not_rofl, Err(LaunchError::NotARofl(_))));
        assert!(matches!(missing, Err(LaunchError::MissingReplay(_))));
        assert!(matches!(
            missing_exe,
            Err(LaunchError::MissingExecutable(_))
        ));
        assert!(matches!(loose, Err(LaunchError::NotInGameFolder(_))));
        assert!(matches!(bad_locale, Err(LaunchError::InvalidLocale(_))));
    }
}
