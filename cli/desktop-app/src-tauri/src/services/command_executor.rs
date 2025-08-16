use std::path::Path;
use std::process::Command;
use tauri::{AppHandle};
use tauri_plugin_store::StoreExt;
use crate::{GOLEM_CLI_PATH, SETTINGS_FILE};

/// A service that executes external CLI commands
#[derive(Debug)]
pub struct GolemCommandExecutor {
    app_handle: Option<AppHandle>,
}

impl Default for GolemCommandExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl GolemCommandExecutor {
    /// Creates a new GolemCommandExecutor instance
    pub fn new() -> Self {
        GolemCommandExecutor { app_handle: None }
    }
    
    /// Creates a new GolemCommandExecutor instance with an AppHandle
    pub fn with_app_handle(app_handle: AppHandle) -> Self {
        GolemCommandExecutor { app_handle: Some(app_handle) }
    }
    
    /// Get the golem-cli path from the app state or store, or use "golem-cli" from PATH as fallback
    pub fn get_golem_cli_path(&self) -> String {
        if let Some(app_handle) = &self.app_handle {
            if let Ok(store) = app_handle.store(SETTINGS_FILE) {
                if let Some(path_value) = store.get(GOLEM_CLI_PATH) {
                    if let Some(path_str) = path_value.as_str() {
                        return path_str.to_string();
                    }
                }
            }
        }

        // Fallback to default if app_handle is None or store lookup fails
        "golem-cli".to_string()
    }
    
    /// Executes the golem-cli command with the given arguments
    pub fn execute_golem_cli(
        &self,
        working_dir: &str,
        subcommand: &str,
        args: &[&str],
    ) -> Result<String, String> {
        // Validate the working directory exists
        if !Path::new(working_dir).exists() {
            return Err(format!("Working directory does not exist: {}", working_dir));
        }

        // Find the golem-cli executable (use store setting or fallback to PATH)
        let golem_cli_path = self.get_golem_cli_path();
        
        // Build the command with the working directory and arguments
        let mut command = Command::new(&golem_cli_path);
        command.current_dir(working_dir);
        command.arg(subcommand);
        
        for arg in args {
            command.arg(arg);
        }
        
        println!("Executing: {} {:?} in {}", golem_cli_path, command, working_dir);
        
        // Execute the command and handle the result
        match command.output() {
            Ok(output) => {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    Ok(stdout)
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    Err(format!("Command execution failed: {}", stderr))
                }
            }
            Err(e) => Err(format!("Failed to execute command: {}", e)),
        }
    }
    
    /// Creates a new Golem application
    pub fn create_application(
        &self, 
        folder_path: &str,
        app_name: &str,
        language: &str,
    ) -> Result<String, String> {

        let result = self.execute_golem_cli(folder_path, "app", &["new", app_name, language])?;
        
        Ok(format!("Successfully created application: {}\n{}", app_name, result))
    }
}
