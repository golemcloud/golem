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
use anyhow::anyhow;
use futures::{SinkExt, StreamExt};
use golem_common::model::oplog::{
    OplogIndex, PublicAgentInvocation, PublicOplogEntry, PublicOplogEntryWithIndex,
};
use golem_common::model::{AgentId, AgentStatus, IdempotencyKey, OwnedAgentId, PromiseId};
use golem_common::schema::SchemaValue;
use golem_common::schema::schema_value::ResultValuePayload;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, ReplayAdmissionStage, TestContext, TestExecutorOverrides,
    TestWorkerExecutor, WorkerExecutorTestDependencies, start, start_with_overrides,
};
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::spawn;
use tokio::sync::watch;
use tokio_tungstenite::WebSocketStream;
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_sdk_ts")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_effect_caller")]
    PrecompiledComponent
);

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerEvent {
    connection: usize,
    payload: String,
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_report_loss_guest_discards_completion_and_replay_stays_terminal(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::payload::HostResponseWebsocketReceiveResponse;
    use golem_common::model::oplog::payload::types::SerializableWebsocketError;
    use golem_common::schema::FromSchema;

    let context = TestContext::new(last_unique_id);
    let mut executor = start(deps, &context).await?;
    let mut peer = ReconnectTestServer::start().await;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("WebsocketTest", "guest-discarded-loss");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_report_loss",
            data_value!(peer.url()),
        )
        .await?;
    let cancel_value = executor
        .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected promise return value"))?;
    let cancel = PromiseId {
        agent_id: worker_id.clone(),
        oplog_idx: extract_oplog_idx_from_promise_id(&cancel_value),
    };
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    executor = start(deps, &context).await?;
    executor
        .invoke_and_await_agent(&component, &agent_id, "noop", data_value!())
        .await?;
    let owner = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let mut end = executor.gate_next_call_end(&worker_id, true).await;
    let mut cancelled = executor.gate_next_wall_clock_now(&owner).await?;
    let key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &key,
            "discard_report_loss",
            crate::raw_params(vec![cancel_value.clone()]),
        )
        .await?;
    tokio::time::timeout(Duration::from_secs(10), end.entered()).await?;
    let discarded_start = end.start_index();
    let prefix = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        prefix
            .iter()
            .any(|entry| entry.oplog_index == discarded_start
                && matches!(&entry.entry, PublicOplogEntry::Start(params)
            if params.function_name == "golem:websocket/client::receive")),
        "{}",
        describe_oplog(&prefix)
    );
    // The guest retains the unpolled receive until its loss End is durable, then drops it.
    executor.complete_promise(&cancel, vec![1]).await?;
    tokio::time::timeout(Duration::from_secs(10), cancelled.entered()).await?;
    drop(end);
    cancelled.release();
    assert!(
        executor
            .invoke_and_await_agent_with_key(
                &component,
                &agent_id,
                &key,
                "discard_report_loss",
                crate::raw_params(vec![cancel_value]),
            )
            .await?
            .into_typed::<bool>()?
    );
    let mut original_discard = None;
    for replay in [false, true] {
        if replay {
            executor.shutdown_and_wait_for_invocation_loops().await?;
            drop(executor);
            executor = start(deps, &context).await?;
        }
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &agent_id, "probe_report_loss", data_value!())
                .await?
                .into_typed::<(bool, bool, bool, bool)>()?,
            (true, true, true, true)
        );
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let ends: Vec<_> = oplog
            .iter()
            .filter(|entry| {
                matches!(&entry.entry,
            PublicOplogEntry::End(params) if params.start_index == discarded_start)
            })
            .collect();
        let discards: Vec<_> = oplog
            .iter()
            .filter(|entry| {
                matches!(&entry.entry,
            PublicOplogEntry::CompletionDiscarded(params) if params.start_index == discarded_start)
            })
            .collect();
        assert_eq!(ends.len(), 1, "{}", describe_oplog(&oplog));
        assert_eq!(discards.len(), 1, "{}", describe_oplog(&oplog));
        assert!(ends[0].oplog_index < discards[0].oplog_index);
        let PublicOplogEntry::End(params) = &ends[0].entry else {
            unreachable!()
        };
        let response = params.response.as_ref().expect("receive response missing");
        let response = HostResponseWebsocketReceiveResponse::from_value(response.value())?;
        assert_eq!(
            response.result,
            Err(SerializableWebsocketError::SessionLost)
        );
        assert!(!oplog.iter().any(|entry| matches!(&entry.entry,
            PublicOplogEntry::CompletionDelivered(params) if params.start_index == discarded_start)
            || matches!(&entry.entry, PublicOplogEntry::Cancelled(params) if params.start_index == discarded_start)),
            "{}", describe_oplog(&oplog));
        if let Some(original) = &original_discard {
            assert_eq!(*discards[0], *original);
        } else {
            original_discard = Some(discards[0].clone());
        }
        assert_eq!(peer.accepted(), 1);
        assert_eq!(peer.completed_handshakes(), 1);
        assert_eq!(
            peer.frames_received(),
            vec!["completed-before-crash".to_string()]
        );
    }
    drop(executor);
    peer.stop().await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn websocket_report_loss_replacement_connect_and_initialization_crashes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    for (initialization, after_end) in [(false, false), (false, true), (true, false), (true, true)]
    {
        let context = TestContext::new(last_unique_id);
        let mut executor = start(deps, &context).await?;
        let mut original = ReconnectTestServer::start().await;
        let mut replacement = ReconnectTestServer::start().await;
        let component = executor
            .component_dep(&context.default_environment_id, host_api_tests)
            .store()
            .await?;
        let agent_id = agent_id!(
            "WebsocketTest",
            format!("replacement-{initialization}-{after_end}")
        );
        let worker_id = executor
            .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
            .await?;
        executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "connect_report_loss",
                data_value!(original.url()),
            )
            .await?;
        executor.shutdown_and_wait_for_invocation_loops().await?;
        drop(executor);
        executor = start(deps, &context).await?;
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &agent_id, "probe_report_loss", data_value!())
                .await?
                .into_typed::<(bool, bool, bool, bool)>()?,
            (true, true, true, true)
        );
        let key = IdempotencyKey::fresh();
        let mut gate = executor
            .gate_next_call_end(&worker_id, initialization || after_end)
            .await;
        executor
            .invoke_agent_with_key(
                &component,
                &agent_id,
                &key,
                "replace_report_loss",
                data_value!(replacement.url()),
            )
            .await?;
        tokio::time::timeout(Duration::from_secs(10), gate.entered()).await?;
        if initialization {
            let next = executor.gate_next_call_end(&worker_id, after_end).await;
            drop(gate);
            gate = next;
            tokio::time::timeout(Duration::from_secs(10), gate.entered()).await?;
        }
        let target = gate.start_index();
        gate.abort();
        executor.shutdown_and_wait_for_invocation_loops().await?;
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let function = if initialization {
            "::receive"
        } else {
            "::connect"
        };
        assert!(oplog.iter().any(|entry| entry.oplog_index == target && matches!(&entry.entry, PublicOplogEntry::Start(params) if params.function_name.ends_with(function))), "{}", describe_oplog(&oplog));
        assert_call_terminal_counts(&oplog, target, usize::from(after_end), 0);
        drop(gate);
        drop(executor);

        // Crash a fresh connect a second time while its original Start is still incomplete.
        if !initialization && !after_end {
            executor = start(deps, &context).await?;
            let mut gate = executor.gate_next_call_end(&worker_id, false).await;
            executor
                .invoke_agent_with_key(
                    &component,
                    &agent_id,
                    &key,
                    "replace_report_loss",
                    data_value!(replacement.url()),
                )
                .await?;
            tokio::time::timeout(Duration::from_secs(10), gate.entered()).await?;
            assert_eq!(gate.start_index(), target);
            gate.abort();
            executor.shutdown_and_wait_for_invocation_loops().await?;
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            assert!(!oplog.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::End(params) if params.start_index == target)), "{}", describe_oplog(&oplog));
            drop(gate);
            drop(executor);
        }
        executor = start(deps, &context).await?;
        let result = executor
            .invoke_and_await_agent_with_key(
                &component,
                &agent_id,
                &key,
                "replace_report_loss",
                data_value!(replacement.url()),
            )
            .await?
            .into_typed::<Result<String, String>>()?;
        let expected = match (initialization, after_end) {
            (false, false) => Ok(RECONNECT_MESSAGES[0].to_string()),
            (true, true) => Ok(RECONNECT_FIRST_MESSAGE.to_string()),
            _ => Err("Initialize error: Error::SessionLost".to_string()),
        };
        assert_eq!(result, expected);
        assert_eq!(original.accepted(), 1);
        assert_eq!(
            replacement.completed_handshakes(),
            if !initialization && !after_end { 3 } else { 1 }
        );
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        assert_eq!(oplog.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::Start(_) if entry.oplog_index == target)).count(), 1);
        assert_call_terminal_counts(&oplog, target, 1, 1);

        // A distinct explicit replacement can initialize normally after either loss outcome.
        let mut fresh = ReconnectTestServer::start().await;
        assert_eq!(
            executor
                .invoke_and_await_agent(
                    &component,
                    &agent_id,
                    "replace_report_loss",
                    data_value!(fresh.url())
                )
                .await?
                .into_typed::<Result<String, String>>()?,
            Ok(RECONNECT_FIRST_MESSAGE.to_string())
        );
        executor.shutdown_and_wait_for_invocation_loops().await?;
        drop(executor);
        executor = start(deps, &context).await?;
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &agent_id, "probe_report_loss", data_value!())
                .await?
                .into_typed::<(bool, bool, bool, bool)>()?,
            (true, true, true, true)
        );
        assert_eq!(fresh.completed_handshakes(), 1);
        assert_eq!(original.accepted(), 1);
        drop(executor);
        original.stop().await?;
        replacement.stop().await?;
        fresh.stop().await?;
    }
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn websocket_report_loss_exact_send_and_receive_crash_prefixes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| config.max_websocket_connections = 1)),
        ..Default::default()
    };
    let start_executor = || start_with_overrides(deps, &context, overrides.clone());
    let mut executor = start_executor().await?;
    let mut peer = ReconnectTestServer::start().await;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("WebsocketTest", "exact-loss-prefixes");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "connect_report_loss",
                data_value!(peer.url())
            )
            .await?
            .into_typed::<String>()?,
        RECONNECT_FIRST_MESSAGE
    );

    // The send effect can have reached the peer. Only its absent End is authoritative.
    let send_key = IdempotencyKey::fresh();
    let mut gate = executor.gate_next_call_end(&worker_id, false).await;
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &send_key,
            "send_persisted_result",
            data_value!("ambiguous-send".to_string()),
        )
        .await?;
    tokio::time::timeout(Duration::from_secs(10), gate.entered()).await?;
    gate.abort();
    executor.shutdown_and_wait_for_invocation_loops().await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let send = oplog
        .iter()
        .rev()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == WEBSOCKET_SEND_FUNCTION => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("pending send Start");
    assert!(!oplog.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::End(params) if params.start_index == send)), "{}", describe_oplog(&oplog));
    let completed_siblings = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if entry.oplog_index < send
                    && (params.function_name == WEBSOCKET_SEND_FUNCTION
                        || params.function_name.ends_with("::receive")) =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(completed_siblings.len(), 2, "{}", describe_oplog(&oplog));
    for sibling in &completed_siblings {
        assert_call_terminal_counts(&oplog, *sibling, 1, 1);
    }
    peer.wait_for_count(
        1,
        |event| matches!(event, ReconnectServerEvent::FrameReceived(text) if text == "completed-before-crash"),
        "observe the historical completed send",
    ).await?;
    drop(gate);
    drop(executor);

    // Repeatedly reconstruct the same unfinished send, first before its loss End,
    // then with that End durable but before guest completion delivery.
    for after_end in [false, true] {
        executor = start_executor().await?;
        let mut admission = executor.gate_next_replay_access_admission(
            &worker_id,
            "client::connect",
            ReplayAdmissionStage::BeforeScope,
        );
        let mut gate = executor.gate_next_call_end(&worker_id, after_end).await;
        executor
            .invoke_agent_with_key(
                &component,
                &agent_id,
                &send_key,
                "send_persisted_result",
                data_value!("ambiguous-send".to_string()),
            )
            .await?;
        tokio::time::timeout(Duration::from_secs(10), admission.entered()).await?;
        let pool = executor.websocket_connection_pool().expect("captured pool");
        let permit = tokio::time::timeout(Duration::from_secs(5), pool.acquire()).await??;
        admission.release();
        tokio::time::timeout(Duration::from_secs(10), gate.entered()).await?;
        gate.abort();
        executor.shutdown_and_wait_for_invocation_loops().await?;
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        assert_call_terminal_counts(&oplog, send, usize::from(after_end), 0);
        assert_eq!(peer.completed_handshakes(), 1);
        drop(gate);
        drop(permit);
        drop(executor);
    }
    executor = start_executor().await?;
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &send_key,
            "send_persisted_result",
            data_value!("ambiguous-send".to_string()),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert_eq!(result, Err("Send error: Error::SessionLost".into()));
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);

    // Async receives on the lost handle must retain the End/delivery split too.
    for after_end in [false, true] {
        executor = start_executor().await?;
        executor
            .invoke_and_await_agent(&component, &agent_id, "noop", data_value!())
            .await?;
        let pool = executor.websocket_connection_pool().expect("captured pool");
        let permit = tokio::time::timeout(Duration::from_secs(5), pool.acquire()).await??;
        let key = IdempotencyKey::fresh();
        let mut gate = executor.gate_next_call_end(&worker_id, after_end).await;
        executor
            .invoke_agent_with_key(
                &component,
                &agent_id,
                &key,
                CONTENTION_METHOD,
                data_value!(CONTENTION_TIMEOUT_MS),
            )
            .await?;
        tokio::time::timeout(Duration::from_secs(10), gate.entered()).await?;
        gate.abort();
        executor.shutdown_and_wait_for_invocation_loops().await?;
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let start = gate.start_index();
        assert_call_terminal_counts(&oplog, start, usize::from(after_end), 0);
        assert!(oplog.iter().any(|entry| entry.oplog_index == start && matches!(&entry.entry, PublicOplogEntry::Start(params) if params.function_name.ends_with("::receive") || params.function_name.ends_with("::receive-with-timeout"))), "{}", describe_oplog(&oplog));
        drop(gate);
        drop(permit);
        drop(executor);
        executor = start_executor().await?;
        assert_eq!(
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent_id,
                    &key,
                    CONTENTION_METHOD,
                    data_value!(CONTENTION_TIMEOUT_MS)
                )
                .await?
                .into_typed::<(String, String)>()?,
            ("session-lost".into(), "session-lost".into())
        );
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        assert_call_terminal_counts(&oplog, start, 1, 1);
        executor.shutdown_and_wait_for_invocation_loops().await?;
        drop(executor);
    }
    executor = start_executor().await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "probe_report_loss", data_value!())
            .await?
            .into_typed::<(bool, bool, bool, bool)>()?,
        (true, true, true, true)
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    for sibling in completed_siblings {
        assert_call_terminal_counts(&oplog, sibling, 1, 1);
    }
    assert_call_terminal_counts(&oplog, send, 1, 1);
    assert_eq!(
        peer.frames_received()
            .iter()
            .filter(|text| text.as_str() == "completed-before-crash")
            .count(),
        1
    );
    assert_eq!(peer.accepted(), 1);
    assert_eq!(peer.completed_handshakes(), 1);
    drop(executor);
    peer.stop().await?;
    Ok(())
}

fn assert_call_terminal_counts(
    oplog: &[PublicOplogEntryWithIndex],
    start: OplogIndex,
    ends: usize,
    delivered: usize,
) {
    assert_eq!(oplog.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(params) if params.start_index == start)).count(), ends, "{}", describe_oplog(oplog));
    assert_eq!(oplog.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::CompletionDelivered(params) if params.start_index == start)).count(), delivered, "{}", describe_oplog(oplog));
    assert!(!oplog.iter().any(|entry| matches!(&entry.entry, PublicOplogEntry::CompletionDiscarded(params) if params.start_index == start) || matches!(&entry.entry, PublicOplogEntry::Cancelled(params) if params.start_index == start)), "{}", describe_oplog(oplog));
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_report_loss_obeys_non_idempotent_guard_and_atomic_rollback(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let mut executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    for atomic in [false, true] {
        let mut peer = ReconnectTestServer::start().await;
        peer.arm_hold();
        let agent_id = agent_id!("WebsocketTest", format!("non-idempotent-atomic-{atomic}"));
        let worker_id = executor
            .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
            .await?;
        let key = IdempotencyKey::fresh();
        let method = if atomic {
            "atomic_report_connect"
        } else {
            "non_idempotent_report_connect"
        };
        executor
            .invoke_agent_with_key(&component, &agent_id, &key, method, data_value!(peer.url()))
            .await?;
        peer.wait_for_count(
            1,
            |event| matches!(event, ReconnectServerEvent::UpgradeRequestRead),
            "read initial held handshake",
        )
        .await?;
        executor.commit_oplog(&worker_id).await?;
        executor.shutdown_and_wait_for_invocation_loops().await?;
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        assert_single_websocket_call_incomplete(&oplog, "golem:websocket/client::connect");
        drop(executor);
        peer.wait_for_count(
            1,
            |event| matches!(event, ReconnectServerEvent::ClientVanishedWhileHeld),
            "observe abandoned handshake",
        )
        .await?;
        executor = start(deps, &context).await?;
        if atomic {
            peer.release_hold();
            assert_eq!(
                executor
                    .invoke_and_await_agent_with_key(
                        &component,
                        &agent_id,
                        &key,
                        method,
                        data_value!(peer.url())
                    )
                    .await?
                    .into_typed::<String>()?,
                RECONNECT_FIRST_MESSAGE
            );
            assert_eq!(peer.accepted(), 2);
            assert_eq!(peer.completed_handshakes(), 1);
        } else {
            let result = tokio::time::timeout(
                Duration::from_secs(10),
                executor.invoke_and_await_agent_with_key(
                    &component,
                    &agent_id,
                    &key,
                    method,
                    data_value!(peer.url()),
                ),
            )
            .await?;
            let error = result.expect_err("incomplete non-idempotent connect must trap");
            assert!(
                format!("{error:?}").contains(
                    "Non-idempotent remote write operation was not completed, cannot retry"
                ),
                "expected central retry guard, not guest connection failure: {error:?}"
            );
            assert_eq!(peer.accepted(), 1);
            assert_eq!(peer.completed_handshakes(), 0);
        }
        peer.stop().await?;
    }
    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_report_loss_preserves_live_disconnect_and_recorded_close(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let mut executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    for closed in [false, true] {
        let mut peer = ReconnectTestServer::start().await;
        let agent_id = agent_id!("WebsocketTest", format!("closed-{closed}"));
        executor
            .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
            .await?;
        executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "connect_report_loss",
                data_value!(peer.url()),
            )
            .await?;
        peer.wait_for_count(1, |event| matches!(event, ReconnectServerEvent::FrameReceived(text) if text == "completed-before-crash"), "receive completed send").await?;
        let method = if closed {
            assert_eq!(
                executor
                    .invoke_and_await_agent(
                        &component,
                        &agent_id,
                        "close_persisted_result",
                        data_value!()
                    )
                    .await?
                    .into_typed::<Result<(), String>>()?,
                Ok(())
            );
            executor.shutdown_and_wait_for_invocation_loops().await?;
            drop(executor);
            executor = start(deps, &context).await?;
            "receive_is_closed"
        } else {
            peer.stop().await?;
            "live_disconnect_is_not_session_lost"
        };
        assert!(
            executor
                .invoke_and_await_agent(&component, &agent_id, method, data_value!())
                .await?
                .into_typed::<bool>()?
        );
        assert_eq!(peer.accepted(), 1);
        assert_eq!(peer.completed_handshakes(), 1);
        if closed {
            peer.stop().await?;
        }
    }
    drop(executor);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconstruction_policies_are_independent_in_one_worker(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let mut report_peer = ReconnectTestServer::start().await;
    let mut automatic_peer = ReconnectTestServer::start().await;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("WebsocketTest", "independent-policies");
    executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "connect_both_policies",
                data_value!(report_peer.url(), automatic_peer.url())
            )
            .await?
            .into_typed::<(String, String)>()?,
        (
            RECONNECT_FIRST_MESSAGE.into(),
            RECONNECT_FIRST_MESSAGE.into()
        )
    );
    report_peer.wait_for_count(1, |event| matches!(event, ReconnectServerEvent::FrameReceived(text) if text == "completed-before-crash"), "receive completed report-policy send").await?;
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start(deps, &context).await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "probe_both_policies", data_value!())
            .await?
            .into_typed::<(bool, String)>()?,
        (true, RECONNECT_MESSAGES[0].into())
    );
    assert_eq!(report_peer.accepted(), 1);
    assert_eq!(
        report_peer.frames_received(),
        vec!["completed-before-crash".to_string()]
    );
    assert_eq!(automatic_peer.accepted(), 2);
    assert_eq!(automatic_peer.completed_handshakes(), 2);
    drop(executor);
    report_peer.stop().await?;
    automatic_peer.stop().await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_report_loss_effect_joins_initialization_before_replacement(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("tool_streaming_effect_caller")] caller: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let mut old_peer = ReconnectTestServer::start().await;
    let mut replacement_peer = ReconnectTestServer::start().await;
    let component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let agent_id = agent_id!("WebsocketRecoveryProbe", "effect-real-loss");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    let key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &key,
            "recover",
            data_value!(old_peer.url(), replacement_peer.url()),
        )
        .await?;
    old_peer.wait_for_count(1, |event| matches!(event, ReconnectServerEvent::FrameReceived(text) if text == "initialize-old"), "receive old initialization").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            executor.commit_oplog(&worker_id).await?;
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            if starts_of_websocket_function(&oplog, "golem:websocket/client::receive").len() == 2 {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    executor.shutdown_and_wait_for_invocation_loops().await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let receives = starts_of_websocket_function(&oplog, "golem:websocket/client::receive");
    assert_eq!(
        receives.len(),
        2,
        "completed greeting and pending read must both exist"
    );
    assert!(!oplog.iter().any(|entry| matches!(&entry.entry,
        PublicOplogEntry::End(params) if params.start_index == receives[1])));
    drop(executor);
    let executor = start(deps, &context).await?;
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &key,
            "recover",
            data_value!(old_peer.url(), replacement_peer.url()),
        )
        .await?
        .into_typed::<(bool, bool, String)>()?;
    assert_eq!(result, (true, true, RECONNECT_FIRST_MESSAGE.into()));
    replacement_peer.wait_for_count(1, |event| matches!(event, ReconnectServerEvent::FrameReceived(text) if text == "initialize-new"), "receive replacement initialization").await?;
    assert_eq!(old_peer.accepted(), 1);
    assert_eq!(
        old_peer.frames_received(),
        vec!["initialize-old".to_string()]
    );
    assert_eq!(replacement_peer.accepted(), 1);
    assert_eq!(
        replacement_peer.frames_received(),
        vec!["initialize-new".to_string()]
    );
    drop(executor);
    old_peer.stop().await?;
    replacement_peer.stop().await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_report_loss_ts_public_wrapper_replacement(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let mut old_peer = ReconnectTestServer::start().await;
    let mut replacement_peer = ReconnectTestServer::start().await;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;
    let agent_id = agent_id!("WebSocketTest", "public-wrapper-loss");
    executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "connectReportLoss",
                data_value!(old_peer.url())
            )
            .await?
            .into_typed::<String>()?,
        RECONNECT_FIRST_MESSAGE
    );
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start(deps, &context).await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "probeReportLoss", data_value!())
            .await?
            .into_typed::<Vec<bool>>()?,
        vec![true; 4]
    );
    assert_eq!(old_peer.accepted(), 1);
    assert_eq!(old_peer.frames_received(), Vec::<String>::new());
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &component,
                &agent_id,
                "replaceLost",
                data_value!(replacement_peer.url())
            )
            .await?
            .into_typed::<String>()?,
        RECONNECT_FIRST_MESSAGE
    );
    replacement_peer.wait_for_count(1,
        |event| matches!(event, ReconnectServerEvent::FrameReceived(text) if text == "initialize-new"),
        "receive public-wrapper replacement initialization").await?;
    assert_eq!(old_peer.accepted(), 1);
    assert_eq!(replacement_peer.completed_handshakes(), 1);
    drop(executor);
    old_peer.stop().await?;
    replacement_peer.stop().await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_report_loss_absorbs_repeated_restart_without_pool_io(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| config.max_websocket_connections = 1)),
        ..Default::default()
    };
    let context = TestContext::new(last_unique_id);
    let mut executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let mut peer = ReconnectTestServer::start().await;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("WebsocketTest", "report-loss-restarts");
    executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_report_loss",
            data_value!(peer.url()),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(first, RECONNECT_FIRST_MESSAGE);

    peer.wait_for_count(
        1,
        |event| {
            matches!(event,
        ReconnectServerEvent::FrameReceived(text) if text == "completed-before-crash")
        },
        "receive the completed send before crashing",
    )
    .await?;
    for _ in 0..3 {
        executor.shutdown_and_wait_for_invocation_loops().await?;
        drop(executor);
        executor = start_with_overrides(deps, &context, overrides.clone()).await?;
        // Activating first reconstructs completed receive and timeout calls.
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &agent_id, "noop", data_value!(),)
                .await?
                .into_typed::<String>()?,
            "ok"
        );
        let pool = executor
            .websocket_connection_pool()
            .expect("captured websocket pool");
        let permit = tokio::time::timeout(Duration::from_secs(5), pool.acquire()).await??;
        // Loss must not wait for even the only permit, including close.
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            executor.invoke_and_await_agent(
                &component,
                &agent_id,
                "probe_report_loss",
                data_value!(),
            ),
        )
        .await??
        .into_typed::<(bool, bool, bool, bool)>()?;
        assert_eq!(result, (true, true, true, true));
        drop(permit);
        assert_eq!(peer.accepted(), 1);
        assert_eq!(peer.completed_handshakes(), 1);
        assert_eq!(
            peer.frames_sent(),
            vec![RECONNECT_FIRST_MESSAGE.to_string()]
        );
        assert_eq!(
            peer.frames_received(),
            vec!["completed-before-crash".to_string()]
        );
    }
    let pool = executor
        .websocket_connection_pool()
        .expect("captured websocket pool");
    let permit = tokio::time::timeout(Duration::from_secs(5), pool.acquire()).await??;
    tokio::time::timeout(
        Duration::from_secs(10),
        executor.invoke_and_await_agent(&component, &agent_id, "drop_persisted", data_value!()),
    )
    .await??;
    drop(permit);
    assert_eq!(peer.accepted(), 1);
    drop(executor);
    peer.stop().await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_report_loss_pending_receives_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let mut peer = ReconnectTestServer::start().await;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let agent_id = agent_id!("WebsocketTest", "report-loss-pending");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_report_loss",
            data_value!(peer.url()),
        )
        .await?;
    let key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            executor.commit_oplog(&worker_id).await?;
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            if contention_receive_starts(&oplog).len() == 2 {
                assert_contention_receives_incomplete(&oplog);
                return Ok::<_, anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    peer.wait_for_count(
        1,
        |event| {
            matches!(event,
        ReconnectServerEvent::FrameReceived(text) if text == "completed-before-crash")
        },
        "receive the completed send before crashing",
    )
    .await?;
    executor.shutdown_and_wait_for_invocation_loops().await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receives_incomplete(&oplog);
    drop(executor);
    let executor = start(deps, &context).await?;
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(result, ("session-lost".into(), "session-lost".into()));
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &agent_id, "probe_report_loss", data_value!())
            .await?
            .into_typed::<(bool, bool, bool, bool)>()?,
        (true, true, true, true)
    );
    assert_eq!(peer.accepted(), 1);
    assert_eq!(peer.completed_handshakes(), 1);
    assert_eq!(
        peer.frames_received(),
        vec!["completed-before-crash".to_string()]
    );
    drop(executor);
    peer.stop().await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_echo_rust(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    let ws_server = spawn(
        async move {
            if let Ok((stream, _)) = listener.accept().await {
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (mut write, mut read) = StreamExt::split(ws_stream);
                while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                    if msg.is_close() {
                        break;
                    }
                    if msg.is_text() || msg.is_binary() {
                        SinkExt::send(&mut write, msg).await.ok();
                    }
                }
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-echo-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "echo",
            data_value!(format!("ws://localhost:{ws_port}"), "hello websocket"),
        )
        .await?;

    assert_eq!(result.into_typed::<String>()?, "hello websocket");

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_echo_rust_oplog_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    // First executor instance + WebSocket echo server
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    // Accept many connections: the second invocation after executor restart
    // performs a new live connect; a single accept() would leave nothing listening.
    let ws_server = spawn(
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (mut write, mut read) = StreamExt::split(ws_stream);
                while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                    if msg.is_close() {
                        break;
                    }
                    if msg.is_text() || msg.is_binary() {
                        SinkExt::send(&mut write, msg).await.ok();
                    }
                }
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-echo-oplog-replay");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    // First invocation: full WebSocket session (connect/send/receive/drop) and
    // one entry persisted in agent-local `echo_history`.
    let first_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "echo_and_record",
            data_value!(format!("ws://localhost:{ws_port}"), "hello websocket"),
        )
        .await?;

    assert_eq!(first_result.into_typed::<String>()?, "hello websocket");

    executor.check_oplog_is_queryable(&worker_id).await?;

    // Drop the executor to force replay on next activation. Keep server running
    // so the second invocation can still do live websocket I/O.
    drop(executor);

    // Restarting does not directly invoke guest functions; replay is performed
    // when this worker is activated by the next invocation.
    let executor = start(deps, &context).await?;

    // Second invocation: replay reconstructs agent state from the first invoke.
    // We append another message and assert we now observe "m1|m2".
    let second_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "echo_and_record",
            data_value!(format!("ws://localhost:{ws_port}"), "hello websocket 2"),
        )
        .await?;

    assert_eq!(
        second_result.into_typed::<String>()?,
        "hello websocket|hello websocket 2"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_reconnect_replays_completed_steps_and_continues_live(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();
    let accepted_connections = Arc::new(AtomicUsize::new(0));
    let transcript = Arc::new(Mutex::new(Vec::<ServerEvent>::new()));
    let accepted_connections_for_server = Arc::clone(&accepted_connections);
    let transcript_for_server = Arc::clone(&transcript);

    let ws_server = spawn(
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let connection = accepted_connections_for_server.fetch_add(1, Ordering::SeqCst);
                let transcript_for_connection = Arc::clone(&transcript_for_server);
                spawn(
                    async move {
                        let ws_stream = tokio_tungstenite::accept_async(stream)
                            .await
                            .expect("WS handshake failed");
                        let (mut write, mut read) = StreamExt::split(ws_stream);
                        while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                            if msg.is_close() {
                                break;
                            }
                            if msg.is_text() {
                                let payload = msg
                                    .to_text()
                                    .expect("text message should decode")
                                    .to_string();
                                transcript_for_connection.lock().unwrap().push(ServerEvent {
                                    connection,
                                    payload,
                                });
                            }
                            if msg.is_text() || msg.is_binary() {
                                SinkExt::send(&mut write, msg).await.ok();
                            }
                        }
                    }
                    .in_current_span(),
                );
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-phase2-reconnect");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let promise_id_value = executor
        .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    let params =
        replay_reconnect_roundtrip_params(format!("ws://localhost:{ws_port}"), &promise_id_value);
    let idempotency_key = IdempotencyKey::fresh();

    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key,
            "replay_reconnect_roundtrip",
            params.clone(),
        )
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    wait_for_server_state(&accepted_connections, &transcript, 1, 2).await?;
    assert_eq!(
        transcript.lock().unwrap().clone(),
        vec![
            ServerEvent {
                connection: 0,
                payload: "msg-1".to_string(),
            },
            ServerEvent {
                connection: 0,
                payload: "msg-2".to_string(),
            },
        ]
    );

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    let executor = start(deps, &context).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);
    assert_eq!(transcript.lock().unwrap().len(), 2);

    let oplog_idx = extract_oplog_idx_from_promise_id(&promise_id_value);
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx,
            },
            vec![1],
        )
        .await?;

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key,
            "replay_reconnect_roundtrip",
            params,
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    wait_for_server_state(&accepted_connections, &transcript, 2, 4).await?;
    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(
                "msg-1|msg-2|msg-3|msg-4".to_string(),
            ))),
        })
    );
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 2);
    assert_eq!(
        transcript.lock().unwrap().clone(),
        vec![
            ServerEvent {
                connection: 0,
                payload: "msg-1".to_string(),
            },
            ServerEvent {
                connection: 0,
                payload: "msg-2".to_string(),
            },
            ServerEvent {
                connection: 1,
                payload: "msg-3".to_string(),
            },
            ServerEvent {
                connection: 1,
                payload: "msg-4".to_string(),
            },
        ]
    );

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    ws_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_reconnect_failure_returns_guest_error(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();
    let accepted_connections = Arc::new(AtomicUsize::new(0));
    let transcript = Arc::new(Mutex::new(Vec::<ServerEvent>::new()));
    let accepted_connections_for_server = Arc::clone(&accepted_connections);
    let transcript_for_server = Arc::clone(&transcript);

    let ws_server = spawn(
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let connection = accepted_connections_for_server.fetch_add(1, Ordering::SeqCst);
                let transcript_for_connection = Arc::clone(&transcript_for_server);
                spawn(
                    async move {
                        let ws_stream = tokio_tungstenite::accept_async(stream)
                            .await
                            .expect("WS handshake failed");
                        let (mut write, mut read) = StreamExt::split(ws_stream);
                        while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                            if msg.is_close() {
                                break;
                            }
                            if msg.is_text() {
                                let payload = msg
                                    .to_text()
                                    .expect("text message should decode")
                                    .to_string();
                                transcript_for_connection.lock().unwrap().push(ServerEvent {
                                    connection,
                                    payload,
                                });
                            }
                            if msg.is_text() || msg.is_binary() {
                                SinkExt::send(&mut write, msg).await.ok();
                            }
                        }
                    }
                    .in_current_span(),
                );
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-failure");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let promise_id_value = executor
        .invoke_and_await_agent(&component, &agent_id, "create_promise", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    let params =
        replay_reconnect_roundtrip_params(format!("ws://localhost:{ws_port}"), &promise_id_value);
    let idempotency_key = IdempotencyKey::fresh();

    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key,
            "replay_reconnect_roundtrip",
            params.clone(),
        )
        .await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    wait_for_server_state(&accepted_connections, &transcript, 1, 2).await?;

    executor.check_oplog_is_queryable(&worker_id).await?;
    ws_server.abort();
    drop(executor);

    let executor = start(deps, &context).await?;

    let oplog_idx = extract_oplog_idx_from_promise_id(&promise_id_value);
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx,
            },
            vec![1],
        )
        .await?;

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &idempotency_key,
            "replay_reconnect_roundtrip",
            params,
        )
        .await?;
    let error = get_result_error_string(result);
    assert!(
        error.contains("ConnectionFailure"),
        "expected reconnect failure to reach the guest, got {error}"
    );
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_closed_connection_stays_terminal_after_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();
    let accepted_connections = Arc::new(AtomicUsize::new(0));
    let accepted_connections_for_server = Arc::clone(&accepted_connections);

    let ws_server = spawn(
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                accepted_connections_for_server.fetch_add(1, Ordering::SeqCst);
                spawn(
                    async move {
                        let ws_stream = tokio_tungstenite::accept_async(stream)
                            .await
                            .expect("WS handshake failed");
                        let (mut write, _read) = StreamExt::split(ws_stream);
                        SinkExt::send(
                            &mut write,
                            tokio_tungstenite::tungstenite::Message::text("msg-0"),
                        )
                        .await
                        .ok();
                        SinkExt::send(
                            &mut write,
                            tokio_tungstenite::tungstenite::Message::Close(None),
                        )
                        .await
                        .ok();
                    }
                    .in_current_span(),
                );
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-terminal-close-replay");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(format!("ws://localhost:{ws_port}")),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, "msg-0");
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);

    let closed_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "receive_next_from_persisted_result",
            data_value!(),
        )
        .await?;
    let closed_error = get_result_error_string(closed_result);
    assert!(
        closed_error.contains("Closed"),
        "expected persisted socket to report closure, got {closed_error}"
    );
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    let executor = start(deps, &context).await?;
    let replayed_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "receive_next_from_persisted_result",
            data_value!(),
        )
        .await?;
    let replayed_error = get_result_error_string(replayed_result);
    assert!(
        replayed_error.contains("Closed"),
        "expected replayed terminal socket to stay closed, got {replayed_error}"
    );
    assert_eq!(
        accepted_connections.load(Ordering::SeqCst),
        1,
        "terminal websocket should not reconnect after replay"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    ws_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_successful_close_terminalizes_handle_and_prevents_reconnect(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();
    let accepted_connections = Arc::new(AtomicUsize::new(0));
    let accepted_connections_for_server = Arc::clone(&accepted_connections);

    let ws_server = spawn(
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                accepted_connections_for_server.fetch_add(1, Ordering::SeqCst);
                spawn(
                    async move {
                        let ws_stream = tokio_tungstenite::accept_async(stream)
                            .await
                            .expect("WS handshake failed");
                        let (mut write, mut read) = StreamExt::split(ws_stream);
                        SinkExt::send(
                            &mut write,
                            tokio_tungstenite::tungstenite::Message::text("msg-0"),
                        )
                        .await
                        .ok();

                        while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                            if msg.is_close() {
                                break;
                            }
                        }
                    }
                    .in_current_span(),
                );
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-close-terminal-replay");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(format!("ws://localhost:{ws_port}")),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, "msg-0");
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);

    let close_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "close_persisted_result",
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    assert_eq!(
        close_result,
        SchemaValue::Result(ResultValuePayload::Ok { value: None })
    );

    let closed_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "receive_next_from_persisted_result",
            data_value!(),
        )
        .await?;
    let closed_error = get_result_error_string(closed_result);
    assert!(
        closed_error.contains("Closed"),
        "expected closed handle to become terminal immediately, got {closed_error}"
    );
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 1);

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);

    let executor = start(deps, &context).await?;
    let replayed_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "receive_next_from_persisted_result",
            data_value!(),
        )
        .await?;
    let replayed_error = get_result_error_string(replayed_result);
    assert!(
        replayed_error.contains("Closed"),
        "expected successfully closed websocket to remain terminal after replay, got {replayed_error}"
    );
    assert_eq!(
        accepted_connections.load(Ordering::SeqCst),
        1,
        "successfully closed websocket should not reconnect after replay"
    );

    executor.check_oplog_is_queryable(&worker_id).await?;
    drop(executor);
    ws_server.abort();

    Ok(())
}

fn replay_reconnect_roundtrip_params(
    url: String,
    promise_id_value: &SchemaValue,
) -> golem_common::schema::TypedSchemaValue {
    crate::raw_params(vec![SchemaValue::String(url), promise_id_value.clone()])
}

fn extract_oplog_idx_from_promise_id(promise_id_value: &SchemaValue) -> OplogIndex {
    let SchemaValue::Record { fields } = promise_id_value else {
        panic!("Expected a record for PromiseId");
    };
    let SchemaValue::U64(oplog_idx) = fields[1] else {
        panic!("Expected second PromiseId field to be oplog_idx");
    };

    OplogIndex::from_u64(oplog_idx)
}

fn get_result_error_string(result: golem_test_framework::dsl::AgentResult) -> String {
    let return_value = result
        .into_return_value()
        .expect("expected a single return value");

    match return_value {
        SchemaValue::Result(ResultValuePayload::Err {
            value: Some(err_value),
        }) => match *err_value {
            SchemaValue::String(s) => s,
            other => panic!("expected error String, got {other:?}"),
        },
        other => panic!("expected Result(Err(...)), got {other:?}"),
    }
}

async fn wait_for_server_state(
    accepted_connections: &Arc<AtomicUsize>,
    transcript: &Arc<Mutex<Vec<ServerEvent>>>,
    expected_connections: usize,
    expected_messages: usize,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let actual_connections = accepted_connections.load(Ordering::SeqCst);
            let actual_messages = transcript.lock().unwrap().len();

            if actual_connections > expected_connections || actual_messages > expected_messages {
                panic!(
                    "Websocket server state overshot: expected {expected_connections}/{expected_messages}, got {actual_connections}/{actual_messages}"
                );
            }

            if actual_connections == expected_connections && actual_messages == expected_messages {
                break;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| {
        anyhow!(
            "Timed out waiting for websocket server state: expected {expected_connections} connections and {expected_messages} messages"
        )
    })
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_receive_with_timeout(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    let ws_server = spawn(
        async move {
            if let Ok((stream, _)) = listener.accept().await {
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (_write, mut read) = StreamExt::split(ws_stream);
                while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                    if msg.is_close() {
                        break;
                    }
                }
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "ws-timeout-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "receive_with_timeout_test",
            data_value!(format!("ws://localhost:{ws_port}"), 1000u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(result, SchemaValue::Option { inner: None });

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_polling_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();
    let message = "Hello from polling test";

    let ws_server = spawn(
        async move {
            if let Ok((stream, _)) = listener.accept().await {
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (mut write, _read) = StreamExt::split(ws_stream);
                let msg = tokio_tungstenite::tungstenite::Message::text(message);
                SinkExt::send(&mut write, msg).await.ok();
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "websocket-polling-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "poll_for_message",
            data_value!(format!("ws://localhost:{ws_port}"), 1000u64),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(message.to_string())))
        })
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_polling_survives_repeated_timeouts(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();
    let message = "delayed polling message";

    let ws_server = spawn(
        async move {
            if let Ok((stream, _)) = listener.accept().await {
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (mut write, _read) = StreamExt::split(ws_stream);
                tokio::time::sleep(Duration::from_millis(150)).await;
                let msg = tokio_tungstenite::tungstenite::Message::text(message);
                SinkExt::send(&mut write, msg).await.ok();
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "websocket-polling-timeout-race-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "poll_until_message_after_timeouts",
            data_value!(format!("ws://localhost:{ws_port}"), 10u64, 30u32),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(message.to_string())))
        })
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_async_bidirectional_test(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    // Echo server that supports multiple inbound/outbound messages on one connection.
    let ws_server = spawn(
        async move {
            if let Ok((stream, _)) = listener.accept().await {
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (mut write, mut read) = StreamExt::split(ws_stream);
                while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                    if msg.is_close() {
                        break;
                    }
                    if msg.is_text() || msg.is_binary() {
                        SinkExt::send(&mut write, msg).await.ok();
                    }
                }
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "websocket-async-bidi-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "async_bidi_test",
            data_value!(format!("ws://localhost:{ws_port}")),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(
                "msg-a|msg-b|msg-c".to_string()
            ))),
        })
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_async_bidirectional_test_oplog_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);

    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    // Keep accepting connections so the second invocation after restart can run live.
    let ws_server = spawn(
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };

                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (mut write, mut read) = StreamExt::split(ws_stream);
                while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                    if msg.is_close() {
                        break;
                    }
                    if msg.is_text() || msg.is_binary() {
                        SinkExt::send(&mut write, msg).await.ok();
                    }
                }
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebsocketTest", "websocket-async-bidi-oplog-replay");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let first_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "async_bidi_test",
            data_value!(format!("ws://localhost:{ws_port}")),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        first_result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(
                "msg-a|msg-b|msg-c".to_string()
            ))),
        })
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    let executor = start(deps, &context).await?;

    let second_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "async_bidi_test",
            data_value!(format!("ws://localhost:{ws_port}")),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;

    assert_eq!(
        second_result,
        SchemaValue::Result(ResultValuePayload::Ok {
            value: Some(Box::new(SchemaValue::String(
                "msg-a|msg-b|msg-c".to_string()
            ))),
        })
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn websocket_echo_ts(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let ws_port = listener.local_addr().unwrap().port();

    let ws_server = spawn(
        async move {
            if let Ok((stream, _)) = listener.accept().await {
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("WS handshake failed");
                let (mut write, mut read) = StreamExt::split(ws_stream);
                while let Some(Ok(msg)) = StreamExt::next(&mut read).await {
                    if msg.is_close() {
                        break;
                    }
                    if msg.is_text() || msg.is_binary() {
                        SinkExt::send(&mut write, msg).await.ok();
                    }
                }
            }
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let mut env_vars = HashMap::new();
    env_vars.insert("WS_PORT".to_string(), ws_port.to_string());

    let agent_id = agent_id!("WebSocketTest", "ws-echo-test");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env_vars, Vec::new())
        .await?;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "echo",
            data_value!(format!("ws://localhost:{ws_port}"), "hello websocket"),
        )
        .await?;

    assert_eq!(result.into_typed::<String>()?, "hello websocket");

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.abort();

    Ok(())
}

// ---- Per-handle reconnect coordination of concurrent receives ----
//
// The `receive` / `receive-with-timeout` helpers must coordinate the live
// reconnect of a replayed websocket handle per resource: at most one call
// performs the reconnect (pool permit + handshake + publish) while concurrent
// calls wait on the per-handle gate and re-read the entry afterwards. Without
// that coordination, concurrent receives each acquire pool permits
// independently; with a single permit, the first publishes its live connection
// holding the only permit for the connection's lifetime, the follower parks on
// the pool forever, and the worker never completes.
//
// The tests below observe the coordination through a real local websocket
// peer instead of instrumenting the executor: the production client runs one
// identical gate / pool permit / `connect_async` path in every test, and the
// peer withholds the 101 handshake response of the reconnect under test
// control, which parks the decided call inside its real `connect_async`
// handshake.

const RECONNECT_FIRST_MESSAGE: &str = "original";
const RECONNECT_MESSAGES: [&str; 2] = ["post-reconnect-a", "post-reconnect-b"];
const CONTENTION_METHOD: &str = "receive_lock_contention_from_persisted";
const CONTENTION_TIMEOUT_MS: u64 = 60_000;
const DIRECT_SEND_MESSAGE: &str = "direct-send-payload";
const WEBSOCKET_SEND_FUNCTION: &str = "golem:websocket/client::send";
const WEBSOCKET_CLOSE_FUNCTION: &str = "golem:websocket/client::close";

/// Test-controlled resolution of a withheld websocket handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoldDirective {
    /// Upgrade requests read while holding park without a handshake response
    /// until the directive changes.
    Holding,
    /// A withheld handshake completes normally; later upgrade requests
    /// handshake immediately.
    Released,
    /// A withheld handshake is rejected with `403 Forbidden`; later upgrade
    /// requests handshake immediately.
    Rejected,
}

/// The real local peer's observable activity. A `FrameSent` event is recorded
/// only after its frame was successfully sent, so the recorded frame counts are
/// acknowledged per-frame evidence rather than send attempts.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ReconnectServerEvent {
    /// A TCP connection was accepted.
    Accepted,
    /// A websocket upgrade request was fully read.
    UpgradeRequestRead,
    /// The 101 handshake response was written.
    HandshakeCompleted,
    /// The withheld handshake was rejected with an HTTP error.
    HandshakeRejected,
    /// The client of a withheld handshake vanished before the hold was
    /// resolved — an abandoned reconnect attempt.
    ClientVanishedWhileHeld,
    /// A text frame was sent and flushed successfully.
    FrameSent(String),
    /// A text frame was received.
    FrameReceived(String),
    /// A websocket close frame was received and replied to.
    CloseFrameReceived,
    /// The connection closed via error or EOF.
    ConnectionClosed,
}

type WsServerStream = WebSocketStream<tokio::net::TcpStream>;
type WsServerSink =
    futures::stream::SplitSink<WsServerStream, tokio_tungstenite::tungstenite::Message>;

/// State shared between the peer's listener and per-connection tasks.
#[derive(Clone)]
struct ServerShared {
    directive_tx: watch::Sender<HoldDirective>,
    directive_rx: watch::Receiver<HoldDirective>,
    events: Arc<Mutex<Vec<ReconnectServerEvent>>>,
    notify: Arc<tokio::sync::Notify>,
    errors: Arc<Mutex<Vec<String>>>,
    completed_handshakes: Arc<AtomicUsize>,
}

fn emit(shared: &ServerShared, event: ReconnectServerEvent) {
    shared.events.lock().unwrap().push(event);
    shared.notify.notify_waiters();
}

/// A real local websocket peer with test-controlled handshake withholding.
///
/// The production client under test runs one identical gate / pool permit /
/// `connect_async` path in every test: instead of instrumenting the executor,
/// the peer reads the client's HTTP upgrade request and withholds the 101
/// response while the test holds it, which parks the client inside its real
/// `connect_async` handshake. Resolving the hold completes the withheld
/// handshake with a manually written 101 or rejects it with an HTTP error,
/// both under test control; the resolution applies to the withheld connection
/// and later upgrade requests handshake immediately.
///
/// Every accepted connection is served by an owned, joinable task, so stopping
/// the peer really closes every connection, and every task failure is
/// collected and surfaced to the test when the peer stops.
struct ReconnectTestServer {
    port: u16,
    shared: ServerShared,
    listener_task: Option<tokio::task::JoinHandle<()>>,
    connection_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl ReconnectTestServer {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (directive_tx, directive_rx) = watch::channel(HoldDirective::Released);
        let shared = ServerShared {
            directive_tx: directive_tx.clone(),
            directive_rx,
            events: Arc::new(Mutex::new(Vec::new())),
            notify: Arc::new(tokio::sync::Notify::new()),
            errors: Arc::new(Mutex::new(Vec::new())),
            completed_handshakes: Arc::new(AtomicUsize::new(0)),
        };
        let connection_tasks = Arc::new(Mutex::new(Vec::new()));

        let listener_shared = shared.clone();
        let task_registry = connection_tasks.clone();
        let listener_task = spawn(
            async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    emit(&listener_shared, ReconnectServerEvent::Accepted);
                    let connection_shared = listener_shared.clone();
                    let task = spawn(
                        async move {
                            let error_shared = connection_shared.clone();
                            if let Err(error) = serve_connection(stream, connection_shared).await {
                                error_shared
                                    .errors
                                    .lock()
                                    .unwrap()
                                    .push(format!("connection task failed: {error}"));
                            }
                        }
                        .in_current_span(),
                    );
                    task_registry.lock().unwrap().push(task);
                }
            }
            .in_current_span(),
        );

        Self {
            port,
            shared,
            listener_task: Some(listener_task),
            connection_tasks,
        }
    }

    fn url(&self) -> String {
        format!("ws://localhost:{}", self.port)
    }

    /// Withholds the handshake responses of upgrade requests that arrive from
    /// now on, until the hold is resolved.
    fn arm_hold(&self) {
        self.shared
            .directive_tx
            .send_replace(HoldDirective::Holding);
    }

    fn release_hold(&self) {
        self.shared
            .directive_tx
            .send_replace(HoldDirective::Released);
    }

    fn reject_hold(&self) {
        self.shared
            .directive_tx
            .send_replace(HoldDirective::Rejected);
    }

    fn count(&self, predicate: impl Fn(&ReconnectServerEvent) -> bool) -> usize {
        self.shared
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| predicate(event))
            .count()
    }

    fn accepted(&self) -> usize {
        self.count(|event| matches!(event, ReconnectServerEvent::Accepted))
    }

    fn completed_handshakes(&self) -> usize {
        self.count(|event| matches!(event, ReconnectServerEvent::HandshakeCompleted))
    }

    fn upgrade_requests_read(&self) -> usize {
        self.count(|event| matches!(event, ReconnectServerEvent::UpgradeRequestRead))
    }

    fn handshakes_rejected(&self) -> usize {
        self.count(|event| matches!(event, ReconnectServerEvent::HandshakeRejected))
    }

    fn clients_vanished_while_held(&self) -> usize {
        self.count(|event| matches!(event, ReconnectServerEvent::ClientVanishedWhileHeld))
    }

    fn close_frames_received(&self) -> usize {
        self.count(|event| matches!(event, ReconnectServerEvent::CloseFrameReceived))
    }

    fn frames_sent(&self) -> Vec<String> {
        self.shared
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                ReconnectServerEvent::FrameSent(payload) => Some(payload.clone()),
                _ => None,
            })
            .collect()
    }

    fn frames_received(&self) -> Vec<String> {
        self.shared
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                ReconnectServerEvent::FrameReceived(payload) => Some(payload.clone()),
                _ => None,
            })
            .collect()
    }

    /// Waits until at least `expected` recorded events match `predicate`.
    async fn wait_for_count(
        &self,
        expected: usize,
        predicate: impl Fn(&ReconnectServerEvent) -> bool,
        description: &str,
    ) -> anyhow::Result<()> {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            // The notified future is created before the condition check so no
            // wake-up between the check and the await is missed.
            let notified = self.shared.notify.notified();
            let count = self.count(&predicate);
            if count >= expected {
                return Ok(());
            }
            let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
                anyhow::bail!(
                    "timed out waiting for the server to {description} \
                     (expected {expected} matching events, got {count}); \
                     events so far: {:?}",
                    self.shared.events.lock().unwrap()
                );
            };
            let _ = tokio::time::timeout(remaining, notified).await;
        }
    }

    /// Stops the peer: aborts and joins the listener and every owned connection
    /// task — which really closes every connection — and surfaces every task
    /// error recorded so far.
    async fn stop(&mut self) -> anyhow::Result<()> {
        if let Some(listener_task) = self.listener_task.take() {
            listener_task.abort();
            let _ = listener_task.await;
        }
        let connection_tasks = std::mem::take(&mut *self.connection_tasks.lock().unwrap());
        for task in connection_tasks {
            task.abort();
            let _ = task.await;
        }
        let errors = self.shared.errors.lock().unwrap();
        anyhow::ensure!(
            errors.is_empty(),
            "the reconnect test server recorded task errors: {errors:?}"
        );
        Ok(())
    }
}

/// Serves one accepted connection: reads the upgrade request, withholds or
/// completes the handshake according to the hold directive, sends the greeting
/// frames of the connection's handshake index and serves the frame loop until
/// the connection closes.
async fn serve_connection(
    mut stream: tokio::net::TcpStream,
    mut shared: ServerShared,
) -> Result<(), String> {
    let request = read_upgrade_request(&mut stream).await?;
    emit(&shared, ReconnectServerEvent::UpgradeRequestRead);
    let key = extract_websocket_key(&request)
        .ok_or_else(|| "the upgrade request carries no Sec-WebSocket-Key".to_string())?;

    let current_directive = *shared.directive_rx.borrow_and_update();
    let directive = match current_directive {
        HoldDirective::Holding => {
            // Withhold the handshake response while the test holds it. If the
            // client vanishes first — an abandoned reconnect attempt — the
            // withheld connection is simply dropped without a response.
            let mut vanish_probe = [0u8; 1];
            let resolved = tokio::select! {
                directive = shared
                    .directive_rx
                    .wait_for(|directive| *directive != HoldDirective::Holding) => {
                    Some(*directive.expect("the hold directive watch channel stays alive"))
                }
                read = stream.read(&mut vanish_probe) => {
                    let _ = read;
                    None
                }
            };
            match resolved {
                Some(directive) => directive,
                None => {
                    emit(&shared, ReconnectServerEvent::ClientVanishedWhileHeld);
                    return Ok(());
                }
            }
        }
        directive => directive,
    };

    match directive {
        HoldDirective::Rejected => {
            stream
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                .await
                .map_err(|error| format!("writing the handshake rejection failed: {error}"))?;
            stream
                .flush()
                .await
                .map_err(|error| format!("flushing the handshake rejection failed: {error}"))?;
            emit(&shared, ReconnectServerEvent::HandshakeRejected);
            // The rejection applied to the withheld connection: later upgrade
            // requests handshake immediately.
            shared.directive_tx.send_replace(HoldDirective::Released);
            return Ok(());
        }
        HoldDirective::Holding => unreachable!("a withheld hold resolves to a different directive"),
        HoldDirective::Released => {
            let accept_key =
                tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
            let response = format!(
                "HTTP/1.1 101 Switching Protocols\r\n\
                 Upgrade: websocket\r\n\
                 Connection: Upgrade\r\n\
                 Sec-WebSocket-Accept: {accept_key}\r\n\
                 \r\n"
            );
            stream
                .write_all(response.as_bytes())
                .await
                .map_err(|error| format!("writing the 101 response failed: {error}"))?;
            stream
                .flush()
                .await
                .map_err(|error| format!("flushing the 101 response failed: {error}"))?;
        }
    }

    let handshake = shared.completed_handshakes.fetch_add(1, Ordering::SeqCst);
    emit(&shared, ReconnectServerEvent::HandshakeCompleted);

    let ws_stream = WebSocketStream::from_raw_socket(
        stream,
        tokio_tungstenite::tungstenite::protocol::Role::Server,
        None,
    )
    .await;
    let (mut write, mut read) = StreamExt::split(ws_stream);

    // The first completed handshake greets the initial connection; later ones
    // deliver the reconnected handle's messages.
    if handshake == 0 {
        send_frame(&mut write, &shared, RECONNECT_FIRST_MESSAGE).await?;
    } else {
        for payload in RECONNECT_MESSAGES {
            send_frame(&mut write, &shared, payload).await?;
        }
    }

    loop {
        match StreamExt::next(&mut read).await {
            Some(Ok(message)) => match message {
                tokio_tungstenite::tungstenite::Message::Text(text) => {
                    emit(
                        &shared,
                        ReconnectServerEvent::FrameReceived(text.to_string()),
                    );
                }
                tokio_tungstenite::tungstenite::Message::Close(_) => {
                    let _ = SinkExt::send(
                        &mut write,
                        tokio_tungstenite::tungstenite::Message::Close(None),
                    )
                    .await;
                    emit(&shared, ReconnectServerEvent::CloseFrameReceived);
                    return Ok(());
                }
                _ => {}
            },
            Some(Err(_)) | None => {
                emit(&shared, ReconnectServerEvent::ConnectionClosed);
                return Ok(());
            }
        }
    }
}

/// Sends one text frame and records it only after it was successfully sent.
async fn send_frame(
    write: &mut WsServerSink,
    shared: &ServerShared,
    payload: &str,
) -> Result<(), String> {
    SinkExt::send(
        write,
        tokio_tungstenite::tungstenite::Message::text(payload),
    )
    .await
    .map_err(|error| format!("sending the frame {payload:?} failed: {error}"))?;
    emit(shared, ReconnectServerEvent::FrameSent(payload.to_string()));
    Ok(())
}

/// Reads the client's HTTP websocket upgrade request up to its header
/// terminator.
async fn read_upgrade_request(stream: &mut tokio::net::TcpStream) -> Result<String, String> {
    let mut request = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if request.len() > 16 * 1024 {
            return Err("the websocket upgrade request exceeded 16 KiB".to_string());
        }
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(String::from_utf8_lossy(&request).into_owned());
        }
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| format!("reading the websocket upgrade request failed: {error}"))?;
        if read == 0 {
            return Err(
                "the websocket upgrade request ended before its header terminator".to_string(),
            );
        }
        request.extend_from_slice(&chunk[..read]);
    }
}

fn extract_websocket_key(request: &str) -> Option<String> {
    request.split("\r\n").find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("Sec-WebSocket-Key") {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

/// The `AgentInvocationStarted` oplog index of the first run of the given
/// method, anchoring which durable-call and interrupt entries belong to that
/// invocation.
fn method_invocation_started(
    oplog: &[PublicOplogEntryWithIndex],
    method: &str,
) -> Option<OplogIndex> {
    oplog.iter().find_map(|entry| {
        let PublicOplogEntry::AgentInvocationStarted(params) = &entry.entry else {
            return None;
        };
        match &params.invocation {
            PublicAgentInvocation::AgentMethodInvocation(invocation)
                if invocation.method_name.replace('-', "_") == method.replace('-', "_") =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        }
    })
}

fn contention_invocation_started(oplog: &[PublicOplogEntryWithIndex]) -> Option<OplogIndex> {
    method_invocation_started(oplog, CONTENTION_METHOD)
}

/// The number of `Interrupted` oplog entries recorded after the given index:
/// one per delivered interrupt of the anchored invocation.
fn interrupted_entries_after(oplog: &[PublicOplogEntryWithIndex], after: OplogIndex) -> usize {
    oplog
        .iter()
        .filter(|entry| entry.oplog_index > after)
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Interrupted(_)))
        .count()
}

/// The `Start` oplog indices of the given websocket client function's durable
/// calls, in oplog order.
fn starts_of_websocket_function(
    oplog: &[PublicOplogEntryWithIndex],
    function: &str,
) -> Vec<OplogIndex> {
    oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == function => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect()
}

/// The receive-family durable `Start`s recorded after the contention run's
/// `AgentInvocationStarted`, as `(start index, function name)` pairs in oplog
/// order. A preserved durable session identity keeps exactly one `Start` per
/// function across interrupt and reconstruction.
fn contention_receive_starts(
    oplog: &[PublicOplogEntryWithIndex],
) -> Vec<(OplogIndex, &'static str)> {
    let Some(invocation_started) = contention_invocation_started(oplog) else {
        return Vec::new();
    };
    oplog
        .iter()
        .filter(|entry| entry.oplog_index > invocation_started)
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) => match params.function_name.as_str() {
                "golem:websocket/client::receive" => Some((entry.oplog_index, "receive")),
                "golem:websocket/client::receive-with-timeout" => {
                    Some((entry.oplog_index, "receive-with-timeout"))
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Asserts the contention run's oplog shows both receive-family `Start`s —
/// one `receive` and one `receive-with-timeout` — each still incomplete: no
/// `End` and no `Cancelled` terminal references them.
fn assert_contention_receives_incomplete(oplog: &[PublicOplogEntryWithIndex]) {
    let starts = contention_receive_starts(oplog);
    assert_eq!(
        starts.len(),
        2,
        "expected exactly one receive and one receive-with-timeout Start, got {starts:?}"
    );
    for (start, function) in starts {
        assert!(
            !oplog
                .iter()
                .any(|entry| matches!(&entry.entry, PublicOplogEntry::End(params)
                    if params.start_index == start)),
            "the interrupted round must leave the {function} Start at {start:?} without an End"
        );
        assert!(
            !oplog.iter().any(
                |entry| matches!(&entry.entry, PublicOplogEntry::Cancelled(params)
                    if params.start_index == start)
            ),
            "the interrupted round must leave the {function} Start at {start:?} without a Cancelled"
        );
    }
}

/// Asserts the contention run's completed oplog keeps the durable session
/// identity of both receives and correlates them with their terminals and
/// deliveries: exactly one `Start` per receive function, exactly one `End` and
/// one `CompletionDelivered` per `Start` with no `Cancelled`, both deliveries
/// after their `End` and everything before the run's single
/// `AgentInvocationFinished`.
fn assert_contention_receive_correlation(oplog: &[PublicOplogEntryWithIndex]) {
    let starts = contention_receive_starts(oplog);
    assert_eq!(
        starts.len(),
        2,
        "expected exactly one receive and one receive-with-timeout Start, got {starts:?}"
    );
    let is_contention_finished = |entry: &&PublicOplogEntryWithIndex| {
        matches!(&entry.entry, PublicOplogEntry::AgentInvocationFinished(params)
            if params.method_name.as_deref().map(|name| name.replace('-', "_"))
                == Some(CONTENTION_METHOD.to_string()))
    };
    let finished = oplog
        .iter()
        .filter(|entry| is_contention_finished(entry))
        .collect::<Vec<_>>();
    assert_eq!(
        finished.len(),
        1,
        "expected exactly one finished contention invocation, got {}",
        finished.len()
    );
    let finished_index = finished[0].oplog_index;
    for (start, function) in &starts {
        let ends = oplog
            .iter()
            .filter(|entry| {
                matches!(&entry.entry, PublicOplogEntry::End(params)
                if params.start_index == *start)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            ends.len(),
            1,
            "expected exactly one End for the {function} Start at {start:?}"
        );
        let deliveries = oplog
            .iter()
            .filter(|entry| {
                matches!(&entry.entry, PublicOplogEntry::CompletionDelivered(params)
                if params.start_index == *start)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            deliveries.len(),
            1,
            "expected exactly one CompletionDelivered for the {function} Start at {start:?}"
        );
        assert!(
            !oplog.iter().any(
                |entry| matches!(&entry.entry, PublicOplogEntry::Cancelled(params)
                    if params.start_index == *start)
            ),
            "expected no Cancelled for the {function} Start at {start:?}"
        );
        assert!(
            ends[0].oplog_index < deliveries[0].oplog_index,
            "the {function} End must precede its CompletionDelivered"
        );
        assert!(
            deliveries[0].oplog_index < finished_index,
            "the {function} CompletionDelivered must precede the contention AgentInvocationFinished"
        );
    }
}

/// Asserts the given websocket client function's single durable call is still
/// incomplete: exactly one `Start`, with no `End` and no `Cancelled`
/// referencing it.
fn assert_single_websocket_call_incomplete(oplog: &[PublicOplogEntryWithIndex], function: &str) {
    let starts = starts_of_websocket_function(oplog, function);
    assert_eq!(
        starts.len(),
        1,
        "expected exactly one {function} Start, got {starts:?}"
    );
    let start = starts[0];
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(&entry.entry, PublicOplogEntry::End(params)
                if params.start_index == start)),
        "the interrupted {function} Start at {start:?} must have no End"
    );
    assert!(
        !oplog.iter().any(
            |entry| matches!(&entry.entry, PublicOplogEntry::Cancelled(params)
                if params.start_index == start)
        ),
        "the interrupted {function} Start at {start:?} must have no Cancelled"
    );
}

/// Asserts the given websocket client function's single durable call kept its
/// durable session identity and completed: exactly one `Start` and one `End`
/// referencing it, with no `Cancelled`. The direct calls — `send` and `close` —
/// never record a `CompletionDelivered` marker even when the call spans an
/// interrupt: the guest task is synchronously blocked inside the host call, so
/// its delivery coincides with the host return and has no pre-delivery
/// divergence window. The marker records the accessor calls' separate
/// guest-facing delivery boundary, which the receive correlation asserts
/// cover.
fn assert_single_websocket_call_completed(oplog: &[PublicOplogEntryWithIndex], function: &str) {
    let starts = starts_of_websocket_function(oplog, function);
    assert_eq!(
        starts.len(),
        1,
        "expected exactly one {function} Start, got {starts:?}"
    );
    let start = starts[0];
    let ends = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::End(params) if params.start_index == start)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        ends.len(),
        1,
        "expected exactly one End for the {function} Start at {start:?}"
    );
    assert!(
        !oplog.iter().any(
            |entry| matches!(&entry.entry, PublicOplogEntry::Cancelled(params)
                if params.start_index == start)
        ),
        "expected no Cancelled for the {function} Start at {start:?}"
    );
}

/// The post-reconnect payloads both receives collect together, sorted so the
/// comparison does not depend on which receive won the reconnect coordination.
fn expected_reconnect_payloads() -> Vec<String> {
    let mut payloads = RECONNECT_MESSAGES
        .iter()
        .map(|payload| payload.to_string())
        .collect::<Vec<_>>();
    payloads.sort();
    payloads
}

/// Both receive results as a sorted multiset.
fn sorted_reconnect_payloads(result: &(String, String)) -> Vec<String> {
    let mut payloads = vec![result.0.clone(), result.1.clone()];
    payloads.sort();
    payloads
}

/// The greeting frames the peer must have sent: the initial connection's
/// greeting followed by the single reconnected connection's messages.
fn expected_reconnect_frames_sent() -> [&'static str; 3] {
    [
        RECONNECT_FIRST_MESSAGE,
        RECONNECT_MESSAGES[0],
        RECONNECT_MESSAGES[1],
    ]
}

/// A compact one-line-per-entry oplog listing for timeout diagnostics.
fn describe_oplog(oplog: &[PublicOplogEntryWithIndex]) -> String {
    oplog
        .iter()
        .map(|entry| {
            let description = match &entry.entry {
                PublicOplogEntry::Start(params) => {
                    format!("Start({})", params.function_name)
                }
                PublicOplogEntry::End(params) => format!("End(@{})", params.start_index),
                PublicOplogEntry::Cancelled(params) => {
                    format!("Cancelled(@{})", params.start_index)
                }
                PublicOplogEntry::CompletionDelivered(params) => {
                    format!("CompletionDelivered(@{})", params.start_index)
                }
                PublicOplogEntry::Interrupted(_) => "Interrupted".to_string(),
                PublicOplogEntry::AgentInvocationStarted(params) => match &params.invocation {
                    PublicAgentInvocation::AgentMethodInvocation(invocation) => {
                        format!("AgentInvocationStarted({})", invocation.method_name)
                    }
                    _ => "AgentInvocationStarted".to_string(),
                },
                PublicOplogEntry::AgentInvocationFinished(params) => format!(
                    "AgentInvocationFinished({})",
                    params.method_name.as_deref().unwrap_or("?")
                ),
                other => {
                    let kind = std::any::type_name_of_val(other)
                        .rsplit("::")
                        .next()
                        .unwrap_or("?");
                    kind.to_string()
                }
            };
            format!("{}: {description}", entry.oplog_index)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Polls the worker's oplog until `predicate` holds. While the worker is
/// live, the oplog query API only exposes entries up to the running
/// invocation's committed protocol barrier, so polling is a deterministic
/// boundary only for committed entries such as `AgentInvocationStarted`; a
/// parked durable call's `Start` becomes queryable once an interrupt or
/// crash commits the oplog.
async fn wait_for_oplog(
    executor: &TestWorkerExecutor,
    worker_id: &AgentId,
    predicate: impl Fn(&[PublicOplogEntryWithIndex]) -> bool,
    description: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
        if predicate(&oplog) {
            return Ok(());
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "timed out waiting for the worker's oplog to {description}; \
             oplog so far: {}",
            describe_oplog(&oplog)
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Waits for the worker to run the contention invocation and for that
/// invocation's committed `AgentInvocationStarted` entry, then settles
/// briefly.
///
/// The committed invocation boundary plus a short settle gives the guest time
/// to reach both receive parks: while the worker is live the parked
/// receive-family `Start`s are not exposed by the oplog query API, and the
/// interrupt that follows commits them, so the post-interrupt
/// incomplete-`Start` assertions prove loudly that both parks were open when
/// the interrupt landed.
async fn wait_for_contention_receives_to_park(
    executor: &TestWorkerExecutor,
    worker_id: &AgentId,
) -> anyhow::Result<()> {
    executor
        .wait_for_status(worker_id, AgentStatus::Running, Duration::from_secs(10))
        .await?;
    wait_for_oplog(
        executor,
        worker_id,
        |oplog| contention_invocation_started(oplog).is_some(),
        "commit the contention invocation's AgentInvocationStarted entry",
        Duration::from_secs(10),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

/// Concurrent `receive` and `receive-with-timeout` on a persisted websocket
/// handle must complete after an interrupt/reconstruct cycle, even with a
/// single-connection pool. Uncoordinated receives each take a pool permit
/// independently: the first publishes its live connection holding the only
/// permit, and the follower parks on the pool forever, so the retained
/// invocation never finishes and the worker never returns to idle. Per-handle
/// reconnect coordination lets one call reconnect while its concurrent peer
/// reuses the published entry, and both receives keep their durable session
/// identity: exactly one `Start` per function across the interrupt, completed
/// by one `End` each and delivered before the invocation finishes.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_concurrent_receives_complete_after_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.max_websocket_connections = 1;
            })),
            ..Default::default()
        },
    )
    .await?;

    let mut ws_server = ReconnectTestServer::start().await;
    let url = ws_server.url();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-contention");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    // The first handshake greets the initial live connection, whose handle is
    // persisted in agent state.
    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(url.clone()),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    assert_eq!(ws_server.accepted(), 1);
    assert_eq!(ws_server.completed_handshakes(), 1);

    // Both receives park on the live connection: the server sends nothing
    // further on it.
    let contention_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;

    wait_for_contention_receives_to_park(&executor, &worker_id).await?;

    // Interrupting abandons both receives with their `Start`s left incomplete.
    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;

    // The interrupt commits the oplog, so both abandoned receive-family
    // `Start`s are recorded and still incomplete: no `End` and no `Cancelled`
    // terminal references either of them.
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receives_incomplete(&oplog);

    // Resuming replays the persisted handle as a `Replay` entry and retries the
    // retained invocation: both receives re-execute live and contend on
    // reconnecting the same handle. With a single-connection pool this only
    // completes if the reconnect is coordinated per handle; otherwise the
    // follower parks on the pool and the worker never returns to idle.
    executor.resume(&worker_id, false).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    // The retained invocation finished exactly once; its recorded result holds
    // both post-reconnect payloads regardless of which receive reconnected.
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    // Exactly one reconnect connection was opened and both its greeting
    // messages were delivered to the receives.
    assert_eq!(ws_server.accepted(), 2);
    assert_eq!(ws_server.completed_handshakes(), 2);
    assert_eq!(ws_server.frames_sent(), expected_reconnect_frames_sent());
    assert_eq!(ws_server.frames_received(), Vec::<String>::new());

    // Re-invoking with the same key replays the recorded result without
    // opening new receive-family durable calls or a new connection.
    let replayed = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(replayed, result);
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    assert_eq!(ws_server.accepted(), 2);
    assert_eq!(ws_server.completed_handshakes(), 2);

    // Completed receives replay from their recorded `End`s without touching
    // the network: after evicting the idle worker and stopping the peer —
    // which really closes every connection — a fresh activation replays every
    // completed websocket call offline.
    assert!(
        executor
            .stop_worker_if_idle(&OwnedAgentId::new(
                context.default_environment_id,
                &worker_id
            ))
            .await?
    );
    ws_server.stop().await?;
    let noop = executor
        .invoke_and_await_agent(&component, &agent_id, "noop", data_value!())
        .await?;
    assert_eq!(noop.into_typed::<String>()?, "ok");
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    assert_eq!(ws_server.accepted(), 2);
    assert_eq!(ws_server.completed_handshakes(), 2);

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);

    Ok(())
}

/// The retry after an interrupt must coordinate the live reconnect of the
/// replayed handle per resource: exactly one accessor call decides to
/// reconnect while its concurrent peer queues on the per-handle gate, and the
/// decided call publishes the live connection for the queued call to reuse
/// instead of each call independently taking a pool permit and opening a
/// second connection. The real peer withholds the decided call's handshake
/// response under test control, which parks the decided call inside its real
/// `connect_async` handshake, so the coordination is observed at a
/// deterministic boundary: while the response is withheld exactly one
/// reconnect TCP connection exists and no handshake completed beyond the
/// initial one, and after the release both receives complete on the single
/// reconnected connection.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_coordination_admits_one_accessor_reconnect(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.max_websocket_connections = 1;
            })),
            ..Default::default()
        },
    )
    .await?;

    let mut ws_server = ReconnectTestServer::start().await;
    let url = ws_server.url();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-coordinate");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(url.clone()),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    assert_eq!(ws_server.accepted(), 1);
    assert_eq!(ws_server.completed_handshakes(), 1);

    let contention_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    wait_for_contention_receives_to_park(&executor, &worker_id).await?;

    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receives_incomplete(&oplog);

    // Withhold the retry's reconnect handshake at the real peer: the decided
    // call parks inside its real `connect_async` handshake, and the queued
    // peer waits on the per-handle gate for the published entry.
    ws_server.arm_hold();
    executor.resume(&worker_id, false).await?;
    ws_server
        .wait_for_count(
            2,
            |event| matches!(event, ReconnectServerEvent::UpgradeRequestRead),
            "read the reconnect's upgrade request",
        )
        .await?;

    // The queued call's position on the per-handle gate is internal to the
    // executor and cannot be observed from the peer; a short settle lets it
    // queue before the hold is resolved.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // While the decided call's handshake is withheld, exactly one reconnect
    // TCP connection exists — the queued call did not open its own — and no
    // handshake completed beyond the initial one.
    assert_eq!(
        ws_server.accepted(),
        2,
        "only the decided call's reconnect connection may be accepted while its handshake is withheld"
    );
    assert_eq!(
        ws_server.completed_handshakes(),
        1,
        "only the initial connection's handshake may be completed while the reconnect's response is withheld"
    );
    assert_eq!(ws_server.upgrade_requests_read(), 2);

    // Completing the withheld handshake lets the decided call publish the live
    // connection and the queued call reuse it; both receives complete.
    ws_server.release_hold();
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    // Exactly one reconnect connection was opened and both its greeting
    // messages were delivered to the receives.
    assert_eq!(ws_server.accepted(), 2);
    assert_eq!(ws_server.completed_handshakes(), 2);
    assert_eq!(ws_server.frames_sent(), expected_reconnect_frames_sent());
    assert_eq!(ws_server.frames_received(), Vec::<String>::new());

    // Re-invoking with the same key replays the recorded result without any
    // new connection.
    let replayed = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(replayed, result);
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    assert_eq!(ws_server.accepted(), 2);
    assert_eq!(ws_server.completed_handshakes(), 2);

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.stop().await?;

    Ok(())
}

/// The reconnect coordination also governs the retry after an executor
/// restart: a crash while both receives are parked leaves their durable calls
/// incomplete, and the same-key invocation on a fresh executor recovers the
/// worker, replays the persisted handle as a `Replay` entry and re-runs both
/// receives. Only one accessor call reconnects the handle while its peer
/// queues on the per-handle gate and reuses the published entry: while the
/// peer withholds the reconnect's handshake response, exactly one reconnect
/// connection exists.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_coordination_after_executor_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.max_websocket_connections = 1;
        })),
        ..Default::default()
    };
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;

    let mut ws_server = ReconnectTestServer::start().await;
    let url = ws_server.url();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-restart");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(url.clone()),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    assert_eq!(ws_server.accepted(), 1);
    assert_eq!(ws_server.completed_handshakes(), 1);

    let contention_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    wait_for_contention_receives_to_park(&executor, &worker_id).await?;

    // Crashing the executor while both receives are parked abandons the
    // retained invocation with its durable calls left incomplete.
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;

    // The same-key invocation on the fresh executor recovers the worker; the
    // recovered handle is a `Replay` entry, and withhold its reconnect
    // handshake at the real peer.
    ws_server.arm_hold();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    ws_server
        .wait_for_count(
            2,
            |event| matches!(event, ReconnectServerEvent::UpgradeRequestRead),
            "read the reconnect's upgrade request",
        )
        .await?;

    // The queued call's position on the per-handle gate is internal to the
    // executor and cannot be observed from the peer; a short settle lets it
    // queue before the hold is resolved.
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(
        ws_server.accepted(),
        2,
        "only the decided call's reconnect connection may be accepted while its handshake is withheld"
    );
    assert_eq!(
        ws_server.completed_handshakes(),
        1,
        "only the initial connection's handshake may be completed while the reconnect's response is withheld"
    );
    assert_eq!(ws_server.upgrade_requests_read(), 2);

    ws_server.release_hold();
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    // Exactly one reconnect connection was opened and both its greeting
    // messages were delivered to the receives.
    assert_eq!(ws_server.accepted(), 2);
    assert_eq!(ws_server.completed_handshakes(), 2);
    assert_eq!(ws_server.frames_sent(), expected_reconnect_frames_sent());
    assert_eq!(ws_server.frames_received(), Vec::<String>::new());

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.stop().await?;

    Ok(())
}

/// The per-handle coordination bounds the reconnect to a single call even when
/// the pool has spare permits: without the coordination both concurrent
/// receives would each acquire a permit and open their own connection. With
/// it, one accessor call reconnects and its peer reuses the published entry,
/// so while the peer withholds the reconnect's handshake response only one
/// reconnect TCP connection exists even though a spare permit is available,
/// and after the release exactly one reconnect connection was opened.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_coordination_with_spare_pool_permits(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.max_websocket_connections = 2;
            })),
            ..Default::default()
        },
    )
    .await?;

    let mut ws_server = ReconnectTestServer::start().await;
    let url = ws_server.url();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-spare");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(url.clone()),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    assert_eq!(ws_server.accepted(), 1);
    assert_eq!(ws_server.completed_handshakes(), 1);

    let contention_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    wait_for_contention_receives_to_park(&executor, &worker_id).await?;

    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receives_incomplete(&oplog);

    ws_server.arm_hold();
    executor.resume(&worker_id, false).await?;
    ws_server
        .wait_for_count(
            2,
            |event| matches!(event, ReconnectServerEvent::UpgradeRequestRead),
            "read the reconnect's upgrade request",
        )
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Even with a spare pool permit, only the decided call's reconnect
    // connection exists while its handshake is withheld: the queued call did
    // not open a connection of its own.
    assert_eq!(
        ws_server.accepted(),
        2,
        "even with a spare permit, only the decided call's reconnect connection may be accepted while its handshake is withheld"
    );
    assert_eq!(ws_server.completed_handshakes(), 1);
    assert_eq!(ws_server.upgrade_requests_read(), 2);

    ws_server.release_hold();
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    // Even with a spare permit, only one new connection was opened: the queued
    // call reused the published entry instead of reconnecting, and the single
    // reconnected connection's greeting went to the receives.
    assert_eq!(ws_server.accepted(), 2);
    assert_eq!(ws_server.completed_handshakes(), 2);
    assert_eq!(ws_server.frames_sent(), expected_reconnect_frames_sent());
    assert_eq!(ws_server.frames_received(), Vec::<String>::new());

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.stop().await?;

    Ok(())
}

/// A failing live reconnect must publish its terminal outcome per handle: the
/// decided accessor call parks its reconnect on the peer's withheld handshake
/// and the test rejects it with an HTTP error. The decided call terminally
/// closes the handle through the same re-verified publication window, its
/// queued peer re-reads the terminal entry and fails with the same error, the
/// pool permit the failed reconnect briefly held is released rather than
/// leaked — the next connection attempt on this executor succeeds — and later
/// connections to the same peer complete normally.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_failure_publishes_terminal_outcome(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.max_websocket_connections = 1;
            })),
            ..Default::default()
        },
    )
    .await?;

    let mut ws_server = ReconnectTestServer::start().await;
    let url = ws_server.url();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-terminal");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(url.clone()),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    assert_eq!(ws_server.accepted(), 1);
    assert_eq!(ws_server.completed_handshakes(), 1);

    let contention_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    wait_for_contention_receives_to_park(&executor, &worker_id).await?;

    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receives_incomplete(&oplog);

    // Withhold the retry's reconnect handshake at the real peer and reject
    // it: the decided call's live handshake fails, and the failure is
    // published terminally per handle.
    ws_server.arm_hold();
    executor.resume(&worker_id, false).await?;
    ws_server
        .wait_for_count(
            2,
            |event| matches!(event, ReconnectServerEvent::UpgradeRequestRead),
            "read the reconnect's upgrade request",
        )
        .await?;
    // The queued call's gate position is not observable from the peer; a
    // short settle lets it queue before the rejection.
    tokio::time::sleep(Duration::from_millis(500)).await;
    ws_server.reject_hold();

    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    assert_eq!(ws_server.handshakes_rejected(), 1);
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    for (side, payload) in [("receive", &result.0), ("receive-with-timeout", &result.1)] {
        assert!(
            payload.contains("Receive error") && payload.contains("ConnectionFailure"),
            "the {side} result must report the failed reconnect, got {payload:?}"
        );
    }
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    // The rejected connection never completed its handshake, and the queued
    // call never opened one of its own.
    assert_eq!(ws_server.accepted(), 2);
    assert_eq!(ws_server.completed_handshakes(), 1);
    assert_eq!(ws_server.upgrade_requests_read(), 2);
    assert_eq!(ws_server.clients_vanished_while_held(), 0);

    // The failed reconnect released the pool permit it briefly held: the pool
    // admits a new connection.
    let pool = executor
        .websocket_connection_pool()
        .expect("the executor's websocket connection pool is captured");
    let permit = tokio::time::timeout(Duration::from_secs(5), pool.acquire())
        .await
        .expect("the failed reconnect must not leak its pool permit")?;
    drop(permit);

    // A fresh connection to the same peer completes normally after the
    // rejection.
    let connect_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_result",
            data_value!(url.clone()),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected return value"))?;
    assert_eq!(
        connect_result,
        SchemaValue::Result(ResultValuePayload::Ok { value: None })
    );
    assert_eq!(ws_server.accepted(), 3);
    assert_eq!(ws_server.completed_handshakes(), 2);

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.stop().await?;

    Ok(())
}

/// The reconnect coordination is scoped per resource handle: two workers, each
/// with its own persisted websocket on its own peer, reconnect their own
/// handles concurrently. While both reconnect handshakes are withheld at their
/// own peers, each peer sees exactly one reconnect connection beyond its
/// initial one and no completed handshake beyond the initial one — neither
/// worker's queued call opens a connection of its own — and after both
/// releases both pairs of receives complete.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn websocket_reconnect_coordination_is_per_handle(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.max_websocket_connections = 2;
            })),
            ..Default::default()
        },
    )
    .await?;

    let mut ws_server_a = ReconnectTestServer::start().await;
    let mut ws_server_b = ReconnectTestServer::start().await;

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id_a = agent_id!("WebsocketTest", "ws-reconnect-per-handle-a");
    let agent_id_b = agent_id!("WebsocketTest", "ws-reconnect-per-handle-b");
    let worker_id_a = executor
        .start_agent_with(
            &component.id,
            agent_id_a.clone(),
            HashMap::new(),
            Vec::new(),
        )
        .await?;
    let worker_id_b = executor
        .start_agent_with(
            &component.id,
            agent_id_b.clone(),
            HashMap::new(),
            Vec::new(),
        )
        .await?;

    let first_a = executor
        .invoke_and_await_agent(
            &component,
            &agent_id_a,
            "connect_and_receive_first",
            data_value!(ws_server_a.url()),
        )
        .await?;
    assert_eq!(first_a.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    let first_b = executor
        .invoke_and_await_agent(
            &component,
            &agent_id_b,
            "connect_and_receive_first",
            data_value!(ws_server_b.url()),
        )
        .await?;
    assert_eq!(first_b.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    assert_eq!(ws_server_a.accepted(), 1);
    assert_eq!(ws_server_a.completed_handshakes(), 1);
    assert_eq!(ws_server_b.accepted(), 1);
    assert_eq!(ws_server_b.completed_handshakes(), 1);

    let contention_key_a = IdempotencyKey::fresh();
    let contention_key_b = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id_a,
            &contention_key_a,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id_b,
            &contention_key_b,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    wait_for_contention_receives_to_park(&executor, &worker_id_a).await?;
    wait_for_contention_receives_to_park(&executor, &worker_id_b).await?;

    executor.interrupt(&worker_id_a).await?;
    executor.interrupt(&worker_id_b).await?;
    executor
        .wait_for_status(
            &worker_id_a,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    executor
        .wait_for_status(
            &worker_id_b,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let oplog_a = executor
        .get_oplog(&worker_id_a, OplogIndex::INITIAL)
        .await?;
    assert_contention_receives_incomplete(&oplog_a);
    let oplog_b = executor
        .get_oplog(&worker_id_b, OplogIndex::INITIAL)
        .await?;
    assert_contention_receives_incomplete(&oplog_b);

    // Withhold both reconnect handshakes at their own peers.
    ws_server_a.arm_hold();
    ws_server_b.arm_hold();
    executor.resume(&worker_id_a, false).await?;
    executor.resume(&worker_id_b, false).await?;
    ws_server_a
        .wait_for_count(
            2,
            |event| matches!(event, ReconnectServerEvent::UpgradeRequestRead),
            "read worker a's reconnect upgrade request",
        )
        .await?;
    ws_server_b
        .wait_for_count(
            2,
            |event| matches!(event, ReconnectServerEvent::UpgradeRequestRead),
            "read worker b's reconnect upgrade request",
        )
        .await?;
    // The queued calls' gate positions are internal to the executor and
    // cannot be observed from the peers; a short settle lets them queue
    // before the holds are resolved.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Each handle saw exactly one decided reconnect and one queued peer: the
    // coordination neither starved nor merged the two handles.
    for (server, worker) in [(&ws_server_a, "a"), (&ws_server_b, "b")] {
        assert_eq!(
            server.accepted(),
            2,
            "the {worker} handle must see exactly one reconnect connection while its handshake is withheld"
        );
        assert_eq!(
            server.completed_handshakes(),
            1,
            "the {worker} handle's reconnect handshake must still be withheld"
        );
    }

    ws_server_a.release_hold();
    ws_server_b.release_hold();
    executor
        .wait_for_status(&worker_id_a, AgentStatus::Idle, Duration::from_secs(20))
        .await?;
    executor
        .wait_for_status(&worker_id_b, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    let result_a = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id_a,
            &contention_key_a,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(
        sorted_reconnect_payloads(&result_a),
        expected_reconnect_payloads()
    );
    let result_b = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id_b,
            &contention_key_b,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(
        sorted_reconnect_payloads(&result_b),
        expected_reconnect_payloads()
    );
    let oplog_a = executor
        .get_oplog(&worker_id_a, OplogIndex::INITIAL)
        .await?;
    assert_contention_receive_correlation(&oplog_a);
    let oplog_b = executor
        .get_oplog(&worker_id_b, OplogIndex::INITIAL)
        .await?;
    assert_contention_receive_correlation(&oplog_b);
    for server in [&ws_server_a, &ws_server_b] {
        assert_eq!(server.accepted(), 2);
        assert_eq!(server.completed_handshakes(), 2);
        assert_eq!(server.frames_sent(), expected_reconnect_frames_sent());
    }

    executor.check_oplog_is_queryable(&worker_id_a).await?;
    executor.check_oplog_is_queryable(&worker_id_b).await?;

    drop(executor);
    ws_server_a.stop().await?;
    ws_server_b.stop().await?;

    Ok(())
}

/// While the decided accessor call is parked acquiring the one-slot pool's
/// permit — the test itself holds the only permit — and its same-handle peer
/// is queued on the per-handle gate, an interrupt abandons both calls before
/// any reconnect connection is opened: no reconnect TCP connection appears,
/// the receives keep their original incomplete `Start`s with typed
/// `Interrupted` entries recorded, and after the test returns the permit the
/// retry after the resume reconnects on the same retained invocation and
/// completes both receives through the single pool slot: the capacity is
/// reused, not leaked.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn websocket_reconnect_coordination_is_interruptible_while_pending_on_pool_permit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.max_websocket_connections = 1;
            })),
            ..Default::default()
        },
    )
    .await?;

    let mut ws_server = ReconnectTestServer::start().await;
    let url = ws_server.url();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-interruptible-pool");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(url.clone()),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    assert_eq!(ws_server.accepted(), 1);
    assert_eq!(ws_server.completed_handshakes(), 1);

    let contention_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    wait_for_contention_receives_to_park(&executor, &worker_id).await?;

    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receives_incomplete(&oplog);

    // Take the pool's only permit from the captured pool before the retry:
    // the decided call parks acquiring it, before opening any connection. The
    // initial connection's permit returned to the pool when the interrupt
    // dropped the worker instance.
    let pool = executor
        .websocket_connection_pool()
        .expect("the executor's websocket connection pool is captured");
    let held_permit = tokio::time::timeout(Duration::from_secs(5), pool.acquire())
        .await
        .expect("the initial connection's permit must be back in the pool after the interrupt")?;

    executor.resume(&worker_id, false).await?;

    // The decided call's pool-park position inside the executor is not
    // directly observable — no connection is opened before the permit — and
    // its peer's gate position is internal as well; a short settle lets both
    // park before the interrupt.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // No reconnect connection was opened while the pool permit was
    // unavailable: the decided call parks on the pool and its peer queues on
    // the per-handle gate.
    assert_eq!(
        ws_server.accepted(),
        1,
        "no reconnect connection may be opened while the pool's only permit is held"
    );

    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receives_incomplete(&oplog);
    let contention_started = contention_invocation_started(&oplog)
        .ok_or_else(|| anyhow!("the contention invocation never started"))?;
    assert!(
        interrupted_entries_after(&oplog, contention_started) >= 2,
        "each of the two interrupts must record a typed Interrupted entry"
    );
    assert_eq!(
        ws_server.accepted(),
        1,
        "no reconnect connection may be opened while the pool's only permit is held"
    );

    // Return the permit and resume: the same retained invocation retries, the
    // decided call acquires the freed permit and the reconnect completes.
    drop(held_permit);
    executor.resume(&worker_id, false).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    // The retry reused the single pool slot exactly once: one reconnect
    // connection, and none of the interrupted reconnect attempts leaked or
    // lingered as held clients at the peer.
    assert_eq!(ws_server.accepted(), 2);
    assert_eq!(ws_server.completed_handshakes(), 2);
    assert_eq!(ws_server.clients_vanished_while_held(), 0);
    assert_eq!(ws_server.frames_sent(), expected_reconnect_frames_sent());

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.stop().await?;

    Ok(())
}

/// While the decided accessor call is parked on the real peer-held websocket
/// handshake and its same-handle peer is queued on the per-handle gate, an
/// interrupt abandons both calls: the peer observes the abandoned held
/// connection, the receives keep their original incomplete `Start`s with typed
/// `Interrupted` entries recorded, and the abandoned reconnect releases the
/// pool permit it held while parked on the handshake. The retry after the
/// resume reconnects on the same retained invocation and completes both
/// receives: the pool capacity is reused, not leaked.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn websocket_reconnect_coordination_is_interruptible_while_pending_on_handshake(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.max_websocket_connections = 1;
            })),
            ..Default::default()
        },
    )
    .await?;

    let mut ws_server = ReconnectTestServer::start().await;
    let url = ws_server.url();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-interruptible");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(url.clone()),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    assert_eq!(ws_server.accepted(), 1);
    assert_eq!(ws_server.completed_handshakes(), 1);

    let contention_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?;
    wait_for_contention_receives_to_park(&executor, &worker_id).await?;

    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receives_incomplete(&oplog);

    // Withhold the retry's reconnect handshake at the real peer: the decided
    // call parks inside its real `connect_async` handshake and its peer queues
    // on the per-handle gate.
    ws_server.arm_hold();
    executor.resume(&worker_id, false).await?;
    ws_server
        .wait_for_count(
            2,
            |event| matches!(event, ReconnectServerEvent::UpgradeRequestRead),
            "read the reconnect's upgrade request",
        )
        .await?;
    // The queued call's gate position is internal to the executor and cannot
    // be observed from the peer; a short settle lets it queue before the
    // interrupt.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(ws_server.accepted(), 2);
    assert_eq!(ws_server.completed_handshakes(), 1);

    // Interrupting abandons both parked calls without resolving the hold.
    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;

    // The abandoned client of the withheld handshake is observed at the peer.
    ws_server
        .wait_for_count(
            1,
            |event| matches!(event, ReconnectServerEvent::ClientVanishedWhileHeld),
            "observe the abandoned held connection",
        )
        .await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receives_incomplete(&oplog);
    let contention_started = contention_invocation_started(&oplog)
        .ok_or_else(|| anyhow!("the contention invocation never started"))?;
    assert!(
        interrupted_entries_after(&oplog, contention_started) >= 2,
        "each of the two interrupts must record a typed Interrupted entry"
    );

    // The abandoned reconnect released the pool permit it held while parked on
    // the withheld handshake.
    let pool = executor
        .websocket_connection_pool()
        .expect("the executor's websocket connection pool is captured");
    let permit = tokio::time::timeout(Duration::from_secs(5), pool.acquire())
        .await
        .expect("the interrupted reconnect must not leak its pool permit")?;
    drop(permit);

    // Resolve the abandoned hold and resume: the same retained invocation
    // retries, the reconnect completes on a fresh connection and both
    // receives return.
    ws_server.release_hold();
    executor.resume(&worker_id, false).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(
        sorted_reconnect_payloads(&result),
        expected_reconnect_payloads()
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    // The abandoned held connection never completed its handshake; the retry's
    // reconnect opened one fresh connection which completed, and the single
    // reconnected connection's greeting went to the receives.
    assert_eq!(ws_server.accepted(), 3);
    assert_eq!(ws_server.completed_handshakes(), 2);
    assert_eq!(ws_server.clients_vanished_while_held(), 1);
    assert_eq!(ws_server.frames_sent(), expected_reconnect_frames_sent());
    assert_eq!(ws_server.frames_received(), Vec::<String>::new());

    // Re-invoking with the same key replays the recorded result without any
    // new connection.
    let replayed = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &contention_key,
            CONTENTION_METHOD,
            data_value!(CONTENTION_TIMEOUT_MS),
        )
        .await?
        .into_typed::<(String, String)>()?;
    assert_eq!(replayed, result);
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_contention_receive_correlation(&oplog);
    assert_eq!(ws_server.accepted(), 3);
    assert_eq!(ws_server.completed_handshakes(), 2);

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.stop().await?;

    Ok(())
}

/// A direct `send` on a persisted handle reconstructed after an executor
/// restart reconnects through the same coordination as the accessors, and an
/// interrupt while its reconnect handshake is withheld at the real peer
/// abandons it: the peer observes the abandoned held connection, the send
/// keeps its original incomplete durable `Start` with a typed `Interrupted`
/// entry recorded, and the retry after the resume reconnects on the same
/// retained invocation, delivers the frame to the peer and keeps the send's
/// durable session identity.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_direct_send_is_interruptible_and_continues(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.max_websocket_connections = 1;
        })),
        ..Default::default()
    };
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;

    let mut ws_server = ReconnectTestServer::start().await;
    let url = ws_server.url();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-direct-send");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(url.clone()),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    assert_eq!(ws_server.accepted(), 1);
    assert_eq!(ws_server.completed_handshakes(), 1);

    // Crash the executor while the worker is idle: the persisted handle's
    // connection is lost, so the send below reconnects it on the fresh
    // executor.
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;

    // Withhold the send's reconnect handshake at the real peer: the send parks
    // inside its real `connect_async` handshake.
    ws_server.arm_hold();
    let send_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent_id,
            &send_key,
            "send_persisted_result",
            data_value!(DIRECT_SEND_MESSAGE),
        )
        .await?;
    // The peer reading the send's reconnect upgrade request is the
    // deterministic boundary proving the send's durable call is parked in its
    // real `connect_async` handshake: the `Start` of a live parked durable
    // call is not exposed by the oplog query API, and the interrupt below
    // commits it for the assertions that follow.
    ws_server
        .wait_for_count(
            2,
            |event| matches!(event, ReconnectServerEvent::UpgradeRequestRead),
            "read the send's reconnect upgrade request",
        )
        .await?;

    executor.interrupt(&worker_id).await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Interrupted,
            Duration::from_secs(10),
        )
        .await?;

    // The abandoned client of the withheld handshake is observed at the peer.
    ws_server
        .wait_for_count(
            1,
            |event| matches!(event, ReconnectServerEvent::ClientVanishedWhileHeld),
            "observe the abandoned held connection",
        )
        .await?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_single_websocket_call_incomplete(&oplog, WEBSOCKET_SEND_FUNCTION);
    let send_started = starts_of_websocket_function(&oplog, WEBSOCKET_SEND_FUNCTION)
        .first()
        .copied()
        .ok_or_else(|| anyhow!("the send's durable Start is missing"))?;
    assert!(
        interrupted_entries_after(&oplog, send_started) >= 1,
        "the interrupt must record a typed Interrupted entry after the send's durable Start"
    );

    // The pool captured from the fresh executor only proves permit accounting
    // on that executor; the retry below reconnecting through the single pool
    // slot — exactly one further connection — is the reuse evidence.

    // Resolve the abandoned hold and resume: the same retained invocation
    // retries, the send reconnects and delivers its frame to the peer.
    ws_server.release_hold();
    executor.resume(&worker_id, false).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(20))
        .await?;

    let send_result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &send_key,
            "send_persisted_result",
            data_value!(DIRECT_SEND_MESSAGE),
        )
        .await?;
    assert_eq!(
        send_result.into_return_value(),
        Some(SchemaValue::Result(ResultValuePayload::Ok { value: None }))
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    // The send is a direct call: its guest is blocked inside the host call, so
    // the completed call records one Start and one End with no
    // CompletionDelivered marker even though it spans the interrupt.
    assert_single_websocket_call_completed(&oplog, WEBSOCKET_SEND_FUNCTION);
    // The abandoned held connection never completed its handshake; the retry's
    // reconnect opened one fresh connection which completed, the peer
    // received the send's frame on it, and its greeting frames were sent.
    assert_eq!(ws_server.accepted(), 3);
    assert_eq!(ws_server.completed_handshakes(), 2);
    assert_eq!(ws_server.clients_vanished_while_held(), 1);
    assert_eq!(ws_server.frames_sent(), expected_reconnect_frames_sent());
    assert_eq!(
        ws_server.frames_received(),
        vec![DIRECT_SEND_MESSAGE.to_string()]
    );

    // Re-invoking with the same key replays the recorded result without any
    // new connection or frame.
    let replayed = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &send_key,
            "send_persisted_result",
            data_value!(DIRECT_SEND_MESSAGE),
        )
        .await?;
    assert_eq!(
        replayed.into_return_value(),
        Some(SchemaValue::Result(ResultValuePayload::Ok { value: None }))
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_single_websocket_call_completed(&oplog, WEBSOCKET_SEND_FUNCTION);
    assert_eq!(ws_server.accepted(), 3);
    assert_eq!(ws_server.completed_handshakes(), 2);
    assert_eq!(
        ws_server.frames_received(),
        vec![DIRECT_SEND_MESSAGE.to_string()]
    );

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.stop().await?;

    Ok(())
}

/// A direct `close` on a persisted handle reconstructed after an executor
/// restart reconnects through the same coordination as the accessors, closes
/// the reconnected connection with a websocket close frame the peer observes
/// and replies to, and publishes a terminal outcome: a later `receive` on the
/// same handle fails without opening any new connection — the terminalized
/// handle does not reconnect.
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn websocket_reconnect_direct_close_reconnects_and_terminalizes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.max_websocket_connections = 1;
        })),
        ..Default::default()
    };
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;

    let mut ws_server = ReconnectTestServer::start().await;
    let url = ws_server.url();

    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let agent_id = agent_id!("WebsocketTest", "ws-reconnect-direct-close");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    let first = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "connect_and_receive_first",
            data_value!(url.clone()),
        )
        .await?;
    assert_eq!(first.into_typed::<String>()?, RECONNECT_FIRST_MESSAGE);
    assert_eq!(ws_server.accepted(), 1);
    assert_eq!(ws_server.completed_handshakes(), 1);

    // Crash the executor while the worker is idle: the persisted handle's
    // connection is lost, so the close below reconnects it on the fresh
    // executor.
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;

    let close_key = IdempotencyKey::fresh();
    let close_result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &agent_id,
            &close_key,
            "close_persisted_result",
            data_value!(),
        )
        .await?;
    assert_eq!(
        close_result.into_return_value(),
        Some(SchemaValue::Result(ResultValuePayload::Ok { value: None }))
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_single_websocket_call_completed(&oplog, WEBSOCKET_CLOSE_FUNCTION);
    // The close reconnected on one fresh connection, and the peer observed
    // and replied to its websocket close frame.
    assert_eq!(ws_server.accepted(), 2);
    assert_eq!(ws_server.completed_handshakes(), 2);
    assert_eq!(ws_server.close_frames_received(), 1);
    assert_eq!(ws_server.frames_sent(), expected_reconnect_frames_sent());

    // A receive on the terminalized handle fails with the closed-connection
    // error and must not reconnect: no new connection is opened.
    let receive_result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "receive_next_from_persisted_result",
            data_value!(),
        )
        .await?;
    let message = get_result_error_string(receive_result);
    assert!(
        message.contains("Receive error") && message.contains("Closed"),
        "expected a closed-connection receive error, got {message:?}"
    );
    assert_eq!(
        ws_server.accepted(),
        2,
        "the terminal handle must not reconnect"
    );
    assert_eq!(ws_server.close_frames_received(), 1);
    assert_eq!(ws_server.frames_sent(), expected_reconnect_frames_sent());

    executor.check_oplog_is_queryable(&worker_id).await?;

    drop(executor);
    ws_server.stop().await?;

    Ok(())
}
