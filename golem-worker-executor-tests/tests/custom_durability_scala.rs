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

//! Custom durable operations of the Scala SDK whose bodies run concurrently within one invocation.

use crate::Tracing;
use axum::Router;
use axum::extract::Query;
use axum::routing::any;
use golem_common::model::oplog::{
    DurableInvocationId, OplogIndex, PublicAgentInvocation, PublicOplogEntry,
    PublicOplogEntryWithIndex,
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
inherit_test_dep!(
    #[tagged_as("tool_streaming_scala")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

const HTTP_SEND: &str = "http::client::send";

/// A callback server that counts its effects. A request whose payload is a key of `holds` is
/// recorded on arrival but answered only after the event named by the value: `x` when the request
/// `x` was answered, `arrived:x` when it arrived.
struct CallbackServer {
    port: u16,
    history: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl CallbackServer {
    async fn start(holds: &[(&str, &str)]) -> anyhow::Result<Self> {
        #[derive(Deserialize)]
        struct Params {
            payload: String,
        }

        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
        let port = listener.local_addr()?.port();
        let history = Arc::new(Mutex::new(Vec::<String>::new()));
        let counter = Arc::new(AtomicU32::new(0));
        let holds: Arc<HashMap<String, String>> = Arc::new(
            holds
                .iter()
                .map(|(held, until)| (held.to_string(), until.to_string()))
                .collect(),
        );
        let answered: Arc<Mutex<HashMap<String, Arc<Notify>>>> = Arc::default();
        let notify_for = move |answered: &Mutex<HashMap<String, Arc<Notify>>>, payload: &str| {
            answered
                .lock()
                .unwrap()
                .entry(payload.to_string())
                .or_insert_with(|| Arc::new(Notify::new()))
                .clone()
        };
        let server_history = history.clone();
        let task = tokio::spawn(
            async move {
                let route = Router::new().route(
                    "/callback",
                    any(move |Query(params): Query<Params>| {
                        let history = server_history.clone();
                        let counter = counter.clone();
                        let holds = holds.clone();
                        let answered = answered.clone();
                        async move {
                            history.lock().unwrap().push(params.payload.clone());
                            notify_for(&answered, &format!("arrived:{}", params.payload))
                                .notify_one();
                            if let Some(until) = holds.get(&params.payload) {
                                notify_for(&answered, until).notified().await;
                            }
                            let result = format!(
                                "{}-{}",
                                counter.fetch_add(1, Ordering::AcqRel),
                                params.payload
                            );
                            notify_for(&answered, &params.payload).notify_one();
                            result
                        }
                    }),
                );
                axum::serve(listener, route).await.unwrap();
            }
            .in_current_span(),
        );
        Ok(Self {
            port,
            history,
            task,
        })
    }

    fn history(&self) -> Vec<String> {
        self.history.lock().unwrap().clone()
    }
}

impl Drop for CallbackServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CallStart {
    index: OplogIndex,
    parent: Option<OplogIndex>,
    invocation_id: Option<DurableInvocationId>,
    owner: Option<OplogIndex>,
}

fn starts_of(oplog: &[PublicOplogEntryWithIndex], function_name: &str) -> Vec<CallStart> {
    oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == function_name => {
                Some(CallStart {
                    index: entry.oplog_index,
                    parent: params.parent_start_index,
                    invocation_id: params.invocation_id,
                    owner: params.observational_owner,
                })
            }
            _ => None,
        })
        .collect()
}

fn method_anchor(oplog: &[PublicOplogEntryWithIndex], method: &str) -> OplogIndex {
    oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(params)
                if matches!(
                    &params.invocation,
                    PublicAgentInvocation::AgentMethodInvocation(invocation)
                        if invocation.method_name == method
                ) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no agent invocation of {method}"))
}

/// Custom `Start`s of `function_name` that belong to the invocation at `anchor`, in ordinal order.
fn custom_starts(
    oplog: &[PublicOplogEntryWithIndex],
    function_name: &str,
    anchor: OplogIndex,
) -> Vec<CallStart> {
    let mut starts: Vec<_> = starts_of(oplog, function_name)
        .into_iter()
        .filter(|start| start.invocation_id.is_some_and(|id| id.anchor == anchor))
        .collect();
    starts.sort_by_key(|start| start.invocation_id.unwrap().ordinal);
    starts
}

fn http_sends_owned_by(oplog: &[PublicOplogEntryWithIndex], owner: Option<OplogIndex>) -> usize {
    starts_of(oplog, HTTP_SEND)
        .into_iter()
        .filter(|start| start.owner == owner)
        .count()
}

/// The index of the `End` that completes the call started at `start`, if any.
fn end_of(oplog: &[PublicOplogEntryWithIndex], start: OplogIndex) -> Option<OplogIndex> {
    oplog.iter().find_map(|entry| match &entry.entry {
        PublicOplogEntry::End(params) if params.start_index == start => Some(entry.oplog_index),
        _ => None,
    })
}

/// HTTP sends at the root of the invocation at `anchor`, outside any custom operation.
fn root_sends(oplog: &[PublicOplogEntryWithIndex], anchor: OplogIndex) -> usize {
    starts_of(oplog, HTTP_SEND)
        .into_iter()
        .filter(|start| {
            start.owner.is_none() && start.invocation_id.is_some_and(|id| id.anchor == anchor)
        })
        .count()
}

fn assert_siblings_with_own_effects(oplog: &[PublicOplogEntryWithIndex], siblings: &[CallStart]) {
    for (ordinal, sibling) in siblings.iter().enumerate() {
        assert_eq!(sibling.parent, None, "concurrent operations are siblings");
        assert_eq!(sibling.invocation_id.unwrap().ordinal, ordinal as u64);
        assert_eq!(
            http_sends_owned_by(oplog, Some(sibling.index)),
            1,
            "each sibling owns exactly its own HTTP call; sends: {:#?}",
            starts_of(oplog, HTTP_SEND)
        );
    }
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn scala_custom_durability_siblings_complete_in_adverse_order(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_scala")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    // The first sibling's request arrives first but is answered after the second one.
    let server = CallbackServer::start(&[("slow", "fast")]).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let agent_id = agent_id!("ScalaCustomDurability", "adverse-order");
    let env = HashMap::from([("PORT".to_string(), server.port.to_string())]);
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let concurrent = executor
        .invoke_and_await_agent(&component, &agent_id, "concurrentNoInput", data_value!())
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(concurrent, vec!["1-slow", "0-fast"]);

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let anchor = method_anchor(&oplog, "concurrentNoInput");
    let siblings = custom_starts(&oplog, "golem-it::test-generator", anchor);
    assert_eq!(siblings.len(), 2, "starts: {siblings:#?}");
    assert_siblings_with_own_effects(&oplog, &siblings);

    executor.simulated_crash(&worker_id).await?;
    let after = executor
        .invoke_and_await_agent(&component, &agent_id, "callback", data_value!("after"))
        .await?
        .into_typed::<String>()?;
    assert_eq!(after, "2-after");
    assert_eq!(
        server.history(),
        vec!["slow", "fast", "after"],
        "replay must not repeat completed custom operations"
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        custom_starts(&oplog, "golem-it::test-generator", anchor),
        siblings,
        "replay must not open new custom operations"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn scala_custom_durability_root_call_while_scope_waits(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_scala")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    // Each request waits for the other one to arrive, so both are in flight at the same time.
    let server =
        CallbackServer::start(&[("held", "arrived:root"), ("root", "arrived:held")]).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let agent_id = agent_id!("ScalaCustomDurability", "root-call");
    let env = HashMap::from([("PORT".to_string(), server.port.to_string())]);
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "rootCallWhileScopeWaits",
            data_value!(),
        )
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(result.len(), 2);
    assert!(
        result[0].ends_with("-held") && result[1].ends_with("-root"),
        "{result:?}"
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let anchor = method_anchor(&oplog, "rootCallWhileScopeWaits");
    let held = custom_starts(&oplog, "golem-it::test-generator", anchor);
    assert_eq!(held.len(), 1, "starts: {held:#?}");
    assert_eq!(held[0].parent, None);
    assert_eq!(http_sends_owned_by(&oplog, Some(held[0].index)), 1);
    assert_eq!(
        http_sends_owned_by(&oplog, None),
        1,
        "the root-level call is an ordinary replayable call; sends: {:#?}",
        starts_of(&oplog, HTTP_SEND)
    );

    executor.simulated_crash(&worker_id).await?;
    let after = executor
        .invoke_and_await_agent(&component, &agent_id, "callback", data_value!("after"))
        .await?
        .into_typed::<String>()?;
    assert_eq!(after, "2-after");
    let mut history = server.history();
    history[..2].sort();
    assert_eq!(history, vec!["held", "root", "after"]);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn scala_custom_durability_nested_operation_has_outer_parent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_scala")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let server = CallbackServer::start(&[]).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let agent_id = agent_id!("ScalaCustomDurability", "nested");
    let env = HashMap::from([("PORT".to_string(), server.port.to_string())]);
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "nestedNoInput", data_value!())
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(result, vec!["0-nested", "1-root-sibling"]);

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let anchor = method_anchor(&oplog, "nestedNoInput");
    let outer = custom_starts(&oplog, "golem-it::test-generator-parent", anchor);
    assert_eq!(outer.len(), 1);
    assert_eq!(outer[0].parent, None);
    assert_eq!(outer[0].invocation_id.unwrap().ordinal, 0);
    // A nested operation's identity is numbered under its enclosing operation's Start.
    let nested = custom_starts(&oplog, "golem-it::test-generator", outer[0].index);
    assert_eq!(
        nested.len(),
        1,
        "starts: {:#?}",
        starts_of(&oplog, "golem-it::test-generator")
    );
    assert_eq!(
        nested[0].parent,
        Some(outer[0].index),
        "the nested operation has the outer operation as its parent"
    );
    assert_eq!(nested[0].invocation_id.unwrap().ordinal, 0);
    let root_sibling = custom_starts(&oplog, "golem-it::test-generator", anchor);
    assert_eq!(root_sibling.len(), 1);
    assert_eq!(
        root_sibling[0].parent, None,
        "the root sibling has no parent"
    );
    assert_eq!(root_sibling[0].invocation_id.unwrap().ordinal, 1);
    assert_eq!(http_sends_owned_by(&oplog, Some(nested[0].index)), 1);
    assert_eq!(http_sends_owned_by(&oplog, Some(root_sibling[0].index)), 1);
    assert_eq!(http_sends_owned_by(&oplog, Some(outer[0].index)), 0);

    executor.simulated_crash(&worker_id).await?;
    let after = executor
        .invoke_and_await_agent(&component, &agent_id, "callback", data_value!("after"))
        .await?
        .into_typed::<String>()?;
    assert_eq!(after, "2-after");
    assert_eq!(server.history(), vec!["nested", "root-sibling", "after"]);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn scala_custom_durability_continuation_runs_in_registering_scope(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_scala")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let server = CallbackServer::start(&[]).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let agent_id = agent_id!("ScalaCustomDurability", "cross-scope");
    let env = HashMap::from([("PORT".to_string(), server.port.to_string())]);
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "crossScopeCompletion", data_value!())
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(result, vec!["1-receiver", "0-completer"]);

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let anchor = method_anchor(&oplog, "crossScopeCompletion");
    let receiver = custom_starts(&oplog, "golem-it::test-receiver", anchor);
    let completer = custom_starts(&oplog, "golem-it::test-completer", anchor);
    assert_eq!(receiver.len(), 1);
    assert_eq!(completer.len(), 1);
    assert_eq!(receiver[0].parent, None);
    assert_eq!(completer[0].parent, None, "operations are siblings");
    assert_eq!(
        http_sends_owned_by(&oplog, Some(receiver[0].index)),
        1,
        "the continuation that the completer resumes runs in the receiver's scope; sends: {:#?}",
        starts_of(&oplog, HTTP_SEND)
    );
    assert_eq!(http_sends_owned_by(&oplog, Some(completer[0].index)), 1);

    executor.simulated_crash(&worker_id).await?;
    let after = executor
        .invoke_and_await_agent(&component, &agent_id, "callback", data_value!("after"))
        .await?
        .into_typed::<String>()?;
    assert_eq!(after, "2-after");
    assert_eq!(server.history(), vec!["completer", "receiver", "after"]);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn scala_custom_durability_recovers_one_completed_and_one_incomplete_sibling(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_scala")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    #[derive(Deserialize)]
    struct Params {
        payload: String,
    }

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let history = Arc::new(Mutex::new(Vec::<String>::new()));
    let second_arrived = Arc::new(Notify::new());
    let checkpoint = Arc::new(Notify::new());
    let resume = Arc::new(tokio::sync::Semaphore::new(0));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    let route_history = history.clone();
    let route_checkpoint = checkpoint.clone();
    let route_resume = resume.clone();
    let route = Router::new().route(
        "/callback",
        any(move |Query(params): Query<Params>| {
            let history = route_history.clone();
            let arrived = second_arrived.clone();
            let checkpoint = route_checkpoint.clone();
            let resume = route_resume.clone();
            async move {
                history.lock().unwrap().push(params.payload.clone());
                match params.payload.as_str() {
                    "first" => arrived.notified().await,
                    "second" => {
                        arrived.notify_one();
                        let _ = resume.acquire().await;
                    }
                    "release" => {
                        // The guest sends this only after the first operation has finished.
                        checkpoint.notify_one();
                        let _ = resume.acquire().await;
                    }
                    _ => {}
                }
                params.payload
            }
        }),
    );
    let server =
        tokio::spawn(async move { axum::serve(listener, route).await.unwrap() }.in_current_span());
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let agent_id = agent_id!("ScalaCustomDurability", "partial-recovery");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::from([("PORT".to_string(), port.to_string())]),
            Vec::new(),
        )
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
                    data_value!(),
                )
                .await
        }
        .in_current_span()
    });
    tokio::time::timeout(std::time::Duration::from_secs(60), checkpoint.notified()).await?;
    executor.commit_oplog(&worker_id).await?;

    let ends = |oplog: &[PublicOplogEntryWithIndex], start: OplogIndex| {
        oplog
            .iter()
            .filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(params) if params.start_index == start))
            .count()
    };
    let before = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let anchor = method_anchor(&before, "concurrentFirstFinishes");
    let siblings = custom_starts(&before, "golem-it::test-generator", anchor);
    assert_eq!(siblings.len(), 2, "starts: {siblings:#?}");
    assert!(siblings.iter().all(|sibling| sibling.parent.is_none()));
    assert_eq!(
        ends(&before, siblings[0].index),
        1,
        "the first sibling has finished"
    );
    assert_eq!(
        ends(&before, siblings[1].index),
        0,
        "the second sibling is incomplete"
    );
    let root_sends: Vec<_> = starts_of(&before, HTTP_SEND)
        .into_iter()
        .filter(|start| start.invocation_id.is_some_and(|id| id.anchor == anchor))
        .collect();
    assert_eq!(
        root_sends.len(),
        1,
        "the release request is a root-level call"
    );
    let release = root_sends[0];
    assert_eq!(release.owner, None);
    assert_eq!(release.parent, None);
    assert_eq!(release.invocation_id.unwrap().ordinal, 2);

    executor.simulated_crash(&worker_id).await?;
    resume.close();
    assert_eq!(
        invocation.await??.into_typed::<Vec<String>>()?,
        vec!["first", "second"]
    );
    let effects = history.lock().unwrap().clone();
    assert_eq!(
        effects.iter().filter(|payload| *payload == "first").count(),
        1
    );
    assert_eq!(
        effects
            .iter()
            .filter(|payload| *payload == "second")
            .count(),
        2
    );

    let recovered = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        custom_starts(&recovered, "golem-it::test-generator", anchor),
        siblings,
        "recovery keeps both identities"
    );
    for sibling in &siblings {
        assert_eq!(ends(&recovered, sibling.index), 1);
        assert!(http_sends_owned_by(&recovered, Some(sibling.index)) >= 1);
    }
    let recovered_root_sends: Vec<_> = starts_of(&recovered, HTTP_SEND)
        .into_iter()
        .filter(|start| start.invocation_id.is_some_and(|id| id.anchor == anchor))
        .collect();
    assert_eq!(
        recovered_root_sends,
        vec![release],
        "the release request completes under its original Start"
    );
    assert_eq!(ends(&recovered, release.index), 1);
    server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn scala_custom_durability_failed_body_leaves_no_scope_behind(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_scala")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let server = CallbackServer::start(&[]).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let agent_id = agent_id!("ScalaCustomDurability", "failed-body");
    let env = HashMap::from([("PORT".to_string(), server.port.to_string())]);
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "failedBody", data_value!())
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let anchor = method_anchor(&oplog, "failedBody");
    let result = result.into_typed::<Vec<String>>()?;
    assert_eq!(
        result,
        vec!["body failed", "1-after-failure", "2-root-after"]
    );

    let failing = custom_starts(&oplog, "golem-it::test-failing", anchor);
    assert_eq!(failing.len(), 1);
    assert_eq!(failing[0].parent, None);
    assert_eq!(failing[0].invocation_id.unwrap().ordinal, 0);
    assert_eq!(http_sends_owned_by(&oplog, Some(failing[0].index)), 1);
    assert_eq!(
        end_of(&oplog, failing[0].index),
        None,
        "a failed body records no result"
    );
    let after = custom_starts(&oplog, "golem-it::test-generator", anchor);
    assert_eq!(after.len(), 1, "starts: {after:#?}");
    assert_eq!(
        after[0].parent, None,
        "the dropped operation is not a parent of later work"
    );
    assert_eq!(after[0].invocation_id.unwrap().ordinal, 1);
    assert_eq!(http_sends_owned_by(&oplog, Some(after[0].index)), 1);
    assert!(end_of(&oplog, after[0].index).is_some());
    assert_eq!(
        root_sends(&oplog, anchor),
        1,
        "the root call after the failure is not owned by it"
    );

    executor.simulated_crash(&worker_id).await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "callback", data_value!("after"))
        .await?;
    let history = server.history();
    let count = |payload: &str| history.iter().filter(|p| *p == payload).count();
    assert_eq!(
        count("fail"),
        1,
        "replay of a completed invocation repeats no effect of the failed body: {history:?}"
    );
    assert_eq!(count("after-failure"), 1, "history: {history:?}");
    assert_eq!(count("root-after"), 1, "history: {history:?}");
    assert_eq!(count("after"), 1, "history: {history:?}");
    let recovered = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        custom_starts(&recovered, "golem-it::test-failing", anchor),
        failing
    );
    assert_eq!(
        custom_starts(&recovered, "golem-it::test-generator", anchor),
        after
    );
    assert_eq!(
        end_of(&recovered, failing[0].index),
        None,
        "replay does not repair the dropped operation"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn scala_custom_durability_parasitic_continuations_follow_the_completer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_scala")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let server = CallbackServer::start(&[("completer-after", "arrived:crossed")]).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let agent_id = agent_id!("ScalaCustomDurability", "parasitic");
    let env = HashMap::from([("PORT".to_string(), server.port.to_string())]);
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "parasiticContinuations",
            data_value!(),
        )
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let anchor = method_anchor(&oplog, "parasiticContinuations");
    let result = result.into_typed::<Vec<String>>()?;
    assert_eq!(result.len(), 3);
    for (answer, payload) in
        result
            .iter()
            .zip(["inside-parasitic", "receiver-after", "completer-after"])
    {
        assert!(answer.ends_with(&format!("-{payload}")), "{result:?}");
    }

    let owned = |function: &str| {
        let starts = custom_starts(&oplog, function, anchor);
        assert_eq!(starts.len(), 1, "{function}");
        assert_eq!(starts[0].parent, None, "{function}");
        http_sends_owned_by(&oplog, Some(starts[0].index))
    };
    assert_eq!(
        owned("golem-it::test-parasitic-inside"),
        2,
        "a parasitic continuation completed in its own scope stays in it"
    );
    assert_eq!(
        owned("golem-it::test-parasitic-completer"),
        3,
        "a parasitic continuation runs in the scope of the code that completes its source"
    );
    assert_eq!(
        owned("golem-it::test-parasitic-receiver"),
        1,
        "a continuation registered through the Golem execution context returns to its scope"
    );
    assert_eq!(root_sends(&oplog, anchor), 0);

    let live_history = server.history();
    executor.simulated_crash(&worker_id).await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "callback", data_value!("after"))
        .await?;
    let mut expected = live_history;
    expected.push("after".to_string());
    assert_eq!(
        server.history(),
        expected,
        "replay must not repeat completed effects"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn scala_custom_durability_promise_conversions_stay_in_scope(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_scala")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let server = CallbackServer::start(&[("root-promise", "arrived:promise-sibling")]).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let agent_id = agent_id!("ScalaCustomDurability", "promise");
    let env = HashMap::from([("PORT".to_string(), server.port.to_string())]);
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "promiseConversion", data_value!())
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let anchor = method_anchor(&oplog, "promiseConversion");
    let result = result.into_typed::<Vec<String>>()?;
    assert!(
        result[0].ends_with("-js-then") && result[1].ends_with("-promise-sibling"),
        "{result:?}"
    );

    let converted = custom_starts(&oplog, "golem-it::test-promise", anchor);
    let sibling = custom_starts(&oplog, "golem-it::test-generator", anchor);
    assert_eq!(converted.len(), 1);
    assert_eq!(sibling.len(), 1);
    assert_eq!(converted[0].parent, None);
    assert_eq!(sibling[0].parent, None);
    assert_eq!(
        http_sends_owned_by(&oplog, Some(converted[0].index)),
        3,
        "a root promise awaited in the body, a future converted to a promise and its JavaScript \
         `then` all continue in the operation; sends: {:#?}",
        starts_of(&oplog, HTTP_SEND)
    );
    assert_eq!(http_sends_owned_by(&oplog, Some(sibling[0].index)), 1);
    assert_eq!(
        root_sends(&oplog, anchor),
        1,
        "the promise started at the root stays at the root"
    );

    let live_history = server.history();
    executor.simulated_crash(&worker_id).await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "callback", data_value!("after"))
        .await?;
    let mut expected = live_history;
    expected.push("after".to_string());
    assert_eq!(
        server.history(),
        expected,
        "replay must not repeat completed effects"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn scala_custom_durability_escaped_work_runs_in_nearest_open_scope(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_scala")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let server = CallbackServer::start(&[]).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let agent_id = agent_id!("ScalaCustomDurability", "escaped");
    let env = HashMap::from([("PORT".to_string(), server.port.to_string())]);
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "escapedWork", data_value!())
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let anchor = method_anchor(&oplog, "escapedWork");
    let result = result.into_typed::<String>()?;
    assert_eq!(result, "inner,1-escaped,0-child");

    let outer = custom_starts(&oplog, "golem-it::test-escape-outer", anchor);
    assert_eq!(outer.len(), 1);
    assert_eq!(outer[0].parent, None);
    let inner = custom_starts(&oplog, "golem-it::test-escape-inner", outer[0].index);
    assert_eq!(inner.len(), 1);
    assert_eq!(inner[0].parent, Some(outer[0].index));
    let child = custom_starts(&oplog, "golem-it::test-escape-child", inner[0].index);
    assert_eq!(child.len(), 1);
    assert_eq!(child[0].parent, Some(inner[0].index));

    let inner_end = end_of(&oplog, inner[0].index).expect("inner operation finished");
    let child_end = end_of(&oplog, child[0].index).expect("child operation finished");
    assert!(
        child_end < inner_end,
        "the inner operation finishes only after the child it did not await"
    );
    assert_eq!(http_sends_owned_by(&oplog, Some(child[0].index)), 1);
    assert_eq!(http_sends_owned_by(&oplog, Some(inner[0].index)), 0);
    let escaped_send = starts_of(&oplog, HTTP_SEND)
        .into_iter()
        .find(|start| start.owner == Some(outer[0].index))
        .expect("the escaped request is owned by the nearest open operation");
    assert!(
        escaped_send.index > inner_end,
        "the escaped request starts after its scope closed"
    );
    assert_eq!(http_sends_owned_by(&oplog, Some(outer[0].index)), 1);
    assert_eq!(root_sends(&oplog, anchor), 0);

    let live_history = server.history();
    executor.simulated_crash(&worker_id).await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "callback", data_value!("after"))
        .await?;
    let mut expected = live_history;
    expected.push("after".to_string());
    assert_eq!(
        server.history(),
        expected,
        "replay must not repeat completed effects"
    );

    Ok(())
}
