// Copyright 2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

//! Scripted OpenAI-compatible chat completions for in-process tests.
//! See the crate README for Rust and JSON scripts and Golem configuration.

use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::task::AbortHandle;

/// One ordered conversation. JSON uses exactly these types, without another schema.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub steps: Vec<Step>,
}

impl Script {
    /// Parse a JSON script. Credentials in scripts must be test-only values.
    pub fn from_json(json: &str) -> anyhow::Result<Self> {
        // Do not include the source: it may contain an authorization expectation.
        serde_json::from_str(json).map_err(|error| {
            anyhow::anyhow!(
                "invalid model script at line {}, column {} ({:?})",
                error.line(),
                error.column(),
                error.classify()
            )
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(default)]
    pub expect: Expectations,
    pub response: ChatResponse,
}

/// Omitted expectations are not checked. Tool names are compared without order,
/// but duplicates count. Authorization mismatches never print either value.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expectations {
    pub tool_names: Option<Vec<String>>,
    pub last_message_role: Option<String>,
    pub last_tool_call_id: Option<String>,
    pub model: Option<String>,
    pub authorization: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatResponse {
    Text {
        text: String,
    },
    ToolCalls {
        calls: Vec<ToolCall>,
    },
    /// Each delta is a separate SSE event, after an assistant-role event.
    /// A finish event and [DONE] are appended automatically.
    Stream {
        deltas: Vec<Delta>,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// The wire protocol requires a JSON string, not a JSON object.
    pub arguments: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallDelta>,
}

/// Use the same index in later deltas to continue a tool call. Set id and name
/// on its first delta; arguments may be split anywhere, including inside keys.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCallDelta {
    pub index: u32,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: String,
}

struct Progress {
    steps: VecDeque<Step>,
    requested: usize,
    failures: Vec<String>,
}

type SharedProgress = Arc<Mutex<Progress>>;

/// Owns one loopback listener and its script. Drop aborts the server and asserts
/// that every step matched. During panic unwinding it only aborts the server.
pub struct ModelMock {
    base_url: String,
    progress: SharedProgress,
    abort: AbortHandle,
}

impl ModelMock {
    pub async fn start(script: Script) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let progress = Arc::new(Mutex::new(Progress {
            steps: script.steps.into(),
            requested: 0,
            failures: Vec::new(),
        }));
        // The fallback also records wrong paths and methods as unexpected calls.
        let router = Router::new()
            .fallback(complete)
            .with_state(progress.clone());
        let server_progress = progress.clone();
        let task = tokio::spawn(async move {
            if axum::serve(listener, router).await.is_err() {
                server_progress
                    .lock()
                    .unwrap()
                    .failures
                    .push("model mock server failed".into());
            }
        });
        Ok(Self {
            base_url,
            progress,
            abort: task.abort_handle(),
        })
    }

    /// The origin without a trailing slash or /v1. The endpoint is
    /// POST {base_url}/v1/chat/completions.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Check after all client calls have completed. Failures remain recorded;
    /// checking them does not disable the drop assertion.
    pub fn verify(&self) -> anyhow::Result<()> {
        let progress = self.progress.lock().unwrap();
        let mut failures = progress.failures.clone();
        if !progress.steps.is_empty() {
            failures.push(format!(
                "step {}: {} unrequested step(s)",
                progress.requested + 1,
                progress.steps.len()
            ));
        }
        anyhow::ensure!(failures.is_empty(), "model mock: {}", failures.join("; "));
        Ok(())
    }
}

impl Drop for ModelMock {
    fn drop(&mut self) {
        self.abort.abort();
        if !std::thread::panicking() {
            self.verify().unwrap();
        }
    }
}

async fn complete(State(progress): State<SharedProgress>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let request = if parts.method != Method::POST || parts.uri.path() != "/v1/chat/completions" {
        Err("unexpected method or path".to_string())
    } else {
        match to_bytes(body, 2 * 1024 * 1024).await {
            Ok(bytes) => {
                serde_json::from_slice::<Value>(&bytes).map_err(|_| "invalid JSON request".into())
            }
            Err(_) => Err("could not read request body (limit 2 MiB)".into()),
        }
    };
    let mut progress = progress.lock().unwrap();
    progress.requested += 1;
    let index = progress.requested;
    let result: Result<Response, String> = (|| {
        let request = request?;
        let step = progress.steps.pop_front().ok_or("script exhausted")?;
        check_request(&step.expect, &parts.headers, &request)?;
        let streaming = request
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if streaming != matches!(step.response, ChatResponse::Stream { .. }) {
            return Err("stream: request and scripted response differ".into());
        }
        let model = request
            .get("model")
            .and_then(Value::as_str)
            .ok_or("model: expected a string")?;
        Ok(respond(step.response, model, index))
    })();
    match result {
        Ok(response) => response,
        Err(error) => {
            let message = format!("step {index}: {error}");
            progress.failures.push(message.clone());
            (StatusCode::BAD_REQUEST, Json(json!({"error": {"message": message, "type": "invalid_request_error", "code": "script_mismatch"}}))).into_response()
        }
    }
}

fn check_request(
    expect: &Expectations,
    headers: &axum::http::HeaderMap,
    request: &Value,
) -> Result<(), String> {
    if let Some(expected) = &expect.authorization {
        let values = headers.get_all("authorization").iter().collect::<Vec<_>>();
        if values.len() != 1 || values[0].as_bytes() != expected.as_bytes() {
            return Err("authorization: value differs (redacted)".into());
        }
    }
    let last = request
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.last());
    for (field, expected, actual) in [
        ("model", &expect.model, request.get("model")),
        (
            "last_message_role",
            &expect.last_message_role,
            last.and_then(|m| m.get("role")),
        ),
        (
            "last_tool_call_id",
            &expect.last_tool_call_id,
            last.and_then(|m| m.get("tool_call_id")),
        ),
    ] {
        if let Some(expected) = expected
            && actual.and_then(Value::as_str) != Some(expected)
        {
            return Err(format!("{field}: value differs or is missing"));
        }
    }
    if let Some(expected) = &expect.tool_names {
        let mut actual = match request.get("tools") {
            None => Vec::new(),
            Some(Value::Array(tools)) => tools
                .iter()
                .map(|tool| {
                    if tool.get("type").and_then(Value::as_str) != Some("function") {
                        return Err("tool_names: expected function tools".to_string());
                    }
                    tool.pointer("/function/name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "tool_names: missing function name".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => return Err("tool_names: expected an array".into()),
        };
        let mut expected = expected.iter().map(String::as_str).collect::<Vec<_>>();
        actual.sort_unstable();
        expected.sort_unstable();
        if actual != expected {
            return Err("tool_names: offered names differ".into());
        }
    }
    Ok(())
}

fn respond(response: ChatResponse, model: &str, index: usize) -> Response {
    let id = format!("chatcmpl-mock-{index}");
    match response {
        ChatResponse::Stream { deltas } => {
            let finish = if deltas.iter().any(|delta| !delta.tool_calls.is_empty()) {
                "tool_calls"
            } else {
                "stop"
            };
            let chunk = |delta: Value, finish: Option<&str>| {
                json!({
                    "id": id, "object": "chat.completion.chunk", "created": 0, "model": model,
                    "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]
                })
            };
            let mut events = vec![chunk(json!({"role": "assistant", "content": ""}), None)];
            for delta in deltas {
                let mut wire = json!({});
                if let Some(content) = delta.content {
                    wire["content"] = json!(content);
                }
                if !delta.tool_calls.is_empty() {
                    wire["tool_calls"] = Value::Array(delta.tool_calls.into_iter().map(|call| {
                        let mut wire = json!({"index": call.index, "function": {"arguments": call.arguments}});
                        if let Some(id) = call.id { wire["id"] = json!(id); wire["type"] = json!("function"); }
                        if let Some(name) = call.name { wire["function"]["name"] = json!(name); }
                        wire
                    }).collect());
                }
                events.push(chunk(wire, None));
            }
            events.push(chunk(json!({}), Some(finish)));
            let events = events
                .into_iter()
                .map(|value| value.to_string())
                .chain(std::iter::once("[DONE]".into()));
            Sse::new(futures::stream::iter(
                events.map(|data| Ok::<_, Infallible>(Event::default().data(data))),
            ))
            .into_response()
        }
        response => {
            let (message, finish) = match response {
                ChatResponse::Text { text } => {
                    (json!({"role": "assistant", "content": text}), "stop")
                }
                ChatResponse::ToolCalls { calls } => (
                    json!({"role": "assistant", "content": null, "tool_calls": calls.into_iter().map(|call| json!({"id": call.id, "type": "function", "function": {"name": call.name, "arguments": call.arguments}})).collect::<Vec<_>>()}),
                    "tool_calls",
                ),
                ChatResponse::Stream { .. } => unreachable!(),
            };
            Json(json!({"id": id, "object": "chat.completion", "created": 0, "model": model, "choices": [{"index": 0, "message": message, "finish_reason": finish}]})).into_response()
        }
    }
}

#[cfg(test)]
test_r::enable!();

#[cfg(test)]
mod tests;
