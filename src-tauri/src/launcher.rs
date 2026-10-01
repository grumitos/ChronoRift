use crate::{client::GAME_EXE, tr};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    ffi::OsString,
    fmt::{self, Display},
    io,
    path::{Path, PathBuf},
    process::Child,
    thread,
};

/// Evento que avisa a la interfaz que el juego iniciado por Reproducir se cerró.
pub const REPLAY_EXITED_EVENT: &str = "replay-exited";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchReplayRequest {
    pub replay_path: String,
    pub executable_path: String,
    pub locale: Option<String>,
}

/// Carga de `replay-exited`: la ruta tal como la envió la interfaz, para que la compare con la
/// repetición en curso, y el código de salida del juego (`None` si no se pudo esperar).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReplayExited {
    pub replay_path: String,
    pub exit_code: Option<i32>,
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
            Self::UnsupportedPlatform => tr!(
                write!(f, "Reproducir solo funciona en Windows"),
                write!(f, "Playing replays only works on Windows")
            ),
            Self::MissingReplay(path) => tr!(
                write!(f, "La repetición no existe: {}", path.display()),
                write!(f, "The replay does not exist: {}", path.display())
            ),
            Self::NotGameExecutable(path) => tr!(
                write!(f, "No es {GAME_EXE}: {}", path.display()),
                write!(f, "This is not {GAME_EXE}: {}", path.display())
            ),
            Self::MissingExecutable(path) => tr!(
                write!(f, "El ejecutable no existe: {}", path.display()),
                write!(f, "The executable does not exist: {}", path.display())
            ),
            Self::MissingGameFolder(path) => tr!(
                write!(
                    f,
                    "El ejecutable no tiene carpeta de juego: {}",
                    path.display()
                ),
                write!(f, "The executable has no game folder: {}", path.display())
            ),
            Self::InvalidLocale(locale) => tr!(
                write!(f, "Idioma no válido: {locale}"),
                write!(f, "Invalid locale: {locale}")
            ),
            Self::Spawn(err) => tr!(
                write!(f, "No se pudo iniciar League of Legends: {err}"),
                write!(f, "Could not start League of Legends: {err}")
            ),
        }
    }
}

impl Error for LaunchError {}

/// Inicia el juego con la repetición y devuelve el proceso, para esperar a que se cierre.
pub fn launch_replay(request: LaunchReplayRequest) -> Result<Child, LaunchError> {
    launch_replay_impl(request)
}

/// Espera al proceso en un hilo propio y llama a `notify` cuando termina. Si esperar falla, el
/// aviso llega igual, sin código de salida: la interfaz nunca queda en "Reproduciendo".
pub fn watch_exit(
    mut game: Child,
    replay_path: String,
    notify: impl FnOnce(ReplayExited) + Send + 'static,
) -> io::Result<()> {
    thread::Builder::new()
        .name("replay-exit".to_string())
        .spawn(move || {
            let exit_code = game.wait().ok().and_then(|status| status.code());
            notify(ReplayExited {
                replay_path,
                exit_code,
            });
        })
        .map(|_| ())
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
fn launch_replay_impl(request: LaunchReplayRequest) -> Result<Child, LaunchError> {
    let plan = plan_launch(request)?;
    std::process::Command::new(&plan.executable)
        .current_dir(&plan.working_dir)
        .args(&plan.args)
        .spawn()
        .map_err(LaunchError::Spawn)
}

#[cfg(not(target_os = "windows"))]
fn launch_replay_impl(_request: LaunchReplayRequest) -> Result<Child, LaunchError> {
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
        process::Command,
        sync::mpsc,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    /// Arma el lanzamiento para un cliente con el diseño de ChronoRift sin iniciar el juego y
    /// comprueba que rutas inexistentes, otros ejecutables y locales con argumentos inyectados se
    /// rechazan antes. El aviso de salida se prueba con un proceso inocuo en lugar del juego.
    #[test]
    fn plans_a_replay_launch_and_reports_the_exit_without_starting_the_game() {
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
        // Línea de comandos completa, en orden: la repetición y los parámetros de League.
        let expected: Vec<OsString> = vec![
            replay.clone().into_os_string(),
            format!("-GameBaseDir={}", build.display()).into(),
            "-SkipRads".into(),
            "-SkipBuild".into(),
            "-EnableLNP".into(),
            "-UseNewX3D=1".into(),
            "-UseNewX3DFramebuffers=1".into(),
            "-Locale=es_MX".into(),
        ];
        assert_eq!(plan.args, expected);

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

        // Aviso de salida con un proceso inocuo que termina con código 3 en lugar del juego: la
        // carga llega tal como la recibe la interfaz, con la ruta que envió.
        #[cfg(windows)]
        let harmless = Command::new("cmd").args(["/C", "exit", "3"]).spawn();
        #[cfg(not(windows))]
        let harmless = Command::new("sh").args(["-c", "exit 3"]).spawn();
        let replay_path = replay.display().to_string();
        let (sender, receiver) = mpsc::channel();
        watch_exit(harmless.unwrap(), replay_path.clone(), move |exited| {
            let _ = sender.send(exited);
        })
        .unwrap();
        let exited = receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("the exit should be reported");
        assert_eq!(
            serde_json::to_value(exited).unwrap(),
            serde_json::json!({ "replayPath": replay_path, "exitCode": 3 })
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Lanzamiento real, solo con CHRONORIFT_REAL_LAUNCH_EXE y CHRONORIFT_REAL_LAUNCH_REPLAY:
    /// arranca el juego con la repetición como la app y termina cuando el juego se cierra, con la
    /// carga de `replay-exited`. Si llega enseguida, el proceso no vive lo que dura la repetición.
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
        let game = launch_replay(request()).expect("the game process should start");
        let (sender, receiver) = mpsc::channel();
        watch_exit(game, replay.clone(), move |exited| {
            let _ = sender.send(exited);
        })
        .expect("the exit watcher should start");
        eprintln!("waiting for the game to close");
        let exited = receiver.recv().expect("the exit should be reported");
        eprintln!("{}", serde_json::to_string(&exited).unwrap());
    }
}
