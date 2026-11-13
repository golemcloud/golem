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
use golem_cli::config::{AuthenticationConfig, Config, ProfileName};
use golem_cli::{fs, versions};
use indoc::formatdoc;
use rmcp::model::{CallToolRequestParams, CallToolResult, Tool};
use rmcp::service::RunningService;
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
    counters_context_with(environments, "").await
}

/// [`counters_context`] with `local` added to the `local` environment of the manifest.
async fn counters_context_with(environments: &[&str], local: &str) -> TestContext {
    let mut ctx = TestContext::new();
    let fixture = workspace_path().join("test-components/it_agent_counters_release.wasm");
    assert!(
        fixture.is_file(),
        "build the agent-counters test component before running the MCP tests"
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

async fn mcp_client(ctx: &TestContext, global_args: &[&str]) -> RunningService<RoleClient, ()> {
    let (transport, _) = TokioChildProcess::builder(server_command(ctx, global_args))
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    ().serve(transport).await.unwrap()
}

async fn call(
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

fn structured(result: &CallToolResult) -> &Value {
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

/// Removes what differs between two reads of the same state: the time of the read, and the
/// order of invocation context attributes, which the server keeps in a hash map.
fn comparable(value: &Value) -> Value {
    const TIME_FIELDS: &[&str] = &["generatedAt"];
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| !TIME_FIELDS.contains(&key.as_str()))
                .map(|(key, value)| {
                    let mut value = comparable(value);
                    if key == "attributes"
                        && let Value::Array(attributes) = &mut value
                    {
                        attributes.sort_by_key(|attribute| attribute.to_string());
                    }
                    (key.clone(), value)
                })
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
    let expected = COMMAND_TABLE
        .iter()
        .filter(|row| !row.interactive)
        .map(tool_name)
        .collect::<BTreeSet<_>>();
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
            &["agent_id", "from", "omit_payloads", "query"],
        ),
        (
            "agent_invoke",
            &[
                "agent_id",
                "arguments",
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
        ("agent list", json!({}), vec![cmd::AGENT, cmd::LIST]),
        (
            "agent oplog",
            json!({"agent_id": agent}),
            vec![cmd::AGENT, "oplog", agent],
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
        let oplog = call(&client, "agent_oplog", json!({"agent_id": agent})).await;
        structured(&oplog)["items"]
            .as_array()
            .unwrap()
            .iter()
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
    let ctx = counters_context_with(&["local"], "    cli:\n      autoConfirm: true").await;
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
    assert_eq!(error_code(&failed), "internal-agent-execution-failed");

    let refused = call(
        &client,
        "agent_invoke",
        json!({"agent_id": agent, "function_name": "get"}),
    )
    .await;
    assert_eq!(
        error_code(&refused),
        "internal-previous-invocation-failed",
        "{refused:?}"
    );
    let message = error_message(&refused);
    assert!(
        message.contains("The previous invocation failed"),
        "{message}"
    );
    assert!(message.contains("agent revert"), "{message}");
    let agent_error = &error(&refused)["payload"]["agent_error"];
    assert!(
        agent_error["stderr"]
            .as_str()
            .is_some_and(|stderr| stderr.contains("value is too large")),
        "{agent_error}"
    );
    client.cancel().await.unwrap();

    // The CLI prints the hint and the server's error.
    let outputs = ctx
        .cli(["-E", "local", cmd::AGENT, cmd::INVOKE, agent, "get"])
        .await;
    assert!(!outputs.success());
    assert!(outputs.stderr_contains("The previous invocation failed"));
    assert!(outputs.stderr_contains("INTERNAL_PREVIOUS_INVOCATION_FAILED"));
}
