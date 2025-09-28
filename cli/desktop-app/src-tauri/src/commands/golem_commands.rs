use crate::services::command_executor::GolemCommandExecutor;
use tauri::AppHandle;

#[tauri::command]
pub async fn call_golem_command(
    app_handle: AppHandle,
    command: String,
    subcommands: Vec<String>,
    folder_path: String,
) -> Result<String, String> {
    // Validate inputs
    if folder_path.is_empty() || command.is_empty() {
        return Err("Folder path and command are required".to_string());
    }

    // Create a new command executor instance with app handle
    let executor = GolemCommandExecutor::with_app_handle(app_handle);

    // Convert Vec<String> to Vec<&str> and add the format flag
    let subcommand_refs: Vec<&str> = subcommands.iter().map(|s| s.as_str()).collect();
    let mut final_subcommands = subcommand_refs;
    final_subcommands.push("--format=json");
    final_subcommands.push("--yes");

    // Execute the command asynchronously
    executor.execute_golem_cli(&folder_path, &command, &final_subcommands).await
}
