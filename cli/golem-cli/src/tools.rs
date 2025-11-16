use std::path::{ Path, PathBuf };
use std::process::Stdio;

use rust_mcp_sdk::schema::{ schema_utils::CallToolError, CallToolResult, TextContent };
use rust_mcp_sdk::{ macros::{ mcp_tool, JsonSchema }, tool_box };

use serde::{ Deserialize, Serialize };
use tokio::{
    io::{ AsyncBufReadExt, BufReader },
    process::Command,
    task::JoinSet,
    time::{ sleep, Duration },
};
use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;
use std::sync::OnceLock; // Rust std version of OnceCell
use tokio::sync::Mutex;

static GOLEM_LOGS: OnceLock<Arc<Mutex<Vec<LogLine>>>> = OnceLock::new();

// ======================================================================================
// Local types
// ======================================================================================

#[derive(Serialize, Clone)]
struct ToolResources {
    #[serde(rename = "relevant_repos")]
    pub relevant_repos: Vec<RepoCrateResource>,
    pub docs: Vec<DocLink>,
}

#[derive(Serialize, Clone)]
struct DocLink {
    pub title: String,
    pub url: String,
    pub score: f32,
}

#[derive(Serialize, Clone)]
struct RepoCrateResource {
    pub repo: String,
    #[serde(rename = "cargo_toml")]
    pub cargo_toml: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
}

#[derive(Serialize, Clone)]
struct SubcommandDescriptor {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subcommands: Vec<SubcommandDescriptor>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub usage: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub arguments: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, String>,
}

#[derive(Serialize, Clone)]
struct ToolDescriptor {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subcommands: Vec<SubcommandDescriptor>,
    pub other: ToolResources,
}

#[derive(Serialize)]
struct ToolsListResult {
    pub tools: Vec<ToolDescriptor>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecutedCommand {
    pub argv: Vec<String>,
    pub cwd: String,
}

#[derive(Debug, Serialize)]
#[derive(Clone)]
struct LogLine {
    pub stream: &'static str, // "stdout" | "stderr"
    pub line: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolResult {
    pub exit_code: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolCallResultPayload {
    ok: bool,
    command: ExecutedCommand,
    logs: Vec<LogLine>,
    result: ToolResult,
}

// ======================================================================================
// Helpers
// ======================================================================================

fn clap_top_level_commands() -> std::collections::BTreeMap<String, String> {
    use clap::CommandFactory;
    use golem_cli::command;
    type Root = command::GolemCliCommand;

    let mut map = std::collections::BTreeMap::<String, String>::new();
    for sc in Root::command().get_subcommands() {
        let name = sc.get_name().to_string();
        let desc = sc
            .get_about()
            .or_else(|| sc.get_long_about())
            .map(|styled| styled.to_string())
            .unwrap_or_else(|| "".to_string());
        map.insert(name, desc);
    }
    map
}

fn collect_subs_with_path(node: &clap::Command, _path: &Vec<String>) -> Vec<SubcommandDescriptor> {
    let mut out = Vec::new();
    for sc in node.get_subcommands() {
        let name = sc.get_name().to_string();
        let desc = sc
            .get_about()
            .or_else(|| sc.get_long_about())
            .map(|s| s.to_string())
            .unwrap_or_default();

        let subs = collect_subs_with_path(sc, &vec![]);

        // Usage comes from clap; if not a leaf we skip it (kept empty)
        let usage = if subs.is_empty() {
            // `render_usage()` needs &mut self → clone and make it mutable.
            let mut tmp = sc.clone();
            tmp.render_usage().to_string()
        } else {
            String::new()
        };

        let (arguments, options) = if subs.is_empty() {
            collect_maps_for(sc)
        } else {
            (BTreeMap::new(), BTreeMap::new())
        };

        out.push(SubcommandDescriptor {
            name,
            description: desc,
            subcommands: subs,
            usage,
            arguments,
            options,
        });
    }
    out
}

fn to_upper_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch == '-' || ch == ' ' {
            out.push('_');
        } else {
            out.push(ch.to_ascii_uppercase());
        }
    }
    out
}

// Split command params into (positional arguments, options/flags)
fn collect_maps_for(node: &clap::Command) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut args = BTreeMap::new();
    let mut opts = BTreeMap::new();

    for a in node.get_arguments() {
        #[allow(deprecated)]
        if a.is_hide_set() {
            continue;
        }

        let desc = a
            .get_long_help()
            .or_else(|| a.get_help())
            .map(|s| s.to_string())
            .unwrap_or_default();

        // Heuristic for positional: has an index and no long/short names
        let is_positional =
            a.get_index().is_some() && a.get_long().is_none() && a.get_short().is_none();

        if is_positional {
            let key = to_upper_snake(a.get_id().as_str());
            if !key.is_empty() {
                args.insert(key, desc);
            }
        } else {
            let key = if let Some(long) = a.get_long() {
                format!("--{}", long)
            } else if let Some(short) = a.get_short() {
                format!("-{}", short)
            } else {
                to_upper_snake(a.get_id().as_str())
            };
            if !key.is_empty() {
                opts.insert(key, desc);
            }
        }
    }

    (args, opts)
}

fn clap_subcommands_for(cmd_name: &str) -> Vec<SubcommandDescriptor> {
    use clap::CommandFactory;
    use golem_cli::command;
    type Root = command::GolemCliCommand;

    for sc in Root::command().get_subcommands() {
        if sc.get_name() == cmd_name {
            return collect_subs_with_path(sc, &vec![cmd_name.to_string()]);
        }
    }
    Vec::new()
}

pub async fn run_golem_process(
    argv: Vec<String>,
    cwd: &Path
) -> Result<ToolCallResultPayload, String> {
    let workdir = cwd.to_path_buf();

    let (first, rest) = argv.split_first().ok_or_else(|| "empty argv".to_string())?;

    // --- Detached path: `golem server run ...` ---
    let is_server_run =
        first == "server" &&
        rest
            .first()
            .map(|s| s.as_str() == "run")
            .unwrap_or(false);

    if is_server_run {
        // --- Case 1: server already running (we have a buffer) ---
        if let Some(buf) = GOLEM_LOGS.get() {
            let logs_guard = buf.lock().await;
            let logs_vec: Vec<LogLine> = logs_guard.clone(); // LogLine: Clone

            return Ok(ToolCallResultPayload {
                ok: true,
                command: ExecutedCommand {
                    argv: argv.clone(),
                    cwd: workdir.display().to_string(),
                },
                logs: logs_vec,
                result: ToolResult { exit_code: 0 },
            });
        }

        // --- Case 2: no server yet -> start it, capture logs ---

        let mut cmd = Command::new("golem");
        cmd.current_dir(&workdir);
        cmd.args(std::iter::once(first).chain(rest.iter()));

        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        // IMPORTANT: don't set kill_on_drop(true) here – we want it to live

        let mut child = cmd.spawn().map_err(|e| format!("spawn `golem server run`: {e}"))?;

        // shared buffer of LogLine (stdout+stderr)
        let buffer = Arc::new(Mutex::new(Vec::<LogLine>::new()));
        let _ = GOLEM_LOGS.set(buffer.clone()); // ignore Err on race

        // --- stdout reader ---
        if let Some(stdout) = child.stdout.take() {
            let buf_clone = buffer.clone();
            tokio::spawn(async move {
                use tokio::io::{ AsyncBufReadExt, BufReader };
                let mut reader = BufReader::new(stdout).lines();

                while let Ok(Some(line)) = reader.next_line().await {
                    // mirror to terminal
                    println!("{line}");
                    // store
                    buf_clone.lock().await.push(LogLine {
                        stream: "stdout",
                        line,
                    });
                }
            });
        }

        // --- stderr reader (THIS is where most logs probably are) ---
        if let Some(stderr) = child.stderr.take() {
            let buf_clone = buffer.clone();
            tokio::spawn(async move {
                use tokio::io::{ AsyncBufReadExt, BufReader };
                let mut reader = BufReader::new(stderr).lines();

                while let Ok(Some(line)) = reader.next_line().await {
                    // also mirror to terminal (optional: prefix with "ERR: ")
                    eprintln!("{line}");
                    // store
                    buf_clone.lock().await.push(LogLine {
                        stream: "stderr",
                        line,
                    });
                }
            });
        }

        // avoid zombies: wait on child in background, but we never block on it here
        tokio::spawn(async move {
            let _ = child.wait().await;
        });

        // ️Wait a bit so startup logs have time to arrive
        // Golem startup logs often take >1s, so let's wait up to ~5s for *any* log.
        let timeout = Duration::from_secs(5);
        let start = std::time::Instant::now();

        loop {
            {
                let logs_guard = buffer.lock().await;
                if !logs_guard.is_empty() {
                    break;
                }
            }
            if start.elapsed() >= timeout {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }

        // Return whatever we've collected so far (maybe still empty if it stayed silent)
        let logs_guard = buffer.lock().await;
        let logs_vec: Vec<LogLine> = logs_guard.clone();

        return Ok(ToolCallResultPayload {
            ok: true,
            command: ExecutedCommand {
                argv: argv.clone(),
                cwd: workdir.display().to_string(),
            },
            logs: logs_vec,
            result: ToolResult { exit_code: 0 },
        });
    }

    // --- End detached path ---

    // --- Semi-attached path: `golem agent stream <AGENT_ID>` ---

    let is_agent_stream =
        first == "agent" &&
        rest
            .first()
            .map(|s| s.as_str() == "stream")
            .unwrap_or(false);

    if is_agent_stream {
        // This command streams forever. We want to capture some logs, then return.
        let mut cmd = Command::new("golem");
        cmd.current_dir(&workdir);
        cmd.args(std::iter::once(first).chain(rest.iter()));
        cmd.kill_on_drop(true);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| format!("spawn golem agent stream: {e}"))?;

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

        // How long we let it stream before we cut it off.
        // Adjust this as you like (seconds, lines, etc.).
        let timeout = Duration::from_secs(5);

        let status_opt = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(res) => Some(res.map_err(|e| format!("wait: {e}"))?),
            Err(_) => {
                // Timed out: stop the streaming process so our readers finish.
                let _ = child.kill().await;
                None
            }
        };

        // Drain the reader tasks (they stop when pipes close).
        while let Some(joined) = set.join_next().await {
            if let Ok(mut part) = joined {
                logs.append(&mut part);
            }
        }

        let (ok, exit_code) = match status_opt {
            Some(status) => (status.success(), status.code().unwrap_or(-1)),
            // Timed out but streaming was fine; treat as "launched OK".
            None => (true, 0),
        };

        return Ok(ToolCallResultPayload {
            ok,
            command: ExecutedCommand {
                argv: argv.clone(),
                cwd: workdir.display().to_string(),
            },
            logs,
            result: ToolResult { exit_code },
        });
    }

    // --- End Semi-attached path

        // --- Semi-detached path: `golem api definition swagger ...` ---

    let is_swagger_ui =
        first == "api"
        && rest
            .get(0)
            .map(|s| s.as_str() == "definition")
            .unwrap_or(false)
        && rest
            .get(1)
            .map(|s| s.as_str() == "swagger")
            .unwrap_or(false);

    if is_swagger_ui {
        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::time::sleep;

        // We WANT this process to live beyond the request,
        // so do NOT set kill_on_drop(true).
        let mut cmd = Command::new("golem");
        cmd.current_dir(&workdir);
        cmd.args(std::iter::once(first).chain(rest.iter()));
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawn `golem api definition swagger`: {e}"))?;

        // Local buffer for this swagger invocation
        let buffer = Arc::new(Mutex::new(Vec::<LogLine>::new()));

        // stdout reader
        if let Some(stdout) = child.stdout.take() {
            let buf_clone = buffer.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    println!("{line}");
                    buf_clone.lock().await.push(LogLine {
                        stream: "stdout".into(),
                        line,
                    });
                }
            });
        }

        // stderr reader
        if let Some(stderr) = child.stderr.take() {
            let buf_clone = buffer.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    eprintln!("{line}");
                    buf_clone.lock().await.push(LogLine {
                        stream: "stderr".into(),
                        line,
                    });
                }
            });
        }

        // Reap child in background to avoid zombies
        tokio::spawn(async move {
            let _ = child.wait().await;
        });

        // Give Swagger some time to start and print:
        // Selected profile: local
        // Browser opened successfully.
        // ╔═
        // ║ Swagger UI running at http://localhost:9007
        // ║ API is deployed at 1 locations
        // ╚═
        let timeout = Duration::from_secs(5); // tweak as needed
        sleep(timeout).await;

        let logs = {
            let guard = buffer.lock().await;
            guard.clone()
        };

        return Ok(ToolCallResultPayload {
            ok: true, // we successfully launched swagger
            command: ExecutedCommand {
                argv: argv.clone(),
                cwd: workdir.display().to_string(),
            },
            logs,
            // This is a "launch" result; the server is still running,
            // so we don't have a real exit code yet.
            result: ToolResult { exit_code: 0 },
        });
    }

    // --- End swagger semi-detached path ---


    // Generic (attached) path: capture output and wait for completion.
    let mut cmd = Command::new("golem");
    cmd.current_dir(&workdir);
    cmd.args(std::iter::once(first).chain(rest.iter()));
    cmd.kill_on_drop(true);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("spawn golem: {e}"))?;

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

    let status = child.wait().await.map_err(|e| format!("wait: {e}"))?;
    while let Some(joined) = set.join_next().await {
        if let Ok(mut part) = joined {
            logs.append(&mut part);
        }
    }

    let exit_code = status.code().unwrap_or(-1);

    Ok(ToolCallResultPayload {
        ok: exit_code == 0,
        command: ExecutedCommand {
            argv,
            cwd: workdir.display().to_string(),
        },
        logs,
        result: ToolResult { exit_code },
    })
}

/// Always return Ok(CallToolResult) with JSON string content.
/// If serialization fails, we still return a JSON error string instead of Err(CallToolError).
fn json_ok<T: Serialize>(val: &T) -> Result<CallToolResult, CallToolError> {
    let s = match serde_json::to_string(val) {
        Ok(s) => s,
        Err(e) => format!(r#"{{"ok":false,"error":"serialization error: {}"}}"#, e),
    };
    Ok(CallToolResult::text_content(vec![TextContent::new(s, None, None)]))
}

// ======================================================================================
//  ListToolsTool
// ======================================================================================

#[mcp_tool(name = "list_tools", description = "List available golem subcommands with metadata.")]
#[derive(Debug, ::serde::Deserialize, ::serde::Serialize, JsonSchema)]
pub struct ListToolsTool {}

impl ListToolsTool {
    pub async fn call_tool(&self) -> Result<CallToolResult, CallToolError> {
        let cmds_with_descs: Vec<(String, String)> = clap_top_level_commands()
            .into_iter()
            .collect();

        let mut tools: Vec<ToolDescriptor> = Vec::new();
        for (cmd_name, desc) in cmds_with_descs.into_iter() {
            let subs = clap_subcommands_for(&cmd_name);
            tools.push(ToolDescriptor {
                name: cmd_name,
                description: desc,
                subcommands: subs,
                other: ToolResources {
                    relevant_repos: vec![],
                    docs: vec![],
                },
            });
        }

        json_ok(&(ToolsListResult { tools }))
    }
}

// ======================================================================================
//  CallToolTool
// ======================================================================================

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemRunInput {
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[mcp_tool(name = "call_tool", description = "Execute a golem subcommand with arguments.")]
#[derive(Debug, ::serde::Deserialize, ::serde::Serialize, JsonSchema)]
pub struct CallToolTool {
    pub name: String,
    pub arguments: GolemRunInput,
}

impl CallToolTool {
    pub async fn call_tool(&self) -> Result<CallToolResult, CallToolError> {
        // Build argv: [name, ...args]
        let mut argv = Vec::with_capacity(1 + self.arguments.args.len());
        argv.push(self.name.clone());
        argv.extend(self.arguments.args.clone());

        // Resolve cwd without generating CallToolError
        let cwd = self.arguments.cwd
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        match run_golem_process(argv, &cwd).await {
            Ok(payload) => json_ok(&payload),
            Err(msg) => {
                let fallback = ToolCallResultPayload {
                    ok: false,
                    command: ExecutedCommand {
                        argv: vec![self.name.clone()],
                        cwd: cwd.display().to_string(),
                    },
                    logs: vec![LogLine {
                        stream: "stderr",
                        line: msg,
                    }],
                    result: ToolResult { exit_code: -1 },
                };
                json_ok(&fallback)
            }
        }
    }
}

// ======================================================================================
//  ListResourcesTool
// ======================================================================================

#[derive(Serialize)]
struct ResourceNode {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ResourceNode>,
}

#[derive(Serialize)]
struct ListResourcesResult {
    pub resources: Vec<ResourceNode>,
}

fn to_resource_tree(cmd_name: &str, subs: &Vec<SubcommandDescriptor>) -> ResourceNode {
    fn build(nodes: &Vec<SubcommandDescriptor>) -> Vec<ResourceNode> {
        let mut out = Vec::new();
        for sc in nodes {
            let children = build(&sc.subcommands);
            let available = if sc.name == "list" { Some(serde_json::Value::Null) } else { None };
            out.push(ResourceNode {
                name: sc.name.clone(),
                available,
                children,
            });
        }
        out
    }

    ResourceNode {
        name: cmd_name.to_string(),
        available: None,
        children: build(subs),
    }
}

#[mcp_tool(
    name = "list_resources",
    description = "Recursively list directories that contain YAML manifests. Returns a nested map where each dir either maps to a manifest filename (if present directly) or to child-dir maps."
)]
#[derive(Debug, ::serde::Deserialize, ::serde::Serialize, JsonSchema)]
pub struct ListResourcesTool {
    /// Optional working directory to start from. If not provided, uses the process current dir, you may need to supply (switch to such directory) this if such command fails.
    #[serde(default)]
    pub cwd: Option<String>,
}

impl ListResourcesTool {
    pub async fn call_tool(&self) -> Result<CallToolResult, CallToolError> {
        // Reuse your shared crawler; keeps output EXACTLY as before:
        let v = crate::tools::discover_manifest_tree(self.cwd.as_deref());
        // Wrap as a JSON tool result using your existing helper
        Ok(crate::tools::json_ok(&v)?)
    }
}

// --- NEW: tiny public helpers ---------------------------

fn is_yaml(p: &Path) -> bool {
    match p.extension().and_then(|s| s.to_str()) {
        Some(ext) => {
            let ext = ext.to_ascii_lowercase();
            ext == "yaml" || ext == "yml"
        }
        None => false,
    }
}

/// Build the nested manifest tree for a given directory.
/// Returns a serde_json::Value shaped like:
/// {"example":"a.yaml","nested":{"child":"b.yml"}, ...}
fn build_tree(dir: &Path) -> Option<serde_json::Value> {
    let mut direct_manifests: Vec<String> = Vec::new();
    let mut child_map: BTreeMap<String, serde_json::Value> = BTreeMap::new();

    let entries = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => {
            return None;
        }
    };

    // Collect first for stable output
    let mut files: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for e in entries.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            // skip hidden dirs
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }
            dirs.push(p);
        } else {
            files.push(p);
        }
    }
    files.sort();
    dirs.sort();

    for p in files {
        if is_yaml(&p) {
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                direct_manifests.push(name.to_string());
            }
        }
    }

    for d in dirs {
        if let Some(child_val) = build_tree(&d) {
            if let Some(name) = d.file_name().and_then(|s| s.to_str()) {
                child_map.insert(name.to_string(), child_val);
            }
        }
    }

    if !child_map.is_empty() {
        Some(serde_json::to_value(child_map).ok()?)
    } else if let Some(first) = direct_manifests.first() {
        Some(serde_json::Value::String(first.clone()))
    } else {
        None
    }
}

/// NEW: expose the top-level discovery as a public helper.
/// Keeps your original JSON *exactly as-is*.
pub fn discover_manifest_tree(cwd: Option<&str>) -> serde_json::Value {
    let root = cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let mut top_map: BTreeMap<String, serde_json::Value> = BTreeMap::new();

    let rd = match fs::read_dir(&root) {
        Ok(rd) => rd,
        Err(e) => {
            return serde_json::json!({
                "ok": false,
                "error": format!("failed to read dir {}: {e}", root.display())
            });
        }
    };

    let mut dirs: Vec<PathBuf> = Vec::new();
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }
            dirs.push(p);
        }
    }
    dirs.sort();

    for d in dirs {
        if let Some(val) = build_tree(&d) {
            if let Some(name) = d.file_name().and_then(|s| s.to_str()) {
                top_map.insert(name.to_string(), val);
            }
        }
    }

    serde_json
        ::to_value(top_map)
        .unwrap_or_else(|e| {
            serde_json::json!({ "ok": false, "error": format!("serialization error: {e}") })
        })
}

// ======================================================================================
//  CoreTools enum
// ======================================================================================

tool_box!(CoreTools, [ListToolsTool, CallToolTool, ListResourcesTool]);
