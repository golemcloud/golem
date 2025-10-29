// CLI Command Executor
// Executes golem-cli commands as subprocesses for MCP tool invocation

use std::process::Stdio;
use tokio::process::Command;
use tokio::io::{AsyncBufReadExt, BufReader};
use serde_json::Value;

/// Execute a golem-cli command and return the output
pub async fn execute_cli_command(
    tool_name: &str,
    arguments: &Option<serde_json::Map<String, Value>>,
) -> anyhow::Result<String> {
    // Convert Map to Value for processing
    let args_value = arguments.as_ref().map(|m| Value::Object(m.clone()));

    // Build command args from tool name and arguments
    let args = build_command_args(tool_name, &args_value)?;

    // Get the current executable path
    let cli_path = std::env::current_exe()?;

    // Execute command
    let output = Command::new(cli_path)
        .args(&args)
        .arg("--format")
        .arg("json")  // Always request JSON output
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        anyhow::bail!("Command failed: {}", stderr)
    }
}

/// Execute a golem-cli command with streaming output for long-running operations
/// Returns a stream of output lines as they are produced
pub async fn execute_cli_command_streaming(
    tool_name: &str,
    arguments: &Option<serde_json::Map<String, Value>>,
    mut progress_callback: impl FnMut(String) -> (),
) -> anyhow::Result<String> {
    // Convert Map to Value for processing
    let args_value = arguments.as_ref().map(|m| Value::Object(m.clone()));

    // Build command args from tool name and arguments
    let args = build_command_args(tool_name, &args_value)?;

    // Get the current executable path
    let cli_path = std::env::current_exe()?;

    // Spawn command with piped outputs
    let mut child = Command::new(cli_path)
        .args(&args)
        .arg("--format")
        .arg("json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Capture stdout for streaming
    let stdout = child.stdout.take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture stdout"))?;

    let stderr = child.stderr.take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture stderr"))?;

    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();

    let mut output_lines = Vec::new();
    let mut error_lines = Vec::new();

    // Read stdout line by line and send progress updates
    loop {
        tokio::select! {
            line = stdout_reader.next_line() => {
                match line? {
                    Some(line) => {
                        // Send progress update
                        progress_callback(format!("[OUT] {}", line));
                        output_lines.push(line);
                    }
                    None => break,
                }
            }
            line = stderr_reader.next_line() => {
                match line? {
                    Some(line) => {
                        // Send progress update for stderr
                        progress_callback(format!("[ERR] {}", line));
                        error_lines.push(line);
                    }
                    None => {}
                }
            }
        }
    }

    // Wait for command to complete
    let status = child.wait().await?;

    if status.success() {
        Ok(output_lines.join("\n"))
    } else {
        anyhow::bail!("Command failed: {}", error_lines.join("\n"))
    }
}

/// Build CLI command arguments from MCP tool name and parameters
fn build_command_args(tool_name: &str, arguments: &Option<Value>) -> anyhow::Result<Vec<String>> {
    let mut args = Vec::new();

    // Parse tool name into CLI subcommands
    // Format: "component_list" -> ["component", "list"]
    //         "worker_invoke" -> ["worker", "invoke"]
    //         "component" -> ["component", "--help"] (single-word tools show help)
    let parts: Vec<&str> = tool_name.split('_').collect();

    if parts.len() == 1 {
        // Single-word tool - treat as help command
        args.push(parts[0].to_string());
        args.push("--help".to_string());
    } else if parts.len() >= 2 {
        // Multi-part tool - split into subcommands
        args.extend(parts.iter().map(|s| s.to_string()));
    } else {
        anyhow::bail!("Invalid tool name: {}", tool_name);
    }

    // Add arguments from MCP request
    if let Some(Value::Object(obj)) = arguments {
        for (key, value) in obj {
            // Convert parameter name from camelCase/snake_case to kebab-case
            let cli_arg = key.replace('_', "-");
            args.push(format!("--{}", cli_arg));

            // Add parameter value
            match value {
                Value::String(s) => args.push(s.clone()),
                Value::Number(n) => args.push(n.to_string()),
                Value::Bool(b) => {
                    if *b {
                        // For boolean flags, just including the flag is enough
                        args.pop(); // Remove the --flag we just added
                        args.push(format!("--{}", cli_arg));
                    }
                }
                Value::Array(arr) => {
                    // For arrays, add multiple instances of the flag
                    for item in arr {
                        args.push(format!("--{}", cli_arg));
                        args.push(item.to_string().trim_matches('"').to_string());
                    }
                    args.pop(); // Remove the last --flag
                }
                _ => {}
            }
        }
    }

    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_build_command_args_simple() {
        let args = build_command_args("component_list", &None).unwrap();
        assert_eq!(args, vec!["component", "list"]);
    }

    #[test]
    fn test_build_command_args_with_string_param() {
        let params = Some(json!({"project": "my-project"}));
        let args = build_command_args("component_list", &params).unwrap();
        assert!(args.contains(&"component".to_string()));
        assert!(args.contains(&"list".to_string()));
        assert!(args.contains(&"--project".to_string()));
        assert!(args.contains(&"my-project".to_string()));
    }

    #[test]
    fn test_build_command_args_with_multiple_params() {
        let params = Some(json!({
            "worker_name": "my-worker",
            "component_name": "my-component"
        }));
        let args = build_command_args("worker_invoke", &params).unwrap();
        assert!(args.contains(&"worker".to_string()));
        assert!(args.contains(&"invoke".to_string()));
        assert!(args.contains(&"--worker-name".to_string()));
        assert!(args.contains(&"my-worker".to_string()));
        assert!(args.contains(&"--component-name".to_string()));
        assert!(args.contains(&"my-component".to_string()));
    }

    #[test]
    fn test_invalid_tool_name() {
        let result = build_command_args("invalid", &None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid tool name format"));
    }
}
