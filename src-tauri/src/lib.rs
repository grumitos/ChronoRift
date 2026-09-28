mod install;
mod launcher;
mod library;
mod locale;
mod rofl;
mod settings;

use install::{
    clients::InstalledClient, ClientInstallPlan, ClientInstallRequest, ClientInstallResult,
    ClientRepairPlan, ClientRepairPlanRequest, ClientRepairRequest, CommandError,
    InstallClientRequest, InstallRuntime,
};
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
fn find_installed_clients(root: String) -> Result<Vec<InstalledClient>, String> {
    Ok(install::clients::find_installed_clients(root))
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
async fn launch_replay(request: launcher::LaunchReplayRequest) -> Result<(), CommandError> {
    tauri::async_runtime::spawn_blocking(move || launcher::launch_replay(request))
        .await
        .map_err(|err| CommandError {
            code: "spawn".into(),
            message: format!("Could not launch League executable: {err}"),
            path: None,
            available_locales: None,
        })?
        .map_err(CommandError::from)
}

#[tauri::command]
async fn plan_client_install(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, InstallRuntime>,
    request: ClientInstallRequest,
) -> Result<ClientInstallPlan, CommandError> {
    Ok(install::plan_client_install(&runtime, &app, request).await?)
}

#[tauri::command]
async fn install_client(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, InstallRuntime>,
    request: InstallClientRequest,
) -> Result<ClientInstallResult, CommandError> {
    Ok(install::install_client(&runtime, &app, request).await?)
}

#[tauri::command]
async fn plan_client_repair(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, InstallRuntime>,
    request: ClientRepairPlanRequest,
) -> Result<ClientRepairPlan, CommandError> {
    Ok(install::plan_client_repair(&runtime, &app, request).await?)
}

#[tauri::command]
async fn repair_client(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, InstallRuntime>,
    request: ClientRepairRequest,
) -> Result<ClientInstallResult, CommandError> {
    Ok(install::repair_client(&runtime, &app, request).await?)
}

#[tauri::command]
fn cancel_client_install(runtime: tauri::State<'_, InstallRuntime>) -> bool {
    runtime.cancel()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(InstallRuntime::default())
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
