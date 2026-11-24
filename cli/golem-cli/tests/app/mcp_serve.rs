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

//! `golem mcp serve` against a local server, driven as a child process by an MCP client.

use super::{TestContext, cmd, flag};
use crate::{Tracing, workspace_path};
use golem_cli::command_table::{COMMAND_TABLE, CommandClass, CommandRow};
use golem_cli::config::{AuthenticationConfig, Config, Profile, ProfileDeletion, ProfileName};
use golem_cli::{fs, versions};
use golem_client::api::{RegistryServiceClient, RegistryServiceClientLive};
use golem_common::model::environment::{EnvironmentName, EnvironmentUpdate};
use indoc::{formatdoc, indoc};
use rmcp::model::{CallToolRequest, CallToolRequestParams, CallToolResult, ClientRequest, Tool};
use rmcp::service::{PeerRequestOptions, RequestHandle, RunningService};
use rmcp::transport::TokioChildProcess;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::process::Stdio;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

inherit_test_dep!(Tracing);

const APP: &str = "mcp-serve";

/// The manifest of the counters component, with `component` added to the component's properties
/// and `local` added to the `local` environment's properties, both as YAML at four spaces.
fn write_manifest(ctx: &TestContext, component: &str, local: &str) {
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {MANIFEST_VERSION}

            app: {APP}

            components:
              it:agent-counters:
                componentWasm: component.wasm
                presets:
                  release: {{}}
            {component}
            environments:
              local:
                server: local
                componentPresets: release
            {local}
              staging:
                server: local
                componentPresets: release
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST},
    )
    .unwrap();
}

/// A local server with the counters component deployed to each of `environments`.
async fn counters_context(environments: &[&str]) -> TestContext {
    counters_context_with(environments, "", &[]).await
}

/// [`counters_context`] with `local` added to the `local` environment of the manifest, and with
/// the environment variables `env` for the server and the CLI.
async fn counters_context_with(
    environments: &[&str],
    local: &str,
    env: &[(&str, &str)],
) -> TestContext {
    let mut ctx = TestContext::new();
    for (key, value) in env {
        ctx.add_env_var(*key, *value);
    }
    let fixture = workspace_path().join("test-components/it_agent_counters_release.wasm");
    assert!(
        fixture.is_file(),
        "build test-components/agent-counters before running the MCP tests"
    );
    fs::copy(&fixture, ctx.cwd_path_join("component.wasm")).unwrap();
    write_manifest(&ctx, "", local);

    ctx.start_server().await;
    for environment in environments {
        let outputs = ctx.cli(["-E", environment, cmd::DEPLOY, flag::YES]).await;
        assert!(outputs.success_or_dump());
    }
    ctx
}

async fn invoke(ctx: &TestContext, environment: &str, agent: &str) {
    let outputs = ctx
        .cli([
            "-E",
            environment,
            cmd::AGENT,
            cmd::INVOKE,
            agent,
            "increment",
            flag::FORMAT,
            "json",
            "--no-stream",
        ])
        .await;
    assert!(outputs.success_or_dump());
}

/// The JSON documents that the CLI prints for `args` with `--format json`.
async fn cli_json(ctx: &TestContext, args: &[&str]) -> Vec<Value> {
    let mut command = args.to_vec();
    command.extend([flag::FORMAT, "json"]);
    let outputs = ctx.cli(command).await;
    assert!(outputs.success_or_dump());
    outputs.stdout_json()
}

fn server_command(ctx: &TestContext, global_args: &[&str]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(&ctx.golem_cli_path);
    command
        .arg("--config-dir")
        .arg(ctx.config_dir.path())
        .args(global_args)
        .args(["mcp", "serve"])
        .env_remove("GOLEM_BUILTIN_LOCAL_URL")
        .envs(&ctx.env)
        .current_dir(fs::absolute_lexical_path(&ctx.working_dir).unwrap())
        .kill_on_drop(true);
    command
}

pub(super) async fn mcp_client(
    ctx: &TestContext,
    global_args: &[&str],
) -> RunningService<RoleClient, ()> {
    let (transport, _) = TokioChildProcess::builder(server_command(ctx, global_args))
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    ().serve(transport).await.unwrap()
}

pub(super) async fn call(
    client: &RunningService<RoleClient, ()>,
    tool: &str,
    arguments: Value,
) -> CallToolResult {
    let Value::Object(arguments) = arguments else {
        panic!("tool arguments must be an object");
    };
    client
        .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(arguments))
        .await
        .unwrap()
}

/// Sends a tool call that the test can cancel.
async fn send_call(
    client: &RunningService<RoleClient, ()>,
    tool: &str,
    arguments: Value,
) -> RequestHandle<RoleClient> {
    let Value::Object(arguments) = arguments else {
        panic!("tool arguments must be an object");
    };
    client
        .send_cancellable_request(
            ClientRequest::CallToolRequest(CallToolRequest::new(
                CallToolRequestParams::new(tool.to_string()).with_arguments(arguments),
            )),
            PeerRequestOptions::no_options(),
        )
        .await
        .unwrap()
}

pub(super) fn structured(result: &CallToolResult) -> &Value {
    assert_ne!(result.is_error, Some(true), "{result:?}");
    result
        .structured_content
        .as_ref()
        .expect("structured content")
}

/// The error envelope of a failed call: JSON text content, and no structured content, which
/// would have to match the tool's output schema.
fn error(result: &CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(true), "{result:?}");
    assert!(result.structured_content.is_none(), "{result:?}");
    let [content] = result.content.as_slice() else {
        panic!("not one content block: {result:?}");
    };
    let text = &content.as_text().expect("text content").text;
    serde_json::from_str(text).unwrap()
}

fn error_code(result: &CallToolResult) -> String {
    error(result)["error"].as_str().unwrap().to_string()
}

fn error_message(result: &CallToolResult) -> String {
    error(result)["payload"]["message"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Every page that `tool` returns for `arguments`, from the first one, each call passing the
/// `cursor` of the previous page as `cursor_argument`.
async fn follow_pages(
    client: &RunningService<RoleClient, ()>,
    tool: &str,
    mut arguments: Value,
    cursor_argument: &str,
) -> Vec<Value> {
    let mut pages = Vec::new();
    loop {
        let page = structured(&call(client, tool, arguments.clone()).await).clone();
        let cursor = page.get("cursor").filter(|value| !value.is_null()).cloned();
        pages.push(page);
        let Some(cursor) = cursor else {
            return pages;
        };
        arguments[cursor_argument] = cursor;
        assert!(pages.len() < 1000, "`{tool}` does not reach its last page");
    }
}

fn items<'a>(pages: &'a [Value], field: &str) -> Vec<&'a Value> {
    pages
        .iter()
        .flat_map(|page| page[field].as_array().unwrap())
        .collect()
}

fn tool_name(row: &CommandRow) -> String {
    row.path.join("_").replace('-', "_")
}

fn properties(tool: &Tool) -> BTreeSet<String> {
    tool.input_schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect()
}

/// Removes what differs between two reads of the same state: the time of the read and the live
/// fields of an agent's metadata. Those come from the executor's status record, which can change
/// while the agent is idle, such as when it grows its memory or is suspended.
fn comparable(value: &Value) -> Value {
    const TIME_FIELDS: &[&str] = &["generatedAt"];
    const LIVE_AGENT_FIELDS: &[&str] =
        &["totalLinearMemorySize", "status", "pendingInvocationCount"];
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| !TIME_FIELDS.contains(&key.as_str()))
                .filter(|(key, _)| {
                    !(object.contains_key("totalLinearMemorySize")
                        && LIVE_AGENT_FIELDS.contains(&key.as_str()))
                })
                .map(|(key, value)| (key.clone(), comparable(value)))
                .collect::<Map<_, _>>(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(comparable).collect()),
        value => value.clone(),
    }
}

#[test]
#[timeout("10 minutes")]
async fn mcp_serve_offers_one_tool_per_management_command() {
    let ctx = counters_context(&["local"]).await;
    let client = mcp_client(&ctx, &["-E", "local"]).await;
    let tools = client.list_all_tools().await.unwrap();

    let offered = tools
        .iter()
        .map(|tool| tool.name.to_string())
        .collect::<BTreeSet<_>>();
    let mut expected = COMMAND_TABLE
        .iter()
        .filter(|row| !row.interactive)
        .map(tool_name)
        .collect::<BTreeSet<_>>();
    // The typed tools, which no command row has.
    expected.insert("agent_oplog_export".to_string());
    assert_eq!(offered, expected);
    for row in COMMAND_TABLE.iter().filter(|row| row.interactive) {
        assert!(!offered.contains(&tool_name(row)), "{}", tool_name(row));
    }

    for row in COMMAND_TABLE.iter().filter(|row| !row.interactive) {
        let tool = tools
            .iter()
            .find(|tool| tool.name == tool_name(row))
            .unwrap();
        assert_eq!(
            properties(tool).contains("confirm"),
            row.destructive,
            "{}",
            tool.name
        );
        assert_eq!(tool.input_schema["additionalProperties"], json!(false));
        for global in ["environment", "format", "profile", "yes", "local", "cloud"] {
            assert!(
                !properties(tool).contains(global),
                "{} takes {global}",
                tool.name
            );
        }
        let annotations = tool.annotations.as_ref().unwrap();
        assert_eq!(
            annotations.read_only_hint,
            Some(row.class == CommandClass::ManagementRead),
            "{}",
            tool.name
        );
        assert_eq!(annotations.destructive_hint, Some(row.destructive));
        assert!(tool.output_schema.is_some(), "{}", tool.name);
    }

    for (name, arguments) in [
        (
            "agent_list",
            &[
                "agent_type_name",
                "component_name",
                "filter",
                "max_count",
                "mode",
                "precise",
                "scan_cursor",
            ][..],
        ),
        (
            "agent_oplog",
            &["agent_id", "cursor", "from", "omit_payloads", "query"],
        ),
        (
            "agent_invoke",
            &[
                "agent_id",
                "arguments",
                "expected_fingerprint",
                "function_name",
                "idempotency_key",
                "schedule_at",
                "trigger",
            ],
        ),
        ("agent_delete", &["agent_id", "confirm"]),
    ] {
        let tool = tools.iter().find(|tool| tool.name == name).unwrap();
        assert_eq!(
            properties(tool),
            arguments
                .iter()
                .map(|argument| argument.to_string())
                .collect::<BTreeSet<_>>(),
            "{name}"
        );
    }

    // An argument that the CLI's parser refuses gets the CLI's error. A destructive call confirms,
    // because a call without `confirm` is refused before the parser reads it.
    for (tool, arguments, cli) in [
        (
            "agent_revert",
            json!({"agent_id": r#"Counter("a")"#, "confirm": true}),
            vec![cmd::AGENT, "revert", r#"Counter("a")"#],
        ),
        (
            "agent_list",
            json!({"mode": "bogus"}),
            vec![cmd::AGENT, cmd::LIST, "--mode", "bogus"],
        ),
    ] {
        let result = call(&client, tool, arguments).await;
        assert_eq!(error_code(&result), "invalid-arguments");
        let message = error_message(&result);
        let first_line = message.lines().next().unwrap();
        let outputs = ctx.cli(cli).await;
        assert!(!outputs.success());
        assert!(
            outputs.stderr_contains(first_line),
            "the CLI does not report `{first_line}`:\n{}",
            outputs.stderr_text()
        );
    }

    // Arguments outside the schema are refused before the command runs.
    let refused = call(
        &client,
        "agent_list",
        json!({"refresh": 400, "environment": "mcp-serve/staging"}),
    )
    .await;
    assert_eq!(error_code(&refused), "invalid-arguments");

    client.cancel().await.unwrap();
}

#[test]
#[timeout("10 minutes")]
async fn mcp_serve_reads_return_the_cli_json() {
    let ctx = counters_context(&["local"]).await;
    let agent = r#"Counter("mcp-reads")"#;
    invoke(&ctx, "local", agent).await;
    invoke(&ctx, "local", agent).await;

    let client = mcp_client(&ctx, &["-E", "local"]).await;
    let reads = [
        (
            "agent get",
            json!({"agent_id": agent}),
            vec![cmd::AGENT, cmd::GET, agent],
        ),
        (
            "agent list",
            json!({}),
            vec![cmd::AGENT, cmd::LIST, "--page"],
        ),
        (
            "agent oplog",
            json!({"agent_id": agent}),
            vec![cmd::AGENT, "oplog", agent, "--page"],
        ),
        (
            "agent files",
            json!({"agent_id": agent}),
            vec![cmd::AGENT, "files", agent],
        ),
    ];
    let covered = reads
        .iter()
        .map(|(path, ..)| *path)
        .collect::<BTreeSet<_>>();
    for row in COMMAND_TABLE
        .iter()
        .filter(|row| row.class == CommandClass::ManagementRead && !row.interactive)
    {
        assert!(
            covered.contains(row.path.join(" ").as_str()),
            "no parity case for `{}`",
            row.path.join(" ")
        );
    }

    for (path, arguments, cli) in reads {
        let tool = path.replace(' ', "_");
        let result = call(&client, &tool, arguments).await;
        let documents = cli_json(&ctx, &cli).await;
        let mcp = structured(&result);
        let mcp_documents = match mcp.get("items") {
            Some(items) => items.as_array().unwrap().clone(),
            None => vec![mcp.clone()],
        };
        assert_eq!(
            comparable(&Value::Array(mcp_documents)),
            comparable(&Value::Array(documents)),
            "`{path}` returns other JSON over MCP than on the CLI"
        );
    }

    // A destructive command needs `confirm` on the call; `--yes` at start confirms nothing.
    client.cancel().await.unwrap();
    let client = mcp_client(&ctx, &["-E", "local", "--yes"]).await;
    let revert = json!({"agent_id": agent, "number_of_invocations": 1});
    let refused = call(&client, "agent_revert", revert.clone()).await;
    assert_eq!(error_code(&refused), "confirmation-required");
    let reverts = async || {
        let pages =
            follow_pages(&client, "agent_oplog", json!({"agent_id": agent}), "cursor").await;
        items(&pages, "entries")
            .into_iter()
            .filter(|item| item["entry"]["type"] == "Revert")
            .count()
    };
    assert_eq!(reverts().await, 0);

    let mut confirmed = revert.clone();
    confirmed["confirm"] = json!(true);
    let reverted = call(&client, "agent_revert", confirmed).await;
    assert_eq!(structured(&reverted)["$type"], "agent.revert");
    assert_eq!(reverts().await, 1);
    client.cancel().await.unwrap();
}

#[test]
#[timeout("10 minutes")]
async fn mcp_serve_destructive_tools_run_only_with_confirm() {
    let ctx = counters_context_with(&["local"], "    cli:\n      autoConfirm: true", &[]).await;
    let agent = r#"Counter("mcp-confirm")"#;
    invoke(&ctx, "local", agent).await;

    // Neither `cli.autoConfirm` nor `--yes` at start confirms a call, also not of a command that
    // does not ask on the CLI.
    let client = mcp_client(&ctx, &["-E", "local", "--yes"]).await;
    for confirm in [json!(null), json!(false)] {
        let mut arguments = json!({"agent_id": agent});
        if !confirm.is_null() {
            arguments["confirm"] = confirm;
        }
        for tool in ["agent_delete", "agent_simulate_crash"] {
            let refused = call(&client, tool, arguments.clone()).await;
            assert_eq!(error_code(&refused), "confirmation-required", "{tool}");
            assert_eq!(error(&refused)["payload"]["answer_with"], "confirm");
        }
    }
    let get = call(&client, "agent_get", json!({"agent_id": agent})).await;
    structured(&get);

    let deleted = call(
        &client,
        "agent_delete",
        json!({"agent_id": agent, "confirm": true}),
    )
    .await;
    structured(&deleted);
    let get = call(&client, "agent_get", json!({"agent_id": agent})).await;
    assert_eq!(error_code(&get), "agent-not-found");
    client.cancel().await.unwrap();
}

#[test]
#[timeout("10 minutes")]
async fn mcp_serve_works_on_the_current_deployment() {
    let ctx = counters_context(&["local"]).await;
    let agent = r#"Counter("mcp-update")"#;
    invoke(&ctx, "local", agent).await;
    let client = mcp_client(&ctx, &["-E", "local"]).await;

    // A deployment during the session makes revision 1 the current one.
    write_manifest(&ctx, "    env:\n      REVISION: '1'", "");
    let outputs = ctx.cli(["-E", "local", cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
    let get = call(&client, "agent_get", json!({"agent_id": agent})).await;
    assert_eq!(structured(&get)["metadata"]["componentRevision"], json!(0));

    let updated = call(
        &client,
        "agent_update",
        json!({"agent_id": agent, "await": true, "confirm": true}),
    )
    .await;
    structured(&updated);
    let get = call(&client, "agent_get", json!({"agent_id": agent})).await;
    assert_eq!(structured(&get)["metadata"]["componentRevision"], json!(1));

    // Each call reads the manifests again: a component renamed in golem.yaml is the one that a
    // call selects, also without a deployment.
    let manifest = std::fs::read_to_string(ctx.cwd_path_join("golem.yaml")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        manifest.replace("it:agent-counters:", "it:agent-renamed:"),
    )
    .unwrap();
    let renamed = call(&client, "agent_list", json!({})).await;
    assert_eq!(error_code(&renamed), "command-failed", "{renamed:?}");
    let message = error_message(&renamed);
    assert!(message.contains("it:agent-renamed"), "{message}");
    fs::write_str(ctx.cwd_path_join("golem.yaml"), manifest).unwrap();
    let listed = call(&client, "agent_list", json!({})).await;
    let listed = structured(&listed).to_string();
    assert!(listed.contains("mcp-update"), "{listed}");
    client.cancel().await.unwrap();
}

#[test]
#[timeout("10 minutes")]
async fn mcp_serve_drops_the_calls_that_the_client_cancels() {
    let ctx = counters_context(&["local"]).await;
    let kept = r#"Counter("mcp-cancel-kept")"#;
    invoke(&ctx, "local", kept).await;
    let client = mcp_client(&ctx, &["-E", "local"]).await;

    // An invocation of ten minutes runs, and a confirmed delete waits for it. The server takes
    // concurrent requests in any order, so the delete is sent once the invocation runs.
    let busy = r#"Counter("mcp-cancel-busy")"#;
    let running = send_call(
        &client,
        "agent_invoke",
        json!({"agent_id": busy, "function_name": "sleep_and_increment",
            "arguments": ["600000"]}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let outputs = ctx
                .cli(["-E", "local", cmd::AGENT, "get", busy, flag::FORMAT, "json"])
                .await;
            if outputs.success()
                && let Some(get) = outputs.stdout_json::<Value>().first()
                && matches!(
                    get["metadata"]["status"].as_str(),
                    Some("Running" | "Suspended")
                )
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("the invocation did not start");
    let queued = send_call(
        &client,
        "agent_delete",
        json!({"agent_id": kept, "confirm": true}),
    )
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    queued.cancel(None).await.unwrap();
    running.cancel(None).await.unwrap();

    // The next call runs once the cancelled invocation stops, and the cancelled delete never ran.
    let get = tokio::time::timeout(
        Duration::from_secs(120),
        call(&client, "agent_get", json!({"agent_id": kept})),
    )
    .await
    .expect("the cancelled invocation still blocks the server");
    let get = structured(&get).to_string();
    assert!(get.contains("mcp-cancel-kept"), "{get}");
    client.cancel().await.unwrap();
}

#[test]
#[timeout("10 minutes")]
async fn mcp_serve_stdout_carries_only_json_rpc() {
    let ctx = counters_context(&["local"]).await;
    let mut child = server_command(&ctx, &["-E", "local"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut stderr = child.stderr.take().unwrap();
    let stderr = tokio::spawn(async move {
        let mut text = String::new();
        stderr.read_to_string(&mut text).await.unwrap();
        text
    });

    let requests = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "agent_invoke",
            "arguments": {"agent_id": r#"Counter("mcp-stdout")"#, "function_name": "increment"}}}),
    ];
    for request in requests {
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
    }

    let mut lines = Vec::new();
    let response = tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            let line = stdout.next_line().await.unwrap().expect("the server ended");
            let message = serde_json::from_str::<Value>(&line).unwrap_or_else(|error| {
                panic!("stdout has a line that is not JSON ({error}): {line}")
            });
            assert_eq!(message["jsonrpc"], "2.0", "{line}");
            lines.push(line);
            if message["id"] == 2 {
                return message;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        response["result"]["structuredContent"]["items"][0]["$type"], "agent.invoke",
        "{response}"
    );

    drop(stdin);
    while let Some(line) = stdout.next_line().await.unwrap() {
        let message = serde_json::from_str::<Value>(&line)
            .unwrap_or_else(|error| panic!("stdout has a line that is not JSON ({error}): {line}"));
        assert_eq!(message["jsonrpc"], "2.0", "{line}");
    }
    assert!(child.wait().await.unwrap().success());
    let stderr = stderr.await.unwrap();
    assert!(
        stderr.contains("Invoking"),
        "the call's progress is not on stderr:\n{stderr}"
    );

    platform::a_client_that_does_not_read_stdout_does_not_keep_the_server(&ctx).await;
}

/// A client that closes stdin and does not read stdout. The response of the call that runs at the
/// end of stdin becomes ready only after it, when the pipe of stdout is full: rmcp gives up the
/// write of that response, but the write stays blocked. The server still exits.
#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    pub(super) async fn a_client_that_does_not_read_stdout_does_not_keep_the_server(
        ctx: &TestContext,
    ) {
        use std::io::{BufRead, Write};

        let (stdout_reader, stdout_writer) = std::io::pipe().unwrap();
        let pipe_size = rustix::pipe::fcntl_setpipe_size(&stdout_writer, 4096).unwrap();
        let mut filler = stdout_writer.try_clone().unwrap();
        let mut child = server_command(ctx, &["-E", "local"])
            .stdin(Stdio::piped())
            .stdout(stdout_writer)
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = std::io::BufReader::new(stdout_reader);

        let initialize = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-03-26", "capabilities": {},
        "clientInfo": {"name": "test", "version": "0"}}});
        stdin
            .write_all(format!("{initialize}\n").as_bytes())
            .await
            .unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        assert!(line.contains(r#""id":1"#), "{line}");
        // The server answers no request now, so nothing else writes to the pipe while it is filled.
        // The response of the call that follows finds no room.
        assert_eq!(rustix::io::ioctl_fionread(stdout.get_ref()).unwrap(), 0);
        filler.write_all(&vec![b' '; pipe_size]).unwrap();

        let requests = [
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "agent_invoke",
            "arguments": {
                "agent_id": r#"Counter("mcp-unread")"#,
                "function_name": "sleep_and_increment",
                "arguments": ["4000"]}}}),
        ];
        for request in requests {
            stdin
                .write_all(format!("{request}\n").as_bytes())
                .await
                .unwrap();
        }
        // The call runs; its response is ready only after the end of stdin.
        tokio::time::sleep(Duration::from_secs(2)).await;
        drop(stdin);

        let status = tokio::time::timeout(Duration::from_secs(60), child.wait())
            .await
            .expect("the server exits when its client does not read stdout")
            .unwrap();
        assert!(status.success(), "{status}");
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    pub(super) async fn a_client_that_does_not_read_stdout_does_not_keep_the_server(
        _ctx: &super::TestContext,
    ) {
    }
}

#[test]
#[timeout("10 minutes")]
async fn mcp_serve_reads_only_its_environment() {
    let ctx = counters_context(&["local", "staging"]).await;
    let local_agent = r#"Counter("in-local")"#;
    let staging_agent = r#"Counter("in-staging")"#;
    invoke(&ctx, "local", local_agent).await;
    invoke(&ctx, "staging", staging_agent).await;

    let client = mcp_client(&ctx, &["-E", &format!("{APP}/local")]).await;
    for tool in client.list_all_tools().await.unwrap() {
        assert!(
            !properties(&tool).contains("environment"),
            "{} takes an environment",
            tool.name
        );
    }

    // A qualified `-E` selects no manifest environment, so `agent list` takes the component, as on
    // the CLI.
    let listed = call(
        &client,
        "agent_list",
        json!({"component_name": "it:agent-counters"}),
    )
    .await;
    let listed = structured(&listed).to_string();
    assert!(listed.contains("in-local"), "{listed}");
    assert!(!listed.contains("in-staging"), "{listed}");

    let staging_listed = cli_json(&ctx, &["-E", "staging", cmd::AGENT, cmd::LIST]).await;
    assert!(
        Value::Array(staging_listed)
            .to_string()
            .contains("in-staging")
    );

    let qualified_local = format!("{APP}/local/{local_agent}");
    let local = call(&client, "agent_get", json!({"agent_id": qualified_local})).await;
    structured(&local);

    for outside in [
        format!("{APP}/staging/{staging_agent}"),
        format!("staging/{staging_agent}"),
        format!("other-app/local/{local_agent}"),
    ] {
        let result = call(&client, "agent_get", json!({"agent_id": outside})).await;
        assert_eq!(error_code(&result), "outside-environment", "{outside}");
    }
    client.cancel().await.unwrap();
}

#[test]
#[timeout("10 minutes")]
async fn mcp_serve_reads_the_profile_token_again_on_each_call() {
    let ctx = counters_context(&["local"]).await;
    // A qualified environment selects no manifest environment, so the calls use the login token
    // of the `local` profile.
    let client = mcp_client(&ctx, &["-E", &format!("{APP}/local")]).await;
    let list = json!({"component_name": "it:agent-counters"});
    structured(&call(&client, "agent_list", list.clone()).await);

    let set_token = |auth: AuthenticationConfig| {
        assert!(
            Config::update_profile(&ProfileName::local(), ctx.config_dir.path(), |profile| {
                profile.auth = auth
            })
            .unwrap()
        );
    };
    set_token(AuthenticationConfig::static_token(
        "not-a-valid-token".to_string(),
    ));
    let refused = call(&client, "agent_list", list.clone()).await;
    assert_eq!(error_code(&refused), "auth-unauthorized", "{refused:?}");

    set_token(AuthenticationConfig::static_builtin_local());
    structured(&call(&client, "agent_list", list).await);
    client.cancel().await.unwrap();
}

#[test]
#[timeout("10 minutes")]
async fn mcp_serve_refuses_calls_when_its_profile_is_gone() {
    let ctx = counters_context(&["local"]).await;
    let config_dir = ctx.config_dir.path();
    let pinned = ProfileName("mcp-custom".to_string());
    let router = url::Url::parse(&format!("http://localhost:{}", ctx.router_port())).unwrap();
    let custom = |auth: AuthenticationConfig| Profile {
        custom_url: Some(router.clone()),
        custom_worker_url: None,
        allow_insecure: false,
        config: Default::default(),
        auth,
    };
    let valid = AuthenticationConfig::static_builtin_local;
    assert!(Config::add_profile(pinned.clone(), custom(valid()), false, config_dir).unwrap());
    let client = mcp_client(
        &ctx,
        &["--profile", &pinned.0, "-E", &format!("{APP}/local")],
    )
    .await;
    let list = json!({"component_name": "it:agent-counters"});
    structured(&call(&client, "agent_list", list.clone()).await);

    // The rotated token is used, and then the deleted profile is not replaced by its start token.
    assert!(
        Config::update_profile(&pinned, config_dir, |profile| {
            profile.auth = AuthenticationConfig::static_token("rotated-not-valid-token".to_string())
        })
        .unwrap()
    );
    let rotated = call(&client, "agent_list", list.clone()).await;
    assert_eq!(error_code(&rotated), "auth-unauthorized", "{rotated:?}");
    assert!(matches!(
        Config::delete_inactive_profile(&pinned, config_dir).unwrap(),
        ProfileDeletion::Deleted
    ));
    let deleted = call(&client, "agent_list", list.clone()).await;
    assert_eq!(error_code(&deleted), "profile-not-found", "{deleted:?}");

    // A renamed profile is another profile.
    let renamed = ProfileName("mcp-custom-renamed".to_string());
    assert!(Config::add_profile(renamed, custom(valid()), false, config_dir).unwrap());
    let after_rename = call(&client, "agent_list", list.clone()).await;
    assert_eq!(
        error_code(&after_rename),
        "profile-not-found",
        "{after_rename:?}"
    );

    // The profile created again under the pinned name works with its own token.
    assert!(Config::add_profile(pinned, custom(valid()), false, config_dir).unwrap());
    structured(&call(&client, "agent_list", list).await);
    client.cancel().await.unwrap();
}

#[test]
#[timeout("10 minutes")]
async fn mcp_serve_reports_a_failed_invocation_with_the_platform_error() {
    let ctx = counters_context(&["local"]).await;
    let agent = r#"FailingCounter("mcp-failed")"#;
    let client = mcp_client(&ctx, &["-E", "local"]).await;

    let failed = call(
        &client,
        "agent_invoke",
        json!({"agent_id": agent, "function_name": "add", "arguments": ["11"]}),
    )
    .await;
    assert_eq!(error_code(&failed), "invocation-failed", "{failed:?}");

    let refused = call(
        &client,
        "agent_invoke",
        json!({"agent_id": agent, "function_name": "get"}),
    )
    .await;
    assert_eq!(
        error_code(&refused),
        "previous-invocation-failed",
        "{refused:?}"
    );
    let message = error_message(&refused);
    assert!(
        message.contains("The previous invocation failed"),
        "{message}"
    );
    assert!(message.contains("agent revert"), "{message}");
    let payload = &error(&refused)["payload"];
    assert!(
        payload["stderr"]
            .as_str()
            .is_some_and(|stderr| stderr.contains("value is too large")),
        "{payload}"
    );
    client.cancel().await.unwrap();

    // The CLI prints the hint and the server's error.
    let outputs = ctx
        .cli(["-E", "local", cmd::AGENT, cmd::INVOKE, agent, "get"])
        .await;
    assert!(!outputs.success());
    assert!(outputs.stderr_contains("The previous invocation failed"));
    assert!(outputs.stderr_contains("previous-invocation-failed"));
}

#[test]
#[timeout("20 minutes")]
async fn mcp_serve_reports_and_cancels_streaming_invocations() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    let outputs = ctx
        .cli([flag::YES, cmd::NEW, "mcp-streams", flag::TEMPLATE, "ts"])
        .await;
    assert!(outputs.success_or_dump());
    ctx.cd("mcp-streams");
    fs::write_str(
        ctx.cwd_path_join("src/counter-agent.ts"),
        indoc! { r#"
            import { z } from 'zod';
            import { AgentStream, defineAgent, method, http, s } from '@golemcloud/golem-ts-sdk';

            export const CounterAgent = defineAgent({
              name: 'CounterAgent',
              id: { name: z.string() },
              http: http.mount('/counters/{name}'),
              methods: {
                explode: method({ input: {}, returns: s.stream(z.number()) }),
                count: method({ input: {}, returns: s.stream(z.number()) }),
                endless: method({ input: {}, returns: s.stream(z.number()) }),
                flood: method({ input: {}, returns: s.stream(z.string()) }),
                value: method({ input: {}, returns: z.number() }),
              },
            });

            export const CounterAgentImpl = CounterAgent.implement({
              init: () => ({}),
              methods: {
                explode() {
                  console.error('intentional stream stderr');
                  throw new Error('intentional stream failure');
                },
                count() {
                  return AgentStream.from(
                    (async function* () {
                      for (let i = 0; i < 3; i++) {
                        yield i;
                      }
                    })(),
                  );
                },
                endless() {
                  return AgentStream.from(
                    (async function* () {
                      for (let i = 0; ; i++) {
                        await new Promise((resolve) => setTimeout(resolve, 10));
                        yield i;
                      }
                    })(),
                  );
                },
                flood() {
                  return AgentStream.from(
                    (async function* () {
                      for (;;) {
                        yield 'x'.repeat(16 * 1024);
                      }
                    })(),
                  );
                },
                value() {
                  return 1;
                },
              },
            });
        "# },
    )
    .unwrap();
    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
    let outputs = ctx
        .cli([
            "retry-policy",
            "create",
            "no-retries",
            "--priority",
            "100",
            "--predicate",
            "true",
            "--policy",
            "never",
        ])
        .await;
    assert!(outputs.success_or_dump());

    let client = mcp_client(&ctx, &[]).await;
    let explode = json!({"agent_id": r#"CounterAgent("mcp-stream")"#, "function_name": "explode"});
    let failed = call(&client, "agent_invoke", explode).await;
    assert_eq!(error_code(&failed), "invocation-failed", "{failed:?}");
    let payload = &error(&failed)["payload"];
    assert!(
        payload["stderr"]
            .as_str()
            .is_some_and(|stderr| stderr.contains("intentional stream stderr")),
        "{payload}"
    );
    assert!(
        error_message(&failed).contains("intentional stream failure"),
        "{payload}"
    );
    // The session's events, which report the failure on the CLI, stay in the payload.
    assert!(
        payload["output"].as_array().is_some_and(|events| events
            .iter()
            .any(|event| event["outcome"] == "invocation-failed")),
        "{payload}"
    );

    // A stream that ends returns all of its events in one result, as the CLI prints them.
    let count = r#"CounterAgent("mcp-count")"#;
    let streamed = call(
        &client,
        "agent_invoke",
        json!({"agent_id": count, "function_name": "count"}),
    )
    .await;
    let events = structured(&streamed)["items"].as_array().unwrap().clone();
    let values = events
        .iter()
        .filter(|event| event["kind"] == "item")
        .map(|event| event["value"].clone())
        .collect::<Vec<_>>();
    assert_eq!(values.len(), 3, "{events:?}");
    assert_eq!(events.last().unwrap()["kind"], "finished", "{events:?}");
    let printed = cli_json(
        &ctx,
        &[cmd::AGENT, cmd::INVOKE, count, "count", "--no-stream"],
    )
    .await;
    // The idempotency key and the stream IDs differ between two invocations.
    let without_ids = |events: &[Value]| {
        events
            .iter()
            .map(|event| {
                let mut event = comparable(event);
                let event_fields = event.as_object_mut().unwrap();
                event_fields.remove("idempotencyKey");
                event_fields.remove("streamId");
                if let Some(Value::Object(value)) = event_fields.get_mut("value") {
                    value.remove("$stream");
                }
                event
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(without_ids(&events), without_ids(&printed));

    // A stream whose events outgrow the limit of a tool result ends the call with an error and
    // is cancelled, so the agent takes the next invocation.
    let flood = r#"CounterAgent("mcp-flood")"#;
    let flooded = tokio::time::timeout(
        Duration::from_secs(120),
        call(
            &client,
            "agent_invoke",
            json!({"agent_id": flood, "function_name": "flood"}),
        ),
    )
    .await
    .expect("the call ends at the output limit");
    assert_eq!(error_code(&flooded), "output-too-large", "{flooded:?}");
    assert_eq!(
        error(&flooded)["payload"]["limit_bytes"],
        4 * 1024 * 1024,
        "{flooded:?}"
    );
    let after = tokio::time::timeout(
        Duration::from_secs(120),
        call(
            &client,
            "agent_invoke",
            json!({"agent_id": flood, "function_name": "value"}),
        ),
    )
    .await
    .expect("the agent still runs the flooding invocation");
    assert_eq!(structured(&after)["items"][0]["result"], "1", "{after:?}");

    // A cancelled call cancels the streams of its invocation on the server, also while it writes
    // their output, so the agent stops producing and takes the next invocation.
    let endless = r#"CounterAgent("mcp-endless")"#;
    let running = send_call(
        &client,
        "agent_invoke",
        json!({"agent_id": endless, "function_name": "endless"}),
    )
    .await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    running.cancel(None).await.unwrap();
    let after = tokio::time::timeout(
        Duration::from_secs(120),
        call(
            &client,
            "agent_invoke",
            json!({"agent_id": endless, "function_name": "value"}),
        ),
    )
    .await
    .expect("the agent still runs the cancelled invocation");
    assert_eq!(structured(&after)["items"][0]["result"], "1", "{after:?}");

    // A client that goes away during a streaming invocation: the server cancels the streams of
    // the invocation on the server before it exits, so the agent takes the next invocation.
    let disconnected = r#"CounterAgent("mcp-disconnect")"#;
    let mut server = server_command(&ctx, &[])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut stdin = server.stdin.take().unwrap();
    let requests = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "agent_invoke",
            "arguments": {"agent_id": disconnected, "function_name": "endless"}}}),
    ];
    for request in requests {
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(60), server.wait())
        .await
        .expect("the server exits after its client went away")
        .unwrap();
    assert!(status.success(), "{status}");
    let after = tokio::time::timeout(
        Duration::from_secs(120),
        call(
            &client,
            "agent_invoke",
            json!({"agent_id": disconnected, "function_name": "value"}),
        ),
    )
    .await
    .expect("the agent still runs the invocation of the client that went away");
    assert_eq!(structured(&after)["items"][0]["result"], "1", "{after:?}");
    // The stream has its terminal: it was ended, not left detached and producing.
    let oplog = cli_json(&ctx, &[cmd::AGENT, "oplog", disconnected]).await;
    assert!(
        oplog
            .iter()
            .any(|entry| entry["entry"]["type"] == "StreamTerminal"),
        "{oplog:?}"
    );
    client.cancel().await.unwrap();
}

#[test]
#[timeout("10 minutes")]
async fn mcp_serve_keeps_its_environment_when_it_is_renamed() {
    let ctx = counters_context(&["local"]).await;
    let agent = r#"Counter("mcp-rename")"#;
    invoke(&ctx, "local", agent).await;
    let client = mcp_client(&ctx, &["-E", &format!("{APP}/local")]).await;
    let get = call(&client, "agent_get", json!({"agent_id": agent})).await;
    let environment_id = structured(&get)["metadata"]["environmentId"]
        .as_str()
        .unwrap()
        .parse::<uuid::Uuid>()
        .unwrap();

    let registry = RegistryServiceClientLive {
        context: golem_client::Context {
            client: reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build(),
            base_url: url::Url::parse(&format!("http://localhost:{}", ctx.router_port())).unwrap(),
            security_token: golem_client::Security::Bearer(
                golem_client::LOCAL_WELL_KNOWN_TOKEN.to_string(),
            ),
        },
    };
    let environment = registry.get_environment(&environment_id).await.unwrap();
    registry
        .update_environment(
            &environment_id,
            &EnvironmentUpdate {
                current_revision: environment.revision,
                name: Some(EnvironmentName("renamed".to_string())),
                compatibility_check: None,
                tool_compatibility_mode: None,
                version_check: None,
                security_overrides: None,
            },
        )
        .await
        .unwrap();

    // The server works in the environment with the pinned ID, under its new name.
    structured(&call(&client, "agent_get", json!({"agent_id": agent})).await);
    let renamed = format!("{APP}/renamed/{agent}");
    structured(&call(&client, "agent_get", json!({"agent_id": renamed})).await);
    let old_name = format!("{APP}/local/{agent}");
    let refused = call(&client, "agent_get", json!({"agent_id": old_name})).await;
    assert_eq!(error_code(&refused), "outside-environment", "{refused:?}");
    client.cancel().await.unwrap();
}

#[test]
#[timeout("10 minutes")]
async fn mcp_serve_reads_the_oplog_and_the_agent_list_page_by_page() {
    // Pages of at most 2, and a scan budget of 1 entry, so that a search or a listing call stops
    // after one storage chunk.
    let ctx = counters_context_with(
        &["local"],
        "",
        &[
            ("GOLEM_HTTP_BATCH_SIZE", "2"),
            ("GOLEM__WORKER_EXECUTOR__LIMITS__SCAN_BUDGET__ENTRIES", "1"),
        ],
    )
    .await;
    let agents = (1..=5)
        .map(|n| format!(r#"Counter("page-{n}")"#))
        .collect::<Vec<_>>();
    for agent in &agents {
        invoke(&ctx, "local", agent).await;
    }
    let agent = agents[0].as_str();
    for _ in 0..4 {
        invoke(&ctx, "local", agent).await;
    }
    let client = mcp_client(&ctx, &["-E", "local"]).await;

    // A search with a rare match: one entry of each invocation.
    let query = "agent-invocation-finished";
    let search = json!({"agent_id": agent, "query": query});
    let first = structured(&call(&client, "agent_oplog", search.clone()).await).clone();
    assert!(first["entries"].as_array().unwrap().len() <= 2, "{first}");
    let cursor = first["cursor"].as_str().expect("a cursor").to_string();

    let pages = follow_pages(&client, "agent_oplog", search.clone(), "cursor").await;
    for page in &pages {
        assert_eq!(page["$type"], "agent.oplog-page");
        assert!(page["entries"].as_array().unwrap().len() <= 2, "{page}");
    }
    assert!(
        pages
            .iter()
            .any(|page| page["entries"] == json!([]) && page.get("cursor").is_some()),
        "no page ended at the scan budget before a match"
    );
    let entries = items(&pages, "entries");
    assert!(entries.len() >= 5, "{entries:?}");
    let stream = cli_json(&ctx, &[cmd::AGENT, "oplog", agent, "--query", query]).await;
    assert_eq!(
        comparable(&json!(entries)),
        comparable(&Value::Array(stream)),
        "the pages are not the CLI's stream"
    );

    // A cursor past the end of the oplog, or of another query, is stale, not an empty last page.
    let [_, revision, query_hash] = cursor.split('-').collect::<Vec<_>>()[..] else {
        panic!("not a cursor: {cursor}");
    };
    let mut past_end = search.clone();
    past_end["cursor"] = json!(format!("1000000-{revision}-{query_hash}"));
    let refused = call(&client, "agent_oplog", past_end).await;
    assert_eq!(error_code(&refused), "stale-cursor", "{refused:?}");
    let other_query =
        json!({"agent_id": agent, "query": "agent-invocation-started", "cursor": cursor});
    let refused = call(&client, "agent_oplog", other_query).await;
    assert_eq!(error_code(&refused), "stale-cursor", "{refused:?}");

    // The agent list pages every agent once.
    let pages = follow_pages(&client, "agent_list", json!({}), "scan_cursor").await;
    for page in &pages {
        assert!(page["agents"].as_array().unwrap().len() <= 2, "{page}");
    }
    let agent_ids = |agents: Vec<&Value>| {
        let mut ids = agents
            .into_iter()
            .map(|agent| agent["agentId"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    };
    let paged = agent_ids(items(&pages, "agents"));
    let listed = cli_json(&ctx, &[cmd::AGENT, cmd::LIST]).await;
    assert_eq!(
        paged,
        agent_ids(listed[0]["agents"].as_array().unwrap().iter().collect())
    );
    assert_eq!(paged.len(), agents.len(), "{paged:?}");
    client.cancel().await.unwrap();
}

/// The oplog indexes of `agent`, through the plain `agent_oplog` tool.
async fn oplog_indexes(client: &RunningService<RoleClient, ()>, agent: &str) -> Vec<u64> {
    let pages = follow_pages(client, "agent_oplog", json!({"agent_id": agent}), "cursor").await;
    items(&pages, "entries")
        .into_iter()
        .map(|entry| entry["index"].as_u64().unwrap())
        .collect()
}

#[test]
#[timeout("10 minutes")]
async fn mcp_serve_exports_one_bounded_oplog_report() {
    // Pages of 2 and a scan budget of 10 entries: a range read charges the entries before `from`
    // and the entries of the range, so a range up to index 10 fits the budget only when every page
    // spends the same budget.
    let ctx = counters_context_with(
        &["local", "staging"],
        "",
        &[
            ("GOLEM_HTTP_BATCH_SIZE", "2"),
            ("GOLEM__WORKER_EXECUTOR__LIMITS__SCAN_BUDGET__ENTRIES", "10"),
        ],
    )
    .await;
    let agent = r#"Counter("mcp-export")"#;
    for _ in 0..4 {
        invoke(&ctx, "local", agent).await;
    }
    let staging_agent = r#"Counter("mcp-export-staging")"#;
    invoke(&ctx, "staging", staging_agent).await;
    let client = mcp_client(&ctx, &["-E", "local"]).await;

    let tools = client.list_all_tools().await.unwrap();
    let export_tool = tools
        .iter()
        .find(|tool| tool.name == "agent_oplog_export")
        .unwrap();
    assert_eq!(
        properties(export_tool),
        [
            "agent_id",
            "from",
            "keep_error_text",
            "keep_scalars",
            "keep_shape",
            "max_entry_bytes",
            "to"
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    );
    let annotations = export_tool.annotations.as_ref().unwrap();
    assert_eq!(annotations.read_only_hint, Some(true));
    let output_schema = Value::Object(export_tool.output_schema.as_deref().unwrap().clone());
    let validator = jsonschema::options().build(&output_schema).unwrap();

    let indexes = oplog_indexes(&client, agent).await;
    let last = *indexes.last().unwrap();
    assert!(
        last > 12,
        "the oplog is too short for the test: {indexes:?}"
    );

    // A range in the middle: both ends included, nothing outside, over five pages of one budget.
    let range = json!({"agent_id": agent, "from": 3, "to": 10, "keep_shape": true});
    let result = call(&client, "agent_oplog_export", range.clone()).await;
    let report = structured(&result).clone();
    assert!(
        validator.is_valid(&report),
        "{:?}",
        validator
            .iter_errors(&report)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(report["$type"], "agent.oplog-export");
    assert_eq!(report["header"]["from"], 3);
    assert_eq!(report["header"]["to"], 10);
    assert_eq!(report["header"]["filtered"], false);
    assert_eq!(report["header"]["keepShape"], true);
    assert_eq!(
        report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["index"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        (3..=10).collect::<Vec<_>>()
    );
    assert!(report["coverage"].is_object());
    let text = report.to_string();
    assert!(
        !text.contains("mcp-export"),
        "the report holds the agent ID"
    );

    // A second call starts an independent report, without continuation data.
    let again = call(&client, "agent_oplog_export", range).await;
    assert_eq!(
        comparable(structured(&again))["entries"],
        comparable(&report)["entries"]
    );

    // A range past the budget fails as a whole, although each page of 2 fits the budget.
    let refused = call(
        &client,
        "agent_oplog_export",
        json!({"agent_id": agent, "from": 3, "to": 12}),
    )
    .await;
    assert_eq!(error_code(&refused), "scan-budget-exceeded", "{refused:?}");
    let message = error_message(&refused);
    assert!(message.contains("--obfuscate"), "{message}");
    let payload = error(&refused)["payload"].clone();
    assert_eq!(
        payload.as_object().unwrap().keys().collect::<Vec<_>>(),
        vec!["message"],
        "{payload}"
    );

    // Bounds outside the oplog, inverted bounds and continuation arguments are refused.
    for (arguments, code) in [
        (
            json!({"agent_id": agent, "from": 2, "to": last + 1}),
            "invalid-arguments",
        ),
        (
            json!({"agent_id": agent, "from": 5, "to": 4}),
            "invalid-arguments",
        ),
        (
            json!({"agent_id": agent, "from": 0, "to": 4}),
            "invalid-arguments",
        ),
        (json!({"agent_id": agent, "to": 4}), "invalid-arguments"),
        (
            json!({"agent_id": agent, "from": 1, "to": 2, "cursor": "2-0-"}),
            "invalid-arguments",
        ),
        (
            json!({"agent_id": agent, "from": 1, "to": 2, "page": true}),
            "invalid-arguments",
        ),
        (
            json!({"agent_id": agent, "from": 1, "to": 2, "exportId": "x"}),
            "invalid-arguments",
        ),
    ] {
        let refused = call(&client, "agent_oplog_export", arguments.clone()).await;
        assert_eq!(error_code(&refused), code, "{arguments}");
    }

    // The server reads only its own environment.
    let outside = call(
        &client,
        "agent_oplog_export",
        json!({"agent_id": format!("{APP}/staging/{staging_agent}"), "from": 1, "to": 2}),
    )
    .await;
    assert_eq!(error_code(&outside), "outside-environment");

    // An agent ID that does not resolve and an argument that does not parse fail without quoting
    // the value, in the result and on the server's stderr: also when the server holds its log
    // back until an error, and when it traces at debug level.
    let marker = "CUSTOMER-MARKER";
    for global_args in [
        &["-E", "local"][..],
        &["-E", "local", "-q"],
        &["-E", "local", "-vvv"],
    ] {
        let (transport, stderr) = TokioChildProcess::builder(server_command(&ctx, global_args))
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stderr = stderr.unwrap();
        let stderr = tokio::spawn(async move {
            let mut text = String::new();
            stderr.read_to_string(&mut text).await.unwrap();
            text
        });
        let marker_client = ().serve(transport).await.unwrap();
        for arguments in [
            json!({"agent_id": format!("a/b/c/d/{marker}"), "from": 1, "to": 2}),
            json!({"agent_id": format!(r#"Counter("{marker}")"#), "from": 1, "to": 2}),
            json!({"agent_id": agent, "from": marker, "to": 2}),
        ] {
            let unresolved = call(&marker_client, "agent_oplog_export", arguments).await;
            assert_eq!(unresolved.is_error, Some(true), "{unresolved:?}");
            assert!(
                !format!("{unresolved:?}").contains(marker),
                "{unresolved:?}"
            );
        }
        marker_client.cancel().await.unwrap();
        let stderr = stderr.await.unwrap();
        assert!(!stderr.contains(marker), "{global_args:?}:\n{stderr}");
    }

    // A cancelled call returns nothing, and the next call is complete on its own.
    let handle = send_call(
        &client,
        "agent_oplog_export",
        json!({"agent_id": agent, "from": 1, "to": 10}),
    )
    .await;
    handle.cancel(None).await.unwrap();
    let after = call(
        &client,
        "agent_oplog_export",
        json!({"agent_id": agent, "from": 1, "to": 2}),
    )
    .await;
    let after = structured(&after);
    assert_eq!(after["entries"].as_array().unwrap().len(), 2);
    assert_eq!(after["entries"][0]["kind"], "Create");

    client.cancel().await.unwrap();
}
