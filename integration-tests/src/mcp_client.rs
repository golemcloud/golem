// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! A minimal MCP client over Streamable HTTP, protocol version 2025-03-26. Each request is sent
//! once; a failed request is an error, never retried.

use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The MCP protocol version the client negotiates.
pub const PROTOCOL_VERSION: &str = "2025-03-26";

/// The client of one MCP session at one URL.
pub struct McpClient {
    http: reqwest::Client,
    url: Box<str>,
    host: Arc<str>,
    session_id: Option<Arc<str>>,
    /// The next JSON-RPC request ID of the session.
    next_id: Arc<AtomicU64>,
}

/// The answer to one POSTed JSON-RPC message.
pub struct McpResponse {
    /// The `Mcp-Session-Id` that the response names.
    pub session_id: Option<Arc<str>>,
    /// The first JSON-RPC message of the response; `None` for an accepted notification.
    pub message: Option<Value>,
}

impl McpClient {
    /// Opens a session at `url` for the domain `host`: `initialize`, then
    /// `notifications/initialized`.
    pub async fn connect(http: reqwest::Client, url: &str, host: &str) -> anyhow::Result<Self> {
        let client = Self::initialize(http, url, host).await?;
        client.send_initialized_notification().await?;
        Ok(client)
    }

    /// Sends `initialize` and keeps the session ID that the server issued.
    pub async fn initialize(http: reqwest::Client, url: &str, host: &str) -> anyhow::Result<Self> {
        let client = Self {
            http,
            url: url.into(),
            host: host.into(),
            session_id: None,
            next_id: Arc::new(AtomicU64::new(1)),
        };
        let response = client
            .post(&json!({
                "jsonrpc": "2.0",
                "id": client.next_id(),
                "method": "initialize",
                "params": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "golem-mcp-test-client", "version": "0.0.1"},
                },
            }))
            .await?;
        match response.message {
            Some(message) if message.get("result").is_some() => Ok(Self {
                session_id: response.session_id,
                ..client
            }),
            other => anyhow::bail!("initialize failed: {other:?}"),
        }
    }

    /// Sends the `notifications/initialized` that follows `initialize`.
    pub async fn send_initialized_notification(&self) -> anyhow::Result<()> {
        self.post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await
            .map(|_| ())
    }

    /// A client of the same session that sends its requests to `url`.
    pub fn at(&self, url: &str) -> Self {
        Self {
            http: self.http.clone(),
            url: url.into(),
            host: self.host.clone(),
            session_id: self.session_id.clone(),
            next_id: self.next_id.clone(),
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// POSTs one JSON-RPC message in the session and reads the whole response.
    pub async fn post(&self, message: &Value) -> anyhow::Result<McpResponse> {
        let request = self
            .http
            .post(&*self.url)
            .header("Host", &*self.host)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(message);
        let request = match &self.session_id {
            Some(session_id) => request.header("Mcp-Session-Id", &**session_id),
            None => request,
        };
        let response = request.send().await?;
        let status = response.status();
        let session_id = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .map(Arc::from);
        let body = response.text().await?;
        if !status.is_success() {
            anyhow::bail!("MCP HTTP request failed with status {status}: {body}");
        }
        Ok(McpResponse {
            session_id,
            message: first_message(&body)?,
        })
    }

    /// Sends a JSON-RPC request and returns its result.
    pub async fn request(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let response = self
            .post(&json!({
                "jsonrpc": "2.0",
                "id": self.next_id(),
                "method": method,
                "params": params,
            }))
            .await?;
        let Some(mut message) = response.message else {
            anyhow::bail!("{method} returned no JSON-RPC response");
        };
        match message.get_mut("result") {
            Some(result) => Ok(result.take()),
            None => anyhow::bail!("MCP error: {}", message["error"]),
        }
    }

    pub async fn list_tools(&self) -> anyhow::Result<Vec<Value>> {
        let result = self.request("tools/list", json!({})).await?;
        Ok(result["tools"].as_array().cloned().unwrap_or_default())
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> anyhow::Result<Value> {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
            .await
    }

    pub async fn list_resources(&self) -> anyhow::Result<Vec<Value>> {
        let result = self.request("resources/list", json!({})).await?;
        Ok(result["resources"].as_array().cloned().unwrap_or_default())
    }

    pub async fn list_resource_templates(&self) -> anyhow::Result<Vec<Value>> {
        let result = self.request("resources/templates/list", json!({})).await?;
        Ok(result["resourceTemplates"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    pub async fn read_resource(&self, uri: &str) -> anyhow::Result<Value> {
        self.request("resources/read", json!({ "uri": uri })).await
    }

    pub async fn list_prompts(&self) -> anyhow::Result<Vec<Value>> {
        let result = self.request("prompts/list", json!({})).await?;
        Ok(result["prompts"].as_array().cloned().unwrap_or_default())
    }

    pub async fn get_prompt(&self, name: &str) -> anyhow::Result<Value> {
        self.request("prompts/get", json!({ "name": name })).await
    }
}

/// The JSON-RPC message of the first SSE event that carries one.
fn first_message(body: &str) -> anyhow::Result<Option<Value>> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .find(|data| !data.is_empty())
        .map(|data| serde_json::from_str(data).map_err(Into::into))
        .transpose()
}
