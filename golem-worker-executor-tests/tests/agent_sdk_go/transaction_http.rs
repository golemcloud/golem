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

//! HTTP calls made by Go transaction steps.

use crate::Tracing;
use crate::transactions::SagaPeer;
use golem_common::model::IdempotencyKey;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use std::collections::HashMap;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// A fallible transaction whose two steps each make an outbound HTTP call.
#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn go_transaction_with_outgoing_http(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let peer = SagaPeer::start("").await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_key!("HttpAgent", "go-transaction-http-1");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), peer.port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let body = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "transaction-callback",
            data_value!("step"),
        )
        .await?
        .into_typed::<String>()?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    assert_eq!(body, "step-1,step-2");
    let effects = peer.effects.lock().unwrap().clone();
    assert_eq!(
        effects.iter().map(|(e, _)| e.as_str()).collect::<Vec<_>>(),
        ["step-1", "step-2"]
    );
    assert_eq!(*peer.attempts.lock().unwrap(), effects);
    assert_ne!(effects[0].1, effects[1].1);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn go_http_compensation_survives_executor_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let peer = SagaPeer::start("undo-step-1").await;
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_key!("HttpAgent", "go-http-compensation-owner-loss");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::from([
                ("PORT".to_string(), peer.port.to_string()),
                ("SAGA_FAIL".to_string(), "true".to_string()),
            ]),
            Vec::new(),
        )
        .await?;
    let key = IdempotencyKey::fresh();
    let mut invocation = {
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        let key = key.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent_id,
                    &key,
                    "transaction-callback",
                    data_value!("step"),
                )
                .await
        })
    };
    tokio::select! {
        _ = peer.held.notified() => {}
        result = &mut invocation => panic!("saga finished before the compensation hold: {result:?}; attempts: {:?}", peer.attempts.lock().unwrap()),
        _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => panic!("saga did not reach the compensation hold within 30s; attempts: {:?}", peer.attempts.lock().unwrap()),
    }
    assert_eq!(
        peer.effects
            .lock()
            .unwrap()
            .iter()
            .map(|(e, _)| e.clone())
            .collect::<Vec<_>>(),
        ["step-1", "step-2", "undo-step-2", "undo-step-1"]
    );
    SagaPeer::assert_checkpoint(
        &executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?,
        3,
    );
    invocation.abort();
    let _ = invocation.await;
    executor.release().await?;
    peer.release.notify_one();

    let executor = start(deps, &context).await?;
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &key,
            "transaction-callback",
            data_value!("step"),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(result, "rolled-back");
    peer.assert_recovered(
        &["step-1", "step-2", "undo-step-2", "undo-step-1"],
        "undo-step-1",
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(&entry.entry,
        PublicOplogEntry::Start(start) if start.function_name == "http::client::send"))
            .count(),
        4,
        "repair must reuse the interrupted HTTP Start"
    );
    let finished = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry,
        PublicOplogEntry::AgentInvocationFinished(done)
            if done.method_name.as_deref() == Some("transaction-callback"))
        })
        .collect::<Vec<_>>();
    assert_eq!(finished.len(), 1);
    for entry in &oplog {
        if matches!(&entry.entry, PublicOplogEntry::Start(_)) {
            assert!(entry.oplog_index < finished[0].oplog_index);
            assert!(
                oplog.iter().any(|terminal| match &terminal.entry {
                    PublicOplogEntry::End(end) => end.start_index == entry.oplog_index,
                    PublicOplogEntry::Cancelled(end) => end.start_index == entry.oplog_index,
                    _ => false,
                }),
                "unsettled Start: {entry:?}"
            );
        }
    }
    executor.check_oplog_is_queryable(&worker_id).await?;
    Ok(())
}
