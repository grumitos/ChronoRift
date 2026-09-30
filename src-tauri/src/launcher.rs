use crate::client::GAME_EXE;
use serde::Deserialize;
use std::{
    error::Error,
    ffi::OsString,
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
    NotGameExecutable(PathBuf),
    MissingExecutable(PathBuf),
    MissingGameFolder(PathBuf),
    InvalidLocale(String),
    Spawn(std::io::Error),
}

impl Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(f, "Reproducir solo funciona en Windows"),
            Self::MissingReplay(path) => {
                write!(f, "La repetición no existe: {}", path.display())
            }
            Self::NotGameExecutable(path) => {
                write!(f, "No es {GAME_EXE}: {}", path.display())
            }
            Self::MissingExecutable(path) => {
                write!(f, "El ejecutable no existe: {}", path.display())
            }
            Self::MissingGameFolder(path) => {
                write!(
                    f,
                    "El ejecutable no tiene carpeta de juego: {}",
                    path.display()
                )
            }
            Self::InvalidLocale(locale) => write!(f, "Idioma no válido: {locale}"),
            Self::Spawn(err) => write!(f, "No se pudo iniciar League of Legends: {err}"),
        }
    }
}

impl Error for LaunchError {}

pub fn launch_replay(request: LaunchReplayRequest) -> Result<(), LaunchError> {
    launch_replay_impl(request)
}

/// Proceso a lanzar, validado por completo antes de ejecutar nada.
#[derive(Debug)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
struct LaunchPlan {
    executable: PathBuf,
    working_dir: PathBuf,
    args: Vec<OsString>,
}

/// Valida rutas y locale y arma los argumentos. `-GameBaseDir` es la carpeta que contiene
/// `Game`, igual que en una instalación de Riot (`<base>\Game\League of Legends.exe`).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn plan_launch(request: LaunchReplayRequest) -> Result<LaunchPlan, LaunchError> {
    let replay_path = PathBuf::from(request.replay_path);
    if !replay_path.is_file() {
        return Err(LaunchError::MissingReplay(replay_path));
    }

    let executable = PathBuf::from(request.executable_path);
    // Solo el juego: cualquier otro programa quedaría abierto con la repetición como argumento.
    let is_game = executable
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(GAME_EXE));
    if !is_game {
        return Err(LaunchError::NotGameExecutable(executable));
    }
    if !executable.is_file() {
        return Err(LaunchError::MissingExecutable(executable));
    }

    let working_dir = executable
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| LaunchError::MissingGameFolder(executable.clone()))?;
    let game_base_dir = working_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| LaunchError::MissingGameFolder(working_dir.clone()))?;

    let locale = request.locale.unwrap_or_else(|| "en_US".to_string());
    if !is_safe_riot_locale(&locale) {
        return Err(LaunchError::InvalidLocale(locale));
    }

    let args = vec![
        replay_path.into_os_string(),
        format!("-GameBaseDir={}", game_base_dir.display()).into(),
        "-SkipRads".into(),
        "-SkipBuild".into(),
        "-EnableLNP".into(),
        "-UseNewX3D=1".into(),
        "-UseNewX3DFramebuffers=1".into(),
        format!("-Locale={locale}").into(),
    ];
    Ok(LaunchPlan {
        executable,
        working_dir,
        args,
    })
}

#[cfg(target_os = "windows")]
fn launch_replay_impl(request: LaunchReplayRequest) -> Result<(), LaunchError> {
    let plan = plan_launch(request)?;
    std::process::Command::new(&plan.executable)
        .current_dir(&plan.working_dir)
        .args(&plan.args)
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
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    /// Arma el lanzamiento para un cliente con el diseño de ChronoRift sin ejecutar nada y
    /// comprueba que rutas inexistentes, otros ejecutables y locales con argumentos inyectados se
    /// rechazan antes.
    #[test]
    fn plans_a_replay_launch_for_a_chronorift_client_without_spawning() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("chronorift-launch-{unique}"));
        let build = root.join("16.18.817.5716");
        let game = build.join("Game");
        fs::create_dir_all(&game).unwrap();
        let executable = game.join("League of Legends.exe");
        fs::write(&executable, b"placeholder, never executed").unwrap();
        let replay = root.join("LA2-1625354615.rofl");
        fs::write(&replay, b"replay").unwrap();
        let request = |replay: &Path, executable: &Path, locale: &str| LaunchReplayRequest {
            replay_path: replay.display().to_string(),
            executable_path: executable.display().to_string(),
            locale: Some(locale.to_string()),
        };

        let plan = plan_launch(request(&replay, &executable, "es_MX")).unwrap();
        assert_eq!(plan.executable, executable);
        assert_eq!(plan.working_dir, game);
        assert_eq!(plan.args[0].as_os_str(), replay.as_os_str());
        assert!(plan
            .args
            .contains(&format!("-GameBaseDir={}", build.display()).into()));
        assert!(plan.args.contains(&"-Locale=es_MX".into()));

        assert!(matches!(
            plan_launch(request(&root.join("missing.rofl"), &executable, "es_MX")),
            Err(LaunchError::MissingReplay(_))
        ));
        assert!(matches!(
            plan_launch(request(
                &replay,
                &root.join("missing").join(GAME_EXE),
                "es_MX"
            )),
            Err(LaunchError::MissingExecutable(_))
        ));
        // Solo `League of Legends.exe`, sin distinguir mayúsculas: otro programa nunca se lanza.
        let other = game.join("Setup.exe");
        fs::write(&other, b"another program, never executed").unwrap();
        assert!(matches!(
            plan_launch(request(&replay, &other, "es_MX")),
            Err(LaunchError::NotGameExecutable(_))
        ));
        let upper_game = root.join("16.19.821.7343").join("Game");
        fs::create_dir_all(&upper_game).unwrap();
        let upper = upper_game.join("LEAGUE OF LEGENDS.EXE");
        fs::write(&upper, b"placeholder, never executed").unwrap();
        assert_eq!(
            plan_launch(request(&replay, &upper, "es_MX"))
                .unwrap()
                .executable,
            upper
        );
        for locale in ["english", "en-us", "en_US --bad", "EN_us"] {
            assert!(
                matches!(
                    plan_launch(request(&replay, &executable, locale)),
                    Err(LaunchError::InvalidLocale(_))
                ),
                "{locale}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// Lanzamiento real, solo con CHRONORIFT_REAL_LAUNCH_EXE y CHRONORIFT_REAL_LAUNCH_REPLAY:
    /// arranca el juego con la repetición y termina; el juego sigue abierto.
    #[cfg(windows)]
    #[test]
    #[ignore = "starts League of Legends; run with --ignored"]
    fn real_launch_starts_the_game_for_a_replay() {
        let (Ok(executable), Ok(replay)) = (
            std::env::var("CHRONORIFT_REAL_LAUNCH_EXE"),
            std::env::var("CHRONORIFT_REAL_LAUNCH_REPLAY"),
        ) else {
            eprintln!("skipped: set CHRONORIFT_REAL_LAUNCH_EXE and CHRONORIFT_REAL_LAUNCH_REPLAY");
            return;
        };
        let locale = std::env::var("CHRONORIFT_REAL_LOCALE").ok();
        let request = || LaunchReplayRequest {
            replay_path: replay.clone(),
            executable_path: executable.clone(),
            locale: locale.clone(),
        };
        eprintln!("{:?}", plan_launch(request()).expect("launch plan"));
        launch_replay(request()).expect("the game process should start");
    }
}
