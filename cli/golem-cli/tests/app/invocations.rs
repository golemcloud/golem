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

use crate::Tracing;
use crate::app::{Output, TestContext, cmd, flag, replace_strings_in_file};
use serde_json::Value;
use std::collections::HashSet;
use test_r::{inherit_test_dep, test, timeout};
use uuid::Uuid;

inherit_test_dep!(Tracing);

const INVOCATION: &str = "invocation";

fn single_json(outputs: &Output) -> Value {
    let mut documents = outputs.stdout_json::<Value>();
    assert_eq!(documents.len(), 1, "{documents:?}");
    documents.remove(0)
}

#[test]
#[timeout("15 minutes")]
async fn invocation_commands_get_list_and_cancel(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    let app_name = "invocations";

    ctx.start_server().await;
    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "rust"])
        .await;
    assert!(outputs.success_or_dump());
    ctx.cd(app_name);
    replace_strings_in_file(
        ctx.cwd_path_join("src/counter_agent.rs"),
        &[
            ("fn new(", "async fn new("),
            (
                "        Self {\n            _name: name,",
                r#"        if name == "failed-constructor" {
            panic!("constructor rejected");
        }
        if name == "blocked-constructor" {
            let promise = golem_rust::create_promise();
            golem_rust::await_promise(&promise).await;
        }
        Self {
            _name: name,"#,
            ),
        ],
    )
    .unwrap();
    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    let agent = format!("CounterAgent(\"{}\")", Uuid::new_v4());

    // A completed invocation, selected by its key, shows its result.
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "--idempotency-key",
            "completed-key",
            &agent,
            "increment",
        ])
        .await;
    assert!(outputs.success_or_dump());
    let outputs = ctx
        .cli([
            flag::FORMAT,
            "json",
            INVOCATION,
            cmd::GET,
            "--agent",
            &agent,
            "--idempotency-key",
            "completed-key",
        ])
        .await;
    assert!(outputs.success_or_dump());
    let completed = single_json(&outputs);
    assert_eq!(completed["$type"], "invocation.get");
    assert_eq!(completed["summary"]["state"], "completed");
    assert_eq!(completed["summary"]["operation"], "increment");
    assert_eq!(completed["result"]["type"], "Method");
    let completed_id = completed["summary"]["invocationId"]
        .as_str()
        .unwrap()
        .to_string();

    // The same invocation, selected by its ID.
    let outputs = ctx
        .cli([flag::FORMAT, "json", INVOCATION, cmd::GET, &completed_id])
        .await;
    assert!(outputs.success_or_dump());
    assert_eq!(single_json(&outputs)["summary"], completed["summary"]);

    // The same key used for a second method selects one invocation per method: `get` shows both,
    // each as `--method` selects it, and `cancel` needs `--method`.
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "--idempotency-key",
            "completed-key",
            &agent,
            "value",
        ])
        .await;
    assert!(outputs.success_or_dump());
    let get_by_key = |method: Option<&'static str>| {
        let mut args = vec![
            flag::FORMAT,
            "json",
            INVOCATION,
            cmd::GET,
            "--agent",
            agent.as_str(),
            "--idempotency-key",
            "completed-key",
        ];
        if let Some(method) = method {
            args.extend(["--method", method]);
        }
        args.into_iter().map(str::to_string).collect::<Vec<_>>()
    };
    let outputs = ctx.cli(get_by_key(None)).await;
    assert!(outputs.success_or_dump());
    let matches = single_json(&outputs);
    assert_eq!(matches["$type"], "invocation.get-matches");
    let matches = matches["invocations"].as_array().unwrap().clone();
    let mut operations = matches
        .iter()
        .map(|invocation| {
            invocation["summary"]["operation"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    operations.sort();
    assert_eq!(operations, vec!["increment", "value"]);
    for method in ["increment", "value"] {
        let outputs = ctx.cli(get_by_key(Some(method))).await;
        assert!(outputs.success_or_dump());
        let selected = single_json(&outputs);
        assert_eq!(selected["$type"], "invocation.get");
        assert!(
            matches
                .iter()
                .any(|invocation| invocation["summary"] == selected["summary"]),
            "{method}: {selected:?} in {matches:?}"
        );
    }
    let outputs = ctx
        .cli([
            INVOCATION,
            "cancel",
            "--agent",
            &agent,
            "--idempotency-key",
            "completed-key",
        ])
        .await;
    assert!(!outputs.success());

    // A call scheduled for later is `scheduled` until it is cancelled.
    let schedule_at = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "--trigger",
            "--schedule-at",
            &schedule_at,
            "--idempotency-key",
            "scheduled-key",
            &agent,
            "increment",
        ])
        .await;
    assert!(outputs.success_or_dump());
    let get_scheduled = [
        flag::FORMAT,
        "json",
        INVOCATION,
        cmd::GET,
        "--agent",
        &agent,
        "--idempotency-key",
        "scheduled-key",
    ];
    let outputs = ctx.cli(get_scheduled).await;
    assert!(outputs.success_or_dump());
    let scheduled = single_json(&outputs);
    assert_eq!(scheduled["summary"]["state"], "scheduled");
    assert_eq!(scheduled["result"], Value::Null);
    let scheduled_id = scheduled["summary"]["invocationId"].as_str().unwrap();

    // The state filter finds only the scheduled call.
    let outputs = ctx
        .cli([
            flag::FORMAT,
            "json",
            INVOCATION,
            cmd::LIST,
            "--agent",
            &agent,
            "--status",
            "scheduled",
        ])
        .await;
    assert!(outputs.success_or_dump());
    let only_scheduled = single_json(&outputs);
    assert_eq!(only_scheduled["$type"], "invocation.list");
    let ids = only_scheduled["invocations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|invocation| invocation["invocationId"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![scheduled_id]);

    // Pages of one return every invocation once: the constructor run and the three calls.
    let mut listed = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut args = vec![
            flag::FORMAT,
            "json",
            INVOCATION,
            cmd::LIST,
            "--agent",
            &agent,
            "--page-size",
            "1",
        ];
        if let Some(cursor) = &cursor {
            args.extend(["--cursor", cursor.as_str()]);
        }
        let outputs = ctx.cli(args).await;
        assert!(outputs.success_or_dump());
        let page = single_json(&outputs);
        let invocations = page["invocations"].as_array().unwrap();
        assert!(invocations.len() <= 1);
        listed.extend(
            invocations
                .iter()
                .map(|invocation| invocation["invocationId"].as_str().unwrap().to_string()),
        );
        match page["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => break,
        }
        assert!(listed.len() <= 4, "{listed:?}");
    }
    assert_eq!(listed.len(), 4, "{listed:?}");
    assert_eq!(listed.iter().collect::<HashSet<_>>().len(), 4);
    assert!(listed.contains(&completed_id));
    assert!(listed.iter().any(|id| id == scheduled_id));

    for (address, expected_state) in [
        (agent.as_str(), "completed"),
        ("CounterAgent(\"failed-constructor\")", "failed"),
        ("CounterAgent(\"blocked-constructor\")", "running"),
    ] {
        if expected_state == "failed" {
            let outputs = ctx.cli([flag::YES, cmd::AGENT, "upsert", address]).await;
            assert!(!outputs.success(), "the constructor must fail");
        } else if expected_state == "running" {
            let outputs = ctx
                .cli([
                    flag::YES,
                    cmd::AGENT,
                    cmd::INVOKE,
                    "--trigger",
                    address,
                    "increment",
                ])
                .await;
            assert!(outputs.success_or_dump());
        }
        let outputs = ctx
            .cli([flag::FORMAT, "json", cmd::AGENT, cmd::GET, address])
            .await;
        assert!(outputs.success_or_dump());
        let metadata = single_json(&outputs)["metadata"].clone();
        let fingerprint: Uuid = metadata["fingerprint"].as_str().unwrap().parse().unwrap();
        let expected_id = format!(
            "{}.{}",
            metadata["agentId"].as_str().unwrap(),
            Uuid::new_v5(&fingerprint, b"constructor")
        );
        let outputs = ctx
            .cli([
                flag::FORMAT,
                "json",
                INVOCATION,
                cmd::LIST,
                "--agent",
                address,
                "--kind",
                "constructor",
            ])
            .await;
        assert!(outputs.success_or_dump());
        let listed = single_json(&outputs);
        let constructors = listed["invocations"].as_array().unwrap();
        assert_eq!(constructors.len(), 1, "{listed}");
        assert_eq!(constructors[0]["invocationId"], expected_id);
        let outputs = ctx
            .cli([flag::FORMAT, "json", INVOCATION, cmd::GET, &expected_id])
            .await;
        assert!(outputs.success_or_dump());
        let got = single_json(&outputs);
        assert_eq!(got["summary"]["invocationId"], expected_id);
        if expected_state == "running" {
            assert!(
                matches!(
                    got["summary"]["state"].as_str(),
                    Some("pending" | "running" | "waiting")
                ),
                "{got}"
            );
        } else {
            assert_eq!(got["summary"]["state"], expected_state);
        }
        if expected_state == "failed" {
            assert_eq!(got["result"]["type"], "Failure");
            assert!(
                got["result"]["error"]
                    .as_str()
                    .unwrap()
                    .contains("constructor rejected"),
                "{got}"
            );
        }
    }

    // Cancel exits 0 and reports `cancelled`; get then exits 130.
    let outputs = ctx
        .cli([
            flag::FORMAT,
            "json",
            INVOCATION,
            "cancel",
            "--agent",
            &agent,
            "--idempotency-key",
            "scheduled-key",
        ])
        .await;
    assert!(outputs.success_or_dump());
    let cancelled = single_json(&outputs);
    assert_eq!(cancelled["$type"], "invocation.cancel");
    assert_eq!(cancelled["state"], "cancelled");
    let outputs = ctx.cli(get_scheduled).await;
    assert_eq!(outputs.exit_code(), Some(130));
    assert_eq!(single_json(&outputs)["summary"]["state"], "cancelled");

    // Cancelling a finished invocation keeps its state.
    let outputs = ctx
        .cli([flag::FORMAT, "json", INVOCATION, "cancel", &completed_id])
        .await;
    assert!(outputs.success_or_dump());
    assert_eq!(single_json(&outputs)["state"], "completed");

    // An unknown key is an error.
    let outputs = ctx
        .cli([
            INVOCATION,
            cmd::GET,
            "--agent",
            &agent,
            "--idempotency-key",
            "unknown-key",
        ])
        .await;
    assert!(!outputs.success());

    // The old command is gone.
    let outputs = ctx
        .cli([cmd::AGENT, "cancel-invocation", &agent, "scheduled-key"])
        .await;
    assert!(!outputs.success());
}
