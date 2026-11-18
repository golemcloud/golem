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

//! `agent create`, `agent upsert`, `agent invoke --expected-fingerprint`,
//! `deploy --redeploy-agents` and the combined `agent list` cursor, against a
//! Rust component with two durable agent types.

use crate::Tracing;
use crate::app::{Output, TestContext, cmd, flag};
use golem_cli::fs;
use golem_cli::versions;
use indoc::{formatdoc, indoc};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

const APP_NAME: &str = "agent-keys";
const COMPONENT_NAME: &str = "agent-keys:rust-main";

/// Deploys an app whose component has two durable agent types: `CounterAgent`, which reads a
/// config value and an environment variable, and `OtherAgent`.
async fn deploy_app(ctx: &mut TestContext) {
    ctx.start_server().await;

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, APP_NAME, flag::TEMPLATE, "rust"])
        .await;
    assert!(outputs.success_or_dump());
    ctx.cd(APP_NAME);

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! { r#"
            manifestVersion: {MANIFEST_VERSION}

            app: {APP_NAME}

            environments:
              local:
                server: local
                componentPresets: debug
              cloud:
                server: cloud
                componentPresets: release

            components:
              {COMPONENT_NAME}:
                templates: rust

            agents:
              CounterAgent:
                config:
                  mark: ''
        "#, MANIFEST_VERSION = versions::sdk::MANIFEST },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("src/counter_agent.rs"),
        indoc! { r#"
            use golem_rust::agentic::Config;
            use golem_rust::{agent_definition, agent_implementation, ConfigSchema};

            #[derive(ConfigSchema)]
            pub struct CounterConfig {
                pub mark: String,
            }

            #[agent_definition]
            pub trait CounterAgent {
                fn new(name: String, #[agent_config] config: Config<CounterConfig>) -> Self;
                fn increment(&mut self) -> u32;
                fn value(&self) -> u32;
                fn describe(&self) -> String;
            }

            struct CounterImpl {
                name: String,
                count: u32,
                config: Config<CounterConfig>,
            }

            #[agent_implementation]
            impl CounterAgent for CounterImpl {
                fn new(name: String, #[agent_config] config: Config<CounterConfig>) -> Self {
                    Self { name, count: 0, config }
                }

                fn increment(&mut self) -> u32 {
                    self.count += 1;
                    self.count
                }

                fn value(&self) -> u32 {
                    self.count
                }

                fn describe(&self) -> String {
                    let mark = self.config.get().expect("counter config").mark;
                    let env = std::env::var("COUNTER_ENV").unwrap_or_default();
                    format!("{}|{}|{}", self.name, mark, env)
                }
            }

            #[agent_definition]
            pub trait OtherAgent {
                fn new(id: String) -> Self;
                fn ping(&self) -> String;
            }

            struct OtherImpl {
                id: String,
            }

            #[agent_implementation]
            impl OtherAgent for OtherImpl {
                fn new(id: String) -> Self {
                    Self { id }
                }

                fn ping(&self) -> String {
                    format!("other:{}", self.id)
                }
            }
        "# },
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
}

async fn cli_json<'a>(ctx: &TestContext, args: impl IntoIterator<Item = &'a str>) -> Output {
    ctx.cli([flag::YES, flag::FORMAT, "json"].into_iter().chain(args))
        .await
}

/// The structured output document of `kind` that a successful command printed.
fn document(outputs: &Output, kind: &str) -> Value {
    assert!(
        outputs.success_or_dump(),
        "the command printing {kind} failed"
    );
    outputs
        .stdout_json::<Value>()
        .into_iter()
        .find(|document| document["$type"] == kind)
        .unwrap_or_else(|| panic!("no {kind} document in the output"))
}

/// The agent ID and fingerprint of an `agent.create` or `agent.upsert` document.
fn reference(document: &Value) -> (String, String) {
    (
        document["agentId"].as_str().unwrap().to_string(),
        document["agentFingerprint"].as_str().unwrap().to_string(),
    )
}

async fn get_metadata(ctx: &TestContext, address: &str) -> Value {
    let outputs = cli_json(ctx, [cmd::AGENT, cmd::GET, address]).await;
    document(&outputs, "agent.get")["metadata"].clone()
}

async fn invoke(ctx: &TestContext, args: &[&str]) -> Output {
    ctx.cli(
        [flag::YES, cmd::AGENT, cmd::INVOKE]
            .into_iter()
            .chain(args.iter().copied()),
    )
    .await
}

async fn invoke_ok(ctx: &TestContext, args: &[&str], expected: &str) {
    let outputs = invoke(ctx, args).await;
    assert!(outputs.success_or_dump(), "invoke {args:?} failed");
    assert!(
        outputs.stdout_contains_ordered(["Invocation result", expected]),
        "invoke {args:?} did not return {expected}"
    );
}

#[test]
#[timeout("20 minutes")]
async fn keyed_create_conflicts_upsert_reaches_it_and_a_stale_fingerprint_is_rejected() {
    let mut ctx = TestContext::new();
    deploy_app(&mut ctx).await;

    let key = r#"CounterAgent("k1")"#;
    let created = document(
        &cli_json(&ctx, [cmd::AGENT, cmd::CREATE, key, "--keyed"]).await,
        "agent.create",
    );
    assert_eq!(created["agentType"], "CounterAgent");
    let (agent_id, fingerprint) = reference(&created);

    let conflict = ctx
        .cli([flag::YES, cmd::AGENT, cmd::CREATE, key, "--keyed"])
        .await;
    assert!(!conflict.success(), "a second keyed create must fail");
    assert!(
        conflict.stderr_contains("AGENT_ALREADY_EXISTS"),
        "a second keyed create must fail with a conflict"
    );

    let upserted = document(
        &cli_json(&ctx, [cmd::AGENT, cmd::UPSERT, key]).await,
        "agent.upsert",
    );
    assert_eq!(
        reference(&upserted),
        (agent_id.clone(), fingerprint.clone())
    );

    let metadata = get_metadata(&ctx, key).await;
    assert_eq!(metadata["agentId"], agent_id.as_str());
    assert_eq!(metadata["agentKey"], key);
    assert_eq!(metadata["fingerprint"], fingerprint.as_str());

    invoke_ok(
        &ctx,
        &[key, "increment", "--expected-fingerprint", &fingerprint],
        "1",
    )
    .await;
    let by_id = format!("CounterAgent[{agent_id}]");
    invoke_ok(
        &ctx,
        &[&by_id, "increment", "--expected-fingerprint", &fingerprint],
        "2",
    )
    .await;

    // Without a key, every create makes a new agent, and none of them is the keyed agent.
    let unkeyed_ids = {
        let mut ids = BTreeSet::new();
        for _ in 0..2 {
            let created = document(
                &cli_json(&ctx, [cmd::AGENT, cmd::CREATE, key]).await,
                "agent.create",
            );
            ids.insert(reference(&created).0);
        }
        ids
    };
    assert_eq!(unkeyed_ids.len(), 2, "{unkeyed_ids:?}");
    assert!(!unkeyed_ids.contains(&agent_id));
    let upserted = document(
        &cli_json(&ctx, [cmd::AGENT, cmd::UPSERT, key]).await,
        "agent.upsert",
    );
    assert_eq!(reference(&upserted).0, agent_id);

    let deleted = ctx.cli([flag::YES, cmd::AGENT, "delete", key]).await;
    assert!(deleted.success_or_dump());

    let recreated = document(
        &cli_json(&ctx, [cmd::AGENT, cmd::CREATE, key, "--keyed"]).await,
        "agent.create",
    );
    let (recreated_id, recreated_fingerprint) = reference(&recreated);
    assert_eq!(recreated_id, agent_id, "the keyed agent keeps its ID");
    assert_ne!(recreated_fingerprint, fingerprint);

    for address in [key, by_id.as_str()] {
        let stale = invoke(
            &ctx,
            &[address, "increment", "--expected-fingerprint", &fingerprint],
        )
        .await;
        assert!(
            !stale.success(),
            "an invocation of {address} with the fingerprint of the deleted agent must fail"
        );
    }

    invoke_ok(
        &ctx,
        &[
            key,
            "value",
            "--expected-fingerprint",
            &recreated_fingerprint,
        ],
        "0",
    )
    .await;
}

#[test]
#[timeout("20 minutes")]
async fn redeploy_keeps_the_id_of_a_keyed_agent_and_recreates_an_unkeyed_agent_with_a_new_id() {
    let mut ctx = TestContext::new();
    deploy_app(&mut ctx).await;

    let keyed_key = r#"CounterAgent("keyed")"#;
    let keyed = document(
        &cli_json(
            &ctx,
            [
                cmd::AGENT,
                cmd::CREATE,
                keyed_key,
                "--keyed",
                "-e",
                "COUNTER_ENV=keyed-env",
                "-c",
                r#"mark="keyed-mark""#,
            ],
        )
        .await,
        "agent.create",
    );
    let (keyed_id, _) = reference(&keyed);

    let unkeyed = document(
        &cli_json(
            &ctx,
            [
                cmd::AGENT,
                cmd::CREATE,
                r#"CounterAgent("unkeyed")"#,
                "-e",
                "COUNTER_ENV=unkeyed-env",
                "-c",
                r#"mark="unkeyed-mark""#,
            ],
        )
        .await,
        "agent.create",
    );
    let (unkeyed_id, _) = reference(&unkeyed);
    let unkeyed_address = format!("CounterAgent[{unkeyed_id}]");

    invoke_ok(&ctx, &[keyed_key, "increment"], "1").await;
    invoke_ok(&ctx, &[&unkeyed_address, "increment"], "1").await;
    invoke_ok(&ctx, &[&unkeyed_address, "increment"], "2").await;

    let unkeyed_before = get_metadata(&ctx, &unkeyed_address).await;
    let keyed_before = get_metadata(&ctx, keyed_key).await;

    let redeployed = document(
        &cli_json(&ctx, [cmd::DEPLOY, "--redeploy-agents"]).await,
        "agent.redeploy",
    );
    assert_eq!(redeployed["redeployed"], true, "{redeployed}");
    assert_eq!(redeployed["errors"], json!([]), "{redeployed}");
    let recreated = redeployed["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| {
            (
                agent["agentId"].as_str().unwrap().to_string(),
                agent["recreatedAgentId"].as_str().unwrap().to_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(recreated.len(), 2, "{redeployed}");
    assert_eq!(
        recreated[&keyed_id], keyed_id,
        "the keyed agent keeps its ID"
    );
    let new_unkeyed_id = recreated[&unkeyed_id].clone();
    assert_ne!(
        new_unkeyed_id, unkeyed_id,
        "the unkeyed agent gets a new ID"
    );
    let new_unkeyed_address = format!("CounterAgent[{new_unkeyed_id}]");

    let keyed_after = get_metadata(&ctx, keyed_key).await;
    assert_eq!(keyed_after["agentId"], keyed_id.as_str());
    assert_eq!(keyed_after["agentKey"], keyed_key);
    assert_ne!(keyed_after["fingerprint"], keyed_before["fingerprint"]);
    assert_eq!(keyed_after["env"], keyed_before["env"]);
    assert_eq!(keyed_after["config"], keyed_before["config"]);

    let unkeyed_after = get_metadata(&ctx, &new_unkeyed_address).await;
    assert_eq!(unkeyed_after["agentId"], new_unkeyed_id.as_str());
    assert_eq!(unkeyed_after["agentKey"], Value::Null);
    assert_eq!(unkeyed_after["env"], unkeyed_before["env"]);
    assert_eq!(unkeyed_after["config"], unkeyed_before["config"]);
    assert_eq!(unkeyed_after["env"]["COUNTER_ENV"], "unkeyed-env");
    assert_eq!(
        unkeyed_after["config"],
        json!([{ "path": ["mark"], "value": "unkeyed-mark" }])
    );

    invoke_ok(&ctx, &[keyed_key, "describe"], "keyed|keyed-mark|keyed-env").await;
    invoke_ok(
        &ctx,
        &[&new_unkeyed_address, "describe"],
        "unkeyed|unkeyed-mark|unkeyed-env",
    )
    .await;
    invoke_ok(&ctx, &[keyed_key, "value"], "0").await;
    invoke_ok(&ctx, &[&new_unkeyed_address, "value"], "0").await;

    let old = ctx
        .cli([flag::YES, cmd::AGENT, cmd::GET, &unkeyed_address])
        .await;
    assert!(
        !old.success(),
        "the old ID of the unkeyed agent must be gone"
    );

    let listed = document(
        &cli_json(&ctx, [cmd::AGENT, cmd::LIST, "CounterAgent"]).await,
        "agent.list",
    );
    let listed_ids = listed["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["agentId"].as_str().unwrap().to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(listed_ids, BTreeSet::from([keyed_id, new_unkeyed_id]));
}

#[test]
#[timeout("20 minutes")]
async fn agent_list_pages_over_two_agent_types_with_one_cursor() {
    let mut ctx = TestContext::new();
    deploy_app(&mut ctx).await;

    let mut expected = BTreeSet::new();
    for i in 0..5 {
        for key in [
            format!(r#"CounterAgent("c{i}")"#),
            format!(r#"OtherAgent("o{i}")"#),
        ] {
            let upserted = document(
                &cli_json(&ctx, [cmd::AGENT, cmd::UPSERT, &key]).await,
                "agent.upsert",
            );
            expected.insert(reference(&upserted).0);
        }
    }
    assert_eq!(expected.len(), 10);

    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    let mut first_cursor = None;
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(pages <= 20, "paging did not end");
        let mut args = vec![
            cmd::AGENT,
            cmd::LIST,
            flag::COMPONENT_NAME,
            COMPONENT_NAME,
            "--max-count",
            "2",
        ];
        if let Some(cursor) = &cursor {
            args.extend(["--scan-cursor", cursor.as_str()]);
        }
        let page = document(&cli_json(&ctx, args).await, "agent.list");
        seen.extend(
            page["agents"]
                .as_array()
                .unwrap()
                .iter()
                .map(|agent| agent["agentId"].as_str().unwrap().to_string()),
        );
        match page["cursor"].as_str() {
            Some(next) => {
                first_cursor.get_or_insert_with(|| next.to_string());
                cursor = Some(next.to_string());
            }
            None => break,
        }
    }
    assert!(pages > 1, "a page size of 2 must need more than one page");
    let unique = seen.iter().cloned().collect::<BTreeSet<_>>();
    assert_eq!(
        unique.len(),
        seen.len(),
        "an agent was listed twice: {seen:?}"
    );
    assert_eq!(unique, expected);

    // The first cursor continues both agent types, so it is rejected for a listing of one.
    let rejected = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::LIST,
            "OtherAgent",
            "--scan-cursor",
            first_cursor.as_deref().unwrap(),
        ])
        .await;
    assert!(!rejected.success());
    assert!(
        rejected.stderr_contains("The cursor continues agent types that are not listed"),
        "a cursor naming an unlisted agent type must be rejected"
    );
}
