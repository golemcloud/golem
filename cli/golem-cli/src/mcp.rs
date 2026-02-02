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
use crate::log::{set_log_output, take_log_buffer, Output};
use async_trait::async_trait;
use mcp_sdk_rs::error::{Error, ErrorCode};
use mcp_sdk_rs::server::{Server, ServerHandler};
use mcp_sdk_rs::transport::stdio::StdioTransport;
use mcp_sdk_rs::transport::{Message, Transport};
use mcp_sdk_rs::types::{
    ClientCapabilities, Implementation, ListResourcesResult, ListToolsResult, MessageContent,
    Resource, ServerCapabilities, Tool, ToolResult, ToolSchema,
};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use futures_util::StreamExt;

pub struct McpHandler<Hooks: CommandHandlerHooks + 'static> {
    handler: Arc<CommandHandler<Hooks>>,
}

impl<Hooks: CommandHandlerHooks + 'static> McpHandler<Hooks> {
    pub fn new(handler: Arc<CommandHandler<Hooks>>) -> Self {
        Self { handler }
    }

    pub async fn run(self, _port: Option<u16>) -> anyhow::Result<()> {
        set_log_output(Output::None);

        let (stdin_tx, stdin_rx) = tokio::sync::mpsc::channel::<String>(100);
        let (stdout_tx, mut stdout_rx) = tokio::sync::mpsc::channel::<String>(100);

        tokio::spawn(async move {
            let stdin = tokio::io::stdin();
            let mut reader = BufReader::new(stdin).lines();
            while let Ok(std::option::Option::Some(line)) = reader.next_line().await {
                let _ = stdin_tx.send(line).await;
            }
        });

        tokio::spawn(async move {
            let mut stdout = tokio::io::stdout();
            while let std::option::Option::Some(msg) = stdout_rx.recv().await {
                let _ = stdout.write_all(msg.as_bytes()).await;
                let _ = stdout.write_all(b"\n").await;
                let _ = stdout.flush().await;
            }
        });

        struct LenientTransport {
            inner: Arc<StdioTransport>,
        }

        #[async_trait]
        impl Transport for LenientTransport {
            async fn send(&self, message: Message) -> Result<(), Error> {
                self.inner.send(message).await
            }

            fn receive(&self) -> std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Message, Error>> + Send>> {
                let inner = self.inner.clone();
                Box::pin(inner.receive().map(|res| {
                    match res {
                        Ok(mut msg) => {
                            if let Message::Request(ref mut req) = msg {
                                if req.method == "initialize" {
                                    if let std::option::Option::Some(ref mut params) = req.params {
                                        if let serde_json::Value::Object(ref mut p) = params {
                                            let client_info = p.get("clientInfo").cloned();
                                            let implementation = p.get("implementation").cloned();
                                            if let std::option::Option::Some(ci) = client_info {
                                                if !p.contains_key("implementation") {
                                                    p.insert("implementation".to_string(), ci);
                                                }
                                            } else if let std::option::Option::Some(imp) = implementation {
                                                if !p.contains_key("clientInfo") {
                                                    p.insert("clientInfo".to_string(), imp);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Ok(msg)
                        }
                        Err(e) => Err(e),
                    }
                }))
            }

            async fn close(&self) -> Result<(), Error> {
                self.inner.close().await
            }
        }

        let transport = Arc::new(LenientTransport {
            inner: Arc::new(StdioTransport::new(stdin_rx, stdout_tx)),
        });
        let handler = Arc::new(self);
        let server = Server::new(transport, handler);

        server.start().await.map_err(|e| anyhow::anyhow!(e.to_string()))?;

        Ok(())
    }

    async fn invoke_cli(&self, args: Vec<String>) -> Result<String, Error> {
        let handler = self.handler.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let res = rt.block_on(async {
                let mut full_args = vec!["golem".to_string()];
                full_args.extend(args);

                match GolemCliCommand::try_parse_from_lenient(full_args, true) {
                    GolemCliCommandParseResult::FullMatch(command) => {
                        let _ = take_log_buffer();
                        set_log_output(Output::BufferedUntilErr);
                        let result = handler.handle_command(command).await;
                        let log_lines = take_log_buffer();
                        let output = log_lines.join("\n");
                        set_log_output(Output::None);

                        match result {
                            Ok(_) => Ok(output),
                            Err(e) => Err(Error::protocol(ErrorCode::InternalError, format!("CLI Error: {}", e))),
                        }
                    }
                    _ => Err(Error::protocol(ErrorCode::InvalidParams, "Invalid command arguments")),
                }
            });
            let _ = tx.send(res);
        });

        rx.await.map_err(|_| Error::protocol(ErrorCode::InternalError, "Internal thread finished unexpectedly"))?
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
            experimental: None,
            logging: None,
            prompts: None,
            resources: Some(serde_json::json!({})),
            tools: Some(serde_json::json!({})),
        })
    }

    async fn shutdown(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn handle_method(
        &self,
        method: &str,
        params: std::option::Option<serde_json::Value>,
    ) -> Result<serde_json::Value, Error> {
        match method {
            "tools/list" => {
                let result = ListToolsResult {
                    tools: vec![
                        Tool {
                            name: "run_command".to_string(),
                            description: "Run a Golem CLI command with full arguments".to_string(),
                            input_schema: std::option::Option::Some(ToolSchema {
                                properties: std::option::Option::Some(serde_json::json!({
                                    "args": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "The command line arguments for golem cli"
                                    }
                                })),
                                required: std::option::Option::Some(vec!["args".to_string()]),
                            }),
                            annotations: None,
                        },
                        Tool {
                            name: "get_info".to_string(),
                            description: "Get environmental and cluster details".to_string(),
                            input_schema: std::option::Option::Some(ToolSchema {
                                properties: std::option::Option::Some(serde_json::json!({})),
                                required: None,
                            }),
                            annotations: None,
                        },
                    ],
                    next_cursor: None,
                };
                Ok(serde_json::to_value(result).unwrap())
            }
            "tools/call" => {
                let params_obj = params.ok_or_else(|| Error::protocol(ErrorCode::InvalidParams, "Missing parameters"))?;
                let name = params_obj.get("name").and_then(|v| v.as_str()).ok_or_else(|| Error::protocol(ErrorCode::InvalidParams, "Missing name"))?;
                let arguments = params_obj.get("arguments").cloned().unwrap_or_else(|| serde_json::json!({}));

                let output = match name {
                    "run_command" => {
                        let args = arguments.get("args")
                            .and_then(|v| v.as_array())
                            .ok_or_else(|| Error::protocol(ErrorCode::InvalidParams, "Missing args array"))?;
                        let arg_strings: Vec<String> = args.iter()
                            .map(|v| v.as_str().unwrap_or_default().to_string())
                            .collect();
                        self.invoke_cli(arg_strings).await?
                    },
                    "get_info" => {
                        self.invoke_cli(vec!["--version".to_string()]).await?
                    },
                    _ => return Err(Error::protocol(ErrorCode::MethodNotFound, format!("Unknown tool: {}", name))),
                };

                let result = ToolResult {
                    content: vec![MessageContent::Text {
                        text: output,
                    }],
                    structured_content: None,
                };
                Ok(serde_json::to_value(result).unwrap())
            }
            "resources/list" => {
                let mut resources = Vec::new();
                if std::path::Path::new("golem.yaml").exists() {
                    resources.push(Resource {
                        uri: "file://./golem.yaml".to_string(),
                        name: "Golem Project Manifest".to_string(),
                        description: Some("The manifest file for the current Golem project".to_string()),
                        mime_type: Some("application/x-yaml".to_string()),
                        size: None,
                    });
                }
                let result = ListResourcesResult {
                    resources,
                    next_cursor: None,
                };
                Ok(serde_json::to_value(result).unwrap())
            }
            _ => Err(Error::protocol(ErrorCode::MethodNotFound, format!("Method not found: {}", method))),
        }
    }
}
