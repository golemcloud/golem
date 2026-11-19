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
use crate::app::{Output, TestContext, cmd, flag};
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

    // Pages of one return every invocation once: the constructor run and the two calls.
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
        assert!(listed.len() <= 3, "{listed:?}");
    }
    assert_eq!(listed.len(), 3, "{listed:?}");
    assert_eq!(listed.iter().collect::<HashSet<_>>().len(), 3);
    assert!(listed.contains(&completed_id));
    assert!(listed.iter().any(|id| id == scheduled_id));

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
