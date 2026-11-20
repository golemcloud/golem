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

//! Custom durability scopes of the Effect SDK under concurrent fibers, with the agent in
//! `test-components/custom-durability-effect`. Mirrors the Rust cases in `durability.rs`.

use crate::Tracing;
use axum::Router;
use axum::extract::Query;
use axum::routing::any;
use golem_common::model::oplog::{
    DurableInvocationId, OplogIndex, PublicAgentInvocation, PublicOplogEntry,
    PublicOplogEntryWithIndex,
};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use pretty_assertions::assert_eq;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::sync::watch;
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("custom_durability_effect")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

const GENERATOR: &str = "golem-it::test-generator";
const PARENT: &str = "golem-it::test-generator-parent";
const FAILING: &str = "golem-it::test-failing";
const HTTP_SEND: &str = "http::client::send";
const TIMER_SLEEP: &str = "clocks::monotonic-clock::wait-for";

/// Answers `/callback?payload=<p>` with `<n>-<p>`, where `n` counts answered requests. A held
/// payload is answered only after its key is published: `arrived:<p>` and `responded:<p>` are
/// published by the server, any other key by the test through `release`.
struct GateServer {
    port: u16,
    history: Arc<Mutex<Vec<String>>>,
    events: Arc<watch::Sender<HashSet<String>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl GateServer {
    async fn start(holds: &[(&str, &str)]) -> anyhow::Result<Self> {
        #[derive(Deserialize)]
        struct Params {
            payload: String,
        }

        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
        let port = listener.local_addr()?.port();
        let history = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(watch::channel(HashSet::new()).0);
        let holds: Arc<HashMap<String, String>> = Arc::new(
            holds
                .iter()
                .map(|(payload, key)| (payload.to_string(), key.to_string()))
                .collect(),
        );
        let counter = Arc::new(AtomicU32::new(0));
        let route_history = history.clone();
        let route_events = events.clone();
        let route = Router::new().route(
            "/callback",
            any(move |Query(params): Query<Params>| {
                let history = route_history.clone();
                let events = route_events.clone();
                let holds = holds.clone();
                let counter = counter.clone();
                async move {
                    let payload = params.payload;
                    history.lock().unwrap().push(payload.clone());
                    events.send_modify(|keys| {
                        keys.insert(format!("arrived:{payload}"));
                    });
                    if let Some(key) = holds.get(&payload) {
                        let mut keys = events.subscribe();
                        let _ = keys.wait_for(|keys| keys.contains(key)).await.map(|_| ());
                    }
                    let result = format!("{}-{payload}", counter.fetch_add(1, Ordering::AcqRel));
                    events.send_modify(|keys| {
                        keys.insert(format!("responded:{payload}"));
                    });
                    result
                }
            }),
        );
        let handle = tokio::spawn(
            async move { axum::serve(listener, route).await.unwrap() }.in_current_span(),
        );
        Ok(Self {
            port,
            history,
            events,
            handle,
        })
    }

    fn release(&self, key: &str) {
        self.events.send_modify(|keys| {
            keys.insert(key.to_string());
        });
    }

    async fn wait_for(&self, key: &str) -> anyhow::Result<()> {
        let mut keys = self.events.subscribe();
        tokio::time::timeout(
            Duration::from_secs(30),
            keys.wait_for(|keys| keys.contains(key)),
        )
        .await?
        .map(|_| ())?;
        Ok(())
    }

    fn history(&self) -> Vec<String> {
        self.history.lock().unwrap().clone()
    }

    fn count(&self, payload: &str) -> usize {
        self.history().iter().filter(|p| *p == payload).count()
    }
}

impl Drop for GateServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Call {
    index: OplogIndex,
    function: String,
    parent: Option<OplogIndex>,
    id: Option<DurableInvocationId>,
    owner: Option<OplogIndex>,
}

fn calls(oplog: &[PublicOplogEntryWithIndex]) -> Vec<Call> {
    oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) => Some(Call {
                index: entry.oplog_index,
                function: params.function_name.clone(),
                parent: params.parent_start_index,
                id: params.invocation_id,
                owner: params.observational_owner,
            }),
            _ => None,
        })
        .collect()
}

fn anchor(oplog: &[PublicOplogEntryWithIndex], method: &str) -> OplogIndex {
    oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(params)
                if matches!(&params.invocation,
                    PublicAgentInvocation::AgentMethodInvocation(call) if call.method_name == method) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no invocation of {method}"))
}

/// The custom operations of one agent invocation: its root operations in initiation order, then
/// each operation's children. A nested operation's identity is anchored at its parent's `Start`.
fn custom_calls(oplog: &[PublicOplogEntryWithIndex], anchor: OplogIndex) -> Vec<Call> {
    let all: Vec<_> = calls(oplog)
        .into_iter()
        .filter(|call| call.function.starts_with("golem-it::"))
        .collect();
    let mut anchors = HashSet::from([anchor]);
    let mut customs: Vec<Call> = Vec::new();
    loop {
        let found: Vec<_> = all
            .iter()
            .filter(|call| call.id.is_some_and(|id| anchors.contains(&id.anchor)))
            .filter(|call| !customs.contains(call))
            .cloned()
            .collect();
        if found.is_empty() {
            break;
        }
        anchors.extend(found.iter().map(|call| call.index));
        customs.extend(found);
    }
    customs.sort_by_key(|call| {
        let id = call.id.unwrap();
        (id.anchor, id.ordinal)
    });
    let identities: HashSet<_> = customs.iter().map(|call| call.id.unwrap()).collect();
    assert_eq!(
        identities.len(),
        customs.len(),
        "custom identities are unique"
    );
    customs
}

/// HTTP sends recorded as observational effects of the custom operation at `owner`.
fn owned_sends(oplog: &[PublicOplogEntryWithIndex], owner: OplogIndex) -> usize {
    calls(oplog)
        .iter()
        .filter(|call| call.function == HTTP_SEND && call.owner == Some(owner))
        .inspect(|call| assert_eq!(call.id, None, "an observational effect has no identity"))
        .count()
}

/// HTTP sends of the agent invocation at `anchor` that no custom operation owns.
fn root_sends(oplog: &[PublicOplogEntryWithIndex], anchor: OplogIndex) -> Vec<Call> {
    calls(oplog)
        .into_iter()
        .filter(|call| call.function == HTTP_SEND && call.owner.is_none())
        .filter(|call| call.id.is_some_and(|id| id.anchor == anchor))
        .collect()
}

fn ends(oplog: &[PublicOplogEntryWithIndex], start: OplogIndex) -> Vec<OplogIndex> {
    oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::End(params) if params.start_index == start)
        })
        .map(|entry| entry.oplog_index)
        .collect()
}

fn single_end(oplog: &[PublicOplogEntryWithIndex], start: OplogIndex) -> OplogIndex {
    let ends = ends(oplog, start);
    assert_eq!(ends.len(), 1, "Start {start} has exactly one End");
    ends[0]
}

/// Two root generators: the first initiated completes last, and each owns only its own send.
fn assert_reversed_siblings(oplog: &[PublicOplogEntryWithIndex], method: &str) {
    let anchor = anchor(oplog, method);
    let customs = custom_calls(oplog, anchor);
    assert_eq!(customs.len(), 2, "{method}: two custom operations");
    for call in &customs {
        assert_eq!(call.function, GENERATOR);
        assert_eq!(
            call.parent, None,
            "{method}: concurrent operations are siblings"
        );
        assert_eq!(
            owned_sends(oplog, call.index),
            1,
            "{method}: each owns its send"
        );
    }
    assert!(
        single_end(oplog, customs[0].index) > single_end(oplog, customs[1].index),
        "{method}: completion order is the reverse of initiation order"
    );
    assert_eq!(
        root_sends(oplog, anchor).len(),
        1,
        "{method}: only the completion signal is unowned"
    );
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn effect_custom_durability_isolates_concurrent_fibers_and_replays_completed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("custom_durability_effect")] custom_durability_effect: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let server = GateServer::start(&[
        ("slow", "arrived:fast-done"),
        ("fast", "arrived:slow"),
        ("timer-slow", "arrived:timer-fast-done"),
        ("timer-fast", "arrived:timer-slow"),
        ("nested-a", "arrived:nested-b-done"),
        ("nested-b", "arrived:nested-a"),
        ("held", "responded:outside"),
    ])
    .await?;
    let port = server.port as f64;
    let component = executor
        .component_dep(&context.default_environment_id, custom_durability_effect)
        .store()
        .await?;
    let agent_id = agent_key!("EffectCustomDurability", "concurrent-fibers");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "warmup", data_value!())
        .await?;
    macro_rules! invoke {
        ($method:expr) => {
            executor
                .invoke_and_await_agent(&component, &agent_id, $method, data_value!(port))
                .await?
                .into_typed::<Vec<String>>()?
        };
    }

    assert_eq!(invoke!("nestedNoInput"), vec!["0-nested", "1-root-sibling"]);
    assert_eq!(invoke!("concurrentNoInput"), vec!["4-slow", "2-fast"]);
    assert_eq!(
        invoke!("concurrentTimerBodies"),
        vec!["7-timer-slow", "5-timer-fast"]
    );
    assert_eq!(
        invoke!("concurrentNested"),
        vec!["10-nested-a", "8-nested-b"]
    );
    assert_eq!(invoke!("outsideScopeCall"), vec!["12-held", "11-outside"]);
    assert_eq!(invoke!("encoderCall"), vec!["13-encoder-body"]);

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    let nested_anchor = anchor(&oplog, "nestedNoInput");
    let nested = custom_calls(&oplog, nested_anchor);
    assert_eq!(nested.len(), 3);
    let outer = nested.iter().find(|call| call.function == PARENT).unwrap();
    assert_eq!(outer.parent, None);
    assert_eq!(owned_sends(&oplog, outer.index), 0);
    let inner = nested
        .iter()
        .find(|call| call.function == GENERATOR && call.parent == Some(outer.index))
        .expect("the nested generator has the outer operation as its parent");
    assert_eq!(owned_sends(&oplog, inner.index), 1);
    let sibling = nested
        .iter()
        .find(|call| call.function == GENERATOR && call.parent.is_none())
        .expect("the later generator is a root operation");
    assert_eq!(owned_sends(&oplog, sibling.index), 1);
    assert!(single_end(&oplog, inner.index) < single_end(&oplog, outer.index));
    assert_eq!(root_sends(&oplog, nested_anchor), vec![]);

    assert_reversed_siblings(&oplog, "concurrentNoInput");
    assert_reversed_siblings(&oplog, "concurrentTimerBodies");

    let concurrent_nested_anchor = anchor(&oplog, "concurrentNested");
    let concurrent_nested = custom_calls(&oplog, concurrent_nested_anchor);
    assert_eq!(concurrent_nested.len(), 4);
    let parents: Vec<_> = concurrent_nested
        .iter()
        .filter(|call| call.function == PARENT)
        .collect();
    assert_eq!(parents.len(), 2);
    let mut children_by_parent = HashMap::new();
    for parent in &parents {
        assert_eq!(parent.parent, None, "concurrent parents are siblings");
        assert_eq!(owned_sends(&oplog, parent.index), 0);
        let children: Vec<_> = concurrent_nested
            .iter()
            .filter(|call| call.function == GENERATOR && call.parent == Some(parent.index))
            .collect();
        assert_eq!(children.len(), 1, "each parent has exactly its own child");
        assert_eq!(owned_sends(&oplog, children[0].index), 1);
        assert!(single_end(&oplog, children[0].index) < single_end(&oplog, parent.index));
        children_by_parent.insert(parent.index, children[0].index);
    }
    assert!(
        single_end(&oplog, children_by_parent[&parents[1].index])
            < single_end(&oplog, children_by_parent[&parents[0].index]),
        "the second child completes first"
    );
    assert_eq!(
        root_sends(&oplog, concurrent_nested_anchor).len(),
        1,
        "only the completion signal is unowned"
    );

    let outside_anchor = anchor(&oplog, "outsideScopeCall");
    let outside = custom_calls(&oplog, outside_anchor);
    assert_eq!(outside.len(), 1);
    assert_eq!(
        owned_sends(&oplog, outside[0].index),
        1,
        "the waiting operation does not own its sibling's call"
    );
    assert_eq!(
        root_sends(&oplog, outside_anchor).len(),
        1,
        "the unscoped sibling call stays a root call"
    );

    let encoder_anchor = anchor(&oplog, "encoderCall");
    let encoder = custom_calls(&oplog, encoder_anchor);
    assert_eq!(encoder.len(), 1);
    assert_eq!(
        owned_sends(&oplog, encoder[0].index),
        2,
        "the response encoder's call belongs to the operation"
    );
    assert_eq!(root_sends(&oplog, encoder_anchor), vec![]);

    let history = server.history();
    let custom_starts = calls(&oplog)
        .iter()
        .filter(|call| call.function.starts_with("golem-it::"))
        .count();
    executor.simulated_crash(&worker_id).await?;
    let after = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "callback",
            data_value!(port, "after".to_string()),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(after, format!("{}-after", history.len()));
    assert_eq!(
        server.history(),
        [history, vec!["after".to_string()]].concat(),
        "replay does not repeat completed effects"
    );
    let recovered = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        calls(&recovered)
            .iter()
            .filter(|call| call.function.starts_with("golem-it::"))
            .count(),
        custom_starts + 1,
        "replay records no new custom Starts"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn effect_custom_durability_recovers_one_completed_and_one_incomplete_sibling(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("custom_durability_effect")] custom_durability_effect: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let server = GateServer::start(&[
        ("first", "arrived:second"),
        ("second", "resume"),
        ("release", "resume"),
    ])
    .await?;
    let port = server.port as f64;
    let component = executor
        .component_dep(&context.default_environment_id, custom_durability_effect)
        .store()
        .await?;
    let agent_id = agent_key!("EffectCustomDurability", "partial-recovery");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "warmup", data_value!())
        .await?;
    let invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &agent_id,
                    "concurrentFirstFinishes",
                    data_value!(port),
                )
                .await
        }
        .in_current_span()
    });
    // The agent sends "release" only after the first operation finished.
    server.wait_for("arrived:release").await?;
    executor.commit_oplog(&worker_id.agent_id).await?;
    let before = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let invocation_anchor = anchor(&before, "concurrentFirstFinishes");
    let siblings = custom_calls(&before, invocation_anchor);
    assert_eq!(siblings.len(), 2);
    for sibling in &siblings {
        assert_eq!(sibling.function, GENERATOR);
        assert_eq!(sibling.parent, None);
        assert_eq!(owned_sends(&before, sibling.index), 1);
    }
    assert_eq!(ends(&before, siblings[0].index).len(), 1);
    assert_eq!(ends(&before, siblings[1].index), vec![]);
    let release = root_sends(&before, invocation_anchor);
    assert_eq!(release.len(), 1, "the release request is a root call");
    assert!(release[0].id.unwrap().ordinal > siblings[1].id.unwrap().ordinal);
    assert_eq!(ends(&before, release[0].index), vec![]);
    let timer_owners: Vec<_> = calls(&before)
        .into_iter()
        .filter(|call| call.function == TIMER_SLEEP)
        .filter(|call| call.index > invocation_anchor)
        .map(|call| call.owner)
        .collect();
    assert_eq!(
        timer_owners,
        vec![Some(siblings[0].index)],
        "the body's timer sleep belongs to the completed sibling"
    );

    executor.simulated_crash(&worker_id).await?;
    server.release("resume");
    let result = invocation.await??.into_typed::<Vec<String>>()?;
    assert_eq!(result[0], "0-first");
    assert!(result[1].ends_with("-second"), "{result:?}");
    assert_eq!(
        server.count("first"),
        1,
        "the completed sibling is not repeated"
    );
    assert_eq!(
        server.count("second"),
        2,
        "the incomplete sibling re-executes"
    );

    let recovered = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        custom_calls(&recovered, invocation_anchor),
        siblings,
        "recovery retains both identities"
    );
    for sibling in &siblings {
        single_end(&recovered, sibling.index);
    }
    assert_eq!(owned_sends(&recovered, siblings[0].index), 1);
    assert!(owned_sends(&recovered, siblings[1].index) >= 1);
    assert!(
        recovered
            .iter()
            .all(|entry| !matches!(&entry.entry, PublicOplogEntry::Jump(_))),
        "call-local recovery does not roll back sibling history"
    );
    assert_eq!(
        root_sends(&recovered, invocation_anchor),
        release,
        "the release request completes under its original Start"
    );
    single_end(&recovered, release[0].index);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn effect_custom_durability_cleans_up_failure_interruption_and_escaped_work(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("custom_durability_effect")] custom_durability_effect: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let server = GateServer::start(&[
        ("interrupted", "never"),
        ("cancel", "arrived:interrupted"),
        ("survivor", "responded:root-probe"),
    ])
    .await?;
    let port = server.port as f64;
    let component = executor
        .component_dep(&context.default_environment_id, custom_durability_effect)
        .store()
        .await?;
    let agent_id = agent_key!("EffectCustomDurability", "cleanup");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "warmup", data_value!())
        .await?;
    macro_rules! invoke {
        ($method:expr) => {
            executor
                .invoke_and_await_agent(&component, &agent_id, $method, data_value!(port))
                .await?
                .into_typed::<Vec<String>>()?
        };
    }

    assert_eq!(
        invoke!("failureAndInterruption"),
        vec!["failed:0-failing", "1-cancel", "2-after-cancel"]
    );
    assert_eq!(
        invoke!("escapedWork"),
        vec!["3-escape-body", "4-escaped", "5-after-escape"]
    );
    assert_eq!(invoke!("survivingChild"), vec!["parent-body", "7-survivor"]);

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;

    let cleanup_anchor = anchor(&oplog, "failureAndInterruption");
    let cleanup = custom_calls(&oplog, cleanup_anchor);
    assert_eq!(
        cleanup
            .iter()
            .map(|call| call.function.as_str())
            .collect::<Vec<_>>(),
        vec![FAILING, GENERATOR, GENERATOR]
    );
    for call in &cleanup {
        assert_eq!(
            call.parent, None,
            "no operation is nested under a closed scope"
        );
        assert_eq!(owned_sends(&oplog, call.index), 1);
    }
    single_end(&oplog, cleanup[0].index);
    single_end(&oplog, cleanup[2].index);
    assert_eq!(
        root_sends(&oplog, cleanup_anchor).len(),
        1,
        "the racing cancel request is a root call"
    );

    let escape_anchor = anchor(&oplog, "escapedWork");
    let escape = custom_calls(&oplog, escape_anchor);
    assert_eq!(
        escape
            .iter()
            .map(|call| call.function.as_str())
            .collect::<Vec<_>>(),
        vec![PARENT, GENERATOR]
    );
    assert_eq!(owned_sends(&oplog, escape[0].index), 1);
    assert_eq!(escape[1].parent, None);
    assert_eq!(owned_sends(&oplog, escape[1].index), 1);
    assert_eq!(
        root_sends(&oplog, escape_anchor).len(),
        1,
        "work that escapes a closed scope runs in the root context"
    );

    let surviving_anchor = anchor(&oplog, "survivingChild");
    let surviving = custom_calls(&oplog, surviving_anchor);
    assert_eq!(
        surviving
            .iter()
            .map(|call| call.function.as_str())
            .collect::<Vec<_>>(),
        vec![PARENT, GENERATOR]
    );
    assert_eq!(surviving[0].parent, None);
    assert_eq!(surviving[1].parent, Some(surviving[0].index));
    assert_eq!(owned_sends(&oplog, surviving[0].index), 0);
    assert_eq!(owned_sends(&oplog, surviving[1].index), 1);
    assert!(
        single_end(&oplog, surviving[1].index) < single_end(&oplog, surviving[0].index),
        "a parent finishes only after its surviving child closed"
    );
    assert_eq!(root_sends(&oplog, surviving_anchor).len(), 1);
    Ok(())
}
