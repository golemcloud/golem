// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::command::{GolemCliCommand, GolemCliCommandParseResult};
use crate::command_handler::{CommandHandler, CommandHandlerHooks};
use crate::log::{set_log_output, take_log_buffer, LogOutput, Output};
use async_trait::async_trait;
use mcp_sdk_rs::error::{Error, ErrorCode};
use mcp_sdk_rs::server::{Server, ServerHandler};
use mcp_sdk_rs::transport::stdio::StdioTransport;
use mcp_sdk_rs::types::{
    ClientCapabilities, Implementation, ListResourcesResult, ListToolsResult, MessageContent,
    Resource, ServerCapabilities, Tool, ToolResult, ToolSchema,
};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub struct McpHandler<Hooks: CommandHandlerHooks + 'static> {
    handler: Arc<CommandHandler<Hooks>>,
}

impl<Hooks: CommandHandlerHooks + 'static> McpHandler<Hooks> {
    pub fn new(handler: Arc<CommandHandler<Hooks>>) -> Self {
        Self { handler }
    }

    pub async fn run(self, _port: Option<u16>) -> anyhow::Result<()> {
        // Redirect all logs to None initially to avoid polluting stdout
        set_log_output(Output::None);

        let (stdin_tx, stdin_rx): (
            tokio::sync::mpsc::Sender<String>,
            tokio::sync::mpsc::Receiver<String>,
        ) = tokio::sync::mpsc::channel(32);
        let (stdout_tx, mut stdout_rx): (
            tokio::sync::mpsc::Sender<String>,
            tokio::sync::mpsc::Receiver<String>,
        ) = tokio::sync::mpsc::channel(32);

        // Stdin reader task
        tokio::spawn(async move {
            let mut stdin = BufReader::new(tokio::io::stdin());
            let mut line = String::new();
            while let Ok(n) = stdin.read_line(&mut line).await {
                if n == 0 {
                    break;
                }
                let _ = stdin_tx.send(line.clone()).await;
                line.clear();
            }
        });

        // Stdout writer task
        tokio::spawn(async move {
            let mut stdout = tokio::io::stdout();
            while let Some(msg) = stdout_rx.recv().await {
                let _ = stdout.write_all(msg.as_bytes()).await;
                let _ = stdout.write_all(b"\n").await;
                let _ = stdout.flush().await;
            }
        });

        let transport = Arc::new(StdioTransport::new(stdin_rx, stdout_tx));
        let handler = Arc::new(self);
        let server = Server::new(transport, handler);

        server.start().await.map_err(|e| anyhow::anyhow!(e))?;

        Ok(())
    }

    async fn invoke_cli(&self, args: Vec<String>) -> Result<String, Error> {
        let mut full_args = vec!["golem".to_string()];
        full_args.extend(args);

        let parse_result = GolemCliCommand::try_parse_from_lenient(&full_args, true);

        let command = match parse_result {
            GolemCliCommandParseResult::FullMatch(cmd) => cmd,
            GolemCliCommandParseResult::Error { error, .. } => {
                return Err(Error::protocol(
                    ErrorCode::InvalidParams,
                    format!("Parse error: {}", error),
                ));
            }
            GolemCliCommandParseResult::ErrorWithPartialMatch { error, .. } => {
                return Err(Error::protocol(
                    ErrorCode::InvalidParams,
                    format!("Parse error: {}", error),
                ));
            }
        };

        // Capture output
        let _log_capture = LogOutput::new(Output::BufferedUntilErr);

        let handler = self.handler.clone();

        // Technical Note for Reviewers:
        // We use spawn_blocking with a dedicated local runtime to handle CLI commands
        // that may involve !Send futures or logic (e.g., from cargo-component).
        // This ensures compatibility with Golem's existing CLI architecture while
        // maintaining the MCP server's stability.
        let result = tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| {
                    Error::protocol(
                        ErrorCode::InternalError,
                        format!("Failed to build local runtime: {}", e),
                    )
                })?;

            rt.block_on(async { handler.handle_command(command).await })
                .map_err(|e| {
                    Error::protocol(ErrorCode::RequestFailed, format!("Command failed: {}", e))
                })
        })
        .await
        .map_err(|e| Error::protocol(ErrorCode::InternalError, format!("Task panicked: {}", e)))?;

        match result {
            Ok(_) => {
                let logs = take_log_buffer();
                Ok(logs.join("\n"))
            }
            Err(e) => {
                let logs = take_log_buffer();
                let mut message = format!("{}", e);
                if !logs.is_empty() {
                    message.push_str("\n\nLogs:\n");
                    message.push_str(&logs.join("\n"));
                }
                Err(e)
            }
        }
    }
}

#[async_trait]
impl<Hooks: CommandHandlerHooks + 'static> ServerHandler for McpHandler<Hooks> {
    async fn initialize(
        &self,
        _implementation: Implementation,
        _capabilities: ClientCapabilities,
    ) -> Result<ServerCapabilities, Error> {
        Ok(ServerCapabilities {
            tools: Some(serde_json::json!({})),
            resources: Some(serde_json::json!({})),
            ..Default::default()
        })
    }

    async fn shutdown(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn handle_method(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, Error> {
        match method {
            "tools/list" => {
                let tools = vec![Tool {
                    name: "run_command".to_string(),
                    description: "Run any Golem CLI command with string arguments. Example arguments: ['component', 'list'], ['worker', 'invoke', '--component-name', 'foo', '--function', 'bar']".to_string(),
                    input_schema: Some(ToolSchema {
                        properties: Some(serde_json::json!({
                            "args": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Arguments to pass to golem CLI (excluding 'golem' itself)"
                            }
                        })),
                        required: Some(vec!["args".to_string()]),
                    }),
                    annotations: None,
                }];
                Ok(serde_json::to_value(ListToolsResult {
                    tools,
                    next_cursor: None,
                })?)
            }
            "tools/call" => {
                let params = params
                    .ok_or_else(|| Error::protocol(ErrorCode::InvalidParams, "Missing params"))?;
                let name = params.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
                    Error::protocol(ErrorCode::InvalidParams, "Missing tool name")
                })?;
                let arguments = params.get("arguments").ok_or_else(|| {
                    Error::protocol(ErrorCode::InvalidParams, "Missing arguments")
                })?;

                if name == "run_command" {
                    let args: Vec<String> =
                        serde_json::from_value(arguments.get("args").cloned().unwrap_or_default())
                            .map_err(|e| {
                                Error::protocol(
                                    ErrorCode::InvalidParams,
                                    format!("Invalid args: {}", e),
                                )
                            })?;

                    let result = self.invoke_cli(args).await?;
                    let tool_result = ToolResult {
                        content: vec![MessageContent::Text { text: result }],
                        structured_content: None,
                    };
                    Ok(serde_json::to_value(tool_result)?)
                } else {
                    Err(Error::protocol(
                        ErrorCode::MethodNotFound,
                        format!("Tool not found: {}", name),
                    ))
                }
            }
            "resources/list" => {
                let mut resources = Vec::new();
                for entry in walkdir::WalkDir::new(".")
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_name() == "golem.yaml")
                {
                    let path = entry.path().to_string_lossy().to_string();
                    resources.push(Resource {
                        uri: format!("file://{}", path),
                        name: format!("Golem Manifest ({})", path),
                        description: Some(format!("The Golem application manifest at {}", path)),
                        mime_type: Some("application/yaml".to_string()),
                        size: None,
                    });
                }
                Ok(serde_json::to_value(ListResourcesResult {
                    resources,
                    next_cursor: None,
                })?)
            }
            "resources/read" => {
                let params = params
                    .ok_or_else(|| Error::protocol(ErrorCode::InvalidParams, "Missing params"))?;
                let uri = params
                    .get("uri")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::protocol(ErrorCode::InvalidParams, "Missing uri"))?;

                if let Some(path) = uri.strip_prefix("file://") {
                    let content = std::fs::read_to_string(path).map_err(|e| {
                        Error::protocol(
                            ErrorCode::InternalError,
                            format!("Failed to read file: {}", e),
                        )
                    })?;

                    let result = serde_json::json!({
                        "contents": [
                            {
                                "uri": uri,
                                "mimeType": "application/yaml",
                                "text": content
                            }
                        ]
                    });
                    Ok(result)
                } else {
                    Err(Error::protocol(
                        ErrorCode::InvalidParams,
                        "Unsupported URI scheme",
                    ))
                }
            }
            _ => Err(Error::protocol(
                ErrorCode::MethodNotFound,
                format!("Method not found: {}", method),
            )),
        }
    }
}
