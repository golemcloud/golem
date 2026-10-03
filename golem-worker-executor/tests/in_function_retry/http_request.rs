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
use golem_common::model::oplog::{
    OplogErrorKind, OplogIndex, PublicAgentInvocation, PublicOplogEntry,
};
use golem_common::model::{AgentStatus, IdempotencyKey, OwnedAgentId, RetryConfig};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::{HttpClientConfig, HttpClientEnabledConfig};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
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
    start_failing_http_server, start_failing_http_server_any_method,
    start_status_code_retry_http_server, start_withheld_status_retry_http_server,
};

#[test]
#[tracing::instrument]
async fn http_status_retry_policy_retries_matching_status(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, Default::default()).await?;

    let (port, counter, idempotency_keys) = start_status_code_retry_http_server(3).await;

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

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "post_with_status_retry_policy",
            data_value!(),
        )
        .await?;

    assert_eq!(result.into_typed::<String>()?, "200 status-retry-ok");
    assert_eq!(counter.load(Ordering::SeqCst), 4);
    {
        let idempotency_keys = idempotency_keys.lock().unwrap();
        assert_eq!(idempotency_keys.len(), 4);
        let first_key = idempotency_keys[0]
            .as_ref()
            .expect("initial HTTP request must have idempotency-key");
        assert!(
            idempotency_keys
                .iter()
                .all(|key| key.as_ref() == Some(first_key)),
            "status-code retries must preserve the original idempotency-key"
        );
    }

    executor.check_oplog_is_queryable(&worker_id).await?;

    Ok(())
}

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
            "post_with_status_retry_policy_p2",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;

    assert_eq!(result, "500 retry-me");
    assert_eq!(counter.load(Ordering::SeqCst), 11);
    let idempotency_keys = idempotency_keys.lock().unwrap();
    let first_key = idempotency_keys[0]
        .as_ref()
        .expect("initial HTTP request must have idempotency-key");
    assert_eq!(idempotency_keys.len(), 11);
    assert!(
        idempotency_keys
            .iter()
            .all(|key| key.as_ref() == Some(first_key)),
        "exhausted status retries must preserve the original idempotency-key"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn p2_status_retry_header_wait_observes_interrupt(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.max_in_function_retry_delay = Duration::from_secs(1);
            config.http_client = HttpClientConfig::Enabled(HttpClientEnabledConfig {
                max_connections_per_host: 2,
                max_total_connections: 2,
                ..Default::default()
            });
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;
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
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);

    let invocation_key = IdempotencyKey::fresh();
    let invocation = executor.invoke_and_await_agent_with_key(
        &component,
        &agent_id,
        &invocation_key,
        "post_with_status_retry_policy_p2",
        data_value!(),
    );
    tokio::pin!(invocation);

    tokio::select! {
        _ = replacement.accepted() => {}
        result = &mut invocation => {
            panic!("invocation completed before replacement headers were withheld: {result:?}")
        }
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut invocation)
            .await
            .is_err(),
        "the invocation must remain pending while replacement headers are withheld"
    );
    assert!(executor.worker_is_loaded(&owned_agent_id).await);
    let status_before_interrupt = executor.attached_agent_status(&worker_id).await?;
    let oplog_before_interrupt = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let batch_start = oplog_before_interrupt
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.request.is_none()
                    && (params.function_name == "<scope:batched-write>"
                        || params.function_name.starts_with("<scope:batched-write:")) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("missing HTTP durability batch start");
    let retry_error_before_interrupt = oplog_before_interrupt
        .iter()
        .find(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Error(params)
                    if params.kind == OplogErrorKind::Invocation
                        && params.retry_from == batch_start
            )
        })
        .expect("missing persisted status retry decision")
        .clone();
    let response_get_starts_before_interrupt: HashSet<_> = oplog_before_interrupt
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name.contains("future_incoming_response")
                    && params.function_name.ends_with("::get") =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect();
    assert!(!response_get_starts_before_interrupt.is_empty());
    let response_get_terminals_before_interrupt: HashSet<_> = oplog_before_interrupt
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::End(params)
                if response_get_starts_before_interrupt.contains(&params.start_index) =>
            {
                Some((entry.oplog_index, params.start_index, "end"))
            }
            PublicOplogEntry::Cancelled(params)
                if response_get_starts_before_interrupt.contains(&params.start_index) =>
            {
                Some((entry.oplog_index, params.start_index, "cancelled"))
            }
            _ => None,
        })
        .collect();

    tokio::time::timeout(Duration::from_secs(2), executor.interrupt(&worker_id))
        .await
        .map_err(|_| anyhow::anyhow!("interrupt waited for replacement response headers"))??;
    let result = tokio::time::timeout(Duration::from_secs(2), &mut invocation)
        .await
        .map_err(|_| anyhow::anyhow!("interrupted invocation did not return promptly"))?;
    let error = result.expect_err("interrupted invocation must not produce an HTTP result");
    assert!(
        error.to_string().contains("Interrupted via the Golem API"),
        "expected typed interruption, got: {error}"
    );
    executor
        .wait_for_status(&worker_id, AgentStatus::Interrupted, Duration::from_secs(2))
        .await?;
    assert!(!executor.worker_is_loaded(&owned_agent_id).await);
    tokio::time::timeout(Duration::from_secs(2), replacement.peer_closed())
        .await
        .map_err(|_| anyhow::anyhow!("replacement request task was not aborted"))?;
    assert_eq!(
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?,
        1,
        "lifecycle interruption must not consume another semantic retry"
    );
    let oplog_after_interrupt = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let method_starts = oplog_after_interrupt
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(params)
                    if matches!(
                        &params.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.idempotency_key == invocation_key
                    )
            )
        })
        .count();
    let method_finishes = oplog_after_interrupt
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationFinished(params)
                    if params.method_name.as_ref().is_some_and(|name|
                        name.replace('-', "_") == "post_with_status_retry_policy_p2")
            )
        })
        .count();
    assert_eq!((method_starts, method_finishes), (1, 0));
    assert!(!oplog_after_interrupt.iter().any(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::End(params) if params.start_index == batch_start
    )));
    assert!(!oplog_after_interrupt.iter().any(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::Cancelled(params) if params.start_index == batch_start
    )));
    let retry_errors_after_interrupt: Vec<_> = oplog_after_interrupt
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Error(params) if params.kind == OplogErrorKind::Invocation
            )
        })
        .collect();
    assert_eq!(retry_errors_after_interrupt.len(), 1);
    assert_eq!(
        retry_errors_after_interrupt[0],
        &retry_error_before_interrupt
    );
    let response_get_starts_after_interrupt: HashSet<_> = oplog_after_interrupt
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name.contains("future_incoming_response")
                    && params.function_name.ends_with("::get") =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect();
    let response_get_terminals_after_interrupt: HashSet<_> = oplog_after_interrupt
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::End(params)
                if response_get_starts_after_interrupt.contains(&params.start_index) =>
            {
                Some((entry.oplog_index, params.start_index, "end"))
            }
            PublicOplogEntry::Cancelled(params)
                if response_get_starts_after_interrupt.contains(&params.start_index) =>
            {
                Some((entry.oplog_index, params.start_index, "cancelled"))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        response_get_starts_after_interrupt, response_get_starts_before_interrupt,
        "interruption must not start another durable response observation"
    );
    assert_eq!(
        response_get_terminals_after_interrupt, response_get_terminals_before_interrupt,
        "interruption must not persist a response or cancellation for the withheld replacement"
    );

    executor.resume(&worker_id, false).await?;
    let resumed = executor.invoke_and_await_agent_with_key(
        &component,
        &agent_id,
        &invocation_key,
        "post_with_status_retry_policy_p2",
        data_value!(),
    );
    tokio::pin!(resumed);
    tokio::select! {
        _ = replacement.resumed_accepted() => {}
        result = &mut resumed => {
            panic!("invocation completed before reconstructed request was gated: {result:?}")
        }
    }
    let status_during_recovery = executor.attached_agent_status(&worker_id).await?;
    assert_eq!(
        status_during_recovery.current_retry_state,
        status_before_interrupt.current_retry_state
    );
    replacement.release_resumed();
    let resumed = tokio::time::timeout(Duration::from_secs(5), &mut resumed)
        .await
        .map_err(|_| anyhow::anyhow!("interrupted status retry did not reconstruct on resume"))??;
    assert_eq!(resumed.into_typed::<String>()?, "200 status-retry-ok");
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?,
        1,
        "reconstruction must reuse the persisted status retry decision"
    );

    let oplog_after_resume = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let method_starts = oplog_after_resume
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(params)
                    if matches!(
                        &params.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.idempotency_key == invocation_key
                    )
            )
        })
        .count();
    let method_finishes = oplog_after_resume
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationFinished(params)
                    if params.method_name.as_ref().is_some_and(|name|
                        name.replace('-', "_") == "post_with_status_retry_policy_p2")
            )
        })
        .count();
    assert_eq!((method_starts, method_finishes), (1, 1));
    assert_eq!(
        oplog_after_resume
            .iter()
            .filter(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::End(params) if params.start_index == batch_start
            ))
            .count(),
        1
    );
    assert!(!oplog_after_resume.iter().any(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::Cancelled(params) if params.start_index == batch_start
    )));
    let retry_errors_after_resume: Vec<_> = oplog_after_resume
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Error(params) if params.kind == OplogErrorKind::Invocation
            )
        })
        .collect();
    assert_eq!(retry_errors_after_resume.len(), 1);
    assert_eq!(retry_errors_after_resume[0], &retry_error_before_interrupt);

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    let first_key = requests[0]
        .idempotency_key
        .as_deref()
        .filter(|key| !key.is_empty())
        .expect("HTTP idempotency key must be present and non-empty");
    for request in requests.iter() {
        assert_eq!(request.request_line, "POST / HTTP/1.1");
        assert_eq!(request.content_length, 9);
        assert_eq!(request.body, b"test-body");
        assert_eq!(request.idempotency_key.as_deref(), Some(first_key));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_zone1_inline_retry_on_transient_connection_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    let (port, connection_counter) = start_failing_http_server(2).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = agent_id!("HttpClient");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?;

    assert_eq!(
        result.into_typed::<String>()?,
        "200 response is test-header test-body"
    );

    // Server received 2 failed + 1 successful = 3 total connections
    let total_connections = connection_counter.load(Ordering::SeqCst);
    assert_eq!(
        total_connections, 3,
        "Expected 3 total connections (2 dropped + 1 successful)"
    );

    // Verify oplog contains 2 in-function retry error entries
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(
        retry_count, 2,
        "Expected 2 in-function retry error entries in oplog"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_zone1_falls_back_to_trap_when_delay_exceeds_threshold(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(100),
                max_delay: Duration::from_millis(100),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_millis(1);
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    let (port, _connection_counter) = start_failing_http_server(1).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = agent_id!("HttpClient");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    // Call should eventually succeed via trap+replay (not inline retry)
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!())
        .await?;

    assert_eq!(
        result.into_typed::<String>()?,
        "200 response is test-header test-body"
    );

    // Verify NO in-function retry error entries in oplog
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(
        retry_count, 0,
        "Expected 0 in-function retry error entries in oplog (should have fallen back to trap)"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_post_fails_permanently_when_idempotence_disabled(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    // 1 connection will be dropped; POST with assume_idempotence=false should NOT retry inline
    let (port, _connection_counter) = start_failing_http_server_any_method(1).await;

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());

    let agent_id = agent_id!("HttpClient4");
    let _worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    // post_non_idempotent sets assume_idempotence=false and uses POST.
    // POST is not idempotent, so inline retry should NOT happen.
    // Trap+replay also cannot recover because the non-idempotent remote write
    // was not completed — replay detects this and fails permanently with
    // "Non-idempotent remote write operation was not completed, cannot retry".
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "post_non_idempotent", data_value!())
        .await;

    assert!(
        result.is_err(),
        "Expected the invocation to fail permanently, but it succeeded: {result:?}"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("cannot retry"),
        "Expected error about non-idempotent write not being retryable, got: {err_msg}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_get_retried_inline_even_when_idempotence_disabled(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };

    let executor = start_with_overrides(deps, &context, overrides).await?;

    // 2 connections will be dropped; GET is inherently idempotent so inline retry should work
    let (port, connection_counter) = start_failing_http_server_any_method(2).await;

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

    // get_idempotent sets assume_idempotence=false and uses GET.
    // GET is inherently idempotent, so inline retry SHOULD still happen.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "get_idempotent", data_value!())
        .await?;

    let result_value = result.into_typed::<String>()?;

    assert!(
        result_value.starts_with("200 "),
        "Expected a successful 200 response, got: {result_value:?}"
    );

    // Server received 2 failed + 1 successful = 3 total connections
    let total_connections = connection_counter.load(Ordering::SeqCst);
    assert_eq!(
        total_connections, 3,
        "Expected 3 total connections (2 dropped + 1 successful)"
    );

    // Verify oplog contains in-function retry error entries (GET is idempotent)
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert_eq!(
        retry_count, 2,
        "Expected 2 in-function retry error entries in oplog (GET is inherently idempotent)"
    );

    Ok(())
}
