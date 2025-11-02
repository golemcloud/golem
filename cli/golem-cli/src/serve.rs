//! MCP HTTP server for golem-cli
//!
//! This module provides a small JSON‑RPC 2.0 HTTP endpoint exposing:
//! - `tools/list`
//! - `tools/call` (executes `golem` with a safe allowlist derived from Clap if enabled)
//!
//! Notes for maintainers / reviewers:
//! - All request/response bodies are strongly typed (no ad‑hoc JSON literals).
//! - Allowed subcommands can be derived from the compiled CLI via Clap (see feature below).
//! - Manifest discovery is delegated to the repo’s canonical logic when present.
//!

use std::collections::{ BTreeMap, BTreeSet };
use std::path::{ Path, PathBuf };
use std::process::Stdio;
use std::fs;
use serde_json::json;
use tokio::fs as aiofs;
use anyhow::{ anyhow, Context, Result };

use axum::{ extract::State, routing::get, Json, Router };
use axum::response::{ IntoResponse, Response };
use axum::extract::rejection::JsonRejection;

use serde::{ Deserialize, Serialize };
use serde_json::Value;
use tokio::io::{ AsyncBufReadExt, BufReader };
use tokio::process::Command;
use tokio::task::JoinSet;
use std::collections::HashMap;

use std::process::Command as StdCommand;

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

#[derive(Serialize)]
pub struct RpcResponse<T: Serialize> {
    #[serde(rename = "jsonrpc")]
    pub jsonrpc: &'static str, // always "2.0"

    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// Success helper
pub fn rpc_ok<T: Serialize>(id: Value, result: T) -> Json<RpcResponse<T>> {
    Json(RpcResponse {
        jsonrpc: "2.0",
        id: Some(id),
        result: Some(result),
        error: None,
    })
}

// Error helper
pub fn rpc_err(
    id: Value,
    code: i32,
    message: impl Into<String>,
    data: Option<Value>
) -> Json<RpcResponse<()>> {
    Json(RpcResponse {
        jsonrpc: "2.0",
        id: Some(id),
        result: None,
        error: Some(RpcError {
            code,
            message: message.into(),
            data,
        }),
    })
}

#[derive(Serialize, Clone)]
pub struct ToolInputSchema {
    #[serde(rename = "type")]
    pub type_tag: &'static str, // "object"
    pub properties: ToolInputProperties,
    pub required: Vec<&'static str>,
}

#[derive(Serialize, Clone)]
pub struct ToolInputProperties {
    pub args: ToolInputPropertyArgs,
    pub cwd: ToolInputPropertyCwd,
}

#[derive(Serialize, Clone)]
pub struct ToolInputPropertyArgs {
    #[serde(rename = "type")]
    pub type_tag: &'static str, // "array"
    pub items: ToolInputPropertyItems,
    pub description: &'static str,
}

#[derive(Serialize, Clone)]
pub struct ToolInputPropertyItems {
    #[serde(rename = "type")]
    pub type_tag: &'static str, // "string"
}

#[derive(Serialize, Clone)]
pub struct ToolInputPropertyCwd {
    #[serde(rename = "type")]
    pub type_tag: &'static str, // "string"
    pub description: &'static str,
}
#[derive(Serialize, Clone, Default)]
pub struct ToolResources {
    #[serde(rename = "relevant_repos")]
    pub relevant_repos: Vec<RepoCrateResource>,
    pub docs: Vec<DocLink>,
}

#[derive(Serialize, Clone)]
pub struct DocLink {
    pub title: String,
    pub url: String,
    pub score: f32,
}

#[derive(Serialize, Clone)]
pub struct RepoCrateResource {
    pub repo: String,
    #[serde(rename = "cargo_toml")]
    pub cargo_toml: serde_json::Value,
}

#[derive(Serialize)]
pub struct ToolDescriptor {
    // order here is what the client will see
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subcommands: Vec<SubcommandDescriptor>,
    pub other: ToolResources,
}

#[derive(Serialize)]
pub struct ToolsListResult {
    pub tools: Vec<ToolDescriptor>,
}


#[derive(Deserialize)]
struct CargoPackage {
    name: String,
    manifest_path: String,
    metadata: Option<PackageMetadata>,
}

#[derive(Deserialize)]
struct PackageMetadata {
    #[serde(rename = "golem_mcp")]
    golem_mcp: Option<GolemMcpMetadata>,
}

#[derive(Deserialize)]
struct GolemMcpMetadata {
    commands: Option<Vec<String>>,
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

#[derive(Deserialize)]
struct NextraEntry {
    title: String,
    data: std::collections::HashMap<String, String>, // anchor -> text
}

#[derive(Clone)]
struct DocPage {
    route: String, // "/cli/agents"
    title: String, // "Golem CLI Agents"
    sections: Vec<(String, String)>, // (anchor, text)
    fulltext_lc: String, // cached lowercase text
}

#[derive(Clone, Default)]
struct DocsIndex {
    pages: Vec<DocPage>,
}

#[derive(Serialize, Clone)]
pub struct SubcommandDescriptor {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Present only for `list` subcommands. Placed right after name & description.
    pub available: Option<serde_json::Value>,
    /// Only the `Arguments:` section from `golem <path> --help`, as a simple JSON object.
    /// Example:
    /// { "AGENT_ID": "Agent ID, ...", "ARGUMENTS...": "Command-line arguments visible for the agent" }
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subcommands: Vec<SubcommandDescriptor>,
}

// ---------- App state ------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub cwd: PathBuf,
    docs_index: Option<DocsIndex>,
}

#[derive(serde::Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
}

const NEXTRA_URL: &str = "https://learn.golem.cloud/_next/static/chunks/nextra-data-en-US.json";
const NEXTRA_CACHE_FILE: &str = ".cache/nextra-data-en-US.json";
const NEXTRA_CACHE_TTL_SECS: u64 = 6 * 60 * 60; // 6h

// ---------- Public entry ---------------------------------------------------------

pub async fn serve_http_mcp(port: u16, cwd: PathBuf) -> Result<()> {
    // Load (or fetch) nextra index into memory, cache on disk under {cwd}/.cache/
    let docs_index = match load_docs_index_from_remote_with_cache(&cwd).await {
        Ok(idx) => Some(idx),
        Err(e) => {
            tracing::warn!("nextra docs index not available: {e}");
            None
        }
    };

    let state = AppState { cwd, docs_index };

    let app = Router::new()
        .route("/mcp", get(get_default).post(handle))
        .with_state(state);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    tracing::info!("golem-cli: MCP HTTP server listening on http://{}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
}



// --------------- Default GET /mcp handler -----------------
async fn get_default() -> impl IntoResponse {
    // Check if a process like: `golem server run` is present in the cmdline
    let golem_server_status = match Command::new("pgrep")
        .arg("-f")
        .arg("golem server run")
        .output()
        .await
    {
        Ok(output) if output.status.success() && !output.stdout.is_empty() => {
            "running".to_string()
        }
        _ => "offline (consider using call_tool with server run)".to_string(),
    };


    // Shared input schema for tool calls
    let input_schema = ToolInputSchema {
        type_tag: "object",
        properties: ToolInputProperties {
            args: ToolInputPropertyArgs {
                type_tag: "array",
                items: ToolInputPropertyItems { type_tag: "string" },
                description: "All CLI words after the command name",
            },
            cwd: ToolInputPropertyCwd {
                type_tag: "string",
                description: "Working directory to run the command in",
            },
        },
        required: vec!["args"],
    };

    let result = json!({
        "available_methods": ["list_tools", "call_tool"],
        "inputSchema": input_schema,
        "message": "MCP server is running. Use POST /mcp with JSON-RPC 2.0.",
        "golem_server": golem_server_status
    });

    // Wrap in same RpcResponse envelope structure (id = null)
    rpc_ok(serde_json::Value::Null, result)
}

// ---------- Handler --------------------------------------------------------------

async fn handle(
    State(state): State<AppState>,
    payload: Result<Json<RpcRequest>, JsonRejection>
) -> Response {
    match payload {
        Ok(Json(req)) => {
            if req.jsonrpc != "2.0" {
                return rpc_err(req.id, -32600, "Invalid Request", None).into_response();
            }

            match req.method.as_str() {
                "list_tools" => {
                    let cmds_with_descs = available_golem_commands();
                    let cmd_index = build_command_index();
                    let docs_idx = state.docs_index.as_ref();

                    
                    // 3. Convert each (name, desc) into a ToolDescriptor (sequential),
                    //    and compute `available` by running the corresponding `list` command.
                    let mut tools: Vec<ToolDescriptor> = Vec::new();
                    for (cmd_name, desc) in cmds_with_descs.into_iter() {
                        let docs = match &docs_idx {
                            Some(idx) => docs_for_command(idx, &cmd_name, &desc),
                            None => Vec::new(),
                        };
                        let subs = {
                            let mut plain = clap_subcommands_for(&cmd_name);
                            enrich_subcommands_available(&state, &cmd_name, &mut plain).await;
                            plain
                        };
                        tools.push(ToolDescriptor {
                            name: cmd_name.clone(),
                            description: desc.clone(),
                            subcommands: subs,
                            other: ToolResources {
                                relevant_repos: cmd_index
                                    .get(&cmd_name)
                                    .cloned()
                                    .unwrap_or_default(),
                                docs,
                            },
                        });
                    }

                    let result = ToolsListResult { tools };
                    rpc_ok(req.id, result).into_response()
                }

                "call_tool" => {
                    // Parse params from request
                    let params: ToolCallParams = match serde_json::from_value(req.params.clone()) {
                        Ok(p) => p,
                        Err(e) => {
                            return rpc_err(
                                req.id,
                                -32602,
                                "Invalid params",
                                Some(serde_json::json!({ "reason": e.to_string() }))
                            ).into_response();
                        }
                    };

                    // Build full argv (prepend tool name)
                    let mut full_args = Vec::with_capacity(1 + params.arguments.args.len());
                    full_args.push(params.name.clone());
                    full_args.extend(params.arguments.args.clone());

                    // Patch args/cwd into the format run_golem expects
                    let patched_params = ToolCallParams {
                        name: params.name.clone(),
                        arguments: GolemRunInput {
                            args: full_args,
                            cwd: params.arguments.cwd.clone(),
                        },
                    };

                    // Call run_golem
                    match run_golem(&state, patched_params).await {
                        Ok(result) => {
                            // IMPORTANT CHANGE:
                            // - We do NOT convert `result` to serde_json::Value.
                            // - We hand the struct directly to rpc_ok.
                            // - rpc_ok wraps it in RpcResponse<ToolResult> which Axum serializes.
                            return rpc_ok(req.id, result).into_response();
                        }

                        Err(e) => {
                            return rpc_err(
                                req.id,
                                -32000,
                                "Tool call failed",
                                Some(serde_json::json!({ "reason": e.to_string() }))
                            ).into_response();
                        }
                    }
                }
                _ => rpc_err(req.id, -32601, "Method not found", None).into_response(),
            }
        }
        Err(_rej) => {
            // Default response for non-JSON-RPC / wrong schema requests
            let result =
                serde_json::json!({
                "message": "MCP endpoint expects JSON-RPC 2.0. POST with method=list_tools|call_tool.",
                "available_methods": ["list_tools", "call_tool"]

            });
            // Unknown id in this scenario
            return rpc_ok(serde_json::Value::Null, result).into_response();
        }
    }
}

// ---------- Tools ----------------------------------------------------------------

async fn run_golem(state: &AppState, params: ToolCallParams) -> Result<ToolCallResult> {
    // Expect: params.arguments.args is already the FULL argv for `golem`,
    // e.g. ["cloud", "project", "list", "--json"]
    let (first, rest) = params.arguments.args
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

    // Detect: `golem server run ...` should be launched and we should return immediately.
    let is_server_run =
        *first == "server" &&
        rest
            .first()
            .map(|s| s.as_str() == "run")
            .unwrap_or(false);

    if is_server_run {
        // Build detached child: no pipes, and do NOT kill on drop.
        let mut cmd = Command::new("golem");
        cmd.current_dir(&workdir);
        cmd.args(std::iter::once(first).chain(rest.iter()));

        // Important differences vs the generic path:
        // - Don't kill on drop (we want it to keep running after the handler returns).
        // - Don't pipe stdout/stderr (avoid filling buffers after we detach).
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        // NOTE: kill_on_drop defaults to false; do not enable it here.

        let mut child = cmd.spawn().context("spawn golem server run")?;

        // Give it a brief moment to fail fast if something is wrong (e.g., bad flags/port in use).
        use tokio::time::{ sleep, Duration };
        sleep(Duration::from_millis(300)).await;

        // If it already exited, surface that as a failure/success; else, treat as launched.
        match child.try_wait()? {
            Some(status) => {
                let exit_code = status.code().unwrap_or(-1);
                return Ok(ToolCallResult {
                    ok: status.success(),
                    command: ExecutedCommand {
                        binary: "golem",
                        args: params.arguments.args.clone(),
                        cwd: workdir.display().to_string(),
                    },
                    logs: Vec::new(), // detached path: we did not capture logs
                    result: ToolResult { exit_code },
                });
            }
            None => {
                // Still running => report success and return immediately.
                // We don't have an exit code yet; use 0 to match "launched OK".
                return Ok(ToolCallResult {
                    ok: true,
                    command: ExecutedCommand {
                        binary: "golem",
                        args: params.arguments.args.clone(),
                        cwd: workdir.display().to_string(),
                    },
                    logs: Vec::new(),
                    result: ToolResult { exit_code: 0 },
                });
            }
        }
    }

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

// ---------- Available resources (dynamic) ---------------------------------------

fn parse_list_output(s: &str) -> serde_json::Value {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
        return v;
    }
    let lines: Vec<serde_json::Value> = s
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::json!({ "line": l }))
        .collect();
    serde_json::Value::Array(lines)
}

// ------------------ Two-pass enrichment for `available` on `list` subcommands ------------------

/// Pass 1: collect all subcommand paths that end with `list`.
fn collect_list_paths(
    tree: &Vec<SubcommandDescriptor>,
    prefix: &mut Vec<String>,
    out: &mut Vec<Vec<String>>
) {
    for sc in tree {
        prefix.push(sc.name.clone());
        if sc.name == "list" {
            out.push(prefix.clone());
        }
        if !sc.subcommands.is_empty() {
            collect_list_paths(&sc.subcommands, prefix, out);
        }
        prefix.pop();
    }
}

/// Pass 2 helper: navigate to a node by path and set its `available` field.
fn set_available_at_path(
    tree: &mut Vec<SubcommandDescriptor>,
    path: &[String],
    val: serde_json::Value
) -> bool {
    if path.is_empty() {
        return false;
    }
    let head = &path[0];
    for node in tree.iter_mut() {
        if &node.name == head {
            if path.len() == 1 {
                // We are at the target node
                node.available = Some(val);
                return true;
            } else {
                return set_available_at_path(&mut node.subcommands, &path[1..], val);
            }
        }
    }
    false
}

/// Driver: gather `list` paths, execute each `golem ... list --format json`, and attach to nodes.
async fn enrich_subcommands_available(
    state: &AppState,
    root: &str,
    subs: &mut Vec<SubcommandDescriptor>
) {
    // 1) collect all paths (relative to root) that end with "list"
    let mut paths: Vec<Vec<String>> = Vec::new();
    let mut prefix: Vec<String> = Vec::new();
    collect_list_paths(subs, &mut prefix, &mut paths);

    // 2) for each path, run the command and set the value
    for path in paths {
        if let Some(val) = available_for_path(state, root, &path).await {
            // the path we collected is the chain of subcommand names; set directly
            let _ = set_available_at_path(subs, &path, val);
        }
    }
}

/// Compute `available` for an arbitrary subcommand path (e.g. ["api","definition","list"]).
async fn available_for_path(
    state: &AppState,
    root: &str,
    full_path: &Vec<String>
) -> Option<serde_json::Value> {
    // Compose argv: <root> <segments-after-root> --format json
    // `full_path` typically starts with the first sub-level name; we prepend `root`.
    let mut argv: Vec<String> = Vec::new();
    argv.push(root.to_string());
    for seg in full_path {
        argv.push(seg.clone());
    }

    // Special case: tokens are under `golem cloud tokens list`.
    // If the path is ["tokens","list"] but root is not "cloud", turn it into ["cloud","tokens","list"].
    if
        root != "cloud" &&
        argv.len() >= 2 &&
        argv[1] == "tokens" &&
        argv.last().map(|s| s.as_str()) == Some("list")
    {
        argv.insert(1, "cloud".to_string());
    }

    argv.push("--format".to_string());
    argv.push("json".to_string());

    let params = ToolCallParams {
        name: argv[0].clone(),
        arguments: GolemRunInput {
            args: argv,
            cwd: Some(state.cwd.display().to_string()),
        },
    };
    match run_golem(state, params).await {
        Ok(res) if res.result.exit_code == 0 => {
            let mut out = String::new();
            for line in res.logs.iter().filter(|l| l.stream == "stdout") {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&line.line);
            }
            Some(parse_list_output(&out))
        }
        _ => None,
    }
}
// ---------- Allowed/Disallowed subcommands --------------------------------------

const DISALLOWED: &[&str] = &["system", "exec"];

fn disallowed_list() -> BTreeSet<String> {
    DISALLOWED.iter()
        .map(|s| s.to_string())
        .collect()
}

fn allowed_top_level_subcommands() -> BTreeSet<String> {
    clap_top_level_commands().keys().cloned().collect()
}

fn available_golem_commands() -> Vec<(String, String)> {
    // clap_top_level_commands() is a BTreeMap,
    // so iteration order is already sorted by command name.
    clap_top_level_commands().into_iter().collect()
}

/// Returns the real top-level commands and their about text
/// exactly like `golem --help` shows.
///
/// Example map entry:
///   "app" -> "Build, deploy application"
fn clap_top_level_commands() -> BTreeMap<String, String> {
    use clap::CommandFactory;
    use golem_cli::command;
    type Root = command::GolemCliCommand;

    let mut map = BTreeMap::new();

    for sc in Root::command().get_subcommands() {
        let name = sc.get_name().to_string();

        // Prefer `about`, else `long_about`, else "".
        // Convert StyledStr -> String explicitly.
        let desc = sc
            .get_about()
            .or_else(|| sc.get_long_about())
            .map(|styled| styled.to_string())
            .unwrap_or_else(|| "".to_string());

        map.insert(name, desc);
    }

    map
}

fn clap_subcommands_for(cmd_name: &str) -> Vec<SubcommandDescriptor> {
    use clap::CommandFactory;
    use golem_cli::command;
    type Root = command::GolemCliCommand;

    // Find the subcommand node by name at the root level
    let mut out = Vec::new();
    for sc in Root::command().get_subcommands() {
        if sc.get_name() == cmd_name {
            // Seed path with the top-level command (e.g. "agent", "app", "cloud", ...)
            out = collect_subs_with_path(sc, &vec![cmd_name.to_string()]);
            break;
        }
    }
    out
}

fn collect_subs_with_path(cmd: &clap::Command, path: &[String]) -> Vec<SubcommandDescriptor> {
    let mut v = Vec::new();
    for sc in cmd.get_subcommands() {
        let name = sc.get_name().to_string();
        let description = sc
            .get_about()
            .or_else(|| sc.get_long_about())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let mut this_path = path.to_vec();
        this_path.push(name.clone());

        // Parse only the Arguments: section into a JSON object.
        let arguments = match golem_help_arguments_json(&this_path) {
            Ok(val) if !val.as_object().map_or(true, |m| m.is_empty()) => Some(val),
            _ => None,
        };

        let subcommands = collect_subs_with_path(sc, &this_path); // recurse
        v.push(SubcommandDescriptor { name, description, available: None, arguments, subcommands });
    }
    v
}

// Given a workspace map of crate name -> manifest path
fn make_repo_resource(crate_name: &str, manifest_path: &str) -> RepoCrateResource {
    // Read Cargo.toml as raw text and optionally parse to toml::Value then to serde_json::Value
    let manifest_str = fs::read_to_string(manifest_path).unwrap_or_else(|_| String::new());

    // parse toml -> json for nicer structure
    let cargo_toml_val: serde_json::Value = toml
        ::from_str::<toml::Value>(&manifest_str)
        .map(|toml_val| serde_json::to_value(toml_val).unwrap())
        .unwrap_or_else(|_| json!({ "error": "unparseable Cargo.toml" }));

    RepoCrateResource {
        repo: crate_name.to_string(),
        cargo_toml: cargo_toml_val,
    }
}

// Build reverse index: command -> Vec<RepoCrateResource>
fn build_command_index() -> HashMap<String, Vec<RepoCrateResource>> {
    let output = StdCommand::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .expect("cargo metadata failed");

    let meta: CargoMetadata = serde_json
        ::from_slice(&output.stdout)
        .expect("invalid cargo metadata json");

    let mut index: HashMap<String, Vec<RepoCrateResource>> = HashMap::new();

    for pkg in meta.packages {
        let repo_name = pkg.name.clone();
        let manifest_path = pkg.manifest_path.clone();

        // Pre-build the RepoCrateResource so we don't re-read the file multiple times
        let repo_resource = make_repo_resource(&repo_name, &manifest_path);

        if
            let Some(md) = pkg.metadata
                .as_ref()
                .and_then(|m| m.golem_mcp.as_ref())
                .and_then(|g| g.commands.as_ref())
        {
            for cmd in md {
                index.entry(cmd.clone()).or_default().push(repo_resource.clone());
            }
        }
    }

    index
}

fn docs_for_command(idx: &DocsIndex, name: &str, desc: &str) -> Vec<DocLink> {
    let name_lc = name.to_lowercase();
    let desc_lc = desc.to_lowercase();

    // crude tokens from desc
    let mut tokens: Vec<&str> = desc_lc
        .split(|c: char| !c.is_alphanumeric() && c != '-')
        .filter(|t| t.len() > 2)
        .take(12)
        .collect();
    tokens.push(&name_lc);

    let mut out: Vec<DocLink> = Vec::new();

    for p in &idx.pages {
        let route_boost =
            (if p.route.starts_with("/cli") { 1.0 } else { 0.0 }) +
            (if p.route.contains("agent") && name_lc.contains("agent") { 0.5 } else { 0.0 }) +
            (if p.route.contains("component") && name_lc.contains("component") {
                0.5
            } else {
                0.0
            });

        // page-wide signals
        let mut page_hits = 0.0;
        if p.fulltext_lc.contains(&name_lc) {
            page_hits += 3.0;
        }
        if p.title.to_lowercase().contains(&name_lc) {
            page_hits += 2.0;
        }
        for t in &tokens {
            if p.fulltext_lc.contains(t) {
                page_hits += 0.2;
            }
        }
        page_hits += route_boost;

        // section-level links
        let mut section_links: Vec<DocLink> = Vec::new();
        for (anchor, text) in &p.sections {
            let tlc = text.to_lowercase();
            let mut score = 0.0;
            if tlc.contains(&name_lc) {
                score += 3.0;
            }
            if anchor.to_lowercase().contains(&name_lc) {
                score += 1.0;
            }
            for t in &tokens {
                if tlc.contains(t) {
                    score += 0.2;
                }
            }
            score += route_boost;

            if score > 0.0 {
                let url = if anchor.is_empty() {
                    format!("https://learn.golem.cloud{}", p.route)
                } else {
                    // Keep only the portion before the first '#'
                    let clean_anchor = anchor.split('#').next().unwrap_or(anchor);
                    format!("https://learn.golem.cloud{}#{}", p.route, clean_anchor)
                };
                section_links.push(DocLink {
                    title: if anchor.is_empty() {
                        p.title.clone()
                    } else {
                        // Split on '#' and keep the rightmost fragment
                        let clean_anchor = anchor
                            .split('#')
                            .last()
                            .unwrap_or(anchor)
                            .trim()
                            .to_string();

                        // Capitalize first letter if present
                        let clean_anchor = clean_anchor
                            .chars()
                            .enumerate()
                            .map(|(i, c)| if i == 0 { c.to_ascii_uppercase() } else { c })
                            .collect::<String>();
                        format!("{} · {}", p.title, clean_anchor)
                    },
                    url,
                    score,
                });
            }
        }

        if section_links.is_empty() && page_hits > 0.5 {
            out.push(DocLink {
                title: p.title.clone(),
                url: format!("https://learn.golem.cloud{}", p.route),
                score: page_hits,
            });
        } else {
            out.extend(section_links);
        }
    }

    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out.dedup_by(|a, b| a.url == b.url);
    out.truncate(5);
    out
}
async fn load_docs_index_from_remote_with_cache(cache_root: &Path) -> anyhow::Result<DocsIndex> {
    // Ensure cache dir exists
    let cache_dir = cache_root.join(".cache");
    if let Err(e) = aiofs::create_dir_all(&cache_dir).await {
        // Don't fail hard if the directory can't be created
        tracing::warn!("failed to create cache dir {:?}: {e}", cache_dir);
    }

    let cache_path = cache_root.join(NEXTRA_CACHE_FILE);

    // 1) Try fresh cache
    if let Some(idx) = try_read_fresh_cache(&cache_path).await? {
        return Ok(idx);
    }

    // 2) Fetch from network
    let client = reqwest::Client::new();
    let resp = client.get(NEXTRA_URL).send().await?;
    if !resp.status().is_success() {
        // If network failed, fall back to stale cache (if any)
        if let Ok(bytes) = aiofs::read(&cache_path).await {
            if let Ok(idx) = parse_nextra_bytes(&bytes) {
                tracing::warn!("network {} -> {}, serving STALE cache", NEXTRA_URL, resp.status());
                return Ok(idx);
            }
        }
        anyhow::bail!("failed to fetch nextra data: HTTP {}", resp.status());
    }

    let bytes = resp.bytes().await?.to_vec();

    // 3) Parse
    let idx = parse_nextra_bytes(&bytes)?;

    // 4) Write cache (best effort)
    if let Err(e) = aiofs::write(&cache_path, &bytes).await {
        tracing::warn!("failed to write nextra cache {:?}: {e}", cache_path);
    }

    Ok(idx)
}

async fn try_read_fresh_cache(cache_path: &Path) -> anyhow::Result<Option<DocsIndex>> {
    if let Ok(meta) = aiofs::metadata(cache_path).await {
        if let Ok(modified) = meta.modified() {
            if let Ok(age) = modified.elapsed() {
                if age.as_secs() <= NEXTRA_CACHE_TTL_SECS {
                    if let Ok(bytes) = aiofs::read(cache_path).await {
                        if let Ok(idx) = parse_nextra_bytes(&bytes) {
                            return Ok(Some(idx));
                        }
                    }
                }
            }
        }
    }
    Ok(None)
}

fn parse_nextra_bytes(bytes: &[u8]) -> anyhow::Result<DocsIndex> {
    let raw: std::collections::HashMap<String, NextraEntry> = serde_json::from_slice(bytes)?;
    let mut pages = Vec::with_capacity(raw.len());

    for (route, entry) in raw {
        let mut body = String::new();
        let mut sections = Vec::with_capacity(entry.data.len());
        for (anchor, text) in entry.data {
            sections.push((anchor.clone(), text.clone()));
            body.push('\n');
            body.push_str(&text);
        }
        let fulltext_lc = format!("{}\n{}", entry.title, body).to_lowercase();
        pages.push(DocPage { route, title: entry.title, sections, fulltext_lc });
    }

    Ok(DocsIndex { pages })
}

/// Runs `golem <path> --help` and returns ONLY the `Arguments:` section
/// as a JSON object: { "<ARG_NAME>": "<description, possibly multi-line>" }
fn golem_help_arguments_json(path: &[String]) -> anyhow::Result<serde_json::Value> {
    use std::process::{ Command, Stdio };
    let mut argv: Vec<String> = path.to_vec();
    argv.push("--help".to_string());

    let out = Command::new("golem")
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    // Prefer stdout; clap writes help to stdout
    let help = if !out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stdout).to_string()
    } else {
        String::from_utf8_lossy(&out.stderr).to_string()
    };
    Ok(parse_arguments_section(&help))
}

/// Extract only the `Arguments:` block and convert to { name: description } JSON.
/// - Keys have `< >` and `[ ]` removed; ellipses ("...") are preserved.
/// - Multi-line descriptions (indented or bulleted) are appended to the previous key, separated by `\n`.
/// - Parsing stops when a new section header is reached (e.g., "Options:", "Flags:", "Subcommands:", "Commands:").
fn parse_arguments_section(help: &str) -> serde_json::Value {
    use serde_json::Value;
    let mut map = serde_json::Map::new();

    // Find start of "Arguments:" section
    let start = match help.find("Arguments:") {
        Some(i) => i + "Arguments:".len(),
        None => {
            return Value::Object(map);
        } // no arguments section
    };

    let mut current_key: Option<String> = None;
    let mut current_desc: String = String::new();

    for raw_line in help[start..].lines() {
        let line = raw_line.trim_end();
        let trimmed = line.trim_start();

        // Stop at next major section
        if
            trimmed.starts_with("Options:") ||
            trimmed.starts_with("Flags:") ||
            trimmed.starts_with("Subcommands:") ||
            trimmed.starts_with("Commands:")
        {
            break;
        }

        // Blank line: keep as paragraph break if we're inside a description
        if trimmed.is_empty() {
            if current_key.is_some() {
                current_desc.push('\n');
            }
            continue;
        }

        // Start of a new argument row: begins with "<" or "[" after indentation
        if trimmed.starts_with('<') || trimmed.starts_with('[') {
            // Commit previous argument (if any)
            if let Some(k) = current_key.take() {
                map.insert(k, Value::String(current_desc.trim_end().to_string()));
                current_desc.clear();
            }

            // Split into "arg spec" and first-line description by 2+ spaces
            let (arg_spec, first_desc) = match split_two_or_more_spaces(trimmed) {
                Some((a, d)) => (a, d),
                None => (trimmed, ""), // defensive
            };

            // Normalize key: strip brackets but keep ellipsis (e.g., "[ARGUMENTS]..." -> "ARGUMENTS...")
            let key = arg_spec
                .trim()
                .trim_matches(|c| (c == '<' || c == '>' || c == '[' || c == ']'))
                .to_string();

            current_key = if key.is_empty() { None } else { Some(key) };

            if !first_desc.is_empty() {
                current_desc.push_str(first_desc.trim());
            }
        } else {
            // Continuation line: append to current description
            if current_key.is_some() {
                let cont = trimmed;
                // Keep bullets nicely; de-indent by two spaces if present
                if cont.starts_with("- ") || cont.starts_with('*') {
                    current_desc.push('\n');
                    current_desc.push_str(cont);
                } else {
                    // normalize common 2-space indent on wrapped lines
                    let deindented = cont.strip_prefix("  ").unwrap_or(cont);
                    current_desc.push('\n');
                    current_desc.push_str(deindented);
                }
            }
        }
    }

    // Commit final pending key
    if let Some(k) = current_key.take() {
        map.insert(k, serde_json::Value::String(current_desc.trim_end().to_string()));
    }

    serde_json::Value::Object(map)
}

/// Helper: split a line into (left, right) on the first run of 2+ spaces.
fn split_two_or_more_spaces(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b' ' && bytes[i + 1] == b' ' {
            // Find end of the run of spaces
            let mut j = i + 2;
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            let (left, right) = s.split_at(i);
            return Some((left, &right[j - i..]));
        }
        i += 1;
    }
    None
}
