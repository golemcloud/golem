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
use serde::{ Deserialize, Serialize };
use serde_json::Value;
use tokio::io::{ AsyncBufReadExt, BufReader };
use tokio::process::Command;
use tokio::task::JoinSet;



#[derive(Debug, Deserialize, schemars::JsonSchema, Clone)]
#[serde(rename_all = "camelCase")]
struct GolemRunInput {
    args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
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
    jsonrpc: &'static str, // "2.0"
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

fn rpc_err(
    id: Value,
    code: i32,
    message: impl Into<String>,
    data: Option<Value>
) -> axum::Json<RpcResponse> {
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

#[derive(Deserialize, Clone, Debug)]
struct ToolCallParams {
    name: String,
    arguments: GolemRunInput, // typed; so `.arguments.args` compiles
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
            // Surface available golem commands instead of just exposing `golem.run`
            let cmds = available_golem_commands();

            // The MCP spec for tools/list expects an object with "tools": [...]
            // where each tool has name/description/inputSchema.
            // We'll model each CLI command as a "tool" with no required args schema
            // beyond "args: string[]". If you want a thinner payload (just strings),
            // see note below.

            let command_schema =
                serde_json::json!({
        "type": "object",
        "properties": {
            "args": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Arguments passed after this command, same as CLI"
            }
        },
        "required": ["args"]
    });

            let tools_json: Vec<_> = cmds
                .into_iter()
                .map(|cmd| {
                    serde_json::json!({
                "name": cmd,
                "description": format!("Golem CLI command '{}'", cmd),
                "inputSchema": command_schema
            })
                })
                .collect();

            rpc_ok(req.id, serde_json::json!({
        "tools": tools_json
    }))
        }

        "tools/call" => {
            // 1. Parse params from the request body
            let params: ToolCallParams = match serde_json::from_value(req.params.clone()) {
                Ok(p) => p,
                Err(e) => {
                    return rpc_err(
                        req.id,
                        -32602,
                        "Invalid params",
                        Some(serde_json::json!({ "reason": e.to_string() }))
                    );
                }
            };

            // 2. The top-level command is the tool name (e.g. "cloud", "app", "profile")
            //    The request only passed trailing args in `params.arguments.args`.
            //    We need to build the *full* argv ("cloud", "project", "list", ...).
            let mut full_args = Vec::with_capacity(1 + params.arguments.args.len());
            full_args.push(params.name.clone());
            full_args.extend(params.arguments.args.clone());

            // 3. Build the call we actually want to execute
            let patched_params = ToolCallParams {
                name: params.name.clone(),
                arguments: GolemRunInput {
                    args: full_args,
                    cwd: params.arguments.cwd.clone(),
                },
            };

            // 4. Run it
            match run_golem(&state, patched_params).await {
                Ok(result) => {
                    match serde_json::to_value(result) {
                        Ok(val) => rpc_ok(req.id, val),
                        Err(e) =>
                            rpc_err(
                                req.id,
                                -32001,
                                "Serialization error",
                                Some(serde_json::json!({ "reason": e.to_string() }))
                            ),
                    }
                }
                Err(e) =>
                    rpc_err(
                        req.id,
                        -32000,
                        "Tool call failed",
                        Some(serde_json::json!({ "reason": e.to_string() }))
                    ),
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
    let cmds = available_golem_commands();

    let command_schema =
        serde_json::json!({
        "type": "object",
        "properties": {
            "args": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Arguments passed after this command, same as CLI"
            }
        },
        "required": ["args"]
    });

    let tools_json: Vec<_> = cmds
        .into_iter()
        .map(|cmd| {
            serde_json::json!({
                "name": cmd,
                "description": format!("Golem CLI command '{}'", cmd),
                "inputSchema": command_schema
            })
        })
        .collect();

    Ok(serde_json::json!({
        "tools": tools_json
    }))
}

async fn run_golem(state: &AppState, params: ToolCallParams) -> Result<ToolCallResult> {
    // Expect: params.arguments.args is already the FULL argv for `golem`,
    // e.g. ["cloud", "project", "list", "--json"]
    let (first, rest) = params
        .arguments
        .args
        .split_first()
        .ok_or_else(|| anyhow!("args is empty"))?;

    // Security gate: only allow blessed top-level commands and reject disallowed ones.
    let allowed = allowed_top_level_subcommands();
    let disallowed = disallowed_list();
    if !allowed.contains(first) || disallowed.contains(first) {
        anyhow::bail!("Disallowed or unknown subcommand '{first}'");
    }

    // Resolve working directory:
    // prefer client-supplied cwd, else fall back to server state cwd
    let workdir: PathBuf = if let Some(cwd_str) = &params.arguments.cwd {
        PathBuf::from(cwd_str)
    } else {
        state.cwd.clone()
    };

    // Build the process: `golem <first> <rest...>`
    let mut cmd = Command::new("golem");
    cmd.current_dir(&workdir);
    cmd.args(std::iter::once(first).chain(rest.iter()));

    // Make sure child dies with us, capture stdout/stderr.
    cmd.kill_on_drop(true);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().context("spawn golem")?;

    // Concurrently read stdout & stderr line by line
    let mut logs: Vec<LogLine> = Vec::new();
    let mut set = JoinSet::new();

    if let Some(out) = child.stdout.take() {
        let mut reader = BufReader::new(out).lines();
        set.spawn(async move {
            let mut lines: Vec<LogLine> = Vec::new();
            while let Ok(Some(line)) = reader.next_line().await {
                lines.push(LogLine {
                    stream: "stdout",
                    line,
                });
            }
            lines
        });
    }

    if let Some(err) = child.stderr.take() {
        let mut reader = BufReader::new(err).lines();
        set.spawn(async move {
            let mut lines: Vec<LogLine> = Vec::new();
            while let Ok(Some(line)) = reader.next_line().await {
                lines.push(LogLine {
                    stream: "stderr",
                    line,
                });
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
    type Root = command::GolemCliCommand;

    // Start with whatever clap says are real top-level subcommands.
    let mut allowed: BTreeSet<String> = Root::command()
        .get_subcommands()
        .map(|sc| sc.get_name().to_string())
        .collect();

    // Explicitly extend with the list we want exposed to MCP:
    let extras = [
        "app",
        "application",
        "component",
        "agent",
        "api",
        "plugin",
        "profile",
        "server",
        "cloud",
        "repl",
        "completion",
        "help",
    ];

    for cmd in extras {
        allowed.insert(cmd.to_string());
    }

    allowed
}

#[cfg(not(feature = "mcp-introspect-clap"))]
fn allowed_top_level_subcommands() -> BTreeSet<String> {
    let mut allowed: BTreeSet<String> = BTreeSet::new();

    // Fallback build (no clap introspection). We still allow exactly what you asked for.
    let extras = [
        "app",
        "application",
        "component",
        "agent",
        "api",
        "plugin",
        "profile",
        "server",
        "cloud",
        "repl",
        "completion",
        "help",
    ];

    for cmd in extras {
        allowed.insert(cmd.to_string());
    }

    allowed
}

fn available_golem_commands() -> Vec<String> {
    use std::collections::BTreeSet;

    // Start from whatever the server currently allows
    let mut cmds: BTreeSet<String> = allowed_top_level_subcommands();

    // Normalize/alias:
    // - Historically the CLI exposed "worker", but UX calls it "agent".
    if cmds.remove("worker") {
        cmds.insert("agent".to_string());
    }

    // We want "application" as an alias for "app".
    if cmds.contains("app") {
        cmds.insert("application".to_string());
    }

    // We also want to expose "repl" even if it's not registered
    // as a normal top-level clap subcommand. The code for that lives in
    // `command_handler/interactive.rs` and `rib_repl.rs`, so just force-add.
    cmds.insert("repl".to_string());

    // We also want to make sure these are present even if clap / fallback
    // doesn't list them explicitly.
    cmds.insert("completion".to_string());
    cmds.insert("help".to_string());

    // And finally, anything else you explicitly asked for:
    cmds.insert("plugin".to_string());
    cmds.insert("profile".to_string());
    cmds.insert("server".to_string());
    cmds.insert("cloud".to_string());
    cmds.insert("api".to_string());
    cmds.insert("component".to_string());
    cmds.insert("app".to_string());

    // Convert to Vec in deterministic (sorted) order
    cmds.into_iter().collect()
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
