use crate::{GOLEM_CLI_PATH, SETTINGS_FILE};
use std::path::Path;
use tauri::{AppHandle};
use tauri_plugin_store::StoreExt;
use crate::services::command_executor::GolemCommandExecutor;

/// Updates the golem-cli path
#[tauri::command]
pub fn set_golem_cli_path(path: String, app_handle: AppHandle) -> Result<(), String> {
    // Validate the path exists
    if !Path::new(&path).exists() {
        return Err(format!("The specified path does not exist: {}", path));
    }

    println!("Updated golem-cli path to: {}", path);
    let store = app_handle.store(SETTINGS_FILE).unwrap();
    store.set(GOLEM_CLI_PATH, path);
    Ok(())
}

/// Gets the currently configured golem-cli path
#[tauri::command]
pub fn get_golem_cli_path( app_handle: AppHandle) -> Result<String, String> {
    Ok(GolemCommandExecutor::with_app_handle(app_handle).get_golem_cli_path())
}

