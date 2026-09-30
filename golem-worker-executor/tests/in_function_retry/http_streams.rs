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
use anyhow::Context;
use golem_common::model::oplog::{
    AgentError, HostResponse, HostResponseStreamChunk, OplogEntry, OplogErrorKind, OplogIndex,
    PublicAgentInvocation, PublicOplogEntry,
};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{AgentId, IdempotencyKey, RetryConfig, RetryPolicyState};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::{HttpClientConfig, HttpClientEnabledConfig};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start, start_with_overrides,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use test_r::{inherit_test_dep, test};
use tokio::spawn;
use tokio::time::timeout;
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

use super::count_oplog_errors_containing;
use super::http_servers::{
    start_body_dropping_http_server, start_body_retry_then_response_retry_http_server,
    start_gated_partial_response_http_server,
    start_gated_partial_response_http_server_with_resume_send_failures,
    start_partial_response_http_server, start_recovery_gated_partial_response_http_server,
    start_write_zeroes_validation_server,
};

async fn run_response_body_pool_limit_control(
    max_connections: usize,
    verify_replay: bool,
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    http_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(move |config| {
            config.retry = RetryConfig {
                max_attempts: 2,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
            config.http_client = HttpClientConfig::Enabled(HttpClientEnabledConfig {
                connect_timeout: Duration::from_secs(30),
                max_connections_per_host: max_connections,
                max_total_connections: max_connections,
                ..Default::default()
            });
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let expected_body = "hi".to_string();
    let (port, mut requests, connection_counter, replacement_ready, release_replacement) =
        start_gated_partial_response_http_server(1, expected_body.as_bytes().to_vec(), 206).await;
    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let authority = format!("127.0.0.1:{port}");
    let agent_id = agent_id!("HttpClient4");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let invocation = executor.invoke_and_await_agent(
        &component,
        &agent_id,
        "get_and_read_body_p2_blocking",
        data_value!(authority),
    );
    let request_gate = async {
        assert_eq!(requests.recv().await, Some(None));
        assert_eq!(requests.recv().await, Some(Some(1)));
        replacement_ready.notified().await;
        release_replacement.notify_one();
    };
    let ((), result) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(request_gate, invocation)
    })
    .await
    .map_err(|_| anyhow::anyhow!("replacement request did not dispatch within five seconds"))?;
    let result = result?.into_typed::<String>()?;

    assert_eq!(result, format!("200 {expected_body}"));
    assert_eq!(connection_counter.load(Ordering::SeqCst), 2);
    assert_eq!(
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?,
        1
    );

    if verify_replay {
        drop(executor);
        let executor = start(deps, &context).await?;
        let replayed = executor
            .invoke_and_await_agent(&component, &agent_id, "stored_full_response", data_value!())
            .await?
            .into_typed::<String>()?;
        assert_eq!(replayed, format!("200 {expected_body}"));
        assert_eq!(
            connection_counter.load(Ordering::SeqCst),
            2,
            "replaying completed body-read progress must not issue another HTTP request"
        );
    }

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_resuming_response_body_releases_single_pool_slot_before_retry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_response_body_pool_limit_control(1, true, last_unique_id, deps, http_tests).await
}

#[test]
#[tracing::instrument]
async fn http_resuming_response_body_with_two_pool_slots(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_response_body_pool_limit_control(2, false, last_unique_id, deps, http_tests).await
}

#[test]
#[tracing::instrument]
async fn http_resumed_response_body_holds_pool_slot_until_consumed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 2,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
            config.http_client = HttpClientConfig::Enabled(HttpClientEnabledConfig {
                connect_timeout: Duration::from_secs(30),
                max_connections_per_host: 1,
                max_total_connections: 1,
                ..Default::default()
            });
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let (port, mut requests, connection_counter, replacement_ready, release_replacement) =
        start_gated_partial_response_http_server(1, b"hi".to_vec(), 206).await;
    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let response_agent_id = agent_id!("HttpClient4");
    executor
        .start_agent_with(
            &component.id,
            response_agent_id.clone(),
            env.clone(),
            Vec::new(),
        )
        .await?;
    let unrelated_agent_id = agent_id!("HttpClient2");
    executor
        .start_agent_with(&component.id, unrelated_agent_id.clone(), env, Vec::new())
        .await?;

    let authority = format!("127.0.0.1:{port}");
    let response_invocation = executor.invoke_and_await_agent(
        &component,
        &response_agent_id,
        "get_and_read_body_p2_blocking",
        data_value!(authority),
    );
    tokio::pin!(response_invocation);

    for expected in [None, Some(1)] {
        let request = tokio::select! {
            request = requests.recv() => request,
            result = &mut response_invocation => {
                panic!("response invocation completed before replacement admission: {result:?}")
            }
        };
        assert_eq!(request, Some(expected));
    }
    tokio::select! {
        _ = replacement_ready.notified() => {}
        result = &mut response_invocation => {
            panic!("response invocation completed before replacement headers: {result:?}")
        }
    }

    let unrelated_invocation =
        executor.invoke_and_await_agent(&component, &unrelated_agent_id, "run", data_value!());
    tokio::pin!(unrelated_invocation);
    tokio::select! {
        request = requests.recv() => {
            panic!("unrelated request acquired the replacement body's pool permit: {request:?}")
        }
        result = &mut response_invocation => {
            panic!("response invocation completed while replacement body was gated: {result:?}")
        }
        result = &mut unrelated_invocation => {
            panic!("unrelated invocation completed while replacement held the permit: {result:?}")
        }
        _ = tokio::time::sleep(Duration::from_millis(300)) => {}
    }

    release_replacement.notify_one();
    let (response_result, unrelated_result, third_request) =
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                &mut response_invocation,
                &mut unrelated_invocation,
                requests.recv()
            )
        })
        .await
        .map_err(|_| anyhow::anyhow!("pool permit was not released after body consumption"))?;

    assert_eq!(response_result?.into_typed::<String>()?, "200 hi");
    assert_eq!(third_request, Some(None));
    assert_eq!(
        unrelated_result?.into_typed::<String>()?,
        "200 ExampleResponse { percentage: 0.25, message: Some(\"permit released\") }"
    );
    assert_eq!(connection_counter.load(Ordering::SeqCst), 3);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_rejected_response_body_replacement_releases_pool_slot(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 2,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
            config.http_client = HttpClientConfig::Enabled(HttpClientEnabledConfig {
                connect_timeout: Duration::from_secs(30),
                max_connections_per_host: 1,
                max_total_connections: 1,
                ..Default::default()
            });
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let (port, mut requests, connection_counter, replacement_ready, release_replacement) =
        start_gated_partial_response_http_server(1, b"hi".to_vec(), 416).await;
    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let response_agent_id = agent_id!("HttpClient4");
    executor
        .start_agent_with(
            &component.id,
            response_agent_id.clone(),
            env.clone(),
            Vec::new(),
        )
        .await?;
    let unrelated_agent_id = agent_id!("HttpClient2");
    executor
        .start_agent_with(&component.id, unrelated_agent_id.clone(), env, Vec::new())
        .await?;

    let authority = format!("127.0.0.1:{port}");
    let failed_invocation = executor.invoke_and_await_agent(
        &component,
        &response_agent_id,
        "get_and_read_body_p2_blocking",
        data_value!(authority),
    );
    let response_gate = async {
        assert_eq!(requests.recv().await, Some(None));
        assert_eq!(requests.recv().await, Some(Some(1)));
        replacement_ready.notified().await;
        release_replacement.notify_one();
    };
    let ((), failed_result) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(response_gate, failed_invocation)
    })
    .await
    .map_err(|_| anyhow::anyhow!("rejected replacement response did not settle"))?;
    assert!(failed_result.is_err());

    let unrelated_invocation =
        executor.invoke_and_await_agent(&component, &unrelated_agent_id, "run", data_value!());
    let (third_request, unrelated_result) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(requests.recv(), unrelated_invocation)
    })
    .await
    .map_err(|_| anyhow::anyhow!("rejected replacement retained the pool permit"))?;
    assert_eq!(third_request, Some(None));
    assert_eq!(
        unrelated_result?.into_typed::<String>()?,
        "200 ExampleResponse { percentage: 0.25, message: Some(\"permit released\") }"
    );
    assert_eq!(connection_counter.load(Ordering::SeqCst), 3);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_output_stream_inline_retry_on_body_write_failure(
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

    // Server that accepts connection, reads partial data, then drops — triggers body write error
    let (port, connection_counter) = start_body_dropping_http_server(2).await;

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

    // post_large_body writes 256KB in 4 chunks. The first 2 connections will fail
    // mid-body-write, triggering output stream inline retry.
    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "post_large_body", data_value!())
        .await?;

    let result_value = result.into_typed::<String>()?;

    // The response should be "200 received <N> bytes" — verify it succeeded
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

    // Verify oplog contains in-function retry error entries.
    // The exact count depends on which stream operation triggers the error
    // (write or flush), but there should be at least 1 retry per failed connection.
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert!(
        retry_count > 0,
        "Expected at least 1 in-function retry error entry in oplog, got {retry_count}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_awaiting_response_retry_resends_full_body_after_output_stream_retry(
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
    let (port, body_lengths) = start_body_retry_then_response_retry_http_server().await;

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
        .invoke_and_await_agent(&component, &agent_id, "post_large_body", data_value!())
        .await?;
    let result_value = result.into_typed::<String>()?;

    const FULL_BODY_LEN: usize = 4 * 64 * 1024;
    assert_eq!(
        result_value,
        format!("200 received {FULL_BODY_LEN} body bytes")
    );

    {
        let body_lengths = body_lengths.lock().unwrap();
        assert!(
            body_lengths.len() >= 3,
            "Expected at least three requests, got body lengths {body_lengths:?}"
        );
        assert!(
            body_lengths[0] >= 64 * 1024,
            "First attempt must receive at least one body chunk before output-stream retry, got {body_lengths:?}"
        );
        assert_eq!(
            body_lengths[1], FULL_BODY_LEN,
            "Output-stream retry should rebuild and send the full body"
        );
        assert_eq!(
            body_lengths[2], FULL_BODY_LEN,
            "Awaiting-response retry must resend the full body, not only the suffix after the previous retry error"
        );
    }

    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert!(
        retry_count > 0,
        "Expected at least one in-function retry error entry in oplog, got {retry_count}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_resuming_response_body_inline_retry_on_body_read_failure(
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

    // Server sends 1024-byte body. First connection sends 256 bytes then drops.
    // Second connection checks for Range header and responds with 206 + remaining bytes.
    let (port, connection_counter, range_counter) =
        start_partial_response_http_server(1, 256, 1024, 200, 200, true).await;

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

    // get_and_read_body_chunked reads in 256-byte chunks, triggering
    // response-body resumption on the partial response drop.
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_read_body_chunked",
            data_value!(),
        )
        .await?;

    let result_value = result.into_typed::<String>()?;

    // Verify the response contains the full body (1024 bytes of sequential pattern)
    assert!(
        result_value.starts_with("200 "),
        "Expected a successful 200 response, got: {result_value:?}"
    );

    // Server received 1 partial + 1 successful = 2 total connections
    let total_connections = connection_counter.load(Ordering::SeqCst);
    assert_eq!(
        total_connections, 2,
        "Expected 2 total connections (1 partial + 1 resumed)"
    );

    let range_requests = range_counter.load(Ordering::SeqCst);
    assert!(
        range_requests > 0,
        "Expected at least 1 range request from response-body resumption retry, got {range_requests}"
    );

    // Verify oplog contains in-function retry error entries for
    // response-body resumption.
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert!(
        retry_count > 0,
        "Expected at least 1 in-function retry error entry in oplog for response-body resumption, got {retry_count}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_blocking_read_payload_outage_reconstructs_same_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_http_p2_read_payload_outage_reconstructs_same_invocation(
        last_unique_id,
        deps,
        http_tests,
        "get_and_blocking_read_body_p2",
        false,
        false,
        false,
        false,
    )
    .await
}

#[test]
#[tracing::instrument]
async fn http_read_payload_outage_reconstructs_same_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_http_p2_read_payload_outage_reconstructs_same_invocation(
        last_unique_id,
        deps,
        http_tests,
        "get_and_read_body_p2",
        false,
        false,
        false,
        false,
    )
    .await
}

#[test]
#[tracing::instrument]
async fn repeated_payload_recovery_preserves_nonzero_semantic_retry_state(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_http_p2_read_payload_outage_reconstructs_same_invocation(
        last_unique_id,
        deps,
        http_tests,
        "get_and_blocking_read_body_p2",
        true,
        false,
        false,
        false,
    )
    .await
}

#[test]
#[tracing::instrument]
async fn completed_prefix_payload_outage_reconstructs_same_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_http_p2_read_payload_outage_reconstructs_same_invocation(
        last_unique_id,
        deps,
        http_tests,
        "get_and_blocking_read_body_p2",
        false,
        true,
        false,
        false,
    )
    .await
}

#[test]
#[tracing::instrument]
async fn idempotent_get_payload_outage_recovers_without_worker_idempotence_override(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_http_p2_read_payload_outage_reconstructs_same_invocation(
        last_unique_id,
        deps,
        http_tests,
        "get_and_blocking_read_body_p2_without_idempotence_override",
        false,
        false,
        false,
        false,
    )
    .await
}

#[test]
#[tracing::instrument]
async fn post_recovery_http_content_failure_is_not_corrupt_history(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_http_p2_read_payload_outage_reconstructs_same_invocation(
        last_unique_id,
        deps,
        http_tests,
        "get_and_blocking_read_body_p2",
        false,
        false,
        true,
        false,
    )
    .await
}

#[test]
#[tracing::instrument]
async fn post_recovery_transient_send_uses_outer_semantic_retry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    assert_http_p2_read_payload_outage_reconstructs_same_invocation(
        last_unique_id,
        deps,
        http_tests,
        "get_and_blocking_read_body_p2_with_delayed_retry",
        false,
        false,
        false,
        true,
    )
    .await
}

#[test]
#[tracing::instrument]
async fn guest_range_ineligibility_precedes_payload_download(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.oplog.max_payload_size = 1;
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(1),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let (port, _connection_counter, _range_counter, mut drop_gate) =
        start_recovery_gated_partial_response_http_server(1, 256, 1024, 200, 200, false).await;
    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let agent_id = agent_id!("HttpClient4");
    let expected_worker_id = AgentId {
        component_id: component.id,
        agent_id: agent_id.to_string(),
    };
    let mut read_starts = executor.probe_http_body_read_starts(&expected_worker_id);
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let mut invocation = spawn({
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &agent_id,
                    "get_and_blocking_read_body_p2_with_range",
                    data_value!(),
                )
                .await
        }
        .in_current_span()
    });

    timeout(Duration::from_secs(20), drop_gate.reached())
        .await
        .context("partial response server did not reach its drop gate")?;
    let first_read_start = timeout(Duration::from_secs(20), read_starts.recv())
        .await
        .context("guest did not start its first body read")?
        .context("body-read start probe closed")?;
    let first_read_end = timeout(Duration::from_secs(20), async {
        loop {
            let oplog = executor
                .get_oplog(&worker_id, OplogIndex::INITIAL)
                .await
                .expect("query oplog while waiting for first body chunk");
            if let Some(end) = oplog.iter().find(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::End(params) if params.start_index == first_read_start
                )
            }) {
                break end.oplog_index;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("first body chunk was not durably recorded")?;
    let first_read_payload = executor
        .external_end_payload_id(&worker_id, first_read_end)
        .await?;
    executor.set_oplog_download_outage_for_payload(&worker_id, first_read_payload);
    drop_gate.release();

    let result = timeout(Duration::from_secs(20), &mut invocation)
        .await
        .context("guest Range request did not complete through ordinary retry")?
        .context("invocation task panicked")??
        .into_typed::<String>()?;
    assert!(result.starts_with("200 "));
    assert!(executor.oplog_download_outage_hits(&worker_id).is_empty());
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(!oplog.iter().any(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::Error(params) if params.kind == OplogErrorKind::Recovery
    )));
    assert!(oplog.iter().any(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::Error(params) if params.kind == OplogErrorKind::Invocation
    )));
    Ok(())
}

async fn assert_http_p2_read_payload_outage_reconstructs_same_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    http_tests: &PrecompiledComponent,
    method_name: &'static str,
    repeat_with_retry_state: bool,
    target_completed_prefix: bool,
    repair_content_failure: bool,
    repair_send_failure: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.oplog.max_payload_size = 1;
            config.retry = RetryConfig {
                max_attempts: 5,
                min_delay: Duration::from_secs(2),
                max_delay: Duration::from_secs(2),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let transport_failures = if repeat_with_retry_state { 2 } else { 1 };
    let (resume_status, resume_supports_range) = if repair_content_failure {
        (201, false)
    } else {
        (200, true)
    };
    let (port, connection_counter, range_counter, mut drop_gate) = if repair_send_failure {
        start_gated_partial_response_http_server_with_resume_send_failures(
            transport_failures,
            256,
            1024,
            200,
            resume_status,
            resume_supports_range,
            1,
        )
        .await
    } else {
        start_recovery_gated_partial_response_http_server(
            transport_failures,
            256,
            1024,
            200,
            resume_status,
            resume_supports_range,
        )
        .await
    };

    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let agent_id = agent_id!("HttpClient4");
    let expected_worker_id = AgentId {
        component_id: component.id,
        agent_id: agent_id.to_string(),
    };
    let mut runtime_disposals = executor.probe_runtime_disposal(&expected_worker_id);
    let mut read_starts = executor.probe_http_body_read_starts(&expected_worker_id);
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    assert_eq!(worker_id, expected_worker_id);
    let invocation_key = IdempotencyKey::new(format!("gol-684-payload-recovery-{method_name}"));

    let mut invocation = spawn({
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        let invocation_key = invocation_key.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent_id,
                    &invocation_key,
                    method_name,
                    data_value!(),
                )
                .await
        }
        .in_current_span()
    });

    timeout(Duration::from_secs(20), drop_gate.reached())
        .await
        .context("partial response server did not reach its drop gate")?;

    let expected_body = (0..128)
        .map(|record| format!("{record:07x}\n"))
        .collect::<String>();
    assert_eq!(expected_body.len(), 1024);
    let expected_prefix = expected_body.as_bytes()[..256].to_vec();
    let expected_first_read = HostResponse::from(HostResponseStreamChunk {
        result: Ok(expected_prefix.clone()),
    })
    .into_typed_schema_value()
    .context("encode expected first blocking-read response")?;
    let first_read_start = timeout(Duration::from_secs(20), read_starts.recv())
        .await
        .context("guest did not start its first body read")?
        .context("blocking-read start probe closed")?;
    let first_read_end = timeout(Duration::from_secs(20), async {
        loop {
            let oplog = executor
                .get_oplog(&worker_id, OplogIndex::INITIAL)
                .await
                .expect("query oplog while waiting for durable body delivery");
            if let Some(first_read_end) = oplog.iter().find(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::End(params)
                        if params.start_index == first_read_start
                            && params.response.as_ref() == Some(&expected_first_read)
                )
            }) {
                break first_read_end.oplog_index;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("guest did not durably receive the expected prefix")?;
    assert!(first_read_start < first_read_end);
    let first_read_payload = executor
        .external_end_payload_id(&worker_id, first_read_end)
        .await?;
    let recovery_retry_from = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if entry.oplog_index == first_read_start => {
                params.parent_start_index
            }
            _ => None,
        })
        .context("body read was not parented to the HTTP durable scope")?;

    assert_eq!(executor.instance_load_count(&worker_id), 1);
    let metadata_before_failure = executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(metadata_before_failure.last_error_kind, None);
    assert_eq!(metadata_before_failure.retry_count, 0);
    let status_before_failure = executor.attached_agent_status(&worker_id).await?;
    assert!(status_before_failure.current_retry_state.is_empty());
    let seeded_retry_state = if repeat_with_retry_state {
        let retry_state = RetryPolicyState::Counter(5);
        let error_index = executor
            .commit_oplog_entry_bypassing_worker_status(
                &worker_id,
                OplogEntry::error(
                    None,
                    OplogErrorKind::Invocation,
                    AgentError::TransientError("seeded semantic retry state".to_string()),
                    recovery_retry_from,
                    false,
                    Some(retry_state.clone()),
                ),
            )
            .await?;
        executor
            .commit_oplog_entry_bypassing_worker_status(
                &worker_id,
                OplogEntry::jump(
                    None,
                    OplogRegion {
                        start: error_index,
                        end: error_index,
                    },
                ),
            )
            .await?;
        Some(HashMap::from([(recovery_retry_from, retry_state)]))
    } else {
        None
    };
    if target_completed_prefix {
        executor.set_oplog_download_outage_for_payload(&worker_id, first_read_payload.clone());
    } else {
        executor.set_oplog_download_outage(&worker_id, true);
    }
    drop_gate.release();

    let metadata_during_recovery = timeout(Duration::from_secs(10), async {
        loop {
            let metadata = executor
                .get_worker_metadata(&worker_id)
                .await
                .expect("query worker metadata during payload outage");
            if metadata.last_error_kind == Some(OplogErrorKind::Recovery) {
                break metadata;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("payload outage was not recorded as a Recovery failure")?;
    assert_eq!(
        metadata_during_recovery.retry_count,
        if repeat_with_retry_state { 5 } else { 0 }
    );
    let status_during_recovery = executor.attached_agent_status(&worker_id).await?;
    assert_eq!(
        &status_during_recovery.current_retry_state,
        seeded_retry_state
            .as_ref()
            .unwrap_or(&status_before_failure.current_retry_state)
    );

    let disposed_generation = timeout(Duration::from_secs(5), runtime_disposals.recv())
        .await
        .context("failed runtime was not physically disposed")?
        .context("runtime disposal probe closed")?;
    assert_eq!(disposed_generation, 1);
    assert_eq!(executor.instance_load_count(&worker_id), 1);

    if repeat_with_retry_state || target_completed_prefix {
        timeout(Duration::from_secs(10), async {
            loop {
                let oplog = executor
                    .get_oplog(&worker_id, OplogIndex::INITIAL)
                    .await
                    .expect("query oplog while waiting for repeated recovery");
                if oplog
                    .iter()
                    .filter(|entry| {
                        matches!(
                            &entry.entry,
                            PublicOplogEntry::Error(params)
                                if params.kind == OplogErrorKind::Recovery
                        )
                    })
                    .count()
                    >= 2
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("replacement runtime did not fail on the persistent payload outage")?;
        let second_disposed_generation = timeout(Duration::from_secs(5), runtime_disposals.recv())
            .await
            .context("replacement runtime was not physically disposed")?
            .context("runtime disposal probe closed")?;
        assert_eq!(second_disposed_generation, 2);
        assert_eq!(executor.instance_load_count(&worker_id), 2);
        if repeat_with_retry_state {
            let status_after_repeated_recovery = executor.attached_agent_status(&worker_id).await?;
            assert_eq!(
                &status_after_repeated_recovery.current_retry_state,
                seeded_retry_state
                    .as_ref()
                    .expect("repeated recovery seeded retry state")
            );
        }
    }

    assert!(
        timeout(Duration::from_millis(200), &mut invocation)
            .await
            .is_err(),
        "payload outage terminally completed the accepted invocation"
    );

    let outage_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let terminal_starts = outage_oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::End(params) => Some(params.start_index),
            PublicOplogEntry::Cancelled(params) => Some(params.start_index),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let incomplete_starts = outage_oplog
        .iter()
        .filter_map(|entry| {
            (entry.oplog_index > first_read_end
                && matches!(&entry.entry, PublicOplogEntry::Start(_))
                && !terminal_starts.contains(&entry.oplog_index))
            .then_some(entry.oplog_index)
        })
        .collect::<Vec<_>>();
    let expected_recovery_count = if repeat_with_retry_state || target_completed_prefix {
        2
    } else {
        1
    };
    assert_eq!(
        incomplete_starts.len(),
        1,
        "every runtime must repair the original unterminated body-read child"
    );
    let second_read_start = incomplete_starts[0];
    let open_read_start = second_read_start;
    assert_eq!(
        outage_oplog
            .iter()
            .filter(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::Error(params) if params.kind == OplogErrorKind::Recovery
            ))
            .count(),
        expected_recovery_count
    );
    if repeat_with_retry_state {
        assert_eq!(
            outage_oplog
                .iter()
                .filter(|entry| matches!(
                    &entry.entry,
                    PublicOplogEntry::Error(params)
                        if params.kind == OplogErrorKind::Invocation
                ))
                .count(),
            1,
            "infrastructure reconstruction must not add a semantic invocation failure"
        );
    }
    let download_hits = executor.oplog_download_outage_hits(&worker_id);
    assert_eq!(download_hits.first(), Some(&first_read_payload));
    assert!(
        download_hits.len() >= expected_recovery_count,
        "each failed runtime must reach the injected payload backend"
    );

    executor.set_oplog_download_outage(&worker_id, false);
    if repair_content_failure {
        let error = timeout(Duration::from_secs(20), &mut invocation)
            .await
            .context("post-recovery HTTP content failure did not finish")?
            .context("invocation task panicked")?
            .expect_err("an unusable replacement response must fail the guest invocation");
        assert!(error.to_string().contains("LastOperationFailed"));
        let metadata = executor.get_worker_metadata(&worker_id).await?;
        assert_eq!(metadata.last_error_kind, Some(OplogErrorKind::Invocation));
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        assert_eq!(
            oplog
                .iter()
                .filter(|entry| matches!(
                    &entry.entry,
                    PublicOplogEntry::Error(params)
                        if params.kind == OplogErrorKind::Recovery
                ))
                .count(),
            1
        );
        assert!(
            !error.to_string().contains("Unexpected oplog entry"),
            "valid history was misclassified as corrupt: {error:#}"
        );
        return Ok(());
    }
    let invocation_result = timeout(Duration::from_secs(20), &mut invocation)
        .await
        .context("original invocation did not complete after payload storage recovered")?
        .context("invocation task panicked")??;
    let actual = invocation_result.into_typed::<String>()?;
    let actual_body = actual
        .strip_prefix("200 ")
        .context("guest result did not contain the expected HTTP status")?;
    assert_eq!(actual_body, expected_body);
    assert!(executor.instance_load_count(&worker_id) >= 2);

    let metadata_after_recovery = timeout(Duration::from_secs(5), async {
        loop {
            let metadata = executor
                .get_worker_metadata(&worker_id)
                .await
                .expect("query worker metadata after recovery");
            if metadata.last_error_kind.is_none() {
                break metadata;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("completed invocation retained stale Recovery status")?;
    assert_eq!(metadata_after_recovery.last_error, None);
    assert_eq!(metadata_after_recovery.last_error_kind, None);
    assert_eq!(metadata_after_recovery.retry_count, 0);
    let status_after_recovery = executor.attached_agent_status(&worker_id).await?;
    if !repeat_with_retry_state {
        assert_eq!(
            status_after_recovery.current_retry_state,
            status_before_failure.current_retry_state
        );
    }

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(oplog.iter().any(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::End(params) if params.start_index == second_read_start
    )));
    assert!(!oplog.iter().any(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::Cancelled(params) if params.start_index == second_read_start
    )));
    assert!(!oplog.iter().any(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::Jump(params)
            if params.jump.contains(first_read_start)
                || params.jump.contains(first_read_end)
                || params.jump.contains(second_read_start)
    )));
    assert_eq!(
        connection_counter.load(Ordering::SeqCst),
        transport_failures + 1 + usize::from(repair_send_failure),
        "reconstruction must continue the response rather than restart the whole request"
    );
    assert_eq!(range_counter.load(Ordering::SeqCst), 1);
    if repair_send_failure {
        assert!(oplog.iter().any(|entry| matches!(
            &entry.entry,
            PublicOplogEntry::Error(params) if params.kind == OplogErrorKind::Invocation
        )));
    }
    let invocation_starts = oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(params)
                    if matches!(
                        &params.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.idempotency_key == invocation_key
                                && method.method_name.replace('-', "_") == method_name
                    )
            )
        })
        .count();
    let invocation_finishes = oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationFinished(params)
                    if params.method_name.as_deref().is_some_and(|name|
                        name.replace('-', "_") == method_name)
            )
        })
        .count();
    assert_eq!((invocation_starts, invocation_finishes), (1, 1));

    eprintln!(
        "GOL-684 recovery ({method_name}): first_read_start={first_read_start}, \
         first_read_end={first_read_end}, prefix_len={}, open_read_start={open_read_start}, \
         failed_payload={first_read_payload:?}, download_hits={}, recovery_kind={:?}, \
         retry_count_during={}, retry_state_during={:?}, disposed_generation={disposed_generation}, \
         instance_loads={}, final_len={}, invocation_pair=({invocation_starts},{invocation_finishes})",
        expected_prefix.len(),
        download_hits.len(),
        metadata_during_recovery.last_error_kind,
        metadata_during_recovery.retry_count,
        status_during_recovery.current_retry_state,
        executor.instance_load_count(&worker_id),
        actual_body.len(),
    );
    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_response_resume_content_failure_remains_invocation_failure(
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
                max_delay: Duration::from_millis(1),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let (port, connection_counter, range_counter) =
        start_partial_response_http_server(1, 256, 1024, 200, 416, false).await;
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

    let error = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "get_and_blocking_read_body_p2",
            data_value!(),
        )
        .await
        .expect_err("a 416 resume response must fail the guest invocation");
    assert!(
        error.to_string().contains("LastOperationFailed"),
        "unexpected content failure: {error:#}"
    );
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(metadata.last_error_kind, Some(OplogErrorKind::Invocation));
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(!oplog.iter().any(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::Error(params) if params.kind == OplogErrorKind::Recovery
    )));
    assert_eq!(connection_counter.load(Ordering::SeqCst), 2);
    assert_eq!(range_counter.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_response_resume_corrupt_history_terminates_without_recovery_loop(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] http_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.oplog.max_payload_size = 1;
            config.retry = RetryConfig {
                max_attempts: 2,
                min_delay: Duration::from_secs(2),
                max_delay: Duration::from_secs(2),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
            config.max_in_function_retry_delay = Duration::from_secs(1);
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let (port, _connection_counter, _range_counter, mut drop_gate) =
        start_recovery_gated_partial_response_http_server(1, 256, 1024, 200, 200, true).await;
    let component = executor
        .component_dep(&context.default_environment_id, http_tests)
        .store()
        .await?;
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let agent_id = agent_id!("HttpClient4");
    let expected_worker_id = AgentId {
        component_id: component.id,
        agent_id: agent_id.to_string(),
    };
    let mut runtime_disposals = executor.probe_runtime_disposal(&expected_worker_id);
    let mut read_starts = executor.probe_http_body_read_starts(&expected_worker_id);
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;
    let mut invocation = spawn({
        let executor = executor.clone();
        let component = component.clone();
        let agent_id = agent_id.clone();
        async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &agent_id,
                    "get_and_blocking_read_body_p2_with_unbounded_retry",
                    data_value!(),
                )
                .await
        }
        .in_current_span()
    });

    timeout(Duration::from_secs(20), drop_gate.reached())
        .await
        .context("partial response server did not reach its drop gate")?;
    let first_read_start = timeout(Duration::from_secs(20), read_starts.recv())
        .await
        .context("guest did not start its first body read")?
        .context("body-read start probe closed")?;
    let second_read_start = timeout(Duration::from_secs(20), read_starts.recv())
        .await
        .context("guest did not advance to its second body read")?
        .context("body-read start probe closed")?;
    let first_read_end = timeout(Duration::from_secs(20), async {
        loop {
            let oplog = executor
                .get_oplog(&worker_id, OplogIndex::INITIAL)
                .await
                .expect("query oplog while waiting for first body chunk");
            if let Some(end) = oplog.iter().find(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::End(params) if params.start_index == first_read_start
                )
            }) {
                break end.oplog_index;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("first body chunk was not durably recorded")?;
    let failed_payload = executor
        .external_end_payload_id(&worker_id, first_read_end)
        .await?;
    executor.set_oplog_download_outage_for_payload(&worker_id, failed_payload.clone());
    drop_gate.release();
    timeout(Duration::from_secs(10), async {
        loop {
            if executor
                .get_worker_metadata(&worker_id)
                .await
                .expect("query metadata during prefix payload outage")
                .last_error_kind
                == Some(OplogErrorKind::Recovery)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("prefix payload outage was not classified as Recovery")?;
    assert_eq!(
        timeout(Duration::from_secs(5), runtime_disposals.recv())
            .await
            .context("payload outage did not retire the runtime")?
            .context("runtime disposal probe closed")?,
        1
    );
    executor.set_oplog_download_outage(&worker_id, false);
    executor.set_oplog_download_corruption(&worker_id, failed_payload.clone());

    let result = timeout(Duration::from_secs(20), &mut invocation)
        .await
        .context("permanent corrupt-history failure entered an unbounded recovery loop")?
        .context("invocation task panicked")?;
    let error = result.expect_err("corrupt recovery data must fail without semantic retry");
    let error = error.to_string();
    assert!(error.contains("Unexpected oplog entry during replay"));
    assert!(error.contains("payload is corrupt"));
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(metadata.last_error_kind, Some(OplogErrorKind::Invocation));
    assert_eq!(metadata.retry_count, 0);
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::Error(params) if params.kind == OplogErrorKind::Recovery
            ))
            .count(),
        1
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::Error(params) if params.kind == OplogErrorKind::Invocation
            ))
            .count(),
        1
    );
    let terminal_starts = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::End(params) => Some(params.start_index),
            PublicOplogEntry::Cancelled(params) => Some(params.start_index),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let incomplete_starts = oplog
        .iter()
        .filter_map(|entry| {
            (entry.oplog_index >= second_read_start
                && matches!(&entry.entry, PublicOplogEntry::Start(_))
                && !terminal_starts.contains(&entry.oplog_index))
            .then_some(entry.oplog_index)
        })
        .collect::<Vec<_>>();
    assert_eq!(incomplete_starts.len(), 1);
    let outage_hits = executor.oplog_download_outage_hits(&worker_id);
    assert!(!outage_hits.is_empty());
    assert!(outage_hits.iter().all(|payload| payload == &failed_payload));
    assert_eq!(
        executor.instance_load_count(&worker_id),
        2,
        "permanent reconstruction failure must ignore the unbounded retry policy"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_resuming_response_body_inline_retry_accepts_matching_non_partial_success_status(
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

    // Server sends a 201 response body, drops mid-stream, then ignores the retry Range
    // header and resends the full body with the same 201 status.
    let (port, connection_counter, range_counter) =
        start_partial_response_http_server(1, 256, 1024, 201, 201, false).await;

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
            "get_and_read_body_chunked",
            data_value!(),
        )
        .await?;

    let result_value = result.into_typed::<String>()?;

    assert!(
        result_value.starts_with("201 "),
        "Expected a successful 201 response, got: {result_value:?}"
    );

    let total_connections = connection_counter.load(Ordering::SeqCst);
    assert_eq!(
        total_connections, 2,
        "Expected 2 total connections (1 partial + 1 resumed)"
    );

    let range_requests = range_counter.load(Ordering::SeqCst);
    assert!(
        range_requests > 0,
        "Expected at least 1 range request from response-body resumption retry, got {range_requests}"
    );

    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert!(
        retry_count > 0,
        "Expected at least 1 in-function retry error entry in oplog for response-body resumption, got {retry_count}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn http_write_zeroes_body_reconstruction(
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

    // Server validates the body contains HEAD + zeroes + 0xAB bytes.
    // First connection drops after reading partial data.
    let (port, connection_counter) = start_write_zeroes_validation_server(1).await;

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
            "post_with_write_zeroes",
            data_value!(),
        )
        .await?;

    let result_value = result.into_typed::<String>()?;

    // Server should validate the body and return "200 body-ok len=2052"
    assert_eq!(
        result_value, "200 body-ok len=2052",
        "Expected server to validate reconstructed body (HEAD + 1024 zeroes + 1024 * 0xAB)"
    );

    // Server received 1 failed + 1 successful = 2 total connections
    let total_connections = connection_counter.load(Ordering::SeqCst);
    assert_eq!(
        total_connections, 2,
        "Expected 2 total connections (1 dropped + 1 successful)"
    );

    // Verify oplog contains in-function retry error entries
    let retry_count =
        count_oplog_errors_containing(&executor, &worker_id, "in-function retry").await?;
    assert!(
        retry_count > 0,
        "Expected at least 1 in-function retry error entry in oplog, got {retry_count}"
    );

    Ok(())
}
