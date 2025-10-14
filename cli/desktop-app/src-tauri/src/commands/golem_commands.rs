use crate::services::command_executor::GolemCommandExecutor;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

#[derive(Debug, Serialize, Deserialize)]
pub struct Template {
    pub language: String,
    pub template: String,
    pub description: String,
}

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
    executor
        .execute_golem_cli(&folder_path, &command, &final_subcommands)
        .await
}

#[tauri::command]
pub async fn get_component_templates(app_handle: AppHandle) -> Result<String, String> {
    // Create a new command executor instance with app handle
    let executor = GolemCommandExecutor::with_app_handle(app_handle);

    // Use a valid directory - get the current working directory or use home
    let working_dir = std::env::current_dir()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_else(|| ".".to_string());

    // Execute the templates command without format=json flag
    let subcommands = vec!["templates", "--yes"];
    let result = executor
        .execute_golem_cli(&working_dir, "component", &subcommands)
        .await?;

    // Parse the text output
    let templates = parse_templates_output(&result)?;

    // Convert to JSON
    serde_json::to_string(&templates).map_err(|e| format!("Failed to serialize templates: {}", e))
}

fn parse_templates_output(output: &str) -> Result<Vec<Template>, String> {
    let mut templates = Vec::new();
    let mut current_language: Option<String> = None;
    let mut current_template: Option<String> = None;
    let mut current_description = String::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip empty lines and header
        if trimmed.is_empty() || trimmed.contains("Available languages") {
            continue;
        }

        // Language line (starts with "- " and no colon)
        if line.starts_with("- ") && !line.contains(':') {
            current_language = Some(line[2..].trim().to_string());
            continue;
        }

        // Template line (starts with "  - ")
        if let Some(template_line) = line.strip_prefix("  - ") {
            // Save previous template if exists
            if let (Some(lang), Some(template)) = (&current_language, &current_template) {
                templates.push(Template {
                    language: lang.clone(),
                    template: template.clone(),
                    description: current_description.trim().to_string(),
                });
            }

            // Parse new template
            if let Some(colon_pos) = template_line.find(':') {
                current_template = Some(template_line[..colon_pos].trim().to_string());
                current_description = template_line[colon_pos + 1..].trim().to_string();
            }
        } else if line.starts_with("     ") {
            // Continuation of description (5 spaces)
            if !current_description.is_empty() {
                current_description.push(' ');
            }
            current_description.push_str(trimmed);
        }
    }

    // Add the last template
    if let (Some(lang), Some(template)) = (current_language, current_template) {
        templates.push(Template {
            language: lang,
            template,
            description: current_description.trim().to_string(),
        });
    }

    Ok(templates)
}
