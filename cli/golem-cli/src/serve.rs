use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use globwalk::GlobWalkerBuilder;
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncReadExt, process::Command, sync::mpsc, task, time};
use tracing::{info};

#[derive(Clone)]
pub struct ServeState {
    pub allowed_subcommands: Vec<&'static str>,
    pub workdir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
    id: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
    id: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

fn ok(id: serde_json::Value, result: serde_json::Value) -> JsonRpcResponse {
    JsonRpcResponse { jsonrpc: "2.0", result: Some(result), error: None, id }
}
fn err(
    id: serde_json::Value,
    code: i32,
    msg: impl Into<String>,
    data: Option<serde_json::Value>,
) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        result: None,
        error: Some(JsonRpcError { code, message: msg.into(), data }),
        id,
    }
}

/// Public entry point called from main
pub async fn serve_http_mcp(port: u16, workdir: PathBuf) -> Result<()> {
    // conservative allowlist for safety
    let allowed_subcommands = vec![
        "version",
        "profile",    // list, set, etc.
        "component",  // list, deploy, etc.
        "worker",     // list, show, etc.
        "deployment", // list, show, etc.
        "templates",  // list, init, etc.
        "login", "logout",
        "help",
    ];
    let state = ServeState { allowed_subcommands, workdir };

    let app = Router::new()
        .route("/mcp", post(handle_json_rpc))
        .with_state(state);

    // Initialize tracing if the binary hasn't configured it already.
    // This is idempotent enough for local dev; keep it lightweight.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .without_time()
        .try_init();

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    info!("golem-cli: MCP HTTP server listening on http://{}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
}

async fn handle_json_rpc(
    State(state): State<ServeState>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let id = req.id.clone();

    match req.method.as_str() {
        "initialize" => {
            let result = serde_json::json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": { "name": "golem-cli", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": {
                    "resources": { "listChanged": false },
                    "tools": {},
                    "logging": { "supports": ["info","warn","error","debug"] }
                }
            });
            (StatusCode::OK, Json(ok(id, result)))
        }

        "tools/list" => {
            let tool = serde_json::json!({
              "name": "golem.run",
              "description": "Run a safe subset of `golem` CLI commands and return stdout/stderr incrementally.",
              "inputSchema": {
                "type": "object",
                "properties": {
                  "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Args after `golem`. Example: [\"component\",\"list\"]"
                  },
                  "cwd": { "type": "string", "description": "Optional working directory" }
                },
                "required": ["args"]
              }
            });
            let result = serde_json::json!({ "tools": [tool], "nextCursor": null });
            (StatusCode::OK, Json(ok(id, result)))
        }

        "tools/call" => {
            // params: { "name": "golem.run", "arguments": { "args": [...], "cwd": "..." } }
            let call = req.params.get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            if call != "golem.run" {
                return (StatusCode::OK, Json(err(id, -32602, "Unknown tool name", None)));
            }

            // parse args
            let args_val = req.params.get("arguments")
                .and_then(|a| a.get("args"))
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            let mut arg_strs: Vec<String> = Vec::with_capacity(args_val.len());
            for a in args_val {
                if let Some(s) = a.as_str() {
                    arg_strs.push(s.to_string());
                } else {
                    return (StatusCode::OK, Json(err(
                        id, -32602, "Tool args must be an array of strings", None
                    )));
                }
            }

            // safety allowlist
            if let Some(first) = arg_strs.first() {
                if !state.allowed_subcommands.iter().any(|s| s == first) {
                    return (StatusCode::OK, Json(err(
                        id,
                        -32602,
                        format!("Disallowed subcommand '{}'", first),
                        Some(serde_json::json!({ "allowed": state.allowed_subcommands }))
                    )));
                }
            } else {
                return (StatusCode::OK, Json(err(id, -32602, "At least one argument is required", None)));
            }

            // working directory
            let cwd = req.params
                .get("arguments")
                .and_then(|a| a.get("cwd"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| state.workdir.clone());

            // --- FIXED: keep rx/logs in this task; use select! to read logs & await child ---
            let (tx, mut rx) = mpsc::unbounded_channel::<serde_json::Value>();
            let mut run_join = task::spawn(run_golem_command(arg_strs.clone(), cwd, tx));

            let mut logs: Vec<serde_json::Value> = Vec::new();
            let mut final_result: Option<serde_json::Value> = None;

            loop {
                tokio::select! {
                    maybe_msg = rx.recv() => {
                        match maybe_msg {
                            Some(msg) => logs.push(msg),
                            None => {
                                // all senders dropped; if process done, we can finish
                                if final_result.is_some() {
                                    break;
                                }
                            }
                        }
                    }
                    res = &mut run_join, if final_result.is_none() => {
                        match res {
                            Ok(Ok(res)) => { final_result = Some(res); }
                            Ok(Err(e)) => {
                                return (StatusCode::OK, Json(err(id.clone(), 1, format!("Command failed: {e:#}"), None)));
                            }
                            Err(join_err) => {
                                return (StatusCode::OK, Json(err(id.clone(), 1, format!("Join error: {join_err}"), None)));
                            }
                        }
                        // continue draining remaining logs until rx closes
                    }
                }
            }

            let result = serde_json::json!({
                "ok": true,
                "command": { "binary": "golem", "args": arg_strs },
                "logs": logs,
                "result": final_result,
            });
            (StatusCode::OK, Json(ok(id, result)))
        }

        "resources/list" => {
            let manifests = discover_manifests(&state.workdir);
            let items: Vec<_> = manifests
                .into_iter()
                .map(|p| {
                    let display = p.strip_prefix(&state.workdir).unwrap_or(&p).to_string_lossy().to_string();
                    serde_json::json!({
                        "uri": format!("file://{}", p.display()),
                        "name": display,
                        "mimeType": mime_from_path(&p),
                        "description": "Golem manifest or related file"
                    })
                })
                .collect();

            let result = serde_json::json!({ "resources": items, "nextCursor": null });
            (StatusCode::OK, Json(ok(id, result)))
        }

        "resources/read" => {
            let Some(uri) = req.params.get("uri").and_then(|v| v.as_str()) else {
                return (StatusCode::OK, Json(err(id, -32602, "Missing 'uri' for resources/read", None)));
            };
            let path = if let Some(stripped) = uri.strip_prefix("file://") {
                PathBuf::from(stripped)
            } else {
                return (StatusCode::OK, Json(err(id, -32602, "Only file:// URIs are supported", None)));
            };
            match tokio::fs::read_to_string(&path).await {
                Ok(contents) => {
                    let result = serde_json::json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": mime_from_path(&path),
                            "text": contents
                        }]
                    });
                    (StatusCode::OK, Json(ok(id, result)))
                }
                Err(e) => (StatusCode::OK, Json(err(id, 2, format!("Could not read file: {e}"), None))),
            }
        }

        _ => (StatusCode::OK, Json(err(id, -32601, "Method not found", None))),
    }
}

async fn run_golem_command(
    args: Vec<String>,
    cwd: PathBuf,
    tx: mpsc::UnboundedSender<serde_json::Value>,
) -> Result<serde_json::Value> {
    // invoke external `golem` binary
    let mut child = Command::new("golem");
    child
        .args(&args)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = child.spawn().context("Failed to spawn `golem`")?;

    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();

    let tx_out = tx.clone();
    let out_task = task::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            let n = stdout.read(&mut buf).await.unwrap_or(0);
            if n == 0 { break; }
            let s = String::from_utf8_lossy(&buf[..n]).to_string();
            for line in s.lines() {
                let _ = tx_out.send(serde_json::json!({"stream":"stdout","line": line}));
            }
        }
    });

    let tx_err = tx;
    let err_task = task::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            let n = stderr.read(&mut buf).await.unwrap_or(0);
            if n == 0 { break; }
            let s = String::from_utf8_lossy(&buf[..n]).to_string();
            for line in s.lines() {
                let _ = tx_err.send(serde_json::json!({"stream":"stderr","line": line}));
            }
        }
    });

    let status = child.wait().await?;
    // give collectors a brief moment to drain
    time::sleep(Duration::from_millis(50)).await;
    let _ = out_task.await;
    let _ = err_task.await;

    Ok(serde_json::json!({
        "exitCode": status.code(),
    }))
}

fn discover_manifests(cwd: &Path) -> Vec<PathBuf> {
    let mut results: Vec<PathBuf> = vec![];

    // nearest in cwd + ancestors
    let mut cur = Some(cwd.to_path_buf());
    while let Some(p) = cur {
        for cand in [
            "golem.yaml", "golem.yml", "golem.json", "golem.toml",
            "manifest.yaml", "manifest.yml", "manifest.json", "manifest.toml",
        ] {
            let f = p.join(cand);
            if f.exists() { results.push(f); }
        }
        cur = p.parent().map(|q| q.to_path_buf());
        if let Some(ref parent) = cur {
            if parent == &PathBuf::from("/") { break; }
        }
    }

    // children (depth 1)
    if let Ok(walker) = GlobWalkerBuilder::from_patterns(cwd, &["*/golem.*", "*/manifest.*"])
        .max_depth(2)
        .follow_links(true)
        .build()
    {
        for entry in walker.filter_map(Result::ok) {
            if entry.file_type().is_file() {
                results.push(entry.path().to_path_buf());
            }
        }
    }

    results.sort();
    results.dedup();
    results
}

fn mime_from_path(p: &Path) -> &'static str {
    match p.extension().and_then(OsStr::to_str) {
        Some("yaml") | Some("yml") => "application/yaml",
        Some("json") => "application/json",
        Some("toml") => "application/toml",
        _ => "text/plain",
    }
}
