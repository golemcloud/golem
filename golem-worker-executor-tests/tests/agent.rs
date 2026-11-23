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
use golem_worker_executor_test_utils::callers;

use golem_api_grpc::proto::golem::worker::InvocationStart;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    CreateWorkerRequest, create_worker_response,
};
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::agent::{AgentKey, AgentMode, ParsedAgentKey};
use golem_common::model::component::{ComponentDto, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{OplogIndex, PublicAgentInvocation, PublicOplogEntry};
use golem_common::model::{
    AgentId, IdempotencyKey, InvocationId, InvocationRequestKey, OwnedAgentId,
};
use golem_common::schema::SchemaValue;
use golem_common::schema::graph::TypedSchemaValue;
use golem_common::{agent_key, data_value};
use golem_test_framework::dsl::{AgentResult, TestDsl};
use golem_worker_executor::services::agent_types::local_type_id;
use golem_worker_executor::storage::scheduler::SchedulerStorage;
use golem_worker_executor::storage::scheduler::sqlite::SqliteSchedulerStorage;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestWorkerExecutor,
    WorkerExecutorTestDependencies, copy_create_entry_to, scheduler_sqlite_storage_config, start,
};
use pretty_assertions::assert_eq;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_rpc")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("constructor_parameter_echo_unnamed")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_update_v1")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_config_moonbit")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

inherit_test_dep!(
    #[tagged_as("agent_sdk_rust")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_sdk_ts")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("permissions_effect")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_scala")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_moonbit")]
    PrecompiledComponent
);

async fn external_invocation_id(
    executor: &TestWorkerExecutor,
    worker: &OwnedAgentId,
    key: &IdempotencyKey,
    method: &str,
) -> anyhow::Result<InvocationId> {
    let fingerprint = executor.get_worker_metadata(worker).await?.fingerprint;
    Ok(
        InvocationRequestKey::external(executor.auth_ctx().idempotency_scope(), Some(key))
            .finalize(worker.agent_id, fingerprint, method)
            .id(),
    )
}

macro_rules! method_failure_test {
    ($test_name:ident, $fixture:literal, $agent_type:literal, $declared:literal, [$($failure:literal),+]) => {
        #[test]
        #[timeout("2m")]
        async fn $test_name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            _tracing: &Tracing,
            #[tagged_as($fixture)] fixture: &PrecompiledComponent,
        ) -> anyhow::Result<()> {
            let context = TestContext::new(last_unique_id);
            let mut executor = start(deps, &context).await?;
            let component = executor
                .component_dep(&context.default_environment_id, fixture)
                .store()
                .await?;
            for failure in [$($failure),+] {
                let agent = agent_key!($agent_type, failure);
                let worker = executor.start_agent(&component.id, agent.clone()).await?;
                assert_eq!(
                    executor.invoke_and_await_agent(&component, &agent, $declared, data_value!())
                        .await?.into_typed::<Result<i64, String>>()?,
                    Err("declared error".to_string())
                );
                assert_eq!(
                    executor.invoke_and_await_agent(&component, &agent, "value", data_value!())
                        .await?.into_typed::<i64>()?,
                    0
                );
                let key = IdempotencyKey::fresh();
                let error = executor.invoke_and_await_agent_with_key(&component, &agent, &key, failure, data_value!())
                    .await.expect_err("the method must fail its invocation");
                assert!(format!("{error:#}").contains("method gave up"), "unexpected error: {error:#}");
                assert_eq!(
                    executor.invoke_and_await_agent(&component, &agent, "value", data_value!())
                        .await?.into_typed::<i64>()?,
                    7,
                    "a fresh call must observe the mutation made before the failure"
                );
                assert_eq!(
                    executor.invoke_and_await_agent(&component, &agent, "increment", data_value!())
                        .await?.into_typed::<i64>()?,
                    8,
                    "the agent must still accept mutations after the failed invocation"
                );
                let before = executor.get_oplog_last_index(&worker).await?;
                executor.release().await?;
                executor = start(deps, &context).await?;
                let error = executor.invoke_and_await_agent_with_key(&component, &agent, &key, failure, data_value!())
                    .await.expect_err("cold lookup must preserve the original failure");
                assert!(format!("{error:#}").contains("method gave up"), "{error:#}");
                assert_eq!(executor.get_oplog_last_index(&worker).await?, before);
                assert_eq!(
                    executor.invoke_and_await_agent(&component, &agent, "value", data_value!())
                        .await?.into_typed::<i64>()?,
                    8,
                    "reconstruction must retain the failed mutation and subsequent work"
                );
                assert_eq!(
                    executor.invoke_and_await_agent(&component, &agent, "increment", data_value!())
                        .await?.into_typed::<i64>()?,
                    9
                );
            }
            Ok(())
        }
    };
}

method_failure_test!(
    rust_method_error_fails_only_its_invocation,
    "agent_sdk_rust",
    "RustMethodFailure",
    "declared_error",
    ["fail"]
);

#[test]
#[timeout("2m")]
async fn method_failure_tail_survives_cold_lookup_and_state_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_key!("RustMethodFailure", "cold-failure-tail");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    let key = IdempotencyKey::fresh();
    let failure = executor
        .invoke_and_await_agent_with_key(&component, &agent, &key, "fail", data_value!())
        .await
        .expect_err("the invocation must fail");
    assert!(format!("{failure:#}").contains("method gave up"));
    let before = executor.get_oplog_last_index(&worker).await?;
    drop(executor);

    let executor = start(deps, &context).await?;
    let failure = executor
        .invoke_and_await_agent_with_key(&component, &agent, &key, "fail", data_value!())
        .await
        .expect_err("cold lookup must retain the failure");
    assert!(format!("{failure:#}").contains("method gave up"));
    assert_eq!(
        executor.get_oplog_last_index(&worker).await?,
        before,
        "lookup must not activate the guest or record a second execution"
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "value", data_value!())
            .await?
            .into_typed::<i64>()?,
        7
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "increment", data_value!())
            .await?
            .into_typed::<i64>()?,
        8
    );
    let invocation_id = external_invocation_id(&executor, &worker, &key, "fail").await?;
    let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    let starts: Vec<_> = oplog.iter().filter(|entry| matches!(
        &entry.entry, PublicOplogEntry::AgentInvocationStarted(start)
            if matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(method) if method.invocation_id == invocation_id)
    )).collect();
    assert_eq!(starts.len(), 1);
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(
                &entry.entry, PublicOplogEntry::AgentInvocationFailed(failure)
                    if failure.start_index == starts[0].oplog_index
            ))
            .count(),
        1
    );
    Ok(())
}

#[test]
#[timeout("2m")]
async fn stream_method_failure_settles_active_http_tail_before_publication_and_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::OplogEntry;
    use golem_common::model::oplog::host_functions::HostFunctionName;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let requests = Arc::new(AtomicUsize::new(0));
    let release_body = Arc::new(tokio::sync::Notify::new());
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let url = format!("http://localhost:{}/", listener.local_addr()?.port());
    let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
        let requests = requests.clone();
        let release_body = release_body.clone();
        async move {
            let route = axum::Router::new().route(
                "/",
                axum::routing::post(move || {
                    let requests = requests.clone();
                    let release_body = release_body.clone();
                    async move {
                        requests.fetch_add(1, Ordering::SeqCst);
                        axum::response::Response::builder()
                            .header("content-length", "16")
                            .body(axum::body::Body::from_stream(futures::stream::once(
                                async move {
                                    release_body.notified().await;
                                    Ok::<_, std::convert::Infallible>("effect completed")
                                },
                            )))
                            .unwrap()
                    }
                }),
            );
            axum::serve(listener, route).await.unwrap();
        }
    }));
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_key!("RustMethodFailure", "stream-failure-tail");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "value", data_value!())
        .await?;
    let mut gate = executor
        .gate_after_next_append(&worker.agent_id, |entry| {
            matches!(
                entry,
                OplogEntry::Start {
                    function_name: HostFunctionName::WallClockNow,
                    ..
                }
            )
        })
        .await;
    let key = IdempotencyKey::fresh();
    let metadata = executor.get_worker_metadata(&worker).await?;
    let invocation_id =
        external_invocation_id(&executor, &worker, &key, "fail_stream_after_http").await?;
    let request_key =
        InvocationRequestKey::external(executor.auth_ctx().idempotency_scope(), Some(&key));
    let (_, input) = data_value!(url).into_parts();
    let invocation = InvocationStart {
        target: Some(id_target(&worker)),
        component_id: Some(component.id.into()),
        method_name: Some("fail_stream_after_http".to_string()),
        input: Some(input.try_into().map_err(anyhow::Error::msg)?),
        mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
        idempotency_key: Some(key.clone().into()),
        component_owner_account_id: Some(component.account_id.into()),
        environment_id: Some(component.environment_id.into()),
        auth_ctx: Some(executor.auth_ctx().into()),
        freshness_disposition:
            golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist as i32,
        attempt_id: Some(uuid::Uuid::new_v4().into()),
        expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
        request_key: Some(request_key.into()),
        caller: Some(callers::anonymous().into()),
        ..Default::default()
    };
    let mut failure = Box::pin(executor.invoke_agent_session(invocation));
    tokio::select! {
        _ = gate.entered() => {}
        result = &mut failure => panic!("stream invocation ended before the HTTP checkpoint: {result:?}"),
    }
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    let queued = IdempotencyKey::fresh();
    let queued_id = external_invocation_id(&executor, &worker, &queued, "value").await?;
    executor
        .invoke_agent_with_key(&component, &agent, &queued, "value", data_value!())
        .await?;
    drop(gate);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut failure)
            .await
            .is_err(),
        "a method failure must not be published while its response-body work is active"
    );
    let held = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert!(
        held.iter().any(|entry| {
            matches!(
                &entry.entry, PublicOplogEntry::Start(start)
                    if start.function_name == "http::types::response::consume-body-chunk"
            ) && !held.iter().any(|terminal| matches!(
                &terminal.entry, PublicOplogEntry::End(end) if end.start_index == entry.oplog_index
            ))
        }),
        "the host must be waiting for real response-body I/O"
    );
    assert!(
        !held
            .iter()
            .any(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationFailed(_)))
    );
    assert!(!held.iter().any(|entry| matches!(
        &entry.entry, PublicOplogEntry::AgentInvocationStarted(start)
            if matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(method)
                if method.invocation_id == queued_id)
    )));
    release_body.notify_one();
    let error = failure
        .as_mut()
        .await
        .expect_err("the method must fail after settling its tail");
    assert!(format!("{error:#}").contains("method gave up"));
    assert_eq!(
        executor
            .invoke_and_await_agent_with_key(&component, &agent, &queued, "value", data_value!())
            .await?
            .into_typed::<i64>()?,
        7
    );
    let entries = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert_eq!(entries.iter().filter(|entry| matches!(
        &entry.entry, PublicOplogEntry::AgentInvocationStarted(start)
            if matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(method)
                if method.invocation_id == invocation_id)
    )).count(), 1);
    let terminal = entries
        .iter()
        .find(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationFailed(_)))
        .unwrap()
        .oplog_index;
    let body = entries
        .iter()
        .find(|entry| {
            matches!(
                &entry.entry, PublicOplogEntry::Start(start)
                    if start.function_name == "http::types::response::consume-body"
            )
        })
        .unwrap()
        .oplog_index;
    let body_end = entries
        .iter()
        .find(|entry| {
            matches!(
                &entry.entry, PublicOplogEntry::End(end) if end.start_index == body
            )
        })
        .expect("the response body must have a completed durable outcome")
        .oplog_index;
    assert!(body_end < terminal);
    drop(failure);
    drop(executor);

    let executor = start(deps, &context).await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "increment", data_value!())
            .await?
            .into_typed::<i64>()?,
        8
    );
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "reconstruction must reuse the completed HTTP effect"
    );
    let reconstructed = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert_eq!(
        reconstructed
            .iter()
            .filter(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationFailed(_)))
            .count(),
        1
    );
    Ok(())
}

#[test]
#[timeout("2m")]
async fn method_failure_repairs_an_incomplete_tail_without_repeating_completed_effects(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::OplogEntry;
    use golem_common::model::oplog::host_functions::HostFunctionName;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let requests = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let url = format!("http://localhost:{}/", listener.local_addr()?.port());
    let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
        let requests = requests.clone();
        async move {
            let route = axum::Router::new().route(
                "/",
                axum::routing::post(move || {
                    let requests = requests.clone();
                    async move {
                        requests.fetch_add(1, Ordering::SeqCst);
                        "effect completed"
                    }
                }),
            );
            axum::serve(listener, route).await.unwrap();
        }
    }));
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_key!("RustMethodFailure", "incomplete-failure-tail");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "value", data_value!())
        .await?;
    let mut gate = executor
        .gate_after_next_append(&worker.agent_id, |entry| {
            matches!(
                entry,
                OplogEntry::Start {
                    function_name: HostFunctionName::WallClockNow,
                    ..
                }
            )
        })
        .await;
    let key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent,
            &key,
            "fail_after_http",
            data_value!(url.clone()),
        )
        .await?;
    gate.entered().await;
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    let queued = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(&component, &agent, &queued, "value", data_value!())
        .await?;
    assert!(
        !executor
            .get_oplog(&worker, OplogIndex::INITIAL)
            .await?
            .iter()
            .any(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationFailed(_)))
    );
    gate.abort();
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(gate);
    drop(executor);

    let executor = start(deps, &context).await?;
    let failure = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent,
            &key,
            "fail_after_http",
            data_value!(url),
        )
        .await
        .expect_err("recovery must finish the original failed invocation");
    assert!(format!("{failure:#}").contains("method gave up"));
    assert_eq!(
        executor
            .invoke_and_await_agent_with_key(&component, &agent, &queued, "value", data_value!())
            .await?
            .into_typed::<i64>()?,
        7,
        "already queued work must not fail with the method"
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "increment", data_value!())
            .await?
            .into_typed::<i64>()?,
        8
    );
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "replay must not repeat the completed HTTP effect"
    );
    let invocation_id = external_invocation_id(&executor, &worker, &key, "fail_after_http").await?;
    let entries = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    let starts: Vec<_> = entries.iter().filter(|entry| matches!(
        &entry.entry, PublicOplogEntry::AgentInvocationStarted(start)
            if matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(method) if method.invocation_id == invocation_id)
    )).collect();
    assert_eq!(starts.len(), 1);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(
                &entry.entry, PublicOplogEntry::AgentInvocationFailed(failure)
                    if failure.start_index == starts[0].oplog_index
            ))
            .count(),
        1
    );
    Ok(())
}

#[test]
#[timeout("2m")]
async fn ephemeral_method_failure_keeps_archived_results_and_never_repairs_a_started_method(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::{OplogEntry, host_functions::HostFunctionName};
    use golem_worker_executor_test_utils::{TestExecutorOverrides, start_with_overrides};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.active_agents.ttl = std::time::Duration::from_secs(2);
            config.oplog.sweep.interval = std::time::Duration::from_millis(100);
        })),
        ..Default::default()
    };
    let mut executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let requests = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let url = format!("http://localhost:{}/", listener.local_addr()?.port());
    let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
        let requests = requests.clone();
        async move {
            let route = axum::Router::new().route(
                "/",
                axum::routing::post(move || {
                    let requests = requests.clone();
                    async move {
                        requests.fetch_add(1, Ordering::SeqCst);
                        "effect completed"
                    }
                }),
            );
            axum::serve(listener, route).await.unwrap();
        }
    }));
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    for (index, incomplete) in [false, true].into_iter().enumerate() {
        let agent = agent_key!(
            "RustEphemeralMethodFailure",
            format!("failure-{incomplete}")
        );
        let key = IdempotencyKey::fresh();
        let worker = ephemeral_agent_id(
            &executor,
            component.environment_id,
            &agent,
            &key,
            "fail_after_http",
        );
        if incomplete {
            let mut gate = executor
                .gate_after_next_append(&worker.agent_id, |entry| {
                    matches!(
                        entry,
                        OplogEntry::Start {
                            function_name: HostFunctionName::WallClockNow,
                            ..
                        }
                    )
                })
                .await;
            executor
                .invoke_agent_with_key(
                    &component,
                    &agent,
                    &key,
                    "fail_after_http",
                    data_value!(url.clone()),
                )
                .await?;
            gate.entered().await;
            assert_eq!(requests.load(Ordering::SeqCst), index + 1);
            assert!(
                !executor
                    .get_oplog(&worker, OplogIndex::INITIAL)
                    .await?
                    .iter()
                    .any(|entry| matches!(
                        &entry.entry,
                        PublicOplogEntry::AgentInvocationFailed(_)
                    ))
            );
            gate.abort();
            executor.shutdown_and_wait_for_invocation_loops().await?;
            drop(gate);
            drop(executor);
            executor = start_with_overrides(deps, &context, overrides.clone()).await?;
        }
        let expected = if incomplete {
            "an ephemeral agent cannot accept another invocation or be resumed"
        } else {
            "method gave up"
        };
        let failure = executor
            .invoke_and_await_agent_with_key(
                &component,
                &agent,
                &key,
                "fail_after_http",
                data_value!(url.clone()),
            )
            .await
            .expect_err("an ephemeral method failure must remain terminal");
        assert!(
            format!("{failure:#}")
                .to_ascii_lowercase()
                .contains(expected),
            "{failure:#}"
        );
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while executor.worker_is_cached(&worker).await
                || crate::oplog_sweep::ephemeral_layer_rows(deps, &context, &worker.agent_id)
                    .await?
                    != 0
            {
                tokio::task::yield_now().await;
            }
            anyhow::Ok(())
        })
        .await??;
        let invocation_id =
            external_invocation_id(&executor, &worker, &key, "fail_after_http").await?;
        let entries = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
        let starts: Vec<_> = entries.iter().filter(|entry| matches!(
            &entry.entry, PublicOplogEntry::AgentInvocationStarted(start)
                if matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(method) if method.invocation_id == invocation_id)
        )).collect();
        assert_eq!(starts.len(), 1);
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(
                    &entry.entry, PublicOplogEntry::AgentInvocationFailed(failure)
                        if failure.start_index == starts[0].oplog_index
                ))
                .count(),
            usize::from(!incomplete)
        );
        let before = executor.get_oplog_last_index(&worker).await?;
        executor.release().await?;
        executor = start_with_overrides(deps, &context, overrides.clone()).await?;
        let failure = executor
            .invoke_and_await_agent_with_key(
                &component,
                &agent,
                &key,
                "fail_after_http",
                data_value!(url.clone()),
            )
            .await
            .expect_err("cold lookup must retain the archived failure");
        assert!(
            format!("{failure:#}")
                .to_ascii_lowercase()
                .contains(expected),
            "{failure:#}"
        );
        assert_eq!(executor.get_oplog_last_index(&worker).await?, before);
        assert_eq!(
            requests.load(Ordering::SeqCst),
            index + 1,
            "archival and cold lookup must not repeat HTTP"
        );
    }
    Ok(())
}

#[test]
#[timeout("1m")]
async fn constructor_failure_refuses_admission_on_an_already_acquired_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::invocation_context::InvocationContextStack;
    use golem_common::model::{AgentInvocation, AgentStatus};
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_worker_executor::services::UsesAllDeps;
    use std::time::Duration;

    let context = TestContext::new(last_unique_id);
    let mut executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_key!("RustMethodFailure", "fail-constructor");
    let worker_id = keyed_agent_id(component.environment_id, &agent);
    let creation_error = executor
        .start_agent(&component.id, agent)
        .await
        .expect_err("create must report the constructor failure");
    assert!(format!("{creation_error:#}").contains("constructor gave up"));
    // Use the acquired shell directly, as ingress does if construction fails after acquisition.
    let mut worker = executor.active_agent(&worker_id).await.unwrap().primary();
    executor
        .wait_for_status(&worker_id, AgentStatus::Failed, Duration::from_secs(10))
        .await?;

    for cold in [false, true] {
        if cold {
            drop(worker);
            executor.release().await?;
            executor = start(deps, &context).await?;
            let healthy = agent_key!("RustMethodFailure", "healthy-after-restart");
            let healthy_id = executor.start_agent(&component.id, healthy.clone()).await?;
            assert_eq!(
                executor
                    .invoke_and_await_agent(&component, &healthy, "increment", data_value!())
                    .await?
                    .into_typed::<i64>()?,
                1
            );
            let healthy_worker = executor.active_agent(&healthy_id).await.unwrap().primary();
            worker = golem_worker_executor::worker::Worker::get_exact_existing_suspended(
                healthy_worker.all(),
                &worker_id,
                callers::anonymous(),
            )
            .await?;
        }
        let before = executor.stored_oplog(&worker_id.agent_id).await;
        for scheduled in [false, true] {
            let invocation = AgentInvocation::AgentMethod {
                idempotency_key: InvocationId(uuid::Uuid::new_v4()),
                method_name: "value".to_string(),
                input: data_value!().into_parts().1,
                invocation_context: InvocationContextStack::fresh(),
                caller: callers::anonymous(),
                scope_card: None,
            };
            let error = if scheduled {
                worker
                    .schedule_invocation(
                        invocation,
                        chrono::Utc::now() + chrono::Duration::hours(1),
                    )
                    .await
                    .expect_err("a failed constructor must refuse a new schedule")
            } else {
                match worker.clone().invoke(invocation).await {
                    Err(error) => error,
                    Ok(_) => panic!("a failed constructor accepted work that can never run"),
                }
            };
            assert!(
                matches!(&error, WorkerExecutorError::PreviousInvocationFailed { .. }),
                "unexpected admission error: {error:#}"
            );
            assert!(format!("{error:#}").contains("constructor gave up"));
        }
        assert_eq!(executor.stored_oplog(&worker_id.agent_id).await, before);
        assert_eq!(
            executor.get_worker_metadata(&worker_id).await?.status,
            AgentStatus::Failed
        );
    }
    Ok(())
}

#[test]
#[timeout("2m")]
async fn method_failure_classification_keeps_constructors_and_rust_panics_fatal(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
    #[tagged_as("agent_sdk_go")] go_fixture: &PrecompiledComponent,
    #[tagged_as("tool_streaming_scala")] scala_fixture: &PrecompiledComponent,
    #[tagged_as("permissions_effect")] effect_fixture: &PrecompiledComponent,
    #[tagged_as("agent_sdk_ts")] ts_fixture: &PrecompiledComponent,
    #[tagged_as("tool_streaming_moonbit")] moonbit_fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::AgentStatus;
    use std::time::Duration;

    let context = TestContext::new(last_unique_id);
    let mut executor = start(deps, &context).await?;
    for (fixture, agent_type, name, method, detail) in [
        (
            fixture,
            "RustMethodFailure",
            "fail-constructor",
            "value",
            "constructor gave up",
        ),
        (
            fixture,
            "RustMethodFailure",
            "panic-control",
            "panic",
            "method panicked",
        ),
        (
            go_fixture,
            "CounterAgent",
            "fail-constructor",
            "value",
            "wasm trap",
        ),
        (
            scala_fixture,
            "ScalaMethodFailure",
            "fail-constructor",
            "value",
            "ScalaMethodFailureImpl",
        ),
        (
            effect_fixture,
            "EffectMethodFailure",
            "fail-constructor",
            "value",
            "constructor gave up",
        ),
        (
            ts_fixture,
            "TsMethodFailure",
            "fail-constructor",
            "value",
            "constructor gave up",
        ),
        (
            moonbit_fixture,
            "MoonBitMethodFailure",
            "fail-constructor",
            "value",
            "constructor gave up",
        ),
    ] {
        let component = executor
            .component_dep(&context.default_environment_id, fixture)
            .store()
            .await?;
        let agent = agent_key!(agent_type, name);
        let worker = if name == "fail-constructor" {
            executor
                .start_agent(&component.id, agent.clone())
                .await
                .expect_err("create must report the constructor failure");
            let worker = keyed_agent_id(component.environment_id, &agent);
            executor
                .wait_for_status(&worker, AgentStatus::Failed, Duration::from_secs(10))
                .await?;
            worker
        } else {
            executor.start_agent(&component.id, agent.clone()).await?
        };
        let key = IdempotencyKey::fresh();
        let failure = executor
            .invoke_and_await_agent_with_key(&component, &agent, &key, method, data_value!())
            .await
            .expect_err("the control must remain fatal");
        assert!(
            format!("{failure:#}").contains(detail),
            "unexpected fatal error: {failure:#}"
        );
        for cold in [false, true] {
            if cold {
                executor.release().await?;
                executor = start(deps, &context).await?;
            }
            let failure = executor
                .invoke_and_await_agent(&component, &agent, "value", data_value!())
                .await
                .expect_err("a fatal control must reject a later invocation");
            let expected = if !cold {
                "Previous invocation failed"
            } else if matches!(agent_type, "ScalaMethodFailure" | "EffectMethodFailure") {
                "golem:agent/guest@2.0.0#initialize"
            } else {
                detail
            };
            assert!(
                format!("{failure:#}").contains(expected),
                "unexpected rejection: {failure:#}"
            );
            assert_eq!(
                executor.get_worker_metadata(&worker).await?.status,
                AgentStatus::Failed
            );
            assert!(
                !executor
                    .get_oplog(&worker, OplogIndex::INITIAL)
                    .await?
                    .iter()
                    .any(|entry| matches!(
                        &entry.entry,
                        PublicOplogEntry::AgentInvocationFailed(_)
                    ))
            );
        }
    }
    Ok(())
}

#[test]
#[timeout("2m")]
async fn method_failure_replay_rejects_success_or_trap_in_place_of_a_local_terminal(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::{AgentError, OplogEntry};

    let context = TestContext::new(last_unique_id);
    let mut executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    for method in ["value", "panic"] {
        let agent = agent_key!("RustMethodFailure", format!("mismatched-{method}"));
        let worker = executor.start_agent(&component.id, agent.clone()).await?;
        executor
            .invoke_and_await_agent(&component, &agent, "value", data_value!())
            .await?;
        let mut gate = executor
            .gate_after_next_append(&worker.agent_id, |entry| {
                matches!(entry, OplogEntry::AgentInvocationStarted { .. })
            })
            .await;
        let key = IdempotencyKey::fresh();
        let invocation_id = external_invocation_id(&executor, &worker, &key, method).await?;
        executor
            .invoke_agent_with_key(&component, &agent, &key, method, data_value!())
            .await?;
        gate.entered().await;
        // The gate holds the status actor's append receipt, so inspect storage directly.
        let entries = executor.stored_oplog(&worker.agent_id).await;
        let anchor = OplogIndex::from_u64(
            entries
                .iter()
                .position(|entry| {
                    matches!(
                        entry, OplogEntry::AgentInvocationStarted { invocation_id: started, .. }
                            if *started == invocation_id
                    )
                })
                .expect("the gated invocation start must be committed") as u64
                + 1,
        );
        // Deliberately forge an incompatible completed outcome. Reconstruction must not
        // accept a successful body or an actual Wasm trap as the recorded method error.
        let terminal = OplogEntry::agent_invocation_failed(
            None,
            invocation_id,
            AgentError::InternalError("recorded method failure".into()),
            anchor,
        );
        let terminal_index = executor
            .commit_oplog_entry_bypassing_worker_status(&worker.agent_id, terminal.clone())
            .await?;
        let persisted_terminal = executor.stored_oplog(&worker.agent_id).await
            [terminal_index.as_u64() as usize - 1]
            .clone();
        gate.abort();
        executor.shutdown_and_wait_for_invocation_loops().await?;
        drop(gate);
        drop(executor);

        executor = start(deps, &context).await?;
        let failure = executor
            .invoke_and_await_agent(&component, &agent, "value", data_value!())
            .await
            .expect_err("incompatible replay must fail closed");
        assert!(
            format!("{failure:#}").contains("Unexpected oplog entry"),
            "unexpected recovery failure: {failure:#}"
        );
        let entries = executor.stored_oplog(&worker.agent_id).await;
        assert_eq!(
            &entries[terminal_index.as_u64() as usize - 1],
            &persisted_terminal
        );
        assert_eq!(entries.iter().filter(|entry| matches!(
            entry, OplogEntry::AgentInvocationFailed { start_index, .. } if *start_index == anchor
        )).count(), 1, "recovery must not replace or duplicate the recorded method failure");
        let lookup = executor
            .invoke_agent_session(InvocationStart {
                target: Some(id_target(&worker)),
                component_id: Some(component.id.into()),
                mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32,
                idempotency_key: Some(IdempotencyKey::new(invocation_id.to_string()).into()),
                component_owner_account_id: Some(component.account_id.into()),
                environment_id: Some(component.environment_id.into()),
                auth_ctx: Some(executor.auth_ctx().into()),
                freshness_disposition:
                    golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                        as i32,
                caller: Some(callers::anonymous().into()),
                ..Default::default()
            })
            .await
            .expect_err("the original key must keep its recorded failure");
        assert!(
            format!("{lookup:#}").contains("recorded method failure"),
            "recorded outcome changed: {lookup:#}"
        );
    }
    Ok(())
}

#[test]
#[timeout("2m")]
async fn lookup_of_a_completed_method_returns_its_value_and_revision(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::worker::{
        AgentInvocationMode, InvocationFreshnessDisposition, InvocationStatus,
        invocation_session_result,
    };

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let agent = agent_key!("RpcBlockingCounter", "lookup-value");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "inc_by", data_value!(5u64))
        .await?;
    let key = IdempotencyKey::fresh();
    let awaited = executor
        .invoke_and_await_agent_with_key(&component, &agent, &key, "get_value", data_value!())
        .await?;
    executor
        .invoke_and_await_agent(&component, &agent, "inc_by", data_value!(2u64))
        .await?;
    let invocation_id = external_invocation_id(&executor, &worker, &key, "get_value").await?;

    let lookup = executor
        .invoke_agent_session(InvocationStart {
            target: Some(id_target(&worker)),
            mode: AgentInvocationMode::Lookup as i32,
            idempotency_key: Some(IdempotencyKey::new(invocation_id.to_string()).into()),
            component_owner_account_id: Some(component.account_id.into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            freshness_disposition: InvocationFreshnessDisposition::MayExist as i32,
            caller: Some(callers::anonymous().into()),
            ..Default::default()
        })
        .await?;
    assert_eq!(lookup.status, Some(InvocationStatus::Complete as i32));
    assert_eq!(lookup.component_revision, Some(component.revision.get()));
    let Some(invocation_session_result::Result::MethodResult(value)) = lookup.result else {
        anyhow::bail!("expected the method result, got {:?}", lookup.result);
    };
    assert_eq!(
        Some(&SchemaValue::try_from(value).map_err(anyhow::Error::msg)?),
        awaited.value()
    );
    assert_eq!(awaited.into_typed::<u64>()?, 5);
    Ok(())
}

method_failure_test!(
    ts_method_error_fails_only_its_invocation,
    "agent_sdk_ts",
    "TsMethodFailure",
    "declaredError",
    ["fail"]
);
method_failure_test!(
    ts_async_method_error_fails_only_its_invocation,
    "agent_sdk_ts",
    "TsMethodFailure",
    "declaredError",
    ["failAsync"]
);
method_failure_test!(
    effect_method_error_fails_only_its_invocation,
    "permissions_effect",
    "EffectMethodFailure",
    "declaredError",
    ["fail"]
);
method_failure_test!(
    scala_method_error_fails_only_its_invocation,
    "tool_streaming_scala",
    "ScalaMethodFailure",
    "declaredError",
    ["fail"]
);
method_failure_test!(
    scala_async_method_error_fails_only_its_invocation,
    "tool_streaming_scala",
    "ScalaMethodFailure",
    "declaredError",
    ["failAsync"]
);
method_failure_test!(
    moonbit_method_error_fails_only_its_invocation,
    "tool_streaming_moonbit",
    "MoonBitMethodFailure",
    "declared_error",
    ["fail"]
);

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn static_config_defaults_and_creation_overrides_survive_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] agent_sdk_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    check_static_config_reconstruction(last_unique_id, deps, agent_sdk_rust).await
}

#[test]
#[timeout("120s")]
async fn moonbit_static_config_defaults_and_creation_overrides_survive_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_config_moonbit")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    check_static_config_reconstruction(last_unique_id, deps, component).await
}

async fn check_static_config_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    guest: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("../../test-data/config-conformance/v1.json"))?;
    let reference = &corpus["reference"];
    let agent_type = reference["agentType"].as_str().unwrap();
    let entries = |layer: &str| {
        reference[layer]
            .as_object()
            .unwrap()
            .iter()
            .map(|(path, value)| AgentConfigEntryDto {
                path: vec![path.clone()],
                value: value.clone().into(),
            })
            .collect()
    };
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, guest)
        .with_agent_config(agent_type, entries("deployment"))
        .store()
        .await?;
    let agent_id = agent_key!(agent_type, "captured-defaults");
    let worker_id = executor
        .start_agent_with(
            &component.id,
            agent_id.clone(),
            HashMap::new(),
            entries("creation"),
        )
        .await?;
    let deployed_agent_id = agent_key!(agent_type, "deployment-defaults");
    let reads = [
        (&deployed_agent_id, &reference["expectedDeploymentRead"]),
        (&agent_id, &reference["expectedRead"]),
    ];
    for (id, expected) in reads {
        let actual = executor
            .invoke_and_await_agent(&component, id, "read", data_value!())
            .await?
            .into_typed::<String>()?;
        assert_eq!(
            &serde_json::from_str::<serde_json::Value>(&actual)?,
            expected
        );
    }

    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start(deps, &context).await?;
    for (id, expected) in reads {
        let actual = executor
            .invoke_and_await_agent(&component, id, "read", data_value!())
            .await?
            .into_typed::<String>()?;
        assert_eq!(
            &serde_json::from_str::<serde_json::Value>(&actual)?,
            expected
        );
    }
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    for entry in &oplog {
        if let PublicOplogEntry::Start(_) = &entry.entry {
            assert!(
                oplog.iter().any(|terminal| matches!(
                    &terminal.entry,
                    PublicOplogEntry::End(end) if end.start_index == entry.oplog_index
                )),
                "config call at {} must have its recorded result",
                entry.oplog_index
            );
        }
    }
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(&entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
            .count(),
        3,
    );
    Ok(())
}

#[test]
#[tracing::instrument]
async fn agent_self_rpc_is_not_allowed(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;
    let agent_id = agent_key!("SelfRpcAgent", "worker-name");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "selfRpc", data_value!())
        .await;

    let err = result.expect_err("Expected an error");
    assert!(
        err.to_string()
            .contains("RPC calls to the same agent are not supported")
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let scope_starts: Vec<_> = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.request.is_none() => Some(entry.oplog_index),
            _ => None,
        })
        .collect();
    for start_index in scope_starts {
        assert!(
            oplog.iter().any(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::End(params) if params.start_index == start_index
                )
            }),
            "durable scope opened at {start_index} was not closed"
        );
    }

    Ok(())
}

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn streaming_schedule_is_rejected_without_creating_or_queueing_a_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let scheduler =
        SqliteSchedulerStorage::configured(&scheduler_sqlite_storage_config(deps, &context))
            .await
            .map_err(anyhow::Error::msg)?;
    let agent_id = agent_key!("StreamingRpcTarget", "rejected-schedule");
    let worker_id = keyed_agent_id(component.environment_id, &agent_id);
    let (_, input) = data_value!(vec![1_u32, 2, 3]).into_parts();
    let input: golem_schema::proto::golem::schema::SchemaValue =
        input.try_into().map_err(anyhow::Error::msg)?;
    let component_id = component.id.to_string();
    let blobs_before = files_below(&deps.blob_storage_root())?
        .into_iter()
        .filter(|path| path.to_string_lossy().contains(&component_id))
        .collect::<HashSet<_>>();

    for schedule_at in [
        None,
        Some(prost_types::Timestamp {
            seconds: chrono::Utc::now().timestamp() + 1,
            nanos: 0,
        }),
    ] {
        let error = executor
            .invoke_agent_session(InvocationStart {
                target: Some(key_target(&agent_id)?),
                constructor: None,
                component_id: Some(component.id.into()),
                method_name: Some("produce".to_string()),
                input: Some(input.clone()),
                mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32,
                schedule_at,
                idempotency_key: Some(IdempotencyKey::fresh().into()),
                component_owner_account_id: Some(component.account_id.into()),
                environment_id: Some(component.environment_id.into()),
                auth_ctx: Some(executor.auth_ctx().into()),
                context: None,
                caller: Some(callers::anonymous().into()),
                freshness_disposition:
                    golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                        as i32,
                config: Vec::new(),
                attempt_id: None,
                expected_callee_fingerprint: None,
                durable_input_mappings: Vec::new(),
                scope_card: None,
                origin_invocation: None,
                external_tool: None,
                request_key: Some(InvocationRequestKey::external(None, None).into()),
            })
            .await
            .expect_err("scheduled streaming invocation must be rejected");
        assert!(
            error
                .to_string()
                .contains("require an attached Await invocation session"),
            "unexpected error: {error}"
        );
    }

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert_eq!(executor.get_worker_metadata_opt(&worker_id).await?, None);
    let assignment =
        golem_common::model::ShardAssignment::unexpiring(1, [golem_common::model::ShardId::new(0)]);
    assert_eq!(
        scheduler
            .count_due(chrono::Utc::now() + chrono::Duration::days(1), &assignment)
            .await?,
        0,
        "rejection must not create an immediate or delayed scheduled action"
    );
    assert_eq!(
        files_below(&deps.blob_storage_root())?
            .into_iter()
            .filter(|path| path.to_string_lossy().contains(&component_id))
            .collect::<HashSet<_>>(),
        blobs_before,
        "rejection must not upload an invocation or result payload"
    );
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn invocation_classification_uses_the_existing_workers_component_revision(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let streaming_component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let streaming_agent_id = agent_key!("StreamingRpcTarget", "pinned-streaming-revision");
    let streaming_worker_id = executor
        .start_agent(&streaming_component.id, streaming_agent_id.clone())
        .await?;
    executor
        .update_component(&streaming_component.id, &agent_rpc.wasm_name)
        .await?;

    let (_, input) = data_value!(vec![1_u32, 2, 3]).into_parts();
    let error = executor
        .invoke_agent_session(InvocationStart {
            target: Some(id_target(&streaming_worker_id)),
            constructor: None,
            component_id: Some(streaming_component.id.into()),
            method_name: Some("produce".to_string()),
            input: Some(input.try_into().map_err(anyhow::Error::msg)?),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32,
            schedule_at: None,
            idempotency_key: Some(IdempotencyKey::fresh().into()),
            component_owner_account_id: Some(streaming_component.account_id.into()),
            environment_id: Some(streaming_component.environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            context: None,
            caller: Some(callers::anonymous().into()),
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            config: Vec::new(),
            attempt_id: None,
            expected_callee_fingerprint: None,
            durable_input_mappings: Vec::new(),
            scope_card: None,
            origin_invocation: None,
            external_tool: None,
            request_key: Some(InvocationRequestKey::external(None, None).into()),
        })
        .await
        .expect_err("the old streaming schema must still reject scheduling");
    assert!(
        error
            .to_string()
            .contains("require an attached Await invocation session"),
        "unexpected error: {error}"
    );
    let streaming_metadata = executor.get_worker_metadata(&streaming_worker_id).await?;
    assert_eq!(
        streaming_metadata.component_revision,
        ComponentRevision::INITIAL
    );
    assert_eq!(streaming_metadata.pending_invocation_count, 0);

    let stream_free_component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;
    let stream_free_agent_id = agent_key!("TestAgent", "pinned-stream-free-revision");
    let stream_free_worker_id = executor
        .start_agent(&stream_free_component.id, stream_free_agent_id.clone())
        .await?;
    executor
        .update_component(&stream_free_component.id, &agent_rpc_rust.wasm_name)
        .await?;

    // The updated component does not implement `TestAgent`, so only the ID still reaches it.
    let output = executor
        .invoke_and_await_agent_by_id(
            &stream_free_component,
            &stream_free_worker_id,
            None,
            "run",
            data_value!(0_f64),
        )
        .await?;
    assert_eq!(
        output.into_return_value(),
        Some(SchemaValue::List {
            elements: Vec::new()
        })
    );
    let stream_free_metadata = executor.get_worker_metadata(&stream_free_worker_id).await?;
    assert_eq!(
        stream_free_metadata.component_revision,
        ComponentRevision::INITIAL
    );
    Ok(())
}

fn files_below(root: &Path) -> anyhow::Result<HashSet<PathBuf>> {
    fn visit(root: &Path, current: &Path, files: &mut HashSet<PathBuf>) -> anyhow::Result<()> {
        if !current.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(current)? {
            let path = entry?.path();
            if path.is_dir() {
                visit(root, &path, files)?;
            } else {
                files.insert(path.strip_prefix(root)?.to_path_buf());
            }
        }
        Ok(())
    }

    let mut files = HashSet::new();
    visit(root, root, &mut files)?;
    Ok(files)
}

#[test]
#[tracing::instrument]
async fn agent_await_parallel_rpc_calls(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .store()
        .await?;

    let unique_id = context.redis_prefix();
    let agent_id = agent_key!("TestAgent", unique_id);
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "run", data_value!(20f64))
        .await;

    executor.check_oplog_is_queryable(&worker_id).await?;

    assert!(result.is_ok());
    Ok(())
}

#[test]
#[tracing::instrument]
async fn rpc_created_agents_do_not_inherit_the_caller_environment(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc")] agent_rpc: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc)
        .with_env(
            "TestAgent",
            vec![
                ("ENV1".to_string(), "1".to_string()),
                ("ENV2".to_string(), "2".to_string()),
            ],
        )
        .store()
        .await?;
    let unique_id = context.redis_prefix();
    let agent_id = agent_key!("TestAgent", unique_id);

    let mut env = HashMap::new();
    env.insert("ENV2".to_string(), "22".to_string());
    env.insert("ENV3".to_string(), "33".to_string());

    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    executor.log_output(&worker_id).await?;

    let result = executor
        .invoke_and_await_agent(&component, &agent_id, "envVarTest", data_value!())
        .await;

    let child_agent_id = agent_key!("ChildAgent", 0.0f64);
    let child_worker_id = keyed_agent_id(component.environment_id, &child_agent_id);

    executor.check_oplog_is_queryable(&worker_id).await?;
    executor.check_oplog_is_queryable(&child_worker_id).await?;

    let child_metadata = executor.get_worker_metadata(&child_worker_id).await?;
    let parent_identity = executor
        .active_agents()
        .try_get(&worker_id)
        .await
        .expect("parent is cached")
        .get_initial_worker_metadata();
    let child_identity = executor
        .active_agents()
        .try_get(&child_worker_id)
        .await
        .expect("child is cached")
        .get_initial_worker_metadata();
    assert_eq!(parent_identity.component_id, component.id);
    assert_eq!(
        parent_identity
            .agent_type_name
            .as_ref()
            .map(|name| name.0.as_str()),
        Some("TestAgent")
    );
    assert_eq!(parent_identity.canonical_key, Some(agent_id.to_string()));
    assert_eq!(child_identity.component_id, component.id);
    assert_eq!(
        child_identity
            .agent_type_name
            .as_ref()
            .map(|name| name.0.as_str()),
        Some("ChildAgent")
    );
    assert_eq!(
        child_identity.canonical_key,
        Some(child_agent_id.to_string())
    );

    let mut parent_env_vars = BTreeMap::new();
    let mut child_env_vars = BTreeMap::new();

    if let Ok(data_value) = result
        && let Some(SchemaValue::Record { fields }) = data_value.into_return_value().as_ref()
    {
        let parent = &fields[0];
        let child = &fields[1];

        if let SchemaValue::List {
            elements: parent_env_vars_list,
        } = parent
        {
            for env_var in parent_env_vars_list {
                if let SchemaValue::Record { fields: env_var_kv } = env_var
                    && let SchemaValue::String(key) = &env_var_kv[0]
                {
                    parent_env_vars.insert(key.clone(), env_var_kv[1].clone());
                }
            }
        }

        if let SchemaValue::List {
            elements: child_env_vars_list,
        } = child
        {
            for env_var in child_env_vars_list {
                if let SchemaValue::Record { fields: env_var_kv } = env_var
                    && let SchemaValue::String(key) = &env_var_kv[0]
                {
                    child_env_vars.insert(key.clone(), env_var_kv[1].clone());
                }
            }
        }
    }

    assert_eq!(
        parent_env_vars.into_iter().collect::<Vec<_>>(),
        vec![
            ("ENV1".to_string(), SchemaValue::String("1".to_string())),
            ("ENV2".to_string(), SchemaValue::String("22".to_string())),
            ("ENV3".to_string(), SchemaValue::String("33".to_string())),
            (
                "GOLEM_AGENT_ID".to_string(),
                SchemaValue::String(worker_id.agent_id.to_string())
            ),
            (
                "GOLEM_AGENT_KEY".to_string(),
                SchemaValue::String(agent_id.to_string())
            ),
            (
                "GOLEM_AGENT_TYPE".to_string(),
                SchemaValue::String("TestAgent".to_string())
            ),
            (
                "GOLEM_COMPONENT_REVISION".to_string(),
                SchemaValue::String("0".to_string())
            ),
        ]
    );
    // An agent created by RPC does not inherit its caller's environment: the child sees only
    // the component's environment of its own type, which is empty here, and its own identity.
    assert_eq!(
        child_env_vars.into_iter().collect::<Vec<_>>(),
        vec![
            (
                "GOLEM_AGENT_ID".to_string(),
                SchemaValue::String(child_worker_id.agent_id.to_string())
            ),
            (
                "GOLEM_AGENT_KEY".to_string(),
                SchemaValue::String(child_agent_id.to_string())
            ),
            (
                "GOLEM_AGENT_TYPE".to_string(),
                SchemaValue::String("ChildAgent".to_string())
            ),
            (
                "GOLEM_COMPONENT_REVISION".to_string(),
                SchemaValue::String("0".to_string())
            ),
        ]
    );
    assert_eq!(child_metadata.env, HashMap::new());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn ephemeral_agent_works(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo_unnamed")]
    constructor_parameter_echo_unnamed: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap();

    let component = executor
        .component_dep(
            &context.default_environment_id,
            constructor_parameter_echo_unnamed,
        )
        .store()
        .await?;

    let agent_id1 = agent_key!("EphemeralEchoAgent", "param1");
    let agent_id2 = agent_key!("EphemeralEchoAgent", "param2");
    let idempotency_key = IdempotencyKey::fresh();
    let result1 = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id1,
            &idempotency_key,
            "changeAndGet",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;

    let result2 = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id1,
            &idempotency_key,
            "changeAndGet",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;

    let result3 = executor
        .invoke_and_await_agent(&component, &agent_id2, "changeAndGet", data_value!())
        .await?
        .into_typed::<String>()?;

    let result4 = executor
        .invoke_and_await_agent(&component, &agent_id2, "changeAndGet", data_value!())
        .await?
        .into_typed::<String>()?;

    assert_eq!(result1, "param1!");
    assert_eq!(result2, "param1!");
    assert_eq!(result3, "param2!");
    assert_eq!(result4, "param2!");
    Ok(())
}

#[test]
#[timeout("30s")]
#[tracing::instrument]
async fn ephemeral_agent_rejects_start_when_monthly_memory_is_exhausted(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use async_trait::async_trait;
    use golem_common::model::account::AccountId;
    use golem_common::model::account_usage::{AccountUsagePeriod, MonthlyUsageMode};
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_service_base::model::MonthlyResourcePolicy;
    use golem_worker_executor::services::resource_limits::{AtomicResourceEntry, ResourceLimits};
    use golem_worker_executor_test_utils::start_with_resource_limits;
    use std::sync::Arc;

    struct ExhaustedMonthlyMemory;

    #[async_trait]
    impl ResourceLimits for ExhaustedMonthlyMemory {
        async fn initialize_account(
            &self,
            _account_id: AccountId,
        ) -> Result<Arc<AtomicResourceEntry>, WorkerExecutorError> {
            Ok(Arc::new(AtomicResourceEntry::new_with_monthly_policy(
                MonthlyResourcePolicy {
                    period: AccountUsagePeriod::current(),
                    mode: MonthlyUsageMode::HardLimit,
                    available_fuel: u64::MAX,
                    available_memory_gb_seconds: 0,
                    available_memory_byte_nanoseconds_remainder: 0,
                    available_durable_storage_byte_seconds: u64::MAX,
                    available_durable_storage_byte_nanoseconds_remainder: 0,
                    available_ephemeral_storage_byte_seconds: u64::MAX,
                    available_ephemeral_storage_byte_nanoseconds_remainder: 0,
                },
                usize::MAX,
                usize::MAX,
                u64::MAX,
                AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
            )))
        }
    }

    let context = TestContext::new(last_unique_id);
    let executor =
        start_with_resource_limits(deps, &context, Arc::new(ExhaustedMonthlyMemory)).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let error = executor
        .invoke_and_await_agent(
            &component,
            &agent_key!("EphemeralCounter", "monthly-memory-exhausted"),
            "increment",
            data_value!(),
        )
        .await
        .expect_err("exhausted monthly memory must reject ephemeral startup");

    assert!(
        error.to_string().contains("monthly memory exhausted"),
        "unexpected error: {error}"
    );
    Ok(())
}

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn immediate_scheduled_ephemeral_invocation_reuses_completed_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo_unnamed")]
    constructor_parameter_echo_unnamed: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(
            &context.default_environment_id,
            constructor_parameter_echo_unnamed,
        )
        .store()
        .await?;
    let logical_agent_id = agent_key!("EphemeralEchoAgent", "immediate-schedule-retry");
    let idempotency_key = IdempotencyKey::fresh();

    let initial_result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &logical_agent_id,
            &idempotency_key,
            "changeAndGet",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(initial_result, "immediate-schedule-retry!");

    let request_key = InvocationRequestKey::external(
        executor.auth_ctx().idempotency_scope(),
        Some(&idempotency_key),
    );
    let worker_id = ephemeral_agent_id(
        &executor,
        component.environment_id,
        &logical_agent_id,
        &idempotency_key,
        "changeAndGet",
    );

    executor
        .invoke_agent_session(InvocationStart {
            target: Some(id_target(&worker_id)),
            constructor: Some(proto_agent_key(&logical_agent_id)?),
            component_id: Some(component.id.into()),
            method_name: Some("changeAndGet".to_string()),
            input: Some(
                SchemaValue::Record { fields: vec![] }
                    .try_into()
                    .map_err(anyhow::Error::msg)?,
            ),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32,
            schedule_at: None,
            idempotency_key: Some(idempotency_key.into()),
            component_owner_account_id: Some(component.account_id.into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            context: None,
            caller: Some(callers::anonymous().into()),
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            config: Vec::new(),
            attempt_id: None,
            expected_callee_fingerprint: None,
            durable_input_mappings: Vec::new(),
            scope_card: None,
            origin_invocation: None,
            external_tool: None,
            request_key: Some(request_key.into()),
        })
        .await?;

    Ok(())
}

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn ephemeral_invocation_lookup_does_not_create_unknown_agent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo_unnamed")]
    constructor_parameter_echo_unnamed: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(
            &context.default_environment_id,
            constructor_parameter_echo_unnamed,
        )
        .store()
        .await?;
    let idempotency_key = IdempotencyKey::fresh();
    let worker_id = ephemeral_agent_id(
        &executor,
        component.environment_id,
        &agent_key!("EphemeralEchoAgent", "unknown-lookup"),
        &idempotency_key,
        "changeAndGet",
    );

    executor
        .invoke_agent_session(InvocationStart {
            target: Some(id_target(&worker_id)),
            constructor: None,
            component_id: Some(component.id.into()),
            method_name: None,
            input: None,
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32,
            schedule_at: None,
            idempotency_key: Some(idempotency_key.into()),
            component_owner_account_id: Some(component.account_id.into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            context: None,
            caller: Some(callers::anonymous().into()),
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            config: Vec::new(),
            attempt_id: None,
            expected_callee_fingerprint: None,
            durable_input_mappings: Vec::new(),
            scope_card: None,
            origin_invocation: None,
            external_tool: None,
            request_key: None,
        })
        .await?;
    assert_eq!(executor.get_worker_metadata_opt(&worker_id).await?, None);

    Ok(())
}

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn scheduled_ephemeral_invocation_uses_schedule_time_component_revision(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo_unnamed")]
    constructor_parameter_echo_unnamed: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(
            &context.default_environment_id,
            constructor_parameter_echo_unnamed,
        )
        .store()
        .await?;
    let idempotency_key = IdempotencyKey::fresh();
    let request_key = InvocationRequestKey::external(
        executor.auth_ctx().idempotency_scope(),
        Some(&idempotency_key),
    );
    let logical_agent_id = agent_key!("EphemeralEchoAgent", "scheduled-revision");
    let worker_id = ephemeral_agent_id(
        &executor,
        component.environment_id,
        &logical_agent_id,
        &idempotency_key,
        "changeAndGet",
    );

    executor
        .invoke_agent_session(InvocationStart {
            target: Some(id_target(&worker_id)),
            constructor: Some(proto_agent_key(&logical_agent_id)?),
            component_id: Some(component.id.into()),
            method_name: Some("changeAndGet".to_string()),
            input: Some(
                SchemaValue::Record { fields: vec![] }
                    .try_into()
                    .map_err(anyhow::Error::msg)?,
            ),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32,
            schedule_at: Some(prost_types::Timestamp {
                seconds: chrono::Utc::now().timestamp() + 3,
                nanos: 0,
            }),
            idempotency_key: Some(idempotency_key.into()),
            component_owner_account_id: Some(component.account_id.into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            context: None,
            caller: Some(callers::anonymous().into()),
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            config: Vec::new(),
            attempt_id: None,
            expected_callee_fingerprint: None,
            durable_input_mappings: Vec::new(),
            scope_card: None,
            origin_invocation: None,
            external_tool: None,
            request_key: Some(request_key.into()),
        })
        .await?;

    let updated_component = executor
        .update_component(&component.id, &constructor_parameter_echo_unnamed.wasm_name)
        .await?;
    assert_ne!(component.revision, updated_component.revision);

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(metadata.component_revision, component.revision);

    Ok(())
}

/// Verifies that `AgentMode` is persisted in the `Create` oplog entry of a Durable agent and that
/// the worker is accessible afterwards (i.e. its oplog can be queried using the persisted mode).
#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn create_oplog_entry_persists_durable_agent_mode(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .with_agent_config(
            "CounterAgent",
            vec![AgentConfigEntryDto {
                path: vec!["var1".to_string()],
                value: serde_json::Value::String("value1".to_string()).into(),
            }],
        )
        .store()
        .await?;
    let agent_id = agent_key!("CounterAgent", "persistence-test");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    // Drive the agent so that the Create entry has been written
    executor
        .invoke_and_await_agent(&component, &agent_id, "increment", data_value!())
        .await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let create_entry = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Create(params) => Some(params),
            _ => None,
        })
        .expect("Expected a Create entry at the start of the oplog");

    assert_eq!(create_entry.agent_mode, AgentMode::Durable);

    // The worker remains queryable using its persisted mode.
    executor.check_oplog_is_queryable(&worker_id).await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(metadata.agent_id, worker_id.agent_id);
    let identity = executor
        .active_agents()
        .try_get(&worker_id)
        .await
        .expect("durable agent is cached")
        .get_initial_worker_metadata();
    assert_eq!(identity.component_id, component.id);
    assert_eq!(
        identity
            .agent_type_name
            .as_ref()
            .map(|name| name.0.as_str()),
        Some("CounterAgent")
    );
    assert_eq!(identity.canonical_key, Some(agent_id.to_string()));

    let unkeyed_worker_id = create_unkeyed_agent(
        &executor,
        &component,
        &agent_key!("CounterAgent", "unkeyed-persistence"),
    )
    .await?;
    let unkeyed_worker = executor
        .active_agents()
        .try_get(&unkeyed_worker_id)
        .await
        .expect("explicitly created durable agent without a key is cached");
    let unkeyed_metadata = unkeyed_worker.get_initial_worker_metadata();
    assert_eq!(unkeyed_metadata.component_id, component.id);
    assert_eq!(unkeyed_metadata.agent_type_name, identity.agent_type_name);
    assert_eq!(unkeyed_metadata.agent_mode, AgentMode::Durable);
    assert_eq!(unkeyed_metadata.canonical_key, None);
    let unkeyed_oplog = golem_worker_executor::services::HasOplog::oplog(unkeyed_worker.as_ref());
    let golem_common::model::oplog::OplogEntry::Create { parameters, .. } =
        unkeyed_oplog.read(OplogIndex::INITIAL).await?
    else {
        panic!("expected stored Create");
    };
    assert_eq!(parameters.component_id, component.id);
    assert_eq!(parameters.agent_type_name, unkeyed_metadata.agent_type_name);
    assert_eq!(parameters.canonical_key, None);

    Ok(())
}

/// Verifies that `AgentMode` is persisted in the `Create` oplog entry of an Ephemeral agent.
#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn create_oplog_entry_persists_ephemeral_agent_mode(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo_unnamed")]
    constructor_parameter_echo_unnamed: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(
            &context.default_environment_id,
            constructor_parameter_echo_unnamed,
        )
        .store()
        .await?;
    let agent_id = agent_key!("EphemeralEchoAgent", "persistence-test");
    let idempotency_key = IdempotencyKey::fresh();
    let worker_id = ephemeral_agent_id(
        &executor,
        component.environment_id,
        &agent_id,
        &idempotency_key,
        "changeAndGet",
    );

    // Trigger an invocation so the worker has actually been instantiated and Create persisted.
    let identity_executor = executor.clone();
    let component_id = component.id;
    let identity_checked = executor.probe_next_primary_context(&worker_id.agent_id, move |ctx| {
        Box::pin(async move {
            let identity = identity_executor
                .active_agents()
                .try_get(ctx.owned_agent_id())
                .await
                .expect("ephemeral agent is cached during context construction")
                .get_initial_worker_metadata();
            assert_eq!(identity.component_id, component_id);
            assert_eq!(
                identity
                    .agent_type_name
                    .as_ref()
                    .map(|name| name.0.as_str()),
                Some("EphemeralEchoAgent")
            );
            assert_eq!(identity.canonical_key, None);
            Ok(())
        })
    });
    executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key,
            "changeAndGet",
            data_value!(),
        )
        .await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let create_entry = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Create(params) => Some(params),
            _ => None,
        })
        .expect("Expected a Create entry at the start of the oplog");

    assert_eq!(create_entry.agent_mode, AgentMode::Ephemeral);
    identity_checked.await??;
    Ok(())
}

#[test]
#[timeout("60s")]
async fn explicit_structural_ephemeral_creation_has_no_canonical_key(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo_unnamed")]
    constructor_parameter_echo_unnamed: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(
            &context.default_environment_id,
            constructor_parameter_echo_unnamed,
        )
        .store()
        .await?;
    let agent_id = agent_key!("EphemeralEchoAgent", "explicit-creation");
    let worker_id = executor
        .create_unkeyed_agent(&component.id, None, agent_id, HashMap::new())
        .await?;
    let worker = executor
        .active_agents()
        .try_get(&worker_id)
        .await
        .expect("explicitly created ephemeral agent is cached");
    let metadata = worker.get_initial_worker_metadata();
    assert_eq!(metadata.agent_mode, AgentMode::Ephemeral);
    let oplog = golem_worker_executor::services::HasOplog::oplog(worker.as_ref());
    let golem_common::model::oplog::OplogEntry::Create { parameters, .. } =
        oplog.read(OplogIndex::INITIAL).await?
    else {
        panic!("expected stored Create");
    };
    assert_eq!(parameters.agent_mode, AgentMode::Ephemeral);
    assert_eq!(parameters.canonical_key, None);
    assert_eq!(metadata.canonical_key, None);
    Ok(())
}

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn reverted_inactive_ephemeral_agent_stays_inactive(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::agent::{RevertLastInvocations, RevertWorkerTarget};

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let logical_agent_id = agent_key!("EphemeralCounter", "reverted-inactive");

    let result = executor
        .invoke_and_await_agent(&component, &logical_agent_id, "increment", data_value!())
        .await?;
    let final_agent_id = result.agent_id().clone();
    let owned_agent_id = final_agent_id.clone();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while executor.worker_is_cached(&owned_agent_id).await {
            tokio::task::yield_now().await;
        }
    })
    .await?;

    // The revert reconstructs the agent for observation only and stops it externally before it
    // appends the cut. The retained `Restart` keeps the agent fail-stop afterwards.
    executor
        .revert(
            &final_agent_id,
            RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                number_of_invocations: 1,
            }),
        )
        .await?;
    let error = executor
        .resume(&final_agent_id, false)
        .await
        .expect_err("an inactive ephemeral agent must not start after a revert");
    assert!(
        error
            .to_string()
            .contains("An ephemeral agent cannot accept another invocation or be resumed"),
        "{error}"
    );
    assert!(
        executor
            .get_oplog(&final_agent_id, OplogIndex::INITIAL)
            .await?
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Revert(_))),
        "the revert must be recorded"
    );
    Ok(())
}

#[test]
#[timeout("60s")]
#[tracing::instrument]
async fn archived_ephemeral_agent_remains_observable_and_can_be_deleted(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("constructor_parameter_echo_unnamed")]
    constructor_parameter_echo_unnamed: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(
            &context.default_environment_id,
            constructor_parameter_echo_unnamed,
        )
        .store()
        .await?;
    let logical_agent_id = agent_key!("EphemeralEchoAgent", "archived-delete");

    let result = executor
        .invoke_and_await_agent(&component, &logical_agent_id, "changeAndGet", data_value!())
        .await?;
    let final_agent_id = result.agent_id().clone();
    assert_eq!(result.into_typed::<String>()?, "archived-delete!");
    let owned_agent_id = final_agent_id.clone();

    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while executor.worker_is_cached(&owned_agent_id).await {
            tokio::task::yield_now().await;
        }
    })
    .await?;

    let metadata = executor.get_worker_metadata(&final_agent_id).await?;
    assert_eq!(metadata.agent_id, final_agent_id.agent_id);
    assert!(
        !executor
            .get_oplog(&final_agent_id, OplogIndex::INITIAL)
            .await?
            .is_empty()
    );

    executor.delete_worker(&final_agent_id).await?;
    assert_eq!(
        executor.get_worker_metadata_opt(&final_agent_id).await?,
        None
    );
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn ephemeral_agent_fails_terminally_instead_of_restarting_after_live_method_started(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::AgentStatus;
    use golem_worker_executor_test_utils::{TestExecutorOverrides, start_with_overrides};
    use std::sync::Arc;
    use std::time::Duration;

    const EXECUTOR_MEMORY_BYTES: u64 = 32 * 1024 * 1024;
    const GROWTH_MARGIN_BYTES: u64 = 512 * 1024;

    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.memory.system_memory_override = Some(EXECUTOR_MEMORY_BYTES);
                config.memory.worker_memory_ratio = 1.0;
                config.memory.component_size_coefficient = 0.0;
                config.memory.oom_retry_config.min_delay = Duration::from_millis(100);
                config.memory.oom_retry_config.max_delay = Duration::from_millis(100);
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let blocker_agent = agent_key!("Counter", "ephemeral-restart-blocker");
    let blocker_worker = executor
        .start_agent(&component.id, blocker_agent.clone())
        .await?;
    let blocker_owned = blocker_worker.clone();
    executor
        .wait_for_status(&blocker_worker, AgentStatus::Idle, Duration::from_secs(10))
        .await?;
    let blocker_bytes = executor.worker_memory_requirement(&blocker_owned).await?;
    assert!(blocker_bytes > GROWTH_MARGIN_BYTES);
    let growth_bytes = EXECUTOR_MEMORY_BYTES - blocker_bytes - GROWTH_MARGIN_BYTES;

    // A running worker cannot be evicted, so the growth traps with out-of-memory.
    executor
        .invoke_agent(
            &component,
            &blocker_agent,
            "busy_for",
            data_value!(3_000u32),
        )
        .await?;
    executor
        .wait_for_status(
            &blocker_worker,
            AgentStatus::Running,
            Duration::from_secs(10),
        )
        .await?;

    let ephemeral_agent = agent_key!("EphemeralCounter", "ephemeral-restart-growing");
    let idempotency_key = IdempotencyKey::fresh();
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &ephemeral_agent,
            &idempotency_key,
            "allocate_memory",
            data_value!(u32::try_from(growth_bytes)?),
        )
        .await;
    let error = result.expect_err("the accepted invocation must fail terminally");
    assert!(error.to_string().contains("Out of memory"), "{error:#}");

    let worker = ephemeral_agent_id(
        &executor,
        component.environment_id,
        &ephemeral_agent,
        &idempotency_key,
        "allocate_memory",
    );
    let owned = worker.clone();
    tokio::time::timeout(Duration::from_secs(10), async {
        while executor.worker_is_cached(&owned).await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    let method_starts: Vec<_> = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry,
            PublicOplogEntry::AgentInvocationStarted(started)
                if matches!(&started.invocation,
                    PublicAgentInvocation::AgentMethodInvocation(method)
                        if method.method_name == "allocate_memory"))
        })
        .map(|entry| entry.oplog_index)
        .collect();
    assert_eq!(method_starts.len(), 1, "{oplog:#?}");
    let method_start = method_starts[0];
    let finishes: Vec<_> = oplog
        .iter()
        .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .map(|entry| entry.oplog_index)
        .collect();
    assert_eq!(finishes.len(), 1, "{oplog:#?}");
    assert!(finishes[0] < method_start, "only the constructor finished");
    assert!(
        oplog.iter().any(|entry| entry.oplog_index > method_start
            && matches!(entry.entry, PublicOplogEntry::Error(_))),
        "{oplog:#?}"
    );
    assert!(
        !oplog.iter().any(|entry| entry.oplog_index > method_start
            && matches!(entry.entry, PublicOplogEntry::Restart(_))),
        "{oplog:#?}"
    );
    assert_eq!(
        executor.get_worker_metadata(&worker).await?.status,
        AgentStatus::Failed
    );
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn ephemeral_agent_fails_terminally_when_active_memory_limit_is_lowered(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use crate::api::MutableResourceLimitsRegistry;
    use golem_common::model::AgentStatus;
    use golem_common::model::account_usage::{AccountUsagePeriod, MonthlyUsageMode};
    use golem_common::model::oplog::PublicAgentInvocation;
    use golem_service_base::model::MonthlyResourcePolicy;
    use golem_worker_executor::services::golem_config::ResourceUsageMeteringConfig;
    use golem_worker_executor::services::resource_limits::ResourceLimitsGrpc;
    use golem_worker_executor_test_utils::start_with_resource_limits_and_configure;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    const ALLOCATION: u32 = 16 * 1024 * 1024;
    const METHOD: &str = "allocate_memory_and_busy_for";
    let registry = Arc::new(MutableResourceLimitsRegistry::new(MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: u64::MAX,
        available_memory_gb_seconds: u64::MAX,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: u64::MAX,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: u64::MAX,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    }));
    let shutdown = CancellationToken::new();
    let _cancel_on_drop = shutdown.clone().drop_guard();
    let metering = ResourceUsageMeteringConfig::default();
    let limits = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_millis(10),
        Duration::ZERO,
        metering,
        shutdown,
    );
    let context = TestContext::new(last_unique_id);
    let executor = start_with_resource_limits_and_configure(
        deps,
        &context,
        limits,
        Arc::new(move |config| config.resource_usage_metering = metering),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent = agent_key!("EphemeralCounter", "active-memory-limit");
    let key = IdempotencyKey::fresh();
    let worker = ephemeral_agent_id(&executor, component.environment_id, &agent, &key, METHOD);
    let owned = worker.clone();
    let invocation = executor.invoke_and_await_agent_with_key(
        &component,
        &agent,
        &key,
        METHOD,
        data_value!(ALLOCATION, 60_000u32),
    );
    let lower_limit = async {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Ok(oplog) = executor.get_oplog(&worker, OplogIndex::INITIAL).await
                    && let Some(start) = oplog.iter().find(|entry| {
                        matches!(&entry.entry,
                            PublicOplogEntry::AgentInvocationStarted(started)
                                if matches!(&started.invocation,
                                    PublicAgentInvocation::AgentMethodInvocation(method)
                                        if method.method_name == METHOD))
                    })
                    && oplog.iter().any(|entry| {
                        entry.oplog_index > start.oplog_index
                            && matches!(entry.entry, PublicOplogEntry::GrowMemory(_))
                    })
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        // The guest has grown its memory inside the active method, not just its constructor.
        registry.set_max_memory_per_worker(u64::from(ALLOCATION / 2));
        anyhow::Ok(())
    };
    let (result, lowered) = tokio::join!(invocation, lower_limit);
    lowered?;
    let error = result.expect_err("active ephemeral suspension must fail the caller");
    assert!(
        error
            .to_string()
            .to_ascii_lowercase()
            .contains("cannot suspend"),
        "{error:#}"
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while executor.worker_is_cached(&owned).await
            || crate::oplog_sweep::ephemeral_layer_rows(deps, &context, &worker.agent_id).await?
                != 0
        {
            tokio::task::yield_now().await;
        }
        anyhow::Ok(())
    })
    .await??;
    assert_eq!(
        executor.get_worker_metadata(&worker).await?.status,
        AgentStatus::Failed
    );
    let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    let starts: Vec<_> = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry,
            PublicOplogEntry::AgentInvocationStarted(started)
                if matches!(&started.invocation,
                    PublicAgentInvocation::AgentMethodInvocation(method)
                        if method.method_name == METHOD))
        })
        .map(|entry| entry.oplog_index)
        .collect();
    assert_eq!(starts.len(), 1, "{oplog:#?}");
    assert!(
        oplog.iter().any(|entry| entry.oplog_index > starts[0]
            && matches!(entry.entry, PublicOplogEntry::Error(_))),
        "{oplog:#?}"
    );
    assert!(
        !oplog.iter().any(|entry| entry.oplog_index > starts[0]
            && matches!(entry.entry, PublicOplogEntry::Suspend(_))),
        "{oplog:#?}"
    );
    let cached = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent,
            &key,
            METHOD,
            data_value!(ALLOCATION, 60_000u32),
        )
        .await
        .expect_err("same-key invocation must return the recorded failure");
    assert!(
        cached
            .to_string()
            .to_ascii_lowercase()
            .contains("cannot suspend"),
        "{cached:#}"
    );
    assert!(
        executor
            .get_oplog(&worker, oplog.last().unwrap().oplog_index.next())
            .await?
            .is_empty(),
        "same-key invocation must not append to the archived oplog"
    );
    Ok(())
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn fork_publication_retry_preserves_independent_state_across_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::durable_stream::StreamSessionRecord;
    use golem_common::schema::FromSchema;

    let context = TestContext::new(last_unique_id);
    let executor = crate::fork::start_with_local_resume(deps, &context, true).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let source = agent_key!("RpcCounter", "fork-retry");
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &source, "inc_by", data_value!(5u64))
        .await?;
    let prefix = executor.get_oplog(&source_id, OplogIndex::INITIAL).await?;
    let cut = prefix.last().unwrap().oplog_index;
    let target_id = crate::fork::fresh_fork_target(&source_id);
    let source_metadata = executor
        .active_agents()
        .try_get(&source_id)
        .await
        .expect("source is cached")
        .get_initial_worker_metadata();
    assert_eq!(source_metadata.component_id, component.id);
    assert_eq!(
        source_metadata
            .agent_type_name
            .as_ref()
            .map(|name| name.0.as_str()),
        Some("RpcCounter")
    );
    assert_eq!(source_metadata.canonical_key, Some(source.to_string()));
    let error = crate::fork::fork_worker_to(&executor, &source_id, &target_id, cut)
        .await
        .expect_err("injected lost response");
    assert!(
        error.to_string().contains("lost fork resume response"),
        "{error}"
    );
    crate::fork::fork_worker_to(&executor, &source_id, &target_id, cut).await?;
    executor
        .invoke_and_await_agent(&component, &source, "inc_by", data_value!(11u64))
        .await?;
    invoke_and_await_agent_by_id(
        &executor,
        &component,
        &target_id,
        None,
        "inc_by",
        data_value!(2u64),
    )
    .await?;
    let fork_metadata = executor
        .active_agents()
        .try_get(&target_id)
        .await
        .expect("fork is cached")
        .get_initial_worker_metadata();
    assert_eq!(fork_metadata.component_id, component.id);
    assert_eq!(
        fork_metadata.agent_type_name,
        source_metadata.agent_type_name
    );
    assert_eq!(fork_metadata.canonical_key, None);
    // Retrying after the target has run must not overwrite it with the old prefix.
    crate::fork::fork_worker_to(&executor, &source_id, &target_id, cut).await?;
    assert!(
        crate::fork::fork_worker_to(&executor, &source_id, &target_id, cut.previous())
            .await
            .is_err()
    );
    drop(executor);
    let executor = crate::fork::start_with_local_resume(deps, &context, false).await?;
    crate::fork::fork_worker_to(&executor, &source_id, &target_id, cut).await?;
    let actual = invoke_and_await_agent_by_id(
        &executor,
        &component,
        &target_id,
        None,
        "get_value",
        data_value!(),
    )
    .await?
    .into_typed::<u64>()?;
    assert_eq!(actual, 7);
    let actual = executor
        .invoke_and_await_agent(&component, &source, "get_value", data_value!())
        .await?
        .into_typed::<u64>()?;
    assert_eq!(actual, 16);
    for (id, expected) in [(&source_id, &source_metadata), (&target_id, &fork_metadata)] {
        let reloaded = executor
            .active_agents()
            .try_get(id)
            .await
            .expect("restarted agent is cached")
            .get_initial_worker_metadata();
        assert_eq!(reloaded.component_id, expected.component_id);
        assert_eq!(reloaded.agent_type_name, expected.agent_type_name);
        assert_eq!(reloaded.canonical_key, expected.canonical_key);
    }
    let fork_history = executor.get_oplog(&target_id, OplogIndex::INITIAL).await?;
    let markers = fork_history
        .iter()
        .filter_map(|entry| {
            let PublicOplogEntry::StreamSession(record) = &entry.entry else {
                return None;
            };
            let StreamSessionRecord::ForkCut(cut) =
                StreamSessionRecord::from_value(record.record.value()).ok()?
            else {
                return None;
            };
            Some((entry.oplog_index, cut))
        })
        .collect::<Vec<_>>();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].0, cut.next());
    assert_eq!(markers[0].1.cut_index, cut);
    assert_eq!(
        markers[0].1.creation_fingerprint,
        executor.get_worker_metadata(&target_id).await?.fingerprint
    );
    Ok(())
}

#[test]
#[timeout("180s")]
async fn pending_payload_reuse_preserves_counter_effects_across_restart_and_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::{OplogEntry, OplogPayload};
    use golem_worker_executor_test_utils::{TestExecutorOverrides, start_with_overrides};
    use std::sync::Arc;
    use std::time::Duration;

    for max_payload_size in [1, 1_000_000] {
        let context = TestContext::new(last_unique_id);
        let overrides = TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.oplog.max_payload_size = max_payload_size;
            })),
            ..Default::default()
        };
        let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
        let component = executor
            .component_dep(&context.default_environment_id, agent_rpc_rust)
            .store()
            .await?;
        let agent = agent_key!("RpcCounter", format!("payload-reuse-{max_payload_size}"));
        let worker_id = executor
            .start_agent(&component.id, agent.clone())
            .await?
            .agent_id;
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &agent, "get_value", data_value!())
                .await?
                .into_typed::<u64>()?,
            0
        );
        let first_key = IdempotencyKey::fresh();
        let stored_first_key = executor
            .external_invocation_key(&worker_id, &first_key, "inc_by")
            .await?;
        let mut started = executor
            .gate_next_invocation_started_append(&worker_id)
            .await;
        let first = {
            let executor = executor.clone();
            let component = component.clone();
            let agent = agent.clone();
            let key = first_key.clone();
            tokio::spawn(async move {
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &agent,
                        &key,
                        "inc_by",
                        data_value!(7u64),
                    )
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(20), started.entered()).await?;
        let history = executor.stored_oplog(&worker_id).await;
        assert!(history.iter().any(|entry| matches!(entry,
            OplogEntry::PendingAgentInvocation { invocation_id: idempotency_key, .. } if *idempotency_key == stored_first_key)));
        assert!(!history.iter().any(|entry| matches!(entry,
            OplogEntry::AgentInvocationStarted { invocation_id: idempotency_key, .. } if *idempotency_key == stored_first_key)));
        started.abort();
        first.abort();
        let _ = first.await;
        drop(executor);

        let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
        executor
            .invoke_and_await_agent_with_key(
                &component,
                &agent,
                &first_key,
                "inc_by",
                data_value!(7u64),
            )
            .await?;
        assert!(executor.instance_load_count(&worker_id) > 0);

        let second_key = IdempotencyKey::fresh();
        let stored_second_key = executor
            .external_invocation_key(&worker_id, &second_key, "inc_by")
            .await?;
        let before_completion_restart = executor.instance_load_count(&worker_id);
        let mut completion = executor.gate_next_agent_invocation_success(&worker_id);
        let second = {
            let executor = executor.clone();
            let component = component.clone();
            let agent = agent.clone();
            let key = second_key.clone();
            tokio::spawn(async move {
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &agent,
                        &key,
                        "inc_by",
                        data_value!(11u64),
                    )
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(20), completion.entered()).await?;
        let history = executor.stored_oplog(&worker_id).await;
        assert!(history.iter().any(|entry| matches!(entry,
            OplogEntry::AgentInvocationStarted { invocation_id: idempotency_key, .. } if *idempotency_key == stored_second_key)));
        assert!(!history.iter().rev().take_while(|entry| !matches!(entry,
            OplogEntry::AgentInvocationStarted { invocation_id: idempotency_key, .. } if *idempotency_key == stored_second_key))
            .any(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. })));
        completion.abort_as_restart();
        tokio::time::timeout(Duration::from_secs(60), second).await???;
        assert!(executor.instance_load_count(&worker_id) > before_completion_restart);
        let output_key = IdempotencyKey::fresh();
        let stored_output_key = executor
            .external_invocation_key(&worker_id, &output_key, "inc_and_return_text")
            .await?;
        let expected_output = "x".repeat(10_000);
        assert_eq!(
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent,
                    &output_key,
                    "inc_and_return_text",
                    data_value!(10_000u32),
                )
                .await?
                .into_typed::<String>()?,
            expected_output
        );
        drop(executor);

        let executor = start_with_overrides(deps, &context, overrides).await?;
        for (key, amount) in [(&first_key, 7u64), (&second_key, 11u64)] {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent,
                    key,
                    "inc_by",
                    data_value!(amount),
                )
                .await?;
        }
        assert_eq!(
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent,
                    &output_key,
                    "inc_and_return_text",
                    data_value!(10_000u32),
                )
                .await?
                .into_typed::<String>()?,
            expected_output
        );
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &agent, "get_value", data_value!())
                .await?
                .into_typed::<u64>()?,
            19
        );
        let history = executor.stored_oplog(&worker_id).await;
        let mut active_key = None;
        let mut completions = HashMap::new();
        for entry in &history {
            match entry {
                OplogEntry::AgentInvocationStarted {
                    invocation_id: idempotency_key,
                    ..
                } => {
                    active_key = Some(idempotency_key);
                }
                OplogEntry::AgentInvocationFinished { .. } => {
                    *completions
                        .entry(active_key.take().expect("Started before Finished"))
                        .or_insert(0usize) += 1;
                }
                _ => {}
            }
        }
        for key in [&stored_first_key, &stored_second_key, &stored_output_key] {
            let pending = history
                .iter()
                .filter_map(|entry| match entry {
                    OplogEntry::PendingAgentInvocation {
                        invocation_id: idempotency_key,
                        payload,
                        ..
                    } if idempotency_key == key => Some(payload),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(pending.len(), 1);
            assert_eq!(
                matches!(pending[0], OplogPayload::External { .. }),
                max_payload_size == 1
            );
            let starts = history
                .iter()
                .filter_map(|entry| match entry {
                    OplogEntry::AgentInvocationStarted {
                        invocation_id: idempotency_key,
                        payload,
                        ..
                    } if idempotency_key == key => Some(payload),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert!(!starts.is_empty());
            for payload in starts {
                assert_eq!(payload, pending[0]);
            }
            assert_eq!(completions.get(key), Some(&1));
        }
    }
    Ok(())
}

#[test]
#[timeout("120s")]
async fn pending_payload_reuse_keeps_manual_snapshot_update_distinct_from_method_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_update_v1")] agent_update_v1: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use std::time::Duration;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_update_v1)
        .store()
        .await?;
    let agent = agent_key!("SnapshotUpdateTest");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
    let key = IdempotencyKey::fresh();
    assert_eq!(
        executor
            .invoke_and_await_agent_with_key(
                &component,
                &agent,
                &key,
                "stable_value",
                data_value!()
            )
            .await?
            .into_typed::<u32>()?,
        7
    );
    executor
        .invoke_and_await_agent(&component, &agent, "pre_snapshot_value", data_value!())
        .await?;
    let target = executor
        .update_component(&component.id, "it_agent_update_v2_release")
        .await?;
    executor
        .manual_update_worker(&worker_id, target.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&worker_id, target.revision, Duration::from_secs(30))
        .await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &component,
                &agent,
                "loaded_snapshot_revision",
                data_value!()
            )
            .await?
            .into_typed::<u32>()?,
        1
    );
    drop(executor);

    let executor = start(deps, &context).await?;
    executor
        .invoke_and_await_agent_with_key(&component, &agent, &key, "stable_value", data_value!())
        .await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "accumulated_value", data_value!())
            .await?
            .into_typed::<u32>()?,
        11
    );
    executor
        .invoke_and_await_agent(&component, &agent, "stable_value", data_value!())
        .await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "accumulated_value", data_value!())
            .await?
            .into_typed::<u32>()?,
        21
    );
    Ok(())
}

fn counter_invocation(
    component: &golem_common::model::component::ComponentDto,
    executor: &golem_worker_executor_test_utils::TestWorkerExecutor,
    worker_id: &OwnedAgentId,
    mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode,
    schedule_at: Option<prost_types::Timestamp>,
    expected_fingerprint: Option<golem_common::model::AgentFingerprint>,
) -> anyhow::Result<InvocationStart> {
    let lookup = mode == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup;
    Ok(InvocationStart {
        target: Some(id_target(worker_id)),
        constructor: None,
        component_id: Some(component.id.into()),
        method_name: (!lookup).then(|| "increment".to_string()),
        input: if lookup {
            None
        } else {
            Some(
                SchemaValue::Record { fields: vec![] }
                    .try_into()
                    .map_err(anyhow::Error::msg)?,
            )
        },
        mode: mode as i32,
        schedule_at,
        idempotency_key: Some(IdempotencyKey::fresh().into()),
        component_owner_account_id: Some(component.account_id.into()),
        environment_id: Some(component.environment_id.into()),
        auth_ctx: Some(executor.auth_ctx().into()),
        context: None,
        caller: Some(callers::anonymous().into()),
        freshness_disposition:
            golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist as i32,
        config: Vec::new(),
        attempt_id: None,
        expected_callee_fingerprint: expected_fingerprint.map(|fingerprint| fingerprint.0.into()),
        durable_input_mappings: Vec::new(),
        scope_card: None,
        origin_invocation: None,
        external_tool: None,
        request_key: (!lookup).then(|| InvocationRequestKey::external(None, None).into()),
    })
}

/// Every invocation mode with `expected_fingerprint`, which must each fail as not found.
async fn assert_every_mode_refuses_fingerprint(
    component: &golem_common::model::component::ComponentDto,
    executor: &golem_worker_executor_test_utils::TestWorkerExecutor,
    worker_id: &OwnedAgentId,
    expected_fingerprint: golem_common::model::AgentFingerprint,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::worker::AgentInvocationMode;
    let later = prost_types::Timestamp {
        seconds: chrono::Utc::now().timestamp() + 3600,
        nanos: 0,
    };
    for (mode, schedule_at) in [
        (AgentInvocationMode::Await, None),
        (AgentInvocationMode::Schedule, None),
        (AgentInvocationMode::Schedule, Some(later)),
        (AgentInvocationMode::Lookup, None),
    ] {
        let error = executor
            .invoke_agent_session(counter_invocation(
                component,
                executor,
                worker_id,
                mode,
                schedule_at,
                Some(expected_fingerprint),
            )?)
            .await
            .expect_err("a stale expected fingerprint must be refused");
        assert!(
            error.to_string().contains("Worker not found"),
            "{mode:?} with schedule_at {schedule_at:?}: unexpected error: {error:#}"
        );
    }
    Ok(())
}

#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn expected_fingerprint_admits_only_the_named_incarnation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::worker::AgentInvocationMode;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let agent = agent_key!("Counter", "expected-fingerprint");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
    let first = executor.get_worker_metadata(&worker_id).await?.fingerprint;

    for expected in [Some(first), None] {
        executor
            .invoke_agent_session(counter_invocation(
                &component,
                &executor,
                &worker_id,
                AgentInvocationMode::Await,
                None,
                expected,
            )?)
            .await?;
    }
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        3,
        "the call with the matching fingerprint and the call without one both ran"
    );

    executor.delete_worker(&worker_id).await?;
    assert_every_mode_refuses_fingerprint(&component, &executor, &worker_id, first).await?;
    assert_eq!(
        executor.get_worker_metadata_opt(&worker_id).await?,
        None,
        "a refused call must not create the agent"
    );

    executor.start_agent(&component.id, agent.clone()).await?;
    let second = executor.get_worker_metadata(&worker_id).await?.fingerprint;
    assert_ne!(first, second);
    // The new incarnation is resident, so the guard compares a worker that is already resolved.
    assert_every_mode_refuses_fingerprint(&component, &executor, &worker_id, first).await?;

    drop(executor);
    let executor = start(deps, &context).await?;
    // After a restart the agent is cold, so the guard compares the metadata it loads.
    assert_every_mode_refuses_fingerprint(&component, &executor, &worker_id, first).await?;

    let metadata = executor.get_worker_metadata(&worker_id).await?;
    assert_eq!(metadata.fingerprint, second);
    assert_eq!(
        metadata.pending_invocation_count, 0,
        "a refused call must not enqueue or schedule an invocation"
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        1,
        "no refused call ran on the new incarnation"
    );
    Ok(())
}

/// The proto form of the agent key of `key`.
pub(crate) fn proto_agent_key(
    key: &ParsedAgentKey,
) -> anyhow::Result<golem_api_grpc::proto::golem::worker::AgentKey> {
    AgentKey::new(key.agent_type.clone(), key.parameters.clone())?
        .try_into()
        .map_err(anyhow::Error::msg)
}

/// An invocation target that reaches the agent with this ID.
pub(crate) fn id_target(
    agent_id: &OwnedAgentId,
) -> golem_api_grpc::proto::golem::worker::AgentTarget {
    golem_api_grpc::proto::golem::worker::AgentTarget {
        target: Some(
            golem_api_grpc::proto::golem::worker::agent_target::Target::Id(
                agent_id.agent_id.into(),
            ),
        ),
    }
}

/// An invocation target that upserts the keyed agent of `key`.
pub(crate) fn key_target(
    key: &ParsedAgentKey,
) -> anyhow::Result<golem_api_grpc::proto::golem::worker::AgentTarget> {
    Ok(golem_api_grpc::proto::golem::worker::AgentTarget {
        target: Some(
            golem_api_grpc::proto::golem::worker::agent_target::Target::Key(proto_agent_key(key)?),
        ),
    })
}

/// The ID of the keyed agent of `key` in `environment_id`.
pub(crate) fn keyed_agent_id(environment_id: EnvironmentId, key: &ParsedAgentKey) -> OwnedAgentId {
    let agent_key = AgentKey::new(key.agent_type.clone(), key.parameters.clone())
        .expect("a test agent key has canonical parameters");
    OwnedAgentId::new(
        environment_id,
        &AgentId::keyed(
            environment_id,
            local_type_id(environment_id, &key.agent_type),
            &agent_key,
        ),
    )
}

/// The ID of the instance that runs the test DSL's ephemeral invocation of `method` on `key`
/// under `idempotency_key`.
pub(crate) fn ephemeral_agent_id(
    executor: &TestWorkerExecutor,
    environment_id: EnvironmentId,
    key: &ParsedAgentKey,
    idempotency_key: &IdempotencyKey,
    method: &str,
) -> OwnedAgentId {
    let owner_key = InvocationRequestKey::external(
        executor.auth_ctx().idempotency_scope(),
        Some(idempotency_key),
    )
    .owner_key(method);
    OwnedAgentId::new(
        environment_id,
        &AgentId::ephemeral_invocation(
            environment_id,
            local_type_id(environment_id, &key.agent_type),
            &owner_key,
        ),
    )
}

/// Creates an agent of the type and constructor parameters of `key` without a key, at a fresh
/// ID, and returns its ID.
pub(crate) async fn create_unkeyed_agent(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    key: &ParsedAgentKey,
) -> anyhow::Result<OwnedAgentId> {
    let agent_id = OwnedAgentId::new(
        component.environment_id,
        &AgentId::random(local_type_id(component.environment_id, &key.agent_type)),
    );
    create_unkeyed_agent_at(executor, component, key, &agent_id).await?;
    Ok(agent_id)
}

/// Creates an agent of the type and constructor parameters of `key` without a key at
/// `agent_id`, such as the instance of an ephemeral invocation before the invocation reaches
/// it. An existing agent at `agent_id` is kept.
pub(crate) async fn create_unkeyed_agent_at(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    key: &ParsedAgentKey,
    agent_id: &OwnedAgentId,
) -> anyhow::Result<()> {
    let response = executor
        .client
        .clone()
        .create_worker(CreateWorkerRequest {
            constructor: Some(proto_agent_key(key)?),
            unkeyed_id: Some(agent_id.agent_id.into()),
            component_id: Some(component.id.into()),
            component_owner_account_id: Some(component.account_id.into()),
            environment_id: Some(component.environment_id.into()),
            ignore_already_existing: true,
            auth_ctx: Some(executor.auth_ctx().into()),
            caller: Some(callers::anonymous().into()),
            ..Default::default()
        })
        .await?
        .into_inner();
    match response.result {
        Some(create_worker_response::Result::Success(_)) => Ok(()),
        other => Err(anyhow::anyhow!(
            "Creating {key} without a key failed: {other:?}"
        )),
    }
}

/// Invokes `method` on the agent with ID `agent_id` and awaits its result.
pub(crate) async fn invoke_and_await_agent_by_id(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    agent_id: &OwnedAgentId,
    idempotency_key: Option<&IdempotencyKey>,
    method: &str,
    params: TypedSchemaValue,
) -> anyhow::Result<AgentResult> {
    let (_, input) = params.into_parts();
    let result = executor
        .invoke_agent_session(InvocationStart {
            target: Some(id_target(agent_id)),
            constructor: None,
            component_id: Some(component.id.into()),
            method_name: Some(method.to_string()),
            input: Some(input.try_into().map_err(anyhow::Error::msg)?),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            schedule_at: None,
            idempotency_key: idempotency_key.cloned().map(Into::into),
            component_owner_account_id: Some(component.account_id.into()),
            environment_id: Some(agent_id.environment_id.into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            context: None,
            caller: Some(callers::anonymous().into()),
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            config: Vec::new(),
            attempt_id: None,
            expected_callee_fingerprint: None,
            durable_input_mappings: Vec::new(),
            scope_card: None,
            origin_invocation: None,
            external_tool: None,
            request_key: Some(
                InvocationRequestKey::external(
                    executor.auth_ctx().idempotency_scope(),
                    idempotency_key,
                )
                .into(),
            ),
        })
        .await?;
    let value = match result.result {
        Some(
            golem_api_grpc::proto::golem::worker::invocation_session_result::Result::MethodResult(
                value,
            ),
        ) => Some(SchemaValue::try_from(value).map_err(anyhow::Error::msg)?),
        _ => None,
    };
    Ok(AgentResult::new(value, agent_id.clone()))
}

/// The target and constructor with which the test DSL invokes `method` on `key` under
/// `idempotency_key`: an ephemeral invocation reaches its own instance by ID and creates it from
/// `key`, and a durable one upserts the keyed agent.
pub(crate) fn invocation_target(
    executor: &TestWorkerExecutor,
    component: &ComponentDto,
    key: &ParsedAgentKey,
    idempotency_key: &IdempotencyKey,
    method: &str,
) -> anyhow::Result<(
    golem_api_grpc::proto::golem::worker::AgentTarget,
    Option<golem_api_grpc::proto::golem::worker::AgentKey>,
)> {
    let ephemeral = component.metadata.agent_types.iter().any(|agent| {
        agent.declaration.type_name == key.agent_type
            && agent.declaration.mode == AgentMode::Ephemeral
    });
    if ephemeral {
        let agent_id = ephemeral_agent_id(
            executor,
            component.environment_id,
            key,
            idempotency_key,
            method,
        );
        Ok((id_target(&agent_id), Some(proto_agent_key(key)?)))
    } else {
        Ok((key_target(key)?, None))
    }
}

/// The agent ID that an invocation start with an ID target reaches.
pub(crate) fn start_target_id(
    start: &InvocationStart,
) -> Option<golem_api_grpc::proto::golem::worker::AgentId> {
    match start.target.as_ref()?.target.as_ref()? {
        golem_api_grpc::proto::golem::worker::agent_target::Target::Id(id) => Some(*id),
        golem_api_grpc::proto::golem::worker::agent_target::Target::Key(_) => None,
    }
}

/// When the ID that a key derives already holds an agent of another key, the key collides: an
/// invocation by the key and a guest's `upsert` of it fail, and neither reaches that agent.
#[test]
#[timeout("2m")]
#[tracing::instrument]
async fn a_key_whose_id_holds_an_agent_of_another_key_collides(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let owner = executor
        .start_agent(&component.id, agent_key!("GolemHostApi", "collision-owner"))
        .await?;
    let caller = agent_key!("GolemHostApi", "collision-caller");
    executor.start_agent(&component.id, caller.clone()).await?;
    let requested = agent_key!("GolemHostApi", "collision-requested");
    let collided = keyed_agent_id(context.default_environment_id, &requested);
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    copy_create_entry_to(deps, &context, &owner, &collided).await?;

    let executor = start(deps, &context).await?;
    let error = executor
        .invoke_and_await_agent(&component, &requested, "get_self_uri", data_value!())
        .await
        .expect_err("an invocation by a colliding key must fail");
    let error = format!("{error:#}");
    assert!(
        error.contains(&format!(
            "Agent {} has another key than the requested",
            collided.agent_id
        )),
        "{error}"
    );
    let upserted = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "outbound_agent_rpc_invoke_and_await_result",
            data_value!("GolemHostApi", "collision-requested", "get_self_uri"),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    let error = upserted.expect_err("a guest upsert of a colliding key must fail");
    assert!(
        error.contains("has another key than the requested"),
        "{error}"
    );
    let oplog = executor.get_oplog(&collided, OplogIndex::INITIAL).await?;
    let method_invocations = oplog
        .iter()
        .filter(|entry| match &entry.entry {
            PublicOplogEntry::PendingAgentInvocation(params) => matches!(
                params.invocation,
                PublicAgentInvocation::AgentMethodInvocation(_)
            ),
            PublicOplogEntry::AgentInvocationStarted(params) => matches!(
                params.invocation,
                PublicAgentInvocation::AgentMethodInvocation(_)
            ),
            _ => false,
        })
        .count();
    assert_eq!(
        method_invocations, 0,
        "no colliding request reaches the agent at the colliding ID: {oplog:#?}"
    );
    executor.shutdown_and_wait_for_invocation_loops().await?;
    Ok(())
}

#[test]
#[timeout("2m")]
async fn a_key_collision_hides_the_other_agents_constructor_failure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::AgentStatus;
    use golem_worker_executor::services::UsesAllDeps;
    use std::time::Duration;

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let owner = agent_key!("RustMethodFailure", "fail-constructor");
    let owner_id = keyed_agent_id(component.environment_id, &owner);
    let error = executor
        .start_agent(&component.id, owner.clone())
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("constructor gave up"));
    executor
        .wait_for_status(&owner_id, AgentStatus::Failed, Duration::from_secs(10))
        .await?;
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let requested = agent_key!("RustMethodFailure", "collision-requested");
    let collided = keyed_agent_id(component.environment_id, &requested);
    golem_worker_executor_test_utils::copy_agent_oplog_to(deps, &context, &owner_id, &collided)
        .await?;

    let executor = start(deps, &context).await?;
    let before = executor.stored_oplog(&collided.agent_id).await;
    for resident in [false, true] {
        if resident {
            let healthy = executor
                .start_agent(&component.id, agent_key!("RustMethodFailure", "healthy"))
                .await?;
            let worker = executor.active_agent(&healthy).await.unwrap().primary();
            golem_worker_executor::worker::Worker::get_exact_existing_suspended(
                worker.all(),
                &collided,
                callers::anonymous(),
            )
            .await?;
        }
        let error = executor
            .invoke_and_await_agent(&component, &requested, "increment", data_value!())
            .await
            .unwrap_err();
        let error = format!("{error:#}");
        assert!(
            error.contains("has another key than the requested"),
            "resident={resident}: {error}"
        );
        assert!(!error.contains("constructor gave up"), "{error}");
        assert_eq!(executor.stored_oplog(&collided.agent_id).await, before);
    }
    let error = executor
        .invoke_and_await_agent(&component, &owner, "increment", data_value!())
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("constructor gave up"));
    executor.shutdown_and_wait_for_invocation_loops().await?;
    Ok(())
}
