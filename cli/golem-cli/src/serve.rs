//! MCP HTTP server for golem-cli
//!
//! This module provides a small JSON‑RPC 2.0 HTTP endpoint exposing:
//! - `initialize`
//! - `tools/list`
//! - `tools/call` (executes `golem` with a safe allowlist derived from Clap if enabled)
//! - `resources/list` and `resources/read` (reuses manifest discovery if available)
//!
//! Notes for maintainers / reviewers:
//! - All request/response bodies are strongly typed (no ad‑hoc JSON literals).
//! - Allowed subcommands can be derived from the compiled CLI via Clap (see feature below).
//! - Manifest discovery is delegated to the repo’s canonical logic when present.
//!
//! Features you can toggle in this file (optional):
//! - `mcp-introspect-clap`: derive allowed subcommands from the real Clap command tree.
//!   Edit `clap_root_command()` to call your project’s root (e.g. `golem_cli::cli::Cli::command()`).
//! - `mcp-reuse-discovery`: use the repo’s manifest discovery instead of globbing.
//!   Edit `discover_manifests_via_repo()` to call the canonical helper.

use std::collections::BTreeSet;
use std::path::{ Path, PathBuf };
use std::process::Stdio;

use anyhow::{ anyhow, Context, Result };
use axum::{ extract::State, routing::post, Json, Router };
use axum::http::StatusCode;
use serde::{ Deserialize, Serialize };
use serde_json::Value;
use tokio::io::{ AsyncBufReadExt, BufReader };
use tokio::process::Command;
use tokio::task::JoinSet;

#[derive(serde::Serialize)]
struct McpTool {
    name: String,
    description: String,
    // JSON Schema (as raw value) describing the tool's "arguments"
    input_schema: serde_json::Value,
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Clone)]
#[serde(rename_all = "camelCase")]
struct GolemRunInput {
    args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
}
fn build_tools_registry() -> Vec<McpTool> {
    use schemars::schema_for;

    let run_schema = schema_for!(GolemRunInput);
    let run_schema_json = serde_json::to_value(&run_schema.schema).expect("schema to JSON");

    vec![McpTool {
        name: "golem.run".to_string(),
        description: "Execute the Golem CLI with validated top-level subcommand allowlist.".to_string(),
        input_schema: run_schema_json,
    }]
}

// ---------- Models: JSON‑RPC 2.0 ------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RpcRequest {
    jsonrpc: String,
    #[serde(default)]
    id: serde_json::Value,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct RpcResponse {
    #[serde(rename = "jsonrpc")]
    jsonrpc: &'static str,  // "2.0"
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Serialize, Deserialize)]
struct RpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

fn rpc_ok(id: Value, result: impl Serialize) -> axum::Json<RpcResponse> {
    axum::Json(RpcResponse {
        jsonrpc: "2.0",
        id: Some(id),
        result: Some(serde_json::to_value(result).expect("serializable result")),
        error: None,
    })
}

fn rpc_err(id: Value, code: i32, message: impl Into<String>, data: Option<Value>) -> axum::Json<RpcResponse> {
    axum::Json(RpcResponse {
        jsonrpc: "2.0",
        id: Some(id),
        result: None,
        error: Some(RpcError { code, message: message.into(), data }),
    })
}

// ---------- Models: MCP protocol (trimmed, typed) --------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializeResult {
    protocol_version: String,
    server_info: ServerInfo,
    capabilities: Capabilities,
}
#[derive(Debug, Serialize)]
struct ServerInfo {
    name: String,
    version: String,
}
#[derive(Debug, Serialize)]
struct Capabilities {
    tools: bool,
    resources: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolList {
    tools: Vec<ToolSpec>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolSpec {
    name: String,
    description: String,
    input_schema: schemars::schema::RootSchema,
}

#[derive(Deserialize, Clone, Debug)]
struct ToolCallParams {
    name: String,
    arguments: GolemRunInput, // typed; so `.arguments.args` compiles
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolArgs {
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolCallResult {
    ok: bool,
    command: ExecutedCommand,
    logs: Vec<LogLine>,
    result: ToolResult,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecutedCommand {
    binary: &'static str,
    args: Vec<String>,
    cwd: String,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolResult {
    exit_code: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogLine {
    stream: &'static str, // "stdout" | "stderr"
    line: String,
}

// Resources
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceList {
    resources: Vec<Resource>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Resource {
    uri: String,
}

#[derive(Debug, Deserialize)]
struct ResourceReadParams {
    uri: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceReadResult {
    contents: Vec<ResourceContent>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceContent {
    uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
    text: String,
}

// ---------- App state ------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub cwd: PathBuf,
}

// ---------- Public entry ---------------------------------------------------------

pub async fn serve_http_mcp(port: u16, cwd: PathBuf) -> Result<()> {
    let state = AppState { cwd };

    let app = Router::new().route("/mcp", post(handle)).with_state(state);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    tracing::info!("golem-cli: MCP HTTP server listening on http://{}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
}

// ---------- Handler --------------------------------------------------------------

async fn handle(State(state): State<AppState>, Json(req): Json<RpcRequest>) -> Json<RpcResponse> {
    if req.jsonrpc != "2.0" {
        return rpc_err(req.id, -32600, "Invalid Request", None);
    }

    match req.method.as_str() {
        "initialize" => {
            let result = InitializeResult {
                protocol_version: "2024-11-05".into(),
                server_info: ServerInfo {
                    name: "golem-cli".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                },
                capabilities: Capabilities { tools: true, resources: true },
            };
            rpc_ok(req.id, result)
        }
        "tools/list" => {
            // ← Generate from your registry so list == call
            let tools = build_tools_registry();
            let tools_json: Vec<_> = tools
                .into_iter()
                .map(
                    |t|
                        serde_json::json!({
            "name": t.name,
            "description": t.description,
            "inputSchema": t.input_schema
        })
                )
                .collect();

            let result = serde_json::json!({ "tools": tools_json, "nextCursor": null });
            rpc_ok(req.id, result)
        }
        "tools/call" => {
            // Parse the whole params object (name + arguments, typed)
            let params: ToolCallParams = match serde_json::from_value(req.params.clone()) {
                Ok(p) => p,
                Err(e) => {
                    return rpc_err(req.id, -32602, format!("Invalid params: {e}"), None);
                }
            };

            // Optional: Only allow tools that appear in tools/list (keeps contract tight)
            {
                use std::collections::BTreeSet;
                let allowed_tool_names: BTreeSet<_> = build_tools_registry()
                    .into_iter()
                    .map(|t| t.name)
                    .collect();
                if !allowed_tool_names.contains(&params.name) {
                    return rpc_err(req.id, -32601, format!("Unknown tool '{}'", params.name), None);
                }
            }

            // Dispatch to your existing runner (signature: &AppState, ToolCallParams)
            match params.name.as_str() {
                "golem.run" => {
                    match run_golem(&state, params).await {
                        Ok(res) => {
                            let val = serde_json
                                ::to_value(res)
                                .map_err(|e| anyhow::anyhow!("serialize ToolCallResult: {e}"))
                                .unwrap_or_else(
                                    |e| serde_json::json!({ "serializationError": e.to_string() })
                                );
                            rpc_ok(req.id, val)
                        }
                        Err(e) => rpc_err(req.id, 1, format!("Command failed: {e:#}"), None),
                    }
                }
                _ => rpc_err(req.id, -32601, "Method not found", None),
            }
        }

        "resources/list" => {
            match resources_list(&state).await {
                Ok(list) => rpc_ok(req.id, list),
                Err(e) =>
                    rpc_err(
                        req.id,
                        -32000,
                        "Resource listing failed",
                        Some(serde_json::json!({ "reason": e.to_string() }))
                    ),
            }
        }
        "resources/read" => {
            let params: ResourceReadParams = match serde_json::from_value(req.params) {
                Ok(v) => v,
                Err(e) => {
                    return rpc_err(
                        req.id,
                        -32602,
                        "Invalid params",
                        Some(serde_json::json!({ "reason": e.to_string() }))
                    );
                }
            };
            match resource_read(params).await {
                Ok(res) => rpc_ok(req.id, res),
                Err(e) =>
                    rpc_err(
                        req.id,
                        -32000,
                        "Resource read failed",
                        Some(serde_json::json!({ "reason": e.to_string() }))
                    ),
            }
        }
        _ => rpc_err(req.id, -32601, "Method not found", None),
    }
}

// ---------- Tools ----------------------------------------------------------------

async fn handle_tools_list() -> anyhow::Result<serde_json::Value> {
    let tools = build_tools_registry();
    Ok(
        serde_json::json!({
        "tools": tools.iter().map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.input_schema
            })
        }).collect::<Vec<_>>()
    })
    )
}
async fn handle_tools_call(
    state: &AppState,
    params: ToolCallParams
) -> anyhow::Result<serde_json::Value> {
    use std::collections::BTreeSet;

    // Contract: name must be listed
    let allowed_tool_names: BTreeSet<_> = build_tools_registry()
        .into_iter()
        .map(|t| t.name)
        .collect();
    if !allowed_tool_names.contains(&params.name) {
        anyhow::bail!("Unknown tool '{}'", params.name);
    }

    match params.name.as_str() {
        "golem.run" => {
            let result = run_golem(state, params).await?; // ToolCallResult
            Ok(serde_json::to_value(result)?) // back to Value for JSON-RPC
        }
        _ => anyhow::bail!("Unknown tool"),
    }
}

async fn run_golem(state: &AppState, params: ToolCallParams) -> Result<ToolCallResult> {
    if params.name.as_str() != "golem.run" {
        return Err(anyhow::anyhow!("Unknown tool '{}'", params.name));
    }

    let (first, rest) = params.arguments.args
        .split_first()
        .ok_or_else(|| anyhow!("args is empty"))?;

    let allowed = allowed_top_level_subcommands(); // Clap-derived when feature on; conservative fallback otherwise
    let disallowed = disallowed_list();

    if !allowed.contains(first) || disallowed.contains(first) {
        anyhow::bail!("Disallowed or unknown subcommand '{first}'");
    }

    let workdir = params.arguments.cwd.as_deref().map(Path::new).unwrap_or(&state.cwd);
    let mut cmd = tokio::process::Command::new("golem");
    cmd.current_dir(workdir).arg(first);
    for a in rest {
        cmd.arg(a);
    }
    cmd.kill_on_drop(true);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().context("spawn golem")?;

    // Concurrently read stdout & stderr line-by-line
    let mut logs: Vec<LogLine> = Vec::new();
    let mut set = JoinSet::new();

    if let Some(out) = child.stdout.take() {
        let mut reader = BufReader::new(out).lines();
        set.spawn(async move {
            let mut lines: Vec<LogLine> = Vec::new();
            while let Ok(Some(line)) = reader.next_line().await {
                lines.push(LogLine { stream: "stdout", line });
            }
            lines
        });
    }
    if let Some(err) = child.stderr.take() {
        let mut reader = BufReader::new(err).lines();
        set.spawn(async move {
            let mut lines: Vec<LogLine> = Vec::new();
            while let Ok(Some(line)) = reader.next_line().await {
                lines.push(LogLine { stream: "stderr", line });
            }
            lines
        });
    }

    while let Some(joined) = set.join_next().await {
        if let Ok(mut part) = joined {
            logs.append(&mut part);
        }
    }

    let status = child.wait().await?;
    let exit_code = status.code().unwrap_or(-1);

    Ok(ToolCallResult {
        ok: status.success(),
        command: ExecutedCommand {
            binary: "golem",
            args: params.arguments.args.clone(),
            cwd: workdir.display().to_string(),
        },
        logs,
        result: ToolResult { exit_code },
    })
}

// ---------- Allowed/Disallowed subcommands --------------------------------------

const DISALLOWED: &[&str] = &["system", "exec"];

fn disallowed_list() -> BTreeSet<String> {
    DISALLOWED.iter()
        .map(|s| s.to_string())
        .collect()
}

#[cfg(feature = "mcp-introspect-clap")]
fn allowed_top_level_subcommands() -> BTreeSet<String> {
    use clap::CommandFactory;
    use golem_cli::command;
    // `serve.rs` is in the same crate as `command.rs`, so use `crate::…`
    type Root = command::GolemCliCommand;

    Root::command()
        .get_subcommands()
        .map(|sc| sc.get_name().to_string())
        .collect()
}

#[cfg(not(feature = "mcp-introspect-clap"))]
fn allowed_top_level_subcommands() -> BTreeSet<String> {
    // Fallback list if you don't enable the feature.
    BTreeSet::from_iter([
        "version".to_string(),
        "profile".to_string(),
        "component".to_string(),
        "worker".to_string(),
        "cloud".to_string(),
        "rib-repl".to_string(),
    ])
}

// ---------- Resources ------------------------------------------------------------

async fn resources_list(state: &AppState) -> Result<ResourceList> {
    let uris = discover_resource_uris(&state.cwd).await?;
    Ok(ResourceList {
        resources: uris
            .into_iter()
            .map(|u| Resource { uri: u })
            .collect(),
    })
}

async fn resource_read(params: ResourceReadParams) -> Result<ResourceReadResult> {
    let uri = params.uri;
    if !uri.starts_with("file://") {
        return Err(anyhow!("Only file:// URIs are supported"));
    }
    let path = &uri["file://".len()..];
    let text = tokio::fs::read_to_string(path).await.with_context(|| format!("read {path}"))?;
    let mime = mime_guess
        ::from_path(path)
        .first_raw()
        .map(|s| s.to_string());
    Ok(ResourceReadResult {
        contents: vec![ResourceContent {
            uri,
            mime_type: mime,
            text,
        }],
    })
}

#[cfg(feature = "mcp-reuse-discovery")]
async fn discover_resource_uris(cwd: &std::path::Path) -> anyhow::Result<Vec<String>> {
    discover_manifests_via_repo(cwd).await
}

#[cfg(not(feature = "mcp-reuse-discovery"))]
async fn discover_resource_uris(cwd: &std::path::Path) -> anyhow::Result<Vec<String>> {
    discover_manifests_via_glob(cwd).await
}

#[cfg(feature = "mcp-reuse-discovery")]
async fn discover_manifests_via_repo(cwd: &std::path::Path) -> anyhow::Result<Vec<String>> {
    use anyhow::anyhow;
    use std::collections::BTreeSet;

    // Optional explicit override (acts like --manifest <path>)
    if let Ok(explicit) = std::env::var("GOLEM_APP_MANIFEST_PATH") {
        let p = to_abs(std::path::PathBuf::from(explicit), cwd);
        if !p.exists() {
            return Err(anyhow!("explicit GOLEM_APP_MANIFEST_PATH not found: {}", p.display()));
        }
        let mut out = BTreeSet::new();
        collect_includes(&p, &mut out).await?;
        return Ok(
            out
                .into_iter()
                .map(|p| format!("file://{}", p.display()))
                .collect()
        );
    }

    // Discover: walk upward for golem.yaml / golem.yml
    let Some(root) = find_root_manifest(cwd) else {
        return Ok(Vec::new());
    };

    // Expand includes (relative to the including file), dedup, cycle-safe
    let mut out = BTreeSet::new();
    collect_includes(&root, &mut out).await?;
    Ok(
        out
            .into_iter()
            .map(|p| format!("file://{}", p.display()))
            .collect()
    )
}

#[cfg(feature = "mcp-reuse-discovery")]
fn to_abs(p: std::path::PathBuf, cwd: &std::path::Path) -> std::path::PathBuf {
    if p.is_absolute() { p } else { cwd.join(p) }
}

#[cfg(feature = "mcp-reuse-discovery")]
fn find_root_manifest(start: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut cur = start;
    loop {
        for name in ["golem.yaml", "golem.yml"] {
            let candidate = cur.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        match cur.parent() {
            Some(parent) => {
                cur = parent;
            }
            None => {
                break;
            }
        }
    }
    None
}

#[cfg(feature = "mcp-reuse-discovery")]
async fn collect_includes(
    root: &std::path::Path,
    out: &mut std::collections::BTreeSet<std::path::PathBuf>
) -> anyhow::Result<()> {
    use serde_yaml::Value;
    use std::collections::VecDeque;
    use std::path::Path;

    let mut q = VecDeque::new();
    let mut seen = std::collections::BTreeSet::new();

    q.push_back(root.to_path_buf());

    while let Some(file) = q.pop_front() {
        let canon = tokio::fs::canonicalize(&file).await.unwrap_or(file.clone());
        if !seen.insert(canon.clone()) {
            continue;
        }
        out.insert(canon.clone());

        let text = match tokio::fs::read_to_string(&canon).await {
            Ok(t) => t,
            Err(_) => {
                continue;
            }
        };
        let parsed: Value = match serde_yaml::from_str(&text) {
            Ok(v) => v,
            Err(_) => {
                continue;
            }
        };

        let dir = canon.parent().unwrap_or(Path::new("."));
        let mut push_rel = |rel: &str| {
            if rel.trim().is_empty() {
                return;
            }
            let p = Path::new(rel);
            let dep = if p.is_absolute() { p.to_path_buf() } else { dir.join(p) };
            match dep.extension().and_then(|s| s.to_str()) {
                Some("yaml") | Some("yml") => q.push_back(dep),
                _ => {}
            }
        };

        if let Some(v) = parsed.get("include") {
            if let Some(s) = v.as_str() {
                push_rel(s);
            }
        }
        if let Some(v) = parsed.get("includes") {
            if let Some(arr) = v.as_sequence() {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        push_rel(s);
                    }
                }
            }
        }
    }
    Ok(())
}

async fn discover_manifests_via_glob(cwd: &Path) -> Result<Vec<String>> {
    let mut results = Vec::new();
    for entry in globwalk::GlobWalkerBuilder
        ::from_patterns(cwd, &["*.yaml", "*.yml", "*.json", "*.toml"])
        .max_depth(5)
        .build()
        .context("glob manifests")? {
        let entry = entry?;
        let path = entry.path().to_path_buf();
        results.push(format!("file://{}", path.display()));
    }
    results.sort();
    results.dedup();
    Ok(results)
}
