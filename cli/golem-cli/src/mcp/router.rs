use axum::{
    extract::State,
    response::{sse::{Event, Sse}, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{convert::Infallible, sync::Arc};
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use crate::context::Context;
use crate::mcp::tools::{handle_tool_call, list_tools};

// --- PROTOCOL STRUCTS ---

#[derive(Serialize, Deserialize, Debug)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    params: Option<Value>,
    id: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug)]
struct JsonRpcResponse {
    jsonrpc: String,
    result: Option<Value>,
    error: Option<JsonRpcError>,
    id: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug)]
struct JsonRpcError {
    code: i32,
    message: String,
}

// --- SERVER STATE ---

#[derive(Clone)]
struct AppState {
    ctx: Arc<Context>,
    tx: broadcast::Sender<String>,
}

// --- ROUTER LOGIC ---

pub fn create_router(ctx: Arc<Context>) -> Router {
    let (tx, _rx) = broadcast::channel(100);
    let state = AppState { ctx, tx };

    Router::new()
        .route("/sse", get(sse_handler))
        .route("/messages", post(message_handler))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

// 1. SSE HANDLER
async fn sse_handler(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.tx.subscribe();

    let stream = async_stream::stream! {
        // MCP Handshake
        yield Ok(Event::default().event("endpoint").data("/messages"));

        // Keep-alive stream
        while let Ok(msg) = rx.recv().await {
            yield Ok(Event::default().data(msg));
        }
    };

    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

// 2. MESSAGE HANDLER
async fn message_handler(
    State(state): State<AppState>,
    Json(req): Json<JsonRpcRequest>,
) -> Json<JsonRpcResponse> {
    if req.jsonrpc != "2.0" {
        return Json(error_response(req.id, -32600, "Invalid Request: Must be JSON-RPC 2.0"));
    }

    let response = match req.method.as_str() {
        "initialize" => success_response(req.id, json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "golem-cli", "version": "1.0.0" }
        })),

        "tools/list" => {
            let tools = list_tools();
            success_response(req.id, json!({ "tools": tools }))
        },

        "tools/call" => {
            match handle_tool_call(&state.ctx, req.params).await {
                Ok(res) => success_response(req.id, res),
                Err(e) => error_response(req.id, -32000, &e.to_string()),
            }
        },

        _ => error_response(req.id, -32601, "Method not found"),
    };

    Json(response)
}

fn success_response(id: Option<Value>, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        result: Some(result),
        error: None,
        id,
    }
}

fn error_response(id: Option<Value>, code: i32, message: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.to_string(),
        }),
        id,
    }
}
