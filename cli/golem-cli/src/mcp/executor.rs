use crate::mcp::context::McpContext;
use anyhow::{anyhow, bail, Result};
use serde_json::{Map, Value};
use std::process::Stdio;
use std::{path::PathBuf, sync::Arc};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct CliExecutor {
    context: Arc<McpContext>,
}

impl CliExecutor {
    pub fn new(context: Arc<McpContext>) -> Self {
        Self { context }
    }

    fn working_dir(&self) -> &PathBuf {
        &self.context.working_dir
    }

    pub async fn execute_cli_command_streaming(
        &self,
        tool_name: &str,
        input_args: &Option<Map<String, Value>>,
    ) -> Result<String> {
        let cli_args = self.build_full_cli_args(tool_name, input_args)?;
        let cli_path = std::env::current_exe()?;

        let mut child = Command::new(cli_path)
            .args(&cli_args)
            .arg("--format")
            .arg("json")
            .current_dir(self.working_dir())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to capture stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("Failed to capture stderr"))?;

        let mut out_reader = BufReader::new(stdout).lines();
        let mut err_reader = BufReader::new(stderr).lines();

        let mut out_lines = Vec::new();
        let mut err_lines = Vec::new();

        loop {
            tokio::select! {
                line = out_reader.next_line() => {
                    match line? {
                        Some(l) => out_lines.push(l),
                        None => break,
                    }
                }
                line = err_reader.next_line() => {
                    if let Some(l) = line? {
                        err_lines.push(l);
                    }
                }
            }
        }

        let status = child.wait().await?;

        if status.success() {
            Ok(out_lines.join("\n"))
        } else {
            let cli_err = err_lines.join("\n");
            bail!("command execution failed {}", cli_err);
        }
    }

    fn build_full_cli_args(
        &self,
        tool_name: &str,
        input_args: &Option<Map<String, Value>>,
    ) -> Result<Vec<String>> {
        let mut args = Vec::new();

        // Split tool name: "agent-list" -> ["agent", "list"]
        for part in tool_name.split('-') {
            args.push(part.to_string());
        }

        if let Some(map) = input_args {
            for (key, value) in map {
                // Handle positional arguments
                if key == "positional_args" {
                    if let Value::Array(arr) = value {
                        for v in arr {
                            if let Some(s) = v.as_str() {
                                args.push(s.to_string());
                            }
                        }
                    }
                    continue;
                }

                // Everything else is a flag
                let flag = format!("--{}", key.replace('_', "-"));

                if value.is_null() {
                    args.push(flag);
                    continue;
                }

                match value {
                    Value::Bool(true) => args.push(flag),
                    Value::Bool(false) => {}
                    Value::String(s) => {
                        args.push(flag);
                        args.push(s.to_string());
                    }
                    Value::Number(n) => {
                        args.push(flag);
                        args.push(n.to_string());
                    }
                    Value::Array(arr) => {
                        for v in arr {
                            if let Some(s) = v.as_str() {
                                args.push(flag.clone());
                                args.push(s.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_build_full_args() {
        let ctx = Arc::new(McpContext {
            working_dir: PathBuf::from("/tmp/work"),
        });
        let executor = CliExecutor::new(ctx);

        let args = executor
            .build_full_cli_args("component-build", &None)
            .unwrap();

        assert_eq!(args, vec!["component", "build"]);
    }

    #[test]
    fn test_build_args_with_flags() {
        let ctx = Arc::new(McpContext {
            working_dir: PathBuf::from("/tmp/work"),
        });
        let executor = CliExecutor::new(ctx);

        let mut map = Map::new();
        map.insert("force_build".to_string(), json!(true));
        map.insert("max_count".to_string(), json!(5));

        let args = executor
            .build_full_cli_args("component-list", &Some(map))
            .unwrap();

        assert_eq!(args[0], "component");
        assert_eq!(args[1], "list");
        assert!(args.contains(&"--force-build".to_string()));
        assert!(args.contains(&"--max-count".to_string()));
        assert!(args.contains(&"5".to_string()));
    }

    #[test]
    fn test_build_args_positional() {
        let ctx = Arc::new(McpContext {
            working_dir: PathBuf::from("/tmp/work"),
        });
        let executor = CliExecutor::new(ctx);

        let mut map = Map::new();
        map.insert(
            "positional_args".to_string(),
            json!(["cart(user-123)", "calculate", "10", "20"]),
        );

        let args = executor
            .build_full_cli_args("agent-invoke", &Some(map))
            .unwrap();

        assert_eq!(
            args,
            vec!["agent", "invoke", "cart(user-123)", "calculate", "10", "20"]
        );
    }

    #[test]
    fn test_flags_and_positionals_together() {
        let ctx = Arc::new(McpContext {
            working_dir: PathBuf::from("/tmp/work"),
        });
        let executor = CliExecutor::new(ctx);
        let mut map = Map::new();
        map.insert("set-active".to_string(), json!("golem-cloud"));
        map.insert("default-format".to_string(), json!("json"));
        map.insert("positional_args".to_string(), json!(["my-profile-name"]));

        let args = executor
            .build_full_cli_args("profile-add", &Some(map))
            .unwrap();

        assert_eq!(
            args,
            vec![
                "profile",
                "add",
                "--set-active",
                "golem-cloud",
                "--default-format",
                "json",
                "my-profile-name",
            ]
        );
    }
}
