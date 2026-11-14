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
use axum::Router;
use axum::extract::Query;
use axum::routing::any;
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use pretty_assertions::assert_eq;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use test_r::{inherit_test_dep, test, timeout};
use tokio::sync::Notify;
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_ts")]
    PrecompiledComponent
);

const GENERATOR: &str = "golem-it::test-generator";
const GENERATOR_PARENT: &str = "golem-it::test-generator-parent";
const HTTP_SEND: &str = "http::client::send";

/// Records each callback in arrival order and answers `<counter>-<payload>`, where the counter is
/// taken when the response is produced. `slow` answers only after `fast` has answered, and
/// `scoped` answers only after `outside` has arrived, so the custom body is still waiting while
/// its sibling, or the outside-scope call, runs and completes first.
async fn start_callback_server() -> (u16, Arc<Mutex<Vec<String>>>) {
    #[derive(Deserialize)]
    struct Params {
        payload: String,
    }

    let history = Arc::new(Mutex::new(Vec::<String>::new()));
    let counter = Arc::new(AtomicU32::new(0));
    let fast_answered = Arc::new(Notify::new());
    let outside_arrived = Arc::new(Notify::new());
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let route_history = history.clone();
    let route = Router::new().route(
        "/callback",
        any(move |Query(params): Query<Params>| {
            let history = route_history.clone();
            let counter = counter.clone();
            let fast_answered = fast_answered.clone();
            let outside_arrived = outside_arrived.clone();
            async move {
                history.lock().unwrap().push(params.payload.clone());
                match params.payload.as_str() {
                    "slow" => fast_answered.notified().await,
                    "scoped" => outside_arrived.notified().await,
                    "outside" => outside_arrived.notify_one(),
                    _ => {}
                }
                let result = format!(
                    "{}-{}",
                    counter.fetch_add(1, Ordering::AcqRel),
                    params.payload
                );
                if params.payload == "fast" {
                    fast_answered.notify_one();
                }
                result
            }
        }),
    );
    tokio::spawn(
        async move {
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );
    (port, history)
}

/// The oplog entries of the single agent invocation of `method_name`, from its
/// `AgentInvocationStarted` (whose index is the invocation anchor) up to its
/// `AgentInvocationFinished`.
fn invocation_entries<'a>(
    oplog: &'a [PublicOplogEntryWithIndex],
    method_name: &str,
) -> (OplogIndex, &'a [PublicOplogEntryWithIndex]) {
    let start = oplog
        .iter()
        .position(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(params)
                    if matches!(
                        &params.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(invocation)
                            if invocation.method_name == method_name
                    )
            )
        })
        .unwrap_or_else(|| panic!("no invocation of {method_name}"));
    let end = oplog[start..]
        .iter()
        .position(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("{method_name} did not finish"));
    (oplog[start].oplog_index, &oplog[start..=end])
}

struct CustomStart {
    index: OplogIndex,
    function_name: String,
    parent: Option<OplogIndex>,
    ordinal: u64,
}

/// Custom durable `Start`s of one agent invocation, ordered by their invocation ordinal.
fn custom_starts(entries: &[PublicOplogEntryWithIndex], anchor: OplogIndex) -> Vec<CustomStart> {
    let mut starts: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == GENERATOR
                    || params.function_name == GENERATOR_PARENT =>
            {
                let id = params
                    .invocation_id
                    .expect("custom Start carries an invocation ID");
                // A root operation is anchored at its agent invocation, a nested one at the Start
                // of its parent.
                assert_eq!(
                    id.anchor,
                    params.parent_start_index.unwrap_or(anchor),
                    "custom Start belongs to its invocation"
                );
                Some(CustomStart {
                    index: entry.oplog_index,
                    function_name: params.function_name.clone(),
                    parent: params.parent_start_index,
                    ordinal: id.ordinal,
                })
            }
            _ => None,
        })
        .collect();
    starts.sort_by_key(|start| (start.ordinal, start.index));
    starts
}

/// The observational owners of the HTTP sends of one invocation, in oplog order. `None` is an
/// ordinary host call outside every custom scope.
fn http_send_owners(entries: &[PublicOplogEntryWithIndex]) -> Vec<Option<OplogIndex>> {
    entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == HTTP_SEND => {
                Some(params.observational_owner)
            }
            _ => None,
        })
        .collect()
}

fn owned_sends(owners: &[Option<OplogIndex>], owner: OplogIndex) -> usize {
    owners.iter().filter(|o| **o == Some(owner)).count()
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn ts_custom_durability_scopes_stay_apart_under_concurrency(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let (port, history) = start_callback_server().await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;
    let agent_id = agent_id!("CustomDurabilityTs", "scopes");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let nested = executor
        .invoke_and_await_agent(&component, &agent_id, "nested", data_value!(port as f64))
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(nested, vec!["0-nested", "1-root-sibling"]);

    let concurrent = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "concurrentSiblings",
            data_value!(port as f64),
        )
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(
        concurrent,
        vec!["3-slow", "2-fast"],
        "the first sibling completes after the second"
    );

    let outside = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "outsideScopeWhileWaiting",
            data_value!(port as f64),
        )
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(
        outside,
        vec!["5-scoped", "4-outside"],
        "the outside call completes while the custom body waits"
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    let (anchor, entries) = invocation_entries(&oplog, "nested");
    let starts = custom_starts(entries, anchor);
    let names: Vec<_> = starts.iter().map(|s| s.function_name.as_str()).collect();
    assert_eq!(names, vec![GENERATOR_PARENT, GENERATOR, GENERATOR]);
    assert_eq!(starts[0].parent, None, "the outer operation is at the root");
    assert_eq!(
        starts[1].parent,
        Some(starts[0].index),
        "the nested operation is a child of the outer one"
    );
    assert_eq!(
        starts[2].parent, None,
        "the later operation is a root sibling"
    );
    let owners = http_send_owners(entries);
    assert_eq!(owners.len(), 2);
    assert_eq!(owned_sends(&owners, starts[1].index), 1);
    assert_eq!(owned_sends(&owners, starts[2].index), 1);

    let (anchor, entries) = invocation_entries(&oplog, "concurrentSiblings");
    let starts = custom_starts(entries, anchor);
    assert_eq!(starts.len(), 2);
    let owners = http_send_owners(entries);
    assert_eq!(owners.len(), 2);
    for (ordinal, start) in starts.iter().enumerate() {
        assert_eq!(start.function_name, GENERATOR);
        assert_eq!(start.ordinal, ordinal as u64);
        assert_eq!(start.parent, None, "concurrent operations are siblings");
        assert_eq!(
            owned_sends(&owners, start.index),
            1,
            "each sibling owns exactly its own HTTP send"
        );
    }

    let (anchor, entries) = invocation_entries(&oplog, "outsideScopeWhileWaiting");
    let starts = custom_starts(entries, anchor);
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].parent, None);
    let owners = http_send_owners(entries);
    assert_eq!(owners.len(), 2);
    assert_eq!(
        owned_sends(&owners, starts[0].index),
        1,
        "the custom operation owns only its own HTTP send"
    );
    assert_eq!(
        owners.iter().filter(|owner| owner.is_none()).count(),
        1,
        "the call outside the scope stays an ordinary host call"
    );

    let before_crash = history.lock().unwrap().clone();
    let mut sorted = before_crash.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec![
            "fast",
            "nested",
            "outside",
            "root-sibling",
            "scoped",
            "slow"
        ]
    );

    executor.simulated_crash(&worker_id).await?;
    let after_recovery = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "callback",
            data_value!(port as f64, "after"),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(after_recovery, "6-after");
    let mut expected = before_crash;
    expected.push("after".to_string());
    assert_eq!(
        *history.lock().unwrap(),
        expected,
        "replay must not repeat completed effects"
    );

    Ok(())
}
