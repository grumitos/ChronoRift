mod client;
mod clients;
mod launcher;
mod library;
mod locale;
mod repair;
mod rofl;
mod settings;

use rofl::ReplayInspection;
use settings::Settings;
use std::path::PathBuf;
use tauri::Manager;

// Async + blocking pool: file I/O must not stall the UI thread.
#[tauri::command]
async fn inspect_replay(path: String) -> Result<ReplayInspection, String> {
    tauri::async_runtime::spawn_blocking(move || rofl::inspect_file(path))
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn scan_replay_folder(path: String) -> Result<library::ReplayLibrary, String> {
    tauri::async_runtime::spawn_blocking(move || library::scan_replay_folder(path))
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn find_installed_clients(root: String) -> Result<Vec<clients::InstalledClient>, String> {
    Ok(clients::find_installed_clients(root))
}

fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(settings::SETTINGS_FILE))
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn load_settings(app: tauri::AppHandle) -> Result<Settings, String> {
    Ok(settings::load_from(&settings_path(&app)?))
}

#[tauri::command]
fn save_settings(app: tauri::AppHandle, settings: Settings) -> Result<Settings, String> {
    settings::save_to(&settings_path(&app)?, &settings).map_err(|err| err.to_string())
}

#[tauri::command]
async fn launch_replay(request: launcher::LaunchReplayRequest) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || launcher::launch_replay(request))
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn install_client(
    app: tauri::AppHandle,
    request: client::ClientInstallRequest,
) -> Result<client::ClientInstallResult, String> {
    client::install_client(app, request)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn plan_client_install(
    app: tauri::AppHandle,
    request: client::ClientInstallRequest,
) -> Result<client::ClientInstallPlan, String> {
    client::plan_client_install(app, request)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn plan_client_repair(
    app: tauri::AppHandle,
    request: client::ClientInstallRequest,
) -> Result<repair::ClientRepairPlan, String> {
    repair::plan_client_repair(app, request)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn repair_client(
    app: tauri::AppHandle,
    request: repair::ClientRepairRequest,
) -> Result<client::ClientInstallResult, String> {
    repair::repair_client(app, request)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn cancel_client_install(app: tauri::AppHandle) -> bool {
    client::cancel_client_install(&app)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(client::InstallCancel::default())
        .manage(client::InstallBusy::default())
        .invoke_handler(tauri::generate_handler![
            inspect_replay,
            scan_replay_folder,
            load_settings,
            save_settings,
            find_installed_clients,
            launch_replay,
            install_client,
            plan_client_install,
            plan_client_repair,
            repair_client,
            cancel_client_install
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
