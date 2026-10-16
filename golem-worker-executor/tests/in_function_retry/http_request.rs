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
use golem_common::model::oplog::{OplogIndex, PublicAgentInvocation, PublicOplogEntry};
use golem_common::model::{AgentStatus, IdempotencyKey, OwnedAgentId};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies,
    start_with_overrides,
};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

use super::count_oplog_errors_containing;
use super::http_servers::{
    start_status_code_retry_http_server, start_withheld_status_retry_http_server,
};

#[test]
#[tracing::instrument]
async fn http_status_retry_policy_exposes_latest_response_when_exhausted(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, Default::default()).await?;
    let (port, counter, idempotency_keys) = start_status_code_retry_http_server(20).await;
    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let agent_id = agent_id!("HttpClient4");
    executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "post_with_status_retry_policy",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(result, "500 retry-me");
    assert_eq!(counter.load(Ordering::SeqCst), 11);
    let idempotency_keys = idempotency_keys.lock().unwrap();
    let first_key = idempotency_keys[0]
        .as_ref()
        .expect("initial request must have an idempotency key");
    assert_eq!(idempotency_keys.len(), 11);
    assert!(
        idempotency_keys
            .iter()
            .all(|key| key.as_ref() == Some(first_key))
    );
    Ok(())
}

#[test]
#[tracing::instrument]
async fn p3_status_retry_header_wait_observes_interrupt(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, Default::default()).await?;
    let (port, attempts, requests, mut replacement) =
        start_withheld_status_retry_http_server().await;
    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let owned = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let key = IdempotencyKey::fresh();
    let invocation = executor.invoke_and_await_agent_with_key(
        &component,
        &agent_id,
        &key,
        "post_with_status_retry_policy",
        data_value!(),
    );
    tokio::pin!(invocation);
    tokio::select! {
        _ = replacement.accepted() => {}
        result = &mut invocation => panic!("completed before replacement headers were withheld: {result:?}"),
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    let retry_state = executor
        .attached_agent_status(&worker_id)
        .await?
        .current_retry_state;
    tokio::time::timeout(Duration::from_secs(2), executor.interrupt(&worker_id)).await??;
    let error = tokio::time::timeout(Duration::from_secs(2), &mut invocation)
        .await?
        .expect_err("interruption must not produce an HTTP result");
    assert!(error.to_string().contains("Interrupted via the Golem API"));
    executor
        .wait_for_status(&worker_id, AgentStatus::Interrupted, Duration::from_secs(2))
        .await?;
    assert!(!executor.worker_is_loaded(&owned).await);
    tokio::time::timeout(Duration::from_secs(2), replacement.peer_closed()).await?;
    let retries = count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(retries, 1);

    executor.resume(&worker_id, false).await?;
    let resumed = executor.invoke_and_await_agent_with_key(
        &component,
        &agent_id,
        &key,
        "post_with_status_retry_policy",
        data_value!(),
    );
    tokio::pin!(resumed);
    tokio::select! {
        _ = replacement.resumed_accepted() => {}
        result = &mut resumed => panic!("completed before reconstructed request was gated: {result:?}"),
    }
    assert_eq!(
        executor
            .attached_agent_status(&worker_id)
            .await?
            .current_retry_state,
        retry_state
    );
    replacement.release_resumed();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), &mut resumed)
            .await??
            .into_typed::<String>()?,
        "200 status-retry-ok"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?,
        retries
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(oplog.iter().filter(|entry| matches!(
        &entry.entry, PublicOplogEntry::AgentInvocationStarted(params)
            if matches!(&params.invocation, PublicAgentInvocation::AgentMethodInvocation(method) if method.idempotency_key == key)
    )).count(), 1);
    assert_eq!(oplog.iter().filter(|entry| matches!(
        &entry.entry, PublicOplogEntry::AgentInvocationFinished(params)
            if params.method_name.as_ref().is_some_and(|name| name.replace('-', "_") == "post_with_status_retry_policy")
    )).count(), 1);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    let first_key = requests[0]
        .idempotency_key
        .as_deref()
        .filter(|key| !key.is_empty())
        .expect("HTTP idempotency key must be present");
    assert!(
        requests
            .iter()
            .all(|request| request.idempotency_key.as_deref() == Some(first_key))
    );
    for request in requests.iter() {
        assert_eq!(request.request_line, "POST / HTTP/1.1");
        assert_eq!(request.content_length, 9);
        assert_eq!(request.body, b"test-body");
    }
    Ok(())
}
