pub mod commands;
pub mod services;

use std::sync::Mutex;
const SETTINGS_FILE:&str = "settings.json";
const GOLEM_CLI_PATH:&str = "golem_cli_path";
// Define storage state to hold app-wide variables
pub struct AppState {
    pub backend_url: Mutex<String>,
    pub golem_cli_path: Mutex<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            backend_url: Mutex::new(String::from("http://localhost:9881")),
            golem_cli_path: Mutex::new(String::new()),
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize app state
    let app_state = AppState::default();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(app_state) // Register app state
        .invoke_handler(tauri::generate_handler![
            commands::golem_commands::create_golem_app,
            commands::golem_commands::call_golem_command,
            commands::backend_commands::set_golem_cli_path,
            commands::backend_commands::get_golem_cli_path
        ])
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_websocket::init())
        .plugin(tauri_plugin_fs::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
