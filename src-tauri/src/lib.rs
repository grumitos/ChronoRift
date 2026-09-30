mod asset_cache;
mod client;
mod launcher;
mod library;
mod rofl;
mod settings;
mod vanguard;

use rofl::ReplayInspection;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use tauri::{Emitter, Manager};

/// Sale de lo ya leído al escanear la carpeta mientras el archivo no cambie.
#[tauri::command]
async fn inspect_replay(
    path: String,
    replays: tauri::State<'_, library::ReplayCache>,
) -> Result<ReplayInspection, String> {
    let replays = replays.inner().clone();
    blocking(move || replays.inspect(Path::new(&path))).await
}

#[tauri::command]
async fn inspect_replay_folder(
    path: String,
    replays: tauri::State<'_, library::ReplayCache>,
) -> Result<library::ReplayLibrary, String> {
    let replays = replays.inner().clone();
    blocking(move || {
        let (library, scanned) = library::scan_folder(path).map_err(|error| error.to_string())?;
        replays.replace(scanned);
        Ok(library)
    })
    .await
}

/// Valida las rutas y crea el proceso fuera del hilo principal: una unidad lenta no congela la
/// ventana. Un hilo espera al juego y avisa con `replay-exited` cuando se cierra.
#[tauri::command]
async fn launch_replay(
    app: tauri::AppHandle,
    request: launcher::LaunchReplayRequest,
) -> Result<(), String> {
    blocking(move || {
        // La ruta tal como la envió la interfaz, que la compara con la repetición en curso.
        let replay_path = request.replay_path.clone();
        let game = launcher::launch_replay(request).map_err(|err| err.to_string())?;
        launcher::watch_exit(game, replay_path, move |exited| {
            // Si la app se está cerrando, no queda a quién avisar.
            let _ = app.emit(launcher::REPLAY_EXITED_EVENT, exited);
        })
        .map_err(|err| {
            format!("El juego se inició, pero ChronoRift no podrá saber cuándo se cierra: {err}")
        })
    })
    .await
}

/// Estado del controlador de Riot Vanguard. Solo lee: ChronoRift nunca lo detiene.
#[tauri::command]
async fn vanguard_status() -> Result<vanguard::ServiceState, String> {
    blocking(|| Ok(vanguard::vanguard_state())).await
}

#[tauri::command]
async fn install_client(
    app: tauri::AppHandle,
    request: client::ClientInstallRequest,
) -> Result<client::ClientInstallResult, String> {
    let connections = settings_path(&app)
        .ok()
        .and_then(|path| settings::load(&path).ok())
        .and_then(|settings| settings.download_connections);
    // La instalación limpia la señal al empezar, y solo si no hay otra en curso: así una segunda
    // llamada no borra el "Detener" de la primera.
    let cancel = app.state::<client::CancelSignal>().inner().clone();
    client::install_client(
        &app,
        request,
        client::DownloadTuning::with_connections(connections),
        &cancel,
    )
    .await
    .map_err(|err| err.to_string())
}

/// Detiene la instalación en curso: aborta las peticiones en vuelo; lo confirmado queda para reanudar.
#[tauri::command]
fn cancel_client_install(cancel: tauri::State<'_, client::CancelSignal>) {
    cancel.cancel();
}

#[tauri::command]
async fn plan_client_install(
    app: tauri::AppHandle,
    request: client::ClientInstallRequest,
) -> Result<client::ClientInstallPlan, String> {
    client::plan_client_install(&app, request)
        .await
        .map_err(|err| err.to_string())
}

/// Estado en disco de las compilaciones pedidas, por versión de repetición. Solo lee. Si la unidad
/// de la carpeta de clientes no está, rechaza con "La carpeta de clientes no está disponible: …".
#[tauri::command]
async fn client_statuses(
    clients_root: String,
    versions: Vec<String>,
) -> Result<HashMap<String, client::LocalClientStatus>, String> {
    blocking(move || {
        client::local_client_statuses(&clients_root, &versions).map_err(|error| error.to_string())
    })
    .await
}

/// Compilaciones con archivos en la carpeta de clientes. Solo lee.
#[tauri::command]
async fn list_clients(clients_root: String) -> Result<Vec<client::LocalClientStatus>, String> {
    blocking(move || client::list_local_clients(&clients_root).map_err(|error| error.to_string()))
        .await
}

#[tauri::command]
async fn get_settings(app: tauri::AppHandle) -> Result<settings::AppSettings, String> {
    let file = settings_path(&app)?;
    blocking(move || settings::load(&file).map_err(|err| err.to_string())).await
}

/// Guarda la carpeta de clientes. No crea la carpeta: eso solo ocurre al confirmar una instalación.
/// Validarla consulta la unidad, que puede ser de red y tardar.
#[tauri::command]
async fn set_clients_root(
    app: tauri::AppHandle,
    path: String,
) -> Result<settings::AppSettings, String> {
    let file = settings_path(&app)?;
    blocking(move || {
        let root = settings::normalize_clients_root(&path).map_err(|err| err.to_string())?;
        settings::update(&file, |settings| {
            settings.clients_root = Some(root.display().to_string());
        })
        .map_err(|err| err.to_string())
    })
    .await
}

/// Guarda si las imágenes de los parches de la biblioteca se descargan de una vez o al abrir cada
/// partida. Conserva los demás ajustes.
#[tauri::command]
async fn set_asset_prefetch(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<settings::AppSettings, String> {
    let file = settings_path(&app)?;
    blocking(move || {
        settings::update(&file, |settings| settings.asset_prefetch = Some(enabled))
            .map_err(|err| err.to_string())
    })
    .await
}

/// Los comandos sincrónicos corren en el hilo principal: el trabajo de disco va al pool bloqueante
/// para que una unidad lenta o de red no congele la ventana.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| format!("Falló una tarea interna: {error}"))?
}

fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(settings::SETTINGS_FILE))
        .map_err(|err| format!("Sin carpeta de configuración: {err}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(client::CancelSignal::default())
        .manage(library::ReplayCache::default())
        .setup(|app| {
            let root = app.path().app_cache_dir()?.join("assets");
            // Temporales de una escritura cortada en una ejecución anterior, sin frenar el inicio.
            let stale = root.clone();
            tauri::async_runtime::spawn_blocking(move || asset_cache::remove_stale_temps(&stale));
            app.manage(asset_cache::AssetCache::new(root));
            Ok(())
        })
        // Asíncrono: una descarga nunca bloquea el hilo principal.
        .register_asynchronous_uri_scheme_protocol(
            asset_cache::SCHEME,
            |context, request, responder| {
                let app = context.app_handle().clone();
                let path = request.uri().path().to_string();
                // CORS solo para la interfaz: `http_response` compara este origen con los de la app.
                let origin = request
                    .headers()
                    .get(tauri::http::header::ORIGIN)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                tauri::async_runtime::spawn(async move {
                    let result = match app.try_state::<asset_cache::AssetCache>() {
                        Some(cache) => cache.get(&path).await,
                        None => Err(asset_cache::AssetError::Upstream(
                            "asset cache not ready".to_string(),
                        )),
                    };
                    responder.respond(asset_cache::http_response(result, origin.as_deref()));
                });
            },
        )
        .invoke_handler(tauri::generate_handler![
            inspect_replay,
            inspect_replay_folder,
            launch_replay,
            vanguard_status,
            install_client,
            cancel_client_install,
            plan_client_install,
            client_statuses,
            list_clients,
            get_settings,
            set_clients_root,
            set_asset_prefetch
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
