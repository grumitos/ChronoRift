mod client;
mod installer;
mod launcher;
mod rofl;

use rofl::ReplayInspection;

#[tauri::command]
fn inspect_replay(path: String) -> Result<ReplayInspection, String> {
    rofl::inspect_file(path).map_err(|err| err.to_string())
}

#[tauri::command]
fn launch_replay(request: launcher::LaunchReplayRequest) -> Result<(), String> {
    launcher::launch_replay(request).map_err(|err| err.to_string())
}

#[tauri::command]
async fn download_artifact(
    app: tauri::AppHandle,
    request: installer::DownloadArtifactRequest,
) -> Result<(), String> {
    installer::download_artifact(app, request)
        .await
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            inspect_replay,
            launch_replay,
            download_artifact,
            install_client
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
