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
use axum::body::{Body, Bytes};
use axum::extract::{Path, Request, State};
use axum::response::Response;
use axum::routing::{get, post};
use futures::StreamExt;
use golem_common::agent_id;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::extraction::extract_component_metadata;
use golem_common::model::agent::{
    AgentMode, AgentTypeName, GolemUserPrincipal, OidcPrincipal, OwnerKind, Principal,
};
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::mcp_import::{McpImport, McpImportSource};
use golem_common::model::oplog::payload::types::{
    SerializableEntityBodyExecution, SerializableToolError, SerializableToolOperationTerminal,
    SerializableToolRpcError,
};
use golem_common::model::oplog::{
    OplogIndex, PublicAgentEntityKind, PublicAgentInvocation, PublicOplogEntry,
    PublicOplogEntryAttribution, PublicOplogEntryWithIndex,
};
use golem_common::model::tool::{
    CompiledToolBinding, ConfigKeyScope, HostToolId, RegisteredTool, SecretKeyScope,
    ToolBindingInput, ToolBindingOwner, ToolDeploymentState, ToolFilesystemAccess, ToolName,
    ToolProvisionConfig, ToolSource,
};
use golem_common::model::tool_middleware::{
    CompiledToolMiddlewareChain, CompiledToolMiddlewareOccurrence, RegisteredToolMiddleware,
    ToolMiddlewareInstallation, ToolMiddlewareName, ToolMiddlewareSource,
};
use golem_common::schema::tool::{OptionShape, ToolMiddleware, ToolMiddlewareScope};
use golem_common::schema::{
    BinaryRestrictions, BinaryValuePayload, FromSchema, SchemaGraph, SchemaType, SchemaValue,
    TypedSchemaValue, VariantValuePayload, build_input_record,
};
use golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache;
use golem_common::{
    data_value,
    model::{
        AgentInvocationResult, AgentStatus, IdempotencyKey, OwnedAgentId, PromiseId, RetryConfig,
    },
};
use golem_mcp_import::tool::{Limits, ProjectedTool};
use golem_service_base::model::mcp_import::McpImportObservation;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::durable_host::tool::{
    ToolAttachmentModeMetadata, ToolBodyAdmissionMetadata, ToolOperationLaneMetadata,
    ToolOperationMetadata, ToolOperationWinnerMetadata, ToolOwnerFailureMetadata,
};
use golem_worker_executor::services::environment_state::{
    EnvironmentStateService, ToolDiscoveryError,
};
use golem_worker_executor::worker::owner_lane::OwnerInvocationId;
use golem_worker_executor_test_utils::agent_deployments_service::TestEnvironmentStateService;
use golem_worker_executor_test_utils::{
    AgentInvocationSuccessGateHandle, LastUniqueId, PrecompiledComponent, ReplayAdmissionStage,
    TestContext, TestExecutorOverrides, TestWorkerExecutor, WorkerExecutorTestDependencies,
    native_streaming_tool_metadata, native_test_tool_metadata, start_with_overrides,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio_stream::wrappers::ReceiverStream;
use wasmtime::Engine;
use wasmtime::component::Component;

mod chunk_f_policy_acceptance;
mod chunk_m_stream_deltas;
mod gol40_k1_audit_acceptance;
mod matrix_client_resource_acceptance;
mod matrix_conformance;
mod middleware_acceptance;
mod moonbit_exports;
mod moonbit_sdk_rows_acceptance;
mod path_policy_acceptance;
mod rust_sdk_gol40_conformance;
mod trapped_leaf_observers;

static SINGLE_STORE_PROBE_PERMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
static ATTACHMENT_PRESSURE_TEST_PERMIT: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(1);

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);

inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_provider")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_caller")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("rate_limit_middleware")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("filesystem_tools")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("audit_middleware")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("javascript_tools")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("typescript_tools")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("git_tool")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("git_network_probe")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("web_fetch")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_ts_provider")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_ts_caller")]
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
inherit_test_dep!(
    #[tagged_as("tool_streaming_moonbit_lifecycle_gol40")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_effect_provider")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_effect_caller")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("large_dynamic_memory")]
    PrecompiledComponent
);

/// One-line structural description of a public oplog entry for test diagnostics.
fn describe_public_entry(entry: &PublicOplogEntry) -> String {
    match entry {
        PublicOplogEntry::Start(params) => format!(
            "Start {} parent={:?} owner={:?} type={:?}",
            params.function_name,
            params.parent_start_index,
            params.observational_owner,
            params.durable_function_type
        ),
        PublicOplogEntry::End(params) => format!(
            "End start={} response={}",
            params.start_index,
            if params.response.is_some() {
                "some"
            } else {
                "none"
            }
        ),
        PublicOplogEntry::Cancelled(params) => format!("Cancelled start={}", params.start_index),
        other => {
            let debug = format!("{other:?}");
            debug
                .split_once(['(', '{'])
                .map(|(name, _)| name.trim().to_string())
                .unwrap_or(debug)
        }
    }
}

/// Indices of the recorded `Start` entries before `boundary` whose terminal was recorded at or
/// after `boundary` (or not at all).
fn incomplete_starts_before(
    recorded: &[PublicOplogEntryWithIndex],
    boundary: OplogIndex,
) -> Vec<OplogIndex> {
    let mut open = BTreeMap::new();
    for entry in recorded.iter().filter(|entry| entry.oplog_index < boundary) {
        match &entry.entry {
            PublicOplogEntry::Start(_) => {
                open.insert(entry.oplog_index, ());
            }
            PublicOplogEntry::End(params) => {
                open.remove(&params.start_index);
            }
            PublicOplogEntry::Cancelled(params) => {
                open.remove(&params.start_index);
            }
            _ => {}
        }
    }
    open.into_keys().collect()
}

/// Which oplog prefix of the recorded single-Store probe history is retained before replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SingleStoreProbePrefix {
    /// The complete recorded history.
    Complete,
    /// The prefix ending with the first direct idempotency-key `Start` (its `End` is discarded).
    FirstDirectStart,
    /// The prefix ending with the first direct idempotency-key `End`; the following direct atomic
    /// region and all later guest calls are discarded.
    FirstDirectEnd,
}

/// Where the replayed consume-body admission is paused while the guest's direct calls proceed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SingleStoreProbeGate {
    None,
    Admission(ReplayAdmissionStage),
}

/// Records a single-Store history in which a host-side response body read (an accessor durable
/// call whose scope, `Start` and `End` are appended by a host task) interleaves with direct,
/// Store-holding guest durable calls (an idempotency key, then nested atomic regions with keys
/// and oplog index reads), reverts the worker to the requested prefix, and replays it on a fresh
/// executor while the replayed body admission is optionally paused at `gate` until the direct
/// call has started waiting for its recorded resolution.
///
/// A correct replay must neither consume the body scope or body `Start` for a positional direct
/// read, nor let the Store-holding direct call wait for progress that only the paused body task
/// can make.
async fn run_single_store_http_atomic_probe(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    caller: &PrecompiledComponent,
    agent_name: &str,
    atomic_first: bool,
    prefix: SingleStoreProbePrefix,
    gate: SingleStoreProbeGate,
) -> anyhow::Result<()> {
    use golem_common::model::worker::{RevertToOplogIndex, RevertWorkerTarget};

    let _single_store_probe_permit = SINGLE_STORE_PROBE_PERMIT
        .acquire()
        .await
        .expect("single-store probe semaphore is not closed");
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let (port, gate_port, server, mut checkpoints) = start_crash_checkpoint_server().await;
    let mut env = HashMap::from([
        ("CALLER_CRASH_CHECKPOINT_PORT".to_string(), port.to_string()),
        (
            "CALLER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
            gate_port.to_string(),
        ),
    ]);
    if atomic_first {
        env.insert("GOL581_ATOMIC_FIRST".to_string(), "1".to_string());
    }
    // The shape under test has the host-side body scope complete before the guest's first
    // direct call, so that the gates below (not the recording) decide how the two interleave on
    // replay. The fixture only yields between dropping the body and the first direct call; a
    // body task that is still waiting on an oplog commit when the yields run out records its
    // remaining entries after the direct `Start`. The guest cannot wait for the scope itself
    // (awaiting the trailers replays identically and would park behind the paused body
    // admission), so the recording is checked against the precondition and repeated on a fresh
    // agent when it did not produce the shape. This re-records before any replay; it never
    // retries a replay.
    const RECORDING_ATTEMPTS: usize = 3;
    let mut attempt = 0;
    let (agent, worker_id, recorded, key_start, key_end) = loop {
        attempt += 1;
        let agent_name = if attempt == 1 {
            agent_name.to_string()
        } else {
            format!("{agent_name}-recording-{attempt}")
        };
        let agent = agent_id!("ToolStreamingCaller", agent_name);
        let worker_id = executor
            .start_agent_with(&component.id, agent.clone(), env.clone(), Vec::new())
            .await?;
        let invocation = executor.invoke_and_await_agent(
            &component,
            &agent,
            "single_store_http_atomic_probe",
            data_value!(),
        );
        let release = async {
            for ordinal in 1..=8 {
                let checkpoint =
                    next_crash_checkpoint(&mut checkpoints, "single-store-http-atomic")
                        .await
                        .map_err(|error| {
                            anyhow::anyhow!(
                                "recording attempt {attempt} checkpoint {ordinal}/8 failed: {error}"
                            )
                        })?;
                checkpoint.release.send(()).map_err(|_| {
                    anyhow::anyhow!(
                        "recording attempt {attempt} checkpoint {ordinal}/8 gate was dropped"
                    )
                })?;
            }
            anyhow::Ok(())
        };
        let (_result, ()) = tokio::try_join!(invocation, release)?;
        let recorded = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        for entry in &recorded {
            eprintln!(
                "SINGLE_STORE_PROBE_RECORDED {} {}",
                entry.oplog_index,
                describe_public_entry(&entry.entry)
            );
        }
        assert!(
            !recorded.iter().any(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::Start(params) if params.function_name == "golem::entity::invoke"
            )),
            "the probe must not involve entity Stores"
        );
        let key_start = recorded
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::api::generate_idempotency-key" =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .ok_or_else(|| {
                anyhow::anyhow!("recorded direct idempotency-key Start was not found")
            })?;
        let key_end = recorded
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::End(params) if params.start_index == key_start => {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("recorded direct idempotency-key End was not found"))?;
        let body_read_before_key = recorded.iter().any(|entry| {
            entry.oplog_index < key_start
                && matches!(&entry.entry, PublicOplogEntry::Start(params)
                    if params.function_name == "http::types::response::consume-body")
        });
        let incomplete_before_key = incomplete_starts_before(&recorded, key_start);
        if body_read_before_key && incomplete_before_key.is_empty() {
            break (agent, worker_id, recorded, key_start, key_end);
        }
        eprintln!(
            "SINGLE_STORE_PROBE_RECORDING attempt {attempt}: the body scope was not complete before the first direct idempotency-key Start {key_start} (body read recorded before it: {body_read_before_key}, incomplete Starts {incomplete_before_key:?}); recording again on a fresh agent"
        );
        if attempt == RECORDING_ATTEMPTS {
            return Err(anyhow::anyhow!(
                "the single-Store probe recording did not complete the body scope before the first direct call in {RECORDING_ATTEMPTS} attempts"
            ));
        }
    };
    let retained = match prefix {
        SingleStoreProbePrefix::Complete => None,
        SingleStoreProbePrefix::FirstDirectStart => Some(key_start),
        SingleStoreProbePrefix::FirstDirectEnd => Some(key_end),
    };
    if let Some(retained) = retained {
        executor
            .revert(
                &worker_id,
                RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                    last_oplog_index: retained,
                }),
            )
            .await?;
    }
    drop(executor);

    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let mut admission_gate = match gate {
        SingleStoreProbeGate::None => None,
        SingleStoreProbeGate::Admission(stage) => {
            Some(executor.gate_next_replay_access_admission(&worker_id, "consume-body", stage))
        }
    };
    // The first direct call after the body read is either a strictly matched idempotency key or
    // the positional atomic-region marker; the signal proves that it waits for replay progress
    // while the body admission is paused.
    let first_direct_call = if atomic_first {
        "BeginAtomicRegion"
    } else {
        "generate_idempotency-key"
    };
    let mut direct_wait = executor.signal_next_direct_replay_wait(&worker_id, first_direct_call);
    let replay = async {
        let invoke = executor.invoke_and_await_agent(
            &component,
            &agent,
            "record_native_order",
            data_value!("R"),
        );
        let coordinate = async {
            if let Some(admission_gate) = admission_gate.as_mut() {
                tokio::time::timeout(std::time::Duration::from_secs(30), admission_gate.entered())
                    .await
                    .map_err(|_| {
                        anyhow::anyhow!("replayed consume-body admission was not reached")
                    })?;
                let (function, start_index) =
                    tokio::time::timeout(std::time::Duration::from_secs(30), direct_wait.fired())
                        .await
                        .map_err(|_| {
                            anyhow::anyhow!(
                                "the guest's first direct call `{first_direct_call}` did not reach its replay wait while consume-body admission was paused"
                            )
                        })?;
                eprintln!(
                    "direct replay wait entered for `{function}` at {start_index} while consume-body admission is paused"
                );
                // The direct call holds the Store while waiting; the body task can only be
                // admitted once the direct call finished, so releasing the gate here does not
                // change the interleaving under test.
                admission_gate.release();
            }
            if retained.is_some() {
                // The retained prefix cuts the probe invocation, so its remainder re-executes
                // live and reaches the crash checkpoint gate for every remaining checkpoint.
                for ordinal in 1..=8 {
                    next_crash_checkpoint(&mut checkpoints, "single-store-http-atomic")
                        .await
                        .map_err(|error| {
                            anyhow::anyhow!(
                                "replay checkpoint {ordinal}/8 failed for retained prefix {prefix:?}: {error}"
                            )
                        })?
                        .release
                        .send(())
                        .map_err(|_| {
                            anyhow::anyhow!(
                                "replay checkpoint {ordinal}/8 gate was dropped for retained prefix {prefix:?}"
                            )
                        })?;
                }
            }
            anyhow::Ok(())
        };
        let (result, ()) = tokio::try_join!(invoke, coordinate)?;
        Ok::<_, anyhow::Error>(result)
    };
    let replay = tokio::time::timeout(std::time::Duration::from_secs(60), replay).await;
    server.abort();
    let replay = match replay {
        Ok(replay) => replay,
        Err(_) => {
            for entry in executor.get_oplog(&worker_id, OplogIndex::INITIAL).await? {
                eprintln!(
                    "SINGLE_STORE_PROBE_TIMEOUT_OPLOG {} {}",
                    entry.oplog_index,
                    describe_public_entry(&entry.entry)
                );
            }
            return Err(anyhow::anyhow!(
                "replay of the single-Store probe history did not make progress"
            ));
        }
    };
    let output: String = replay?.into_typed()?;
    assert_eq!(output, "R");
    assert!(
        checkpoints.try_recv().is_err(),
        "replay must not repeat a recorded checkpoint"
    );
    let replayed = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let last_recorded = recorded
        .last()
        .map(|entry| entry.oplog_index)
        .ok_or_else(|| anyhow::anyhow!("the recorded history is empty"))?;
    let is_key_start = |entry: &PublicOplogEntry| {
        matches!(
            entry,
            PublicOplogEntry::Start(params)
                if params.function_name == "golem::api::generate_idempotency-key"
        )
    };
    let recorded_keys = recorded
        .iter()
        .filter(|entry| is_key_start(&entry.entry))
        .count();
    let re_executed_keys = replayed
        .iter()
        .filter(|entry| entry.oplog_index > last_recorded && is_key_start(&entry.entry))
        .count();
    let expected_re_executed = match prefix {
        SingleStoreProbePrefix::Complete => 0,
        // The retained first key keeps its recorded `Start` (reused when incomplete, replayed
        // when complete); every discarded key re-executes with a fresh `Start`.
        SingleStoreProbePrefix::FirstDirectStart | SingleStoreProbePrefix::FirstDirectEnd => {
            recorded_keys - 1
        }
    };
    assert_eq!(
        re_executed_keys, expected_re_executed,
        "exactly the discarded direct calls re-execute after replaying the retained prefix"
    );
    let is_body_read = |entry: &PublicOplogEntry| {
        matches!(
            entry,
            PublicOplogEntry::Start(params)
                if params.function_name == "http::types::response::consume-body"
        )
    };
    let recorded_body_starts = recorded
        .iter()
        .filter(|entry| is_body_read(&entry.entry))
        .count();
    let re_executed_body_starts = replayed
        .iter()
        .filter(|entry| entry.oplog_index > last_recorded && is_body_read(&entry.entry))
        .count();
    let (retained_body_starts, discarded_checkpoints) = match retained {
        None => (recorded_body_starts, 0),
        Some(retained) => {
            let retained_body_starts = recorded
                .iter()
                .filter(|entry| entry.oplog_index <= retained && is_body_read(&entry.entry))
                .count();
            let retained_checkpoints = recorded
                .iter()
                .filter(|entry| {
                    entry.oplog_index <= retained
                        && matches!(&entry.entry, PublicOplogEntry::Start(params)
                            if params.function_name == "http::client::send")
                })
                .count();
            let recorded_checkpoints = recorded
                .iter()
                .filter(|entry| {
                    matches!(&entry.entry, PublicOplogEntry::Start(params)
                        if params.function_name == "http::client::send")
                })
                .count();
            (
                retained_body_starts,
                recorded_checkpoints - retained_checkpoints,
            )
        }
    };
    // Every retained body read keeps its recorded `Start` (completed reads replay, incomplete
    // ones complete the existing `Start` live); only the checkpoints discarded by the cut open
    // new body reads.
    let discarded_body_starts = recorded_body_starts - retained_body_starts;
    assert_eq!(
        re_executed_body_starts, discarded_body_starts,
        "no retained body read may repeat its completed effect with a fresh Start (retained {retained_body_starts} of {recorded_body_starts} body Starts, {discarded_checkpoints} discarded checkpoints)"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn single_store_body_read_and_direct_calls_replay_complete_history(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_single_store_http_atomic_probe(
        last_unique_id,
        deps,
        caller,
        "single-store-complete-control",
        false,
        SingleStoreProbePrefix::Complete,
        SingleStoreProbeGate::None,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn direct_call_waits_without_blocking_body_admission_paused_before_scope(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_single_store_http_atomic_probe(
        last_unique_id,
        deps,
        caller,
        "single-store-direct-before-scope",
        false,
        SingleStoreProbePrefix::Complete,
        SingleStoreProbeGate::Admission(ReplayAdmissionStage::BeforeScope),
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn direct_call_waits_without_blocking_body_admission_paused_after_scope(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_single_store_http_atomic_probe(
        last_unique_id,
        deps,
        caller,
        "single-store-direct-after-scope",
        false,
        SingleStoreProbePrefix::Complete,
        SingleStoreProbeGate::Admission(ReplayAdmissionStage::AfterScope),
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn positional_atomic_marker_does_not_consume_unclaimed_body_scope_start(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_single_store_http_atomic_probe(
        last_unique_id,
        deps,
        caller,
        "single-store-atomic-before-scope",
        true,
        SingleStoreProbePrefix::Complete,
        SingleStoreProbeGate::Admission(ReplayAdmissionStage::BeforeScope),
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn positional_atomic_marker_does_not_consume_unclaimed_body_start(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_single_store_http_atomic_probe(
        last_unique_id,
        deps,
        caller,
        "single-store-atomic-after-scope",
        true,
        SingleStoreProbePrefix::Complete,
        SingleStoreProbeGate::Admission(ReplayAdmissionStage::AfterScope),
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn retained_prefix_ending_at_direct_start_replays_with_paused_body_admission(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_single_store_http_atomic_probe(
        last_unique_id,
        deps,
        caller,
        "single-store-prefix-direct-start",
        false,
        SingleStoreProbePrefix::FirstDirectStart,
        SingleStoreProbeGate::Admission(ReplayAdmissionStage::AfterScope),
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn retained_prefix_ending_at_direct_end_replays_with_paused_body_admission(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_single_store_http_atomic_probe(
        last_unique_id,
        deps,
        caller,
        "single-store-prefix-direct-end",
        false,
        SingleStoreProbePrefix::FirstDirectEnd,
        SingleStoreProbeGate::Admission(ReplayAdmissionStage::AfterScope),
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn retained_prefix_ending_at_direct_start_replays_under_natural_scheduling(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_single_store_http_atomic_probe(
        last_unique_id,
        deps,
        caller,
        "single-store-prefix-direct-start-control",
        false,
        SingleStoreProbePrefix::FirstDirectStart,
        SingleStoreProbeGate::None,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn retained_prefix_ending_at_direct_end_replays_under_natural_scheduling(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_single_store_http_atomic_probe(
        last_unique_id,
        deps,
        caller,
        "single-store-prefix-direct-end-control",
        false,
        SingleStoreProbePrefix::FirstDirectEnd,
        SingleStoreProbeGate::None,
    )
    .await
}

#[derive(Debug, FromSchema)]
struct StreamEvidence {
    output: Vec<u8>,
    chunks_read: u32,
    bytes_read: u64,
    output_closed: bool,
    completion: String,
}

#[derive(Debug, FromSchema)]
struct RedactionEvidence {
    output: Vec<u8>,
    stdout_terminal: String,
    stderr: Vec<u8>,
    stderr_terminal: String,
    outcome: String,
}

#[derive(Debug, FromSchema)]
struct ClockedStreamEvidence {
    before_tool_nanos: u64,
    after_tool_nanos: u64,
    stream: StreamEvidence,
}

#[derive(Debug, FromSchema)]
struct CliToolEvidence {
    exit_code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq, FromSchema)]
struct TypedOutputEvidence {
    label: String,
    ordinal: u32,
}

#[derive(Debug, PartialEq, Eq, FromSchema)]
struct TypedInputEvidence {
    label: String,
    ordinal: u32,
}

#[derive(Debug, FromSchema)]
struct TsStreamEvidence {
    output: Vec<u8>,
    bytes_read: u64,
}

#[derive(Debug, FromSchema)]
struct TsCompletionEvidence {
    output: Vec<u8>,
    stdout_terminal: String,
    result_terminal: String,
}

#[derive(Debug, FromSchema)]
struct TsDualOutputEvidence {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    result_terminal: String,
}

#[derive(Debug, FromSchema)]
struct ScalaStreamEvidence {
    output: String,
    bytes_read: i64,
}

#[derive(Debug, FromSchema)]
struct ScalaCleanupEvidence {
    error: String,
    stdin_cancelled: bool,
    stdout_terminal: String,
}

#[derive(Debug, FromSchema)]
struct ScalaOutputEvidence {
    bytes: Vec<i32>,
    terminal: String,
    result: String,
}

#[derive(Debug, FromSchema)]
struct ScalaDualOutputEvidence {
    stdout: Vec<i32>,
    stderr: Vec<i32>,
    result: String,
}

#[derive(Debug, FromSchema)]
struct MoonBitDualOutputEvidence {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    result: String,
}

#[derive(Debug, PartialEq, Eq, FromSchema)]
struct SecretPolicyObservation {
    label: String,
    config_resolved: bool,
    configured_secret_revealed: bool,
    input_secret_revealed: bool,
}

#[derive(Debug, PartialEq, Eq, FromSchema)]
struct SecretPolicyEvidence {
    middleware: Vec<SecretPolicyObservation>,
    leaf_revealed: bool,
}

pub(crate) fn deployment_state(
    owner_account_id: AccountId,
    provider_component_id: golem_common::model::component::ComponentId,
    provider_revision: ComponentRevision,
    provider_component_name: &str,
    caller_agent_type: &str,
    definitions: Vec<golem_common::schema::tool::Tool>,
) -> ToolDeploymentState {
    let deployment_revision = DeploymentRevision::try_from(1_u64).unwrap();
    let account_email = AccountEmail::new("test@golem");
    let mut registered_tools = BTreeMap::new();

    for definition in definitions {
        let root_name = definition
            .commands
            .nodes
            .first()
            .expect("tool definition has a root command")
            .name
            .clone();
        let name = ToolName::try_from(root_name).expect("valid discovered tool name");
        registered_tools.insert(
            name,
            RegisteredTool {
                deployment_revision,
                release_id: None,
                metadata_digest: Default::default(),
                definition,
                provision: ToolProvisionConfig::default(),
                component_bindings: Default::default(),
                source: ToolSource::Component {
                    component_id: provider_component_id,
                    component_revision: provider_revision,
                    component_name: ComponentName(provider_component_name.to_string()),
                },
                owner_account_id,
                owner_account_email: account_email.clone(),
                metadata_version: "0.1.0".to_string(),
            },
        );
    }

    let agent_type = AgentTypeName(caller_agent_type.to_string());
    let bindings = registered_tools
        .iter()
        .map(|(name, tool)| {
            let filesystem_access = if name.as_str() == "capable-streaming" {
                ToolFilesystemAccess::Allowed
            } else {
                ToolFilesystemAccess::Unset
            };
            (
                name.clone(),
                CompiledToolBinding {
                    deployment_revision,
                    release_id: tool.release_id,
                    metadata_digest: tool.metadata_digest,
                    owner: ToolBindingOwner::AgentType {
                        agent_type_name: agent_type.clone(),
                    },
                    tool_name: name.clone(),
                    version: tool.definition.version.clone(),
                    metadata_version: tool.metadata_version.clone(),
                    account_id: owner_account_id,
                    account_email: account_email.clone(),
                    parameters: NormalizedJsonValue::new(serde_json::json!({})),
                    config_keys_readable: Default::default(),
                    secret_keys_readable: SecretKeyScope::All,
                    secret_keys_revealable: SecretKeyScope::All,
                    filesystem_access,
                    source: tool.source.clone(),
                },
            )
        })
        .collect();

    let owner = ToolBindingOwner::AgentType {
        agent_type_name: agent_type,
    };
    ToolDeploymentState {
        deployment_revision,
        registered_tools,
        tool_bindings: BTreeMap::from([(owner, bindings)]),
        mcp_imports: Vec::new(),
        tool_middleware_configuration: Default::default(),
        registered_tool_middlewares: BTreeMap::new(),
        tool_middleware_chains: BTreeMap::new(),
    }
}

fn filesystem_tool_input(fields: Vec<(&str, SchemaType, SchemaValue)>) -> TypedSchemaValue {
    TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(
            fields
                .iter()
                .map(|(name, body, _)| golem_common::schema::NamedFieldType {
                    name: (*name).to_string(),
                    body: body.clone(),
                    metadata: Default::default(),
                })
                .collect(),
        )),
        SchemaValue::Record {
            fields: fields.into_iter().map(|(_, _, value)| value).collect(),
        },
    )
}

fn filesystem_optional_line(line: Option<u64>) -> (SchemaType, SchemaValue) {
    (
        SchemaType::option(SchemaType::u64()),
        SchemaValue::Option {
            inner: line.map(|line| Box::new(SchemaValue::U64(line))),
        },
    )
}

fn filesystem_cursor_none() -> (SchemaType, SchemaValue) {
    (
        SchemaType::option(SchemaType::record(vec![
            golem_common::schema::NamedFieldType {
                name: "byte-offset".to_string(),
                body: SchemaType::u64(),
                metadata: Default::default(),
            },
            golem_common::schema::NamedFieldType {
                name: "line".to_string(),
                body: SchemaType::u64(),
                metadata: Default::default(),
            },
        ])),
        SchemaValue::Option { inner: None },
    )
}

async fn invoke_filesystem_tool(
    executor: &TestWorkerExecutor,
    worker_id: &golem_common::model::AgentId,
    fingerprint: golem_common::model::AgentFingerprint,
    principal: Principal,
    definitions: &BTreeMap<ToolName, golem_common::schema::tool::Tool>,
    tool_name: &str,
    input: TypedSchemaValue,
) -> anyhow::Result<
    Result<
        Option<SchemaValue>,
        golem_common::model::oplog::payload::types::SerializableToolRpcError,
    >,
> {
    let tool_name = ToolName::try_from(tool_name).unwrap();
    let definition = &definitions[&tool_name];
    let command_index = definition
        .command_index_by_path(&[])
        .expect("filesystem tool command exists");
    let input_schema = definition.canonical_input_record_schema(command_index)?;
    let (provided_schema, provided_value) = input.into_parts();
    let SchemaType::Record {
        fields: provided_fields,
        ..
    } = provided_schema.root
    else {
        anyhow::bail!("test tool input schema must be a record");
    };
    let SchemaValue::Record {
        fields: provided_values,
    } = provided_value
    else {
        anyhow::bail!("test tool input value must be a record");
    };
    anyhow::ensure!(
        provided_fields.len() == provided_values.len(),
        "test tool input schema/value field counts differ"
    );
    let mut values_by_name = BTreeMap::new();
    for (field, value) in provided_fields.into_iter().zip(provided_values) {
        anyhow::ensure!(
            values_by_name.insert(field.name.clone(), value).is_none(),
            "duplicate test tool input field '{}'",
            field.name
        );
    }
    let SchemaType::Record {
        fields: canonical_fields,
        ..
    } = &input_schema.root
    else {
        anyhow::bail!("canonical tool input schema must be a record");
    };
    let mut canonical_values = Vec::with_capacity(canonical_fields.len());
    for field in canonical_fields {
        canonical_values.push(
            values_by_name
                .remove(&field.name)
                .ok_or_else(|| anyhow::anyhow!("missing test tool input field '{}'", field.name))?,
        );
    }
    anyhow::ensure!(
        values_by_name.is_empty(),
        "unexpected test tool input fields: {}",
        values_by_name
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    let input_value = SchemaValue::Record {
        fields: canonical_values,
    };
    let output = executor
        .invoke_external_tool(
            worker_id,
            fingerprint,
            IdempotencyKey::fresh(),
            tool_name,
            Vec::new(),
            TypedSchemaValue::new(input_schema, input_value),
            InvocationContextStack::fresh(),
            principal,
            None,
        )
        .await?;
    let AgentInvocationResult::ExternalTool { result } = output.result else {
        anyhow::bail!("expected external tool result, got {output:?}");
    };
    Ok(result.map(|result| result.result.map(|value| value.into_parts().1)))
}

async fn invoke_filesystem_tool_success(
    executor: &TestWorkerExecutor,
    worker_id: &golem_common::model::AgentId,
    fingerprint: golem_common::model::AgentFingerprint,
    principal: Principal,
    definitions: &BTreeMap<ToolName, golem_common::schema::tool::Tool>,
    tool_name: &str,
    input: TypedSchemaValue,
) -> anyhow::Result<SchemaValue> {
    match invoke_filesystem_tool(
        executor,
        worker_id,
        fingerprint,
        principal,
        definitions,
        tool_name,
        input,
    )
    .await?
    {
        Ok(Some(value)) => Ok(value),
        Ok(None) => anyhow::bail!("filesystem tool '{tool_name}' returned no value"),
        Err(error) => anyhow::bail!("filesystem tool '{tool_name}' failed: {error:?}"),
    }
}

async fn invoke_cli_tool_version(
    executor: &TestWorkerExecutor,
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    environment_state: &TestEnvironmentStateService,
    caller_component: &golem_common::model::component::ComponentDto,
    provider: &PrecompiledComponent,
    package_name: &str,
    tool_name: &str,
    expected_version: &str,
    expected_stdout: &str,
) -> anyhow::Result<()> {
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        package_name,
        "ToolStreamingCaller",
        metadata.tools,
    );
    for bindings in deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.filesystem_access = ToolFilesystemAccess::Allowed;
        }
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let evidence: CliToolEvidence = executor
        .invoke_and_await_agent(
            caller_component,
            &agent_id!("ToolStreamingCaller", format!("{tool_name}-version")),
            "builtin_cli",
            data_value!(tool_name, "/workspace", vec!["--version"]),
        )
        .await?
        .into_typed()?;
    assert_eq!(evidence.exit_code, 0);
    assert!(evidence.stderr.is_empty());
    let stdout = String::from_utf8(evidence.stdout)?;
    if expected_version.is_empty() {
        assert!(stdout.starts_with('v'));
        assert!(stdout.ends_with('\n'));
    } else {
        assert_eq!(stdout, expected_stdout);
    }
    Ok(())
}

async fn invoke_cli_tool(
    executor: &TestWorkerExecutor,
    caller_component: &golem_common::model::component::ComponentDto,
    agent_name: &str,
    tool_name: &str,
    cwd: &str,
    args: Vec<&str>,
) -> anyhow::Result<CliToolEvidence> {
    executor
        .invoke_and_await_agent(
            caller_component,
            &agent_id!("ToolStreamingCaller", agent_name),
            "builtin_cli",
            data_value!(tool_name, cwd, args),
        )
        .await?
        .into_typed()
}

async fn invoke_git_tool_success(
    executor: &TestWorkerExecutor,
    worker_id: &golem_common::model::AgentId,
    fingerprint: golem_common::model::AgentFingerprint,
    principal: Principal,
    definition: &golem_common::schema::tool::Tool,
    command: &str,
    fields: BTreeMap<&str, SchemaValue>,
) -> anyhow::Result<Option<SchemaValue>> {
    invoke_git_tool_success_with_key(
        executor,
        worker_id,
        fingerprint,
        IdempotencyKey::fresh(),
        principal,
        definition,
        command,
        fields,
    )
    .await
}

async fn invoke_git_tool_success_with_key(
    executor: &TestWorkerExecutor,
    worker_id: &golem_common::model::AgentId,
    fingerprint: golem_common::model::AgentFingerprint,
    idempotency_key: IdempotencyKey,
    principal: Principal,
    definition: &golem_common::schema::tool::Tool,
    command: &str,
    fields: BTreeMap<&str, SchemaValue>,
) -> anyhow::Result<Option<SchemaValue>> {
    let command_path = vec![command.to_string()];
    let command_index = definition
        .command_index_by_path(&command_path)
        .ok_or_else(|| anyhow::anyhow!("git command '{command}' does not exist"))?;
    let model = definition.canonical_input_model(command_index)?;
    let schema = model.record_schema.clone();
    let values = model
        .fields
        .iter()
        .map(|field| {
            fields
                .get(field.name.as_str())
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing canonical git field '{}'", field.name))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let output = executor
        .invoke_external_tool(
            worker_id,
            fingerprint,
            idempotency_key,
            ToolName::try_from("git").unwrap(),
            command_path,
            TypedSchemaValue::new(schema, SchemaValue::Record { fields: values }),
            InvocationContextStack::fresh(),
            principal,
            None,
        )
        .await?;
    let AgentInvocationResult::ExternalTool { result } = output.result else {
        anyhow::bail!("expected git tool result, got {output:?}");
    };
    match result {
        Ok(result) => Ok(result.result.map(|value| value.into_parts().1)),
        Err(error) => anyhow::bail!("git {command} failed: {error:?}"),
    }
}

fn optional_string(value: Option<&str>) -> SchemaValue {
    SchemaValue::Option {
        inner: value.map(|value| Box::new(SchemaValue::String(value.to_string()))),
    }
}

fn string_list(values: &[&str]) -> SchemaValue {
    SchemaValue::List {
        elements: values
            .iter()
            .map(|value| SchemaValue::String((*value).to_string()))
            .collect(),
    }
}

fn assert_filesystem_tool_error(
    result: Result<Option<SchemaValue>, SerializableToolRpcError>,
    expected_name: &str,
) -> anyhow::Result<()> {
    match result {
        Err(SerializableToolRpcError::RemoteToolError(error)) => match error.as_ref() {
            SerializableToolError::CustomError(error) if error.name == expected_name => Ok(()),
            other => {
                anyhow::bail!("expected filesystem tool error '{expected_name}', got {other:?}")
            }
        },
        other => anyhow::bail!("expected filesystem tool error '{expected_name}', got {other:?}"),
    }
}

fn optional_u64(value: Option<u64>) -> (SchemaType, SchemaValue) {
    (
        SchemaType::option(SchemaType::u64()),
        SchemaValue::Option {
            inner: value.map(|value| Box::new(SchemaValue::U64(value))),
        },
    )
}

fn optional_u32(value: Option<u32>) -> (SchemaType, SchemaValue) {
    (
        SchemaType::option(SchemaType::u32()),
        SchemaValue::Option {
            inner: value.map(|value| Box::new(SchemaValue::U32(value))),
        },
    )
}

fn optional_bool(value: Option<bool>) -> (SchemaType, SchemaValue) {
    (
        SchemaType::option(SchemaType::bool()),
        SchemaValue::Option {
            inner: value.map(|value| Box::new(SchemaValue::Bool(value))),
        },
    )
}

fn web_fetch_input(
    url: String,
    timeout_ms: Option<u64>,
    max_response_bytes: Option<u64>,
    max_redirects: Option<u32>,
    convert_html_to_text: Option<bool>,
) -> TypedSchemaValue {
    let timeout_ms = optional_u64(timeout_ms);
    let max_response_bytes = optional_u64(max_response_bytes);
    let max_redirects = optional_u32(max_redirects);
    let convert_html_to_text = optional_bool(convert_html_to_text);
    filesystem_tool_input(vec![
        ("url", SchemaType::string(), SchemaValue::String(url)),
        ("timeout-ms", timeout_ms.0, timeout_ms.1),
        (
            "max-response-bytes",
            max_response_bytes.0,
            max_response_bytes.1,
        ),
        ("max-redirects", max_redirects.0, max_redirects.1),
        (
            "convert-html-to-text",
            convert_html_to_text.0,
            convert_html_to_text.1,
        ),
    ])
}

async fn invoke_web_fetch(
    executor: &TestWorkerExecutor,
    worker_id: &golem_common::model::AgentId,
    fingerprint: golem_common::model::AgentFingerprint,
    principal: Principal,
    definition: &golem_common::schema::tool::Tool,
    idempotency_key: IdempotencyKey,
    input: TypedSchemaValue,
) -> anyhow::Result<
    Result<
        Option<SchemaValue>,
        golem_common::model::oplog::payload::types::SerializableToolRpcError,
    >,
> {
    let command_index = definition
        .command_index_by_path(&[])
        .expect("web-fetch root command exists");
    let input_schema = definition.canonical_input_record_schema(command_index)?;
    let (_, input_value) = input.into_parts();
    let output = executor
        .invoke_external_tool(
            worker_id,
            fingerprint,
            idempotency_key,
            ToolName::try_from("web-fetch").unwrap(),
            Vec::new(),
            TypedSchemaValue::new(input_schema, input_value),
            InvocationContextStack::fresh(),
            principal,
            None,
        )
        .await?;
    let AgentInvocationResult::ExternalTool { result } = output.result else {
        anyhow::bail!("expected web-fetch external tool result, got {output:?}");
    };
    Ok(result.map(|result| result.result.map(|value| value.into_parts().1)))
}

fn expect_web_fetch_success(
    result: Result<Option<SchemaValue>, SerializableToolRpcError>,
) -> anyhow::Result<(String, u16, Option<String>, String, bool)> {
    let Ok(Some(SchemaValue::Record { fields })) = result else {
        anyhow::bail!("expected successful web-fetch record, got {result:?}");
    };
    let [
        SchemaValue::String(final_url),
        SchemaValue::U16(status),
        SchemaValue::Option {
            inner: content_type,
        },
        SchemaValue::String(content),
        SchemaValue::Bool(truncated),
    ] = fields.as_slice()
    else {
        anyhow::bail!("unexpected web-fetch result fields: {fields:?}");
    };
    let content_type = match content_type {
        Some(value) => match value.as_ref() {
            SchemaValue::String(value) => Some(value.clone()),
            other => anyhow::bail!("unexpected web-fetch content type: {other:?}"),
        },
        None => None,
    };
    Ok((
        final_url.clone(),
        *status,
        content_type,
        content.clone(),
        *truncated,
    ))
}

fn assert_web_fetch_error(
    result: Result<Option<SchemaValue>, SerializableToolRpcError>,
    expected_name: &str,
) -> anyhow::Result<()> {
    match result {
        Err(SerializableToolRpcError::RemoteToolError(error)) => match error.as_ref() {
            SerializableToolError::CustomError(error) if error.name == expected_name => Ok(()),
            other => anyhow::bail!("expected web-fetch error '{expected_name}', got {other:?}"),
        },
        other => anyhow::bail!("expected web-fetch error '{expected_name}', got {other:?}"),
    }
}

fn install_middleware_chain(
    deployment: &mut ToolDeploymentState,
    agent_type: &AgentTypeName,
    tool_name: &ToolName,
    middleware_component_id: golem_common::model::component::ComponentId,
    middleware_component_revision: ComponentRevision,
    middleware_component_name: &str,
    middleware_definitions: &[ToolMiddleware],
    occurrences: Vec<(&str, TypedSchemaValue)>,
) {
    let mut effective_definition = deployment.registered_tools[tool_name].definition.clone();
    let owner_account_id = deployment.registered_tools[tool_name].owner_account_id;
    let account_email = AccountEmail::new("middleware@golem");
    let registrations = middleware_definitions
        .iter()
        .map(|definition| {
            let name = ToolMiddlewareName::try_from(definition.name.as_str()).unwrap();
            let registration = RegisteredToolMiddleware {
                deployment_revision: deployment.deployment_revision,
                release_id: None,
                definition: definition.clone(),
                provision: ToolProvisionConfig::default(),
                source: ToolMiddlewareSource::Component {
                    component_id: middleware_component_id,
                    component_revision: middleware_component_revision,
                    component_name: ComponentName(middleware_component_name.to_string()),
                },
                owner_account_id,
                owner_account_email: account_email.clone(),
                metadata_version: "0.1.0".to_string(),
                metadata_digest: Default::default(),
            };
            (name, registration)
        })
        .collect::<BTreeMap<_, _>>();
    let mut compiled_occurrences = occurrences
        .into_iter()
        .rev()
        .map(|(name, parameters)| {
            let name = ToolMiddlewareName::try_from(name).unwrap();
            let middleware = registrations[&name].clone();
            let (expected_definition, presented_definition) = match &middleware.definition.scope {
                ToolMiddlewareScope::Universal => (None, None),
                ToolMiddlewareScope::Monomorphic(scope) => {
                    (scope.expected.clone(), Some(scope.presented.clone()))
                }
            };
            let next_effective_definition = effective_definition.clone();
            let compatibility = expected_definition.as_ref().map(|expected| {
                golem_common::schema::tool::compatibility::compile_tool_compatibility(
                    expected,
                    &next_effective_definition,
                    golem_common::schema::tool::compatibility::ToolCompatibilityMode::StructuralSubtype,
                )
                .expect("fixture contracts must be compatible")
            });
            if let Some(presented) = &presented_definition {
                effective_definition = presented.clone();
            }
            CompiledToolMiddlewareOccurrence {
                middleware,
                parameters,
                provision: ToolProvisionConfig::default(),
                config_keys_readable: Default::default(),
                secret_keys_readable: SecretKeyScope::All,
                secret_keys_revealable: SecretKeyScope::All,
                filesystem_access: ToolFilesystemAccess::Unset,
                expected_definition,
                presented_definition,
                next_effective_definition,
                compatibility,
            }
        })
        .collect::<Vec<_>>();
    compiled_occurrences.reverse();
    deployment.registered_tool_middlewares.extend(registrations);
    deployment
        .tool_middleware_chains
        .entry(ToolBindingOwner::AgentType {
            agent_type_name: agent_type.clone(),
        })
        .or_default()
        .insert(
            tool_name.clone(),
            CompiledToolMiddlewareChain {
                deployment_revision: deployment.deployment_revision,
                owner: ToolBindingOwner::AgentType {
                    agent_type_name: agent_type.clone(),
                },
                tool_name: tool_name.clone(),
                effective_definition,
                occurrences: compiled_occurrences,
            },
        );
}

fn empty_middleware_parameters(definition: &ToolMiddleware) -> TypedSchemaValue {
    TypedSchemaValue::new(
        definition.parameter_schema.clone(),
        SchemaValue::Record { fields: vec![] },
    )
}

fn parameterized_middleware_parameters(definition: &ToolMiddleware) -> TypedSchemaValue {
    TypedSchemaValue::new(
        definition.parameter_schema.clone(),
        SchemaValue::Record {
            fields: vec![
                SchemaValue::String("typed-prefix".to_string()),
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::Record {
                            fields: vec![
                                SchemaValue::String("first".to_string()),
                                SchemaValue::Bool(true),
                            ],
                        },
                        SchemaValue::Record {
                            fields: vec![
                                SchemaValue::String("ignored".to_string()),
                                SchemaValue::Bool(false),
                            ],
                        },
                        SchemaValue::Record {
                            fields: vec![
                                SchemaValue::String("second".to_string()),
                                SchemaValue::Bool(true),
                            ],
                        },
                    ],
                },
            ],
        },
    )
}

fn secret_policy_middleware_parameters(
    definition: &ToolMiddleware,
    label: &str,
) -> TypedSchemaValue {
    TypedSchemaValue::new(
        definition.parameter_schema.clone(),
        SchemaValue::Record {
            fields: vec![SchemaValue::String(label.to_string())],
        },
    )
}

fn audit_middleware_parameters(
    definition: &ToolMiddleware,
    label: &str,
    sink_url: &str,
) -> TypedSchemaValue {
    TypedSchemaValue::new(
        definition.parameter_schema.clone(),
        SchemaValue::Record {
            fields: vec![
                SchemaValue::String(label.to_string()),
                SchemaValue::String(sink_url.to_string()),
            ],
        },
    )
}

fn output_redaction_parameters(
    definition: &ToolMiddleware,
    structured: Vec<(&str, &str, &str)>,
    stdout: Vec<(&str, &str)>,
) -> TypedSchemaValue {
    TypedSchemaValue::new(
        definition.parameter_schema.clone(),
        SchemaValue::Record {
            fields: vec![
                SchemaValue::List {
                    elements: structured
                        .into_iter()
                        .map(|(selector, pattern, replacement)| SchemaValue::Record {
                            fields: vec![
                                SchemaValue::String(selector.to_string()),
                                SchemaValue::String(pattern.to_string()),
                                SchemaValue::String(replacement.to_string()),
                            ],
                        })
                        .collect(),
                },
                SchemaValue::List {
                    elements: stdout
                        .into_iter()
                        .map(|(pattern, replacement)| SchemaValue::Record {
                            fields: vec![
                                SchemaValue::String(pattern.to_string()),
                                SchemaValue::String(replacement.to_string()),
                            ],
                        })
                        .collect(),
                },
            ],
        },
    )
}

fn human_approval_middleware_parameters(
    definition: &ToolMiddleware,
    request_url: &str,
    policy: &str,
) -> TypedSchemaValue {
    TypedSchemaValue::new(
        definition.parameter_schema.clone(),
        SchemaValue::Record {
            fields: vec![
                SchemaValue::String(request_url.to_string()),
                SchemaValue::String(policy.to_string()),
            ],
        },
    )
}

fn native_deployment_state(
    owner_account_id: AccountId,
    caller_agent_type: &str,
    mut definition: golem_common::schema::tool::Tool,
    helper_definition: golem_common::schema::tool::Tool,
) -> ToolDeploymentState {
    definition.commands.nodes[0].name = "native-streaming".to_string();
    let definition_digest =
        golem_common::model::tool_release::tool_metadata_digest("0.1.0", &definition).unwrap();
    let helper_digest =
        golem_common::model::tool_release::tool_metadata_digest("0.1.0", &helper_definition)
            .unwrap();
    let deployment_revision = DeploymentRevision::try_from(1_u64).unwrap();
    let account_email = AccountEmail::new("test@golem");
    let tool_name = ToolName::try_from("native-streaming").unwrap();
    let source = ToolSource::Host {
        host_tool_id: HostToolId::try_from("executor-native-test".to_string()).unwrap(),
        implementation_version: "1.0.0".to_string(),
    };
    let registered = RegisteredTool {
        deployment_revision,
        release_id: None,
        definition,
        provision: ToolProvisionConfig::default(),
        source: source.clone(),
        owner_account_id,
        owner_account_email: account_email.clone(),
        metadata_version: "0.1.0".to_string(),
        metadata_digest: definition_digest,
        component_bindings: BTreeMap::new(),
    };
    let agent_type = AgentTypeName(caller_agent_type.to_string());
    let owner = ToolBindingOwner::AgentType {
        agent_type_name: agent_type.clone(),
    };
    let binding = CompiledToolBinding {
        deployment_revision,
        release_id: None,
        owner: owner.clone(),
        tool_name: tool_name.clone(),
        version: registered.definition.version.clone(),
        metadata_version: registered.metadata_version.clone(),
        metadata_digest: registered.metadata_digest,
        account_id: owner_account_id,
        account_email,
        parameters: NormalizedJsonValue::new(serde_json::json!({})),
        config_keys_readable: Default::default(),
        secret_keys_readable: SecretKeyScope::All,
        secret_keys_revealable: SecretKeyScope::All,
        filesystem_access: ToolFilesystemAccess::Unset,
        source,
    };
    let mut state = ToolDeploymentState {
        deployment_revision,
        registered_tools: BTreeMap::from([(tool_name.clone(), registered)]),
        tool_bindings: BTreeMap::from([(owner.clone(), BTreeMap::from([(tool_name, binding)]))]),
        mcp_imports: Vec::new(),
        tool_middleware_configuration: Default::default(),
        registered_tool_middlewares: BTreeMap::new(),
        tool_middleware_chains: BTreeMap::new(),
    };
    let helper_name = ToolName::try_from("native-durable-helper").unwrap();
    let helper_source = ToolSource::Host {
        host_tool_id: HostToolId::try_from("executor-native-helper".to_string()).unwrap(),
        implementation_version: "1.0.0".to_string(),
    };
    state.registered_tools.insert(
        helper_name.clone(),
        RegisteredTool {
            deployment_revision,
            release_id: None,
            definition: helper_definition,
            provision: ToolProvisionConfig::default(),
            source: helper_source.clone(),
            owner_account_id,
            owner_account_email: AccountEmail::new("test@golem"),
            metadata_version: "0.1.0".to_string(),
            metadata_digest: helper_digest,
            component_bindings: BTreeMap::new(),
        },
    );
    state.tool_bindings.get_mut(&owner).unwrap().insert(
        helper_name.clone(),
        CompiledToolBinding {
            deployment_revision,
            release_id: None,
            owner,
            tool_name: helper_name,
            version: "1.0.0".to_string(),
            metadata_version: "0.1.0".to_string(),
            metadata_digest: helper_digest,
            account_id: owner_account_id,
            account_email: AccountEmail::new("test@golem"),
            parameters: NormalizedJsonValue::new(serde_json::json!({})),
            config_keys_readable: Default::default(),
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
            filesystem_access: ToolFilesystemAccess::Unset,
            source: helper_source,
        },
    );
    state
}

struct ReorderedToolActivationService {
    inner: TestEnvironmentStateService,
    first_deployment: std::sync::RwLock<Option<Arc<ToolDeploymentState>>>,
    activation_calls: AtomicUsize,
    first_call_blocked: tokio::sync::Notify,
    release_first_call: tokio::sync::Notify,
}

impl Default for ReorderedToolActivationService {
    fn default() -> Self {
        Self {
            inner: TestEnvironmentStateService::default(),
            first_deployment: std::sync::RwLock::new(None),
            activation_calls: AtomicUsize::new(0),
            first_call_blocked: tokio::sync::Notify::new(),
            release_first_call: tokio::sync::Notify::new(),
        }
    }
}

impl ReorderedToolActivationService {
    fn set_tool_deployment(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
        component_id: golem_common::model::component::ComponentId,
        component_revision: ComponentRevision,
        deployment: Option<ToolDeploymentState>,
    ) {
        *self.first_deployment.write().unwrap() = deployment.as_ref().map(|deployment| {
            let mut deployment = deployment.clone();
            deployment.tool_bindings.clear();
            Arc::new(deployment)
        });
        self.inner.set_tool_deployment(
            environment_id,
            component_id,
            component_revision,
            deployment,
        );
    }

    fn activation_calls(&self) -> usize {
        self.activation_calls.load(Ordering::SeqCst)
    }

    async fn wait_for_first_call_to_block(&self) {
        self.first_call_blocked.notified().await;
    }

    fn release_first_call(&self) {
        self.release_first_call.notify_one();
    }
}

#[async_trait::async_trait]
impl EnvironmentStateService for ReorderedToolActivationService {
    async fn get_agent_deployment(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
        agent_type: &AgentTypeName,
    ) -> Result<
        Option<golem_service_base::model::AgentDeploymentDetails>,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        self.inner
            .get_agent_deployment(environment_id, agent_type)
            .await
    }

    async fn get_agent_secrets(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
    ) -> Result<
        HashMap<
            golem_common::model::agent_secret::CanonicalAgentSecretPath,
            golem_service_base::model::agent_secret::AgentSecret,
        >,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        self.inner.get_agent_secrets(environment_id).await
    }

    async fn get_agent_secret_revision(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
        agent_secret_id: golem_common::model::agent_secret::AgentSecretId,
        path: golem_common::model::agent_secret::CanonicalAgentSecretPath,
        revision: golem_common::model::agent_secret::AgentSecretRevision,
    ) -> Result<
        Option<golem_service_base::model::agent_secret::AgentSecret>,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        self.inner
            .get_agent_secret_revision(environment_id, agent_secret_id, path, revision)
            .await
    }

    async fn get_retry_policies(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
    ) -> Result<
        Vec<golem_common::model::retry_policy::NamedRetryPolicy>,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        self.inner.get_retry_policies(environment_id).await
    }

    async fn get_live_tool_deployment_state(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
        component_id: golem_common::model::component::ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Option<Arc<ToolDeploymentState>>, ToolDiscoveryError> {
        match self.activation_calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                self.first_call_blocked.notify_one();
                self.release_first_call.notified().await;
                Ok(self.first_deployment.read().unwrap().clone())
            }
            1 => {
                self.inner
                    .get_live_tool_deployment_state(
                        environment_id,
                        component_id,
                        component_revision,
                    )
                    .await
            }
            ordinal => panic!(
                "replay unexpectedly performed tool activation lookup number {}",
                ordinal + 1
            ),
        }
    }
}

fn assert_evidence(evidence: &StreamEvidence, output: &[u8], chunks_read: u32, bytes_read: u64) {
    assert_eq!(evidence.output, output);
    assert_eq!(evidence.chunks_read, chunks_read);
    assert_eq!(evidence.bytes_read, bytes_read);
    assert!(!evidence.output_closed);
    assert_eq!(evidence.completion, "ok");
}

type HttpGateState = (
    Arc<tokio::sync::Barrier>,
    tokio::sync::mpsc::UnboundedSender<(String, Vec<u8>)>,
    tokio::sync::mpsc::UnboundedSender<(String, Vec<u8>)>,
);

async fn gated_http_stream(State(state): State<HttpGateState>, request: Request) -> Response<Body> {
    let tag = request.uri().path().trim_start_matches('/').to_string();
    let mut request_body = request.into_body().into_data_stream();
    let first = request_body
        .next()
        .await
        .expect("gated HTTP request has an initial body chunk")
        .expect("read gated HTTP request body");
    state
        .1
        .send((tag.clone(), first.to_vec()))
        .expect("record initial gated HTTP upload");
    state.0.wait().await;

    let (response_tx, response_rx) = tokio::sync::mpsc::channel(1);
    tokio::spawn(async move {
        let mut received = first.to_vec();
        response_tx
            .send(Ok::<_, Infallible>(Bytes::from(format!("http-{tag}:"))))
            .await
            .expect("send gated HTTP response marker");
        response_tx
            .send(Ok::<_, Infallible>(first))
            .await
            .expect("echo initial gated HTTP upload");
        while let Some(chunk) = request_body.next().await {
            let chunk = chunk.expect("read remaining gated HTTP request body");
            received.extend_from_slice(&chunk);
            response_tx
                .send(Ok::<_, Infallible>(chunk))
                .await
                .expect("echo remaining gated HTTP upload");
        }
        state
            .2
            .send((tag, received))
            .expect("record completed gated HTTP upload");
    });

    Response::builder()
        .status(200)
        .body(Body::from_stream(ReceiverStream::new(response_rx)))
        .expect("build gated HTTP response")
}

async fn start_gated_http_server() -> (
    u16,
    tokio::task::JoinHandle<()>,
    tokio::sync::mpsc::UnboundedReceiver<(String, Vec<u8>)>,
    tokio::sync::mpsc::UnboundedReceiver<(String, Vec<u8>)>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind gated HTTP server");
    let port = listener.local_addr().expect("gated HTTP address").port();
    let (first_tx, first_rx) = tokio::sync::mpsc::unbounded_channel();
    let (complete_tx, complete_rx) = tokio::sync::mpsc::unbounded_channel();
    let app = Router::new()
        .route("/{tag}", post(gated_http_stream))
        .with_state((
            Arc::new(tokio::sync::Barrier::new(2)),
            first_tx,
            complete_tx,
        ));
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve gated HTTP requests");
    });
    (port, task, first_rx, complete_rx)
}

#[derive(Clone)]
struct IdempotencyEffectState {
    keys: Arc<tokio::sync::Mutex<Vec<String>>>,
    effects: Arc<tokio::sync::Mutex<std::collections::BTreeSet<String>>>,
    attempts: tokio::sync::mpsc::UnboundedSender<String>,
}

async fn idempotency_effect(
    State(state): State<IdempotencyEffectState>,
    request: Request,
) -> Response<Body> {
    let key = request
        .headers()
        .get("idempotency-key")
        .expect("outgoing effect has an idempotency key")
        .to_str()
        .expect("idempotency key is text")
        .to_string();
    let attempt = {
        let mut keys = state.keys.lock().await;
        keys.push(key.clone());
        keys.len()
    };
    state.effects.lock().await.insert(key.clone());
    state
        .attempts
        .send(key)
        .expect("record idempotent effect attempt");
    if attempt == 1 {
        std::future::pending::<()>().await;
    }
    Response::builder()
        .status(200)
        .body(Body::empty())
        .expect("build idempotent effect response")
}

async fn start_idempotency_effect_server() -> (
    u16,
    tokio::task::JoinHandle<()>,
    tokio::sync::mpsc::UnboundedReceiver<String>,
    Arc<tokio::sync::Mutex<Vec<String>>>,
    Arc<tokio::sync::Mutex<std::collections::BTreeSet<String>>>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind idempotency effect server");
    let port = listener.local_addr().expect("effect server address").port();
    let (attempts, attempt_rx) = tokio::sync::mpsc::unbounded_channel();
    let keys = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let effects = Arc::new(tokio::sync::Mutex::new(std::collections::BTreeSet::new()));
    let app = Router::new()
        .route("/effect", post(idempotency_effect))
        .with_state(IdempotencyEffectState {
            keys: keys.clone(),
            effects: effects.clone(),
            attempts,
        });
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve idempotency effects");
    });
    (port, task, attempt_rx, keys, effects)
}

async fn start_native_order_http_server() -> (u16, tokio::task::JoinHandle<()>, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind native-order HTTP server");
    let port = listener
        .local_addr()
        .expect("native-order HTTP address")
        .port();
    let requests = Arc::new(AtomicUsize::new(0));
    let route_requests = requests.clone();
    let app = Router::new().route(
        "/",
        post(move || {
            let requests = route_requests.clone();
            async move {
                requests.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::NO_CONTENT
            }
        }),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve native-order HTTP requests");
    });
    (port, task, requests)
}

struct WebFetchHttpServers {
    source_port: u16,
    target_port: u16,
    source_server: tokio::task::JoinHandle<()>,
    source_requests: Arc<AtomicUsize>,
    target_requests: Arc<AtomicUsize>,
    target_server: tokio::task::JoinHandle<()>,
    bounded_body_gate: Arc<tokio::sync::Notify>,
    timeout_started: tokio::sync::oneshot::Receiver<()>,
    timeout_cancelled: tokio::sync::oneshot::Receiver<()>,
    interrupted_started: tokio::sync::oneshot::Receiver<()>,
    interrupted_requests: Arc<AtomicUsize>,
}

async fn start_web_fetch_http_servers() -> WebFetchHttpServers {
    let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_port = target_listener.local_addr().unwrap().port();
    let target_requests = Arc::new(AtomicUsize::new(0));
    let target_requests_for_route = target_requests.clone();
    let target_server = tokio::spawn(async move {
        let app = Router::new().route(
            "/cross-host-final",
            get(move || async move {
                target_requests_for_route.fetch_add(1, Ordering::SeqCst);
                Response::builder()
                    .header("content-type", "text/plain; charset=utf-8")
                    .body(Body::from("cross-host target"))
                    .unwrap()
            }),
        );
        axum::serve(target_listener, app).await.unwrap();
    });

    let source_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source_port = source_listener.local_addr().unwrap().port();
    let cross_host_location = format!("http://127.0.0.1:{target_port}/cross-host-final");
    let source_requests = Arc::new(AtomicUsize::new(0));
    let source_requests_for_routes = source_requests.clone();
    let bounded_body_gate = Arc::new(tokio::sync::Notify::new());
    let bounded_body_gate_for_route = bounded_body_gate.clone();
    let (timeout_started_tx, timeout_started) = tokio::sync::oneshot::channel();
    let timeout_started_tx = Arc::new(tokio::sync::Mutex::new(Some(timeout_started_tx)));
    let (timeout_cancelled_tx, timeout_cancelled) = tokio::sync::oneshot::channel();
    let timeout_cancelled_tx = Arc::new(tokio::sync::Mutex::new(Some(timeout_cancelled_tx)));
    let (interrupted_started_tx, interrupted_started) = tokio::sync::oneshot::channel();
    let interrupted_started_tx = Arc::new(tokio::sync::Mutex::new(Some(interrupted_started_tx)));
    let interrupted_requests = Arc::new(AtomicUsize::new(0));
    let interrupted_requests_for_route = interrupted_requests.clone();
    let source_server = tokio::spawn(async move {
        let app = Router::new()
            .route(
                "/html",
                get(|| async {
                    Response::builder()
                        .header("content-type", "text/html; charset=utf-8")
                        .body(Body::from(
                            "<html><body><h1>Fetch title</h1><script>hidden()</script><p>Readable body.</p></body></html>",
                        ))
                        .unwrap()
                }),
            )
            .route(
                "/missing",
                get(|| async {
                    Response::builder()
                        .status(axum::http::StatusCode::NOT_FOUND)
                        .header("content-type", "text/plain")
                        .body(Body::from("missing body"))
                        .unwrap()
                }),
            )
            .route(
                "/relative",
                get(|| async {
                    Response::builder()
                        .status(axum::http::StatusCode::FOUND)
                        .header("location", "/relative-final")
                        .body(Body::empty())
                        .unwrap()
                }),
            )
            .route(
                "/relative-final",
                get(|| async {
                    Response::builder()
                        .header("content-type", "text/plain")
                        .body(Body::from("relative target"))
                        .unwrap()
                }),
            )
            .route(
                "/cross-host",
                get(move || {
                    let location = cross_host_location.clone();
                    async move {
                        Response::builder()
                            .status(axum::http::StatusCode::TEMPORARY_REDIRECT)
                            .header("location", location)
                            .body(Body::empty())
                            .unwrap()
                    }
                }),
            )
            .route(
                "/streaming",
                get(move || {
                    let gate = bounded_body_gate_for_route.clone();
                    async move {
                        let (body_tx, body_rx) = tokio::sync::mpsc::channel(3);
                        tokio::spawn(async move {
                            body_tx
                                .send(Ok::<_, Infallible>(Bytes::from_static(b"abc")))
                                .await
                                .ok();
                            // Reaching the configured limit must stop the fetch even if the peer
                            // keeps the response stream open without sending another byte.
                            body_tx
                                .send(Ok(Bytes::from_static(b"de")))
                                .await
                                .ok();
                            gate.notified().await;
                            body_tx
                                .send(Ok(Bytes::from_static(b"ghi")))
                                .await
                                .ok();
                        });
                        Response::builder()
                            .header("content-type", "text/plain")
                            .body(Body::from_stream(ReceiverStream::new(body_rx)))
                            .unwrap()
                    }
                }),
            )
            .route(
                "/slow-stream",
                get(move || {
                    let started = timeout_started_tx.clone();
                    let cancelled = timeout_cancelled_tx.clone();
                    async move {
                        let (body_tx, body_rx) = tokio::sync::mpsc::channel(1);
                        tokio::spawn(async move {
                            if body_tx
                                .send(Ok::<_, Infallible>(Bytes::from_static(b"started")))
                                .await
                                .is_err()
                            {
                                return;
                            }
                            if let Some(started) = started.lock().await.take() {
                                started.send(()).ok();
                            }
                            body_tx.closed().await;
                            if let Some(cancelled) = cancelled.lock().await.take() {
                                cancelled.send(()).ok();
                            }
                        });
                        Response::builder()
                            .header("content-type", "text/plain")
                            .body(Body::from_stream(ReceiverStream::new(body_rx)))
                            .unwrap()
                    }
                }),
            )
            .route(
                "/slow-headers",
                get(|| async {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    Response::builder()
                        .header("content-type", "text/plain")
                        .body(Body::from("too late"))
                        .unwrap()
                }),
            )
            .route(
                "/interrupted",
                get(move || {
                    let attempt = interrupted_requests_for_route.fetch_add(1, Ordering::SeqCst);
                    let started = interrupted_started_tx.clone();
                    async move {
                        if attempt > 0 {
                            return Response::builder()
                                .header("content-type", "text/plain")
                                .body(Body::from("retried after interruption"))
                                .unwrap();
                        }

                        let (body_tx, body_rx) = tokio::sync::mpsc::channel(1);
                        tokio::spawn(async move {
                            if body_tx
                                .send(Ok::<_, Infallible>(Bytes::from_static(b"started")))
                                .await
                                .is_err()
                            {
                                return;
                            }
                            if let Some(started) = started.lock().await.take() {
                                started.send(()).ok();
                            }
                            body_tx.closed().await;
                        });
                        Response::builder()
                            .header("content-type", "text/plain")
                            .body(Body::from_stream(ReceiverStream::new(body_rx)))
                            .unwrap()
                    }
                }),
            )
            .route(
                "/invalid-utf8",
                get(|| async {
                    Response::builder()
                        .header("content-type", "text/plain; charset=utf-8")
                        .body(Body::from(Bytes::from_static(&[0xff])))
                        .unwrap()
                }),
            )
            .route(
                "/binary",
                get(|| async {
                    Response::builder()
                        .header("content-type", "application/octet-stream")
                        .body(Body::from(Bytes::from_static(&[0, 1, 2, 3])))
                        .unwrap()
                }),
            )
            .route(
                "/cycle",
                get(|| async {
                    Response::builder()
                        .status(axum::http::StatusCode::MOVED_PERMANENTLY)
                        .header("location", "/cycle#again")
                        .body(Body::empty())
                        .unwrap()
                }),
            )
            .layer(axum::middleware::from_fn(
                move |request: Request, next: axum::middleware::Next| {
                let requests = source_requests_for_routes.clone();
                async move {
                    requests.fetch_add(1, Ordering::SeqCst);
                    next.run(request).await
                }
                },
            ));
        axum::serve(source_listener, app).await.unwrap();
    });

    WebFetchHttpServers {
        source_port,
        target_port,
        source_server,
        source_requests,
        target_requests,
        target_server,
        bounded_body_gate,
        timeout_started,
        timeout_cancelled,
        interrupted_started,
        interrupted_requests,
    }
}
async fn start_trap_attempt_server() -> (u16, tokio::task::JoinHandle<()>, Arc<AtomicUsize>) {
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind trap-attempt server");
    let port = listener.local_addr().expect("trap-attempt address").port();
    let attempts = Arc::new(AtomicUsize::new(0));
    let recorded_attempts = attempts.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut connection, _) = listener
                .accept()
                .await
                .expect("accept trap-attempt connection");
            connection
                .write_all(&[u8::from(
                    recorded_attempts.fetch_add(1, Ordering::SeqCst) != 0,
                )])
                .await
                .expect("write trap-attempt response");
        }
    });
    (port, task, attempts)
}

struct CrashCheckpointArrival {
    name: String,
    release: tokio::sync::oneshot::Sender<()>,
}

async fn announce_crash_checkpoint(
    Path(name): Path<String>,
    State(current_name): State<Arc<tokio::sync::RwLock<Option<String>>>>,
) -> axum::http::StatusCode {
    *current_name.write().await = Some(name);
    axum::http::StatusCode::NO_CONTENT
}

async fn start_crash_checkpoint_server() -> (
    u16,
    u16,
    tokio_util::task::AbortOnDropHandle<()>,
    tokio::sync::mpsc::UnboundedReceiver<CrashCheckpointArrival>,
) {
    use tokio::io::AsyncWriteExt;

    let announcement_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind crash checkpoint server");
    let announcement_port = announcement_listener
        .local_addr()
        .expect("crash checkpoint address")
        .port();
    let gate_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind crash checkpoint gate");
    let gate_port = gate_listener
        .local_addr()
        .expect("crash checkpoint gate address")
        .port();
    let (arrivals, received) = tokio::sync::mpsc::unbounded_channel();
    let current_name = Arc::new(tokio::sync::RwLock::new(None));
    let app = Router::new()
        .route("/{name}", post(announce_crash_checkpoint))
        .with_state(current_name.clone());
    let task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        let announcements = axum::serve(announcement_listener, app);
        let gates = async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = gate_listener.accept() => {
                        let (mut connection, _) = accepted.expect("accept crash checkpoint gate");
                        let name = current_name
                            .read()
                            .await
                            .clone()
                            .expect("checkpoint name announced before gate connection");
                        let arrivals = arrivals.clone();
                        connections.spawn(async move {
                            eprintln!("crash checkpoint server received `{name}`");
                            let (release, wait) = tokio::sync::oneshot::channel();
                            arrivals
                                .send(CrashCheckpointArrival { name, release })
                                .expect("record crash checkpoint arrival");
                            if wait.await.is_ok() {
                                connection
                                    .write_all(&[1])
                                    .await
                                    .expect("release crash checkpoint gate");
                            }
                        });
                    }
                    completed = connections.join_next(), if !connections.is_empty() => {
                        completed
                            .expect("crash checkpoint gate task exists")
                            .expect("serve crash checkpoint gate");
                    }
                }
            }
        };
        tokio::select! {
            result = announcements => result.expect("serve crash checkpoint announcements"),
            () = gates => unreachable!("crash checkpoint gate listener does not stop"),
        }
    }));
    (announcement_port, gate_port, task, received)
}

#[derive(Debug)]
struct PromiseCheckpointArrival {
    name: String,
    oplog_idx: OplogIndex,
}

async fn announce_promise_checkpoint(
    Path(name): Path<String>,
    State(arrivals): State<tokio::sync::mpsc::UnboundedSender<PromiseCheckpointArrival>>,
    body: Bytes,
) -> axum::http::StatusCode {
    let oplog_idx = OplogIndex::from_u64(
        std::str::from_utf8(&body)
            .expect("promise checkpoint body is UTF-8")
            .parse()
            .expect("promise checkpoint body is an oplog index"),
    );
    arrivals
        .send(PromiseCheckpointArrival { name, oplog_idx })
        .expect("record promise checkpoint arrival");
    axum::http::StatusCode::NO_CONTENT
}

async fn start_promise_checkpoint_server() -> (
    u16,
    tokio::task::JoinHandle<()>,
    tokio::sync::mpsc::UnboundedReceiver<PromiseCheckpointArrival>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind promise checkpoint server");
    let port = listener
        .local_addr()
        .expect("promise checkpoint address")
        .port();
    let (arrivals, received) = tokio::sync::mpsc::unbounded_channel();
    let app = Router::new()
        .route("/{name}", post(announce_promise_checkpoint))
        .with_state(arrivals);
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve promise checkpoint announcements");
    });
    (port, task, received)
}

async fn next_promise_checkpoint(
    arrivals: &mut tokio::sync::mpsc::UnboundedReceiver<PromiseCheckpointArrival>,
    expected: &str,
) -> anyhow::Result<PromiseCheckpointArrival> {
    let arrival = tokio::time::timeout(std::time::Duration::from_secs(30), arrivals.recv())
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for `{expected}` promise checkpoint"))?
        .ok_or_else(|| anyhow::anyhow!("promise checkpoint server stopped before `{expected}`"))?;
    assert_eq!(arrival.name, expected);
    Ok(arrival)
}

async fn wait_for_promise_checkpoint_to_await(
    executor: &TestWorkerExecutor,
    worker_id: &golem_common::model::AgentId,
    checkpoint: &PromiseCheckpointArrival,
) -> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            // A resident owner can park with a buffered Start; oplog reads only see commits.
            executor.commit_oplog(worker_id).await?;
            let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
            let checkpoint_parent = oplog.iter().find_map(|entry| {
                (entry.oplog_index == checkpoint.oplog_idx).then(|| match &entry.entry {
                    PublicOplogEntry::Start(parameters)
                        if parameters.function_name == "golem::api::create_promise" =>
                    {
                        Ok(parameters.parent_start_index)
                    }
                    other => Err(anyhow::anyhow!(
                        "promise checkpoint `{}` announced index {} for {}",
                        checkpoint.name,
                        checkpoint.oplog_idx,
                        describe_public_entry(other)
                    )),
                })
            });
            if let Some(checkpoint_parent) = checkpoint_parent {
                let checkpoint_parent = checkpoint_parent?;
                if oplog.iter().any(|entry| {
                    entry.oplog_index > checkpoint.oplog_idx
                        && matches!(
                            &entry.entry,
                            PublicOplogEntry::Start(parameters)
                                if parameters.function_name == "golem::api::get_promise_result"
                                    && parameters.parent_start_index == checkpoint_parent
                        )
                }) {
                    return Ok::<_, anyhow::Error>(());
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "timed out waiting for `{}` promise checkpoint to enter its durable wait",
            checkpoint.name
        )
    })??;
    Ok(())
}

async fn wait_for_active_tool_operations(
    executor: &TestWorkerExecutor,
    agent_id: &OwnedAgentId,
    expected: usize,
) -> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if executor
                .active_entity_metadata(agent_id)
                .await
                .is_some_and(|active| active.tool_operations.operations.len() == expected)
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for {expected} active tool operations"))?;
    Ok(())
}

async fn wait_for_owner_replay_settling(
    executor: &TestWorkerExecutor,
    agent_id: &OwnedAgentId,
) -> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if executor.owner_replay_is_settling(agent_id).await? {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for owner replay to enter settlement"))??;
    Ok(())
}

async fn wait_for_completed_entity_terminal(
    executor: &TestWorkerExecutor,
    worker_id: &golem_common::model::AgentId,
) -> anyhow::Result<OplogIndex> {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
            if let Some(start_index) = oplog.iter().find_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::entity::invoke"
                        && oplog.iter().any(|candidate| {
                            matches!(
                                &candidate.entry,
                                PublicOplogEntry::End(params)
                                    if params.start_index == entry.oplog_index
                            )
                        }) =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            }) {
                return Ok::<_, anyhow::Error>(start_index);
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for completed entity terminal"))?
}

async fn next_crash_checkpoint(
    arrivals: &mut tokio::sync::mpsc::UnboundedReceiver<CrashCheckpointArrival>,
    expected: &str,
) -> anyhow::Result<CrashCheckpointArrival> {
    let arrival = tokio::time::timeout(std::time::Duration::from_secs(30), arrivals.recv())
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for `{expected}` checkpoint"))?
        .ok_or_else(|| anyhow::anyhow!("checkpoint server stopped before `{expected}`"))?;
    assert_eq!(arrival.name, expected);
    Ok(arrival)
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn middleware_chain_dispatches_universal_monomorphic_and_typed_parameters_in_all_modes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let middleware_component = executor
        .component(
            &context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .name("golem-it:tool-streaming-rust-middleware")
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let definition = |name: &str| {
        middleware_metadata
            .tool_middlewares
            .iter()
            .find(|definition| definition.name == name)
            .unwrap()
    };
    let universal = definition("streaming-universal-pass-through");
    let streaming_pass_through = definition("streaming-monomorphic-pass-through");
    let transform = definition("streaming-transform");
    let parameterized = definition("streaming-parameterized");
    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let tool_name = ToolName::try_from("middleware-probe").unwrap();
    let deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools.clone(),
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-chain-dispatch");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;

    let uninstalled: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "middleware_probe_modes",
            data_value!("uninstalled"),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        uninstalled,
        [
            "leaf(sync-uninstalled)",
            "fire-and-forget-admitted",
            "leaf(async-uninstalled)",
        ],
        "uploaded discoverable middleware metadata must not install itself"
    );

    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools.clone(),
    );
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &tool_name,
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![
            (
                universal.name.as_str(),
                empty_middleware_parameters(universal),
            ),
            (
                transform.name.as_str(),
                empty_middleware_parameters(transform),
            ),
            (
                parameterized.name.as_str(),
                parameterized_middleware_parameters(parameterized),
            ),
        ],
    );
    let streaming_tool_name = ToolName::try_from("streaming").unwrap();
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &streaming_tool_name,
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![
            (
                universal.name.as_str(),
                empty_middleware_parameters(universal),
            ),
            (
                streaming_pass_through.name.as_str(),
                empty_middleware_parameters(streaming_pass_through),
            ),
        ],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    executor.simulated_crash(&worker_id).await?;

    let results: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "middleware_probe_modes",
            data_value!("dispatch"),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        results,
        [
            "transform-out(leaf(typed-prefix[first,second](transform-in(sync-dispatch))))",
            "fire-and-forget-admitted",
            "transform-out(leaf(typed-prefix[first,second](transform-in(async-dispatch))))",
        ]
    );
    let stream_input = b"middleware-byte-stream-parity".to_vec();
    let streamed: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!("marker-echo", stream_input.clone(), 5_u32),
        )
        .await?
        .into_typed()?;
    let mut expected_output = b"marker:".to_vec();
    expected_output.extend_from_slice(&stream_input);
    assert_eq!(streamed.output, expected_output);
    assert_eq!(streamed.chunks_read, stream_input.len().div_ceil(5) as u32);
    assert_eq!(streamed.bytes_read, stream_input.len() as u64);
    assert!(!streamed.output_closed);
    assert_eq!(streamed.completion, "ok");

    let starts_before_terminal_rows = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .into_iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::entity::invoke"
            )
        })
        .count();
    let terminal_rows: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "rust_provider_terminal_rows",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(&terminal_rows[..2], ["marker:", "finished"]);
    assert!(terminal_rows[2].contains("Declared"), "{terminal_rows:?}");
    assert_eq!(
        &terminal_rows[3..],
        [
            "marker:",
            "stream producer failed",
            "false",
            "marker:",
            "false",
        ]
    );
    let starts_after_terminal_rows = executor
        .get_oplog(&worker_id, OplogIndex::INITIAL)
        .await?
        .into_iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::entity::invoke"
            )
        })
        .count();
    assert_eq!(
        starts_after_terminal_rows - starts_before_terminal_rows,
        9,
        "three provider calls through two middleware layers must each execute exactly once"
    );
    if let Some(active) = executor
        .active_entity_metadata(&OwnedAgentId::new(
            context.default_environment_id,
            &worker_id,
        ))
        .await
    {
        assert!(active.tool_operations.operations.is_empty());
    }

    let short_circuit = definition("streaming-short-circuit");
    let mut redeployed = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools,
    );
    install_middleware_chain(
        &mut redeployed,
        &agent_type,
        &tool_name,
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![(
            short_circuit.name.as_str(),
            empty_middleware_parameters(short_circuit),
        )],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(redeployed),
    );
    executor.simulated_crash(&worker_id).await?;
    let after_redeployment: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "middleware_probe_modes",
            data_value!("redeployed"),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        after_redeployment,
        [
            "short(sync-redeployed)",
            "fire-and-forget-admitted",
            "short(async-redeployed)",
        ]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn dynamic_mcp_uses_effective_middleware_metadata_and_replays_pinned_host_leaf_offline(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use serde_json::{Value, json};
    use std::sync::Mutex;

    let requests = Arc::new(Mutex::new(Vec::<(Value, String)>::new()));
    let (pending_started, mut pending_started_rx) = tokio::sync::mpsc::unbounded_channel();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let handler = post({
        let requests = requests.clone();
        move |headers: axum::http::HeaderMap, axum::Json(body): axum::Json<Value>| {
            let requests = requests.clone();
            let pending_started = pending_started.clone();
            async move {
                assert_eq!(body["method"], "tools/call");
                assert_eq!(body["params"]["name"], "upstream-probe");
                let value = body["params"]["arguments"]["value"]
                    .as_str()
                    .expect("projected string argument");
                let key = headers["idempotency-key"].to_str().unwrap().to_string();
                requests.lock().unwrap().push((body.clone(), key));
                if value.ends_with("-pending") {
                    pending_started.send(()).unwrap();
                    std::future::pending::<()>().await;
                }
                axum::Json(json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                    "result": {
                        "content": [{"type": "text", "text": format!("stdout:{value}")}],
                        "structuredContent": {"evidence": format!("leaf({value})")}
                    }
                }))
            }
        }
    });
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/mcp", handler))
            .await
            .unwrap();
    }));

    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let middleware_component = executor
        .component(
            &context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .name("golem-it:tool-streaming-rust-middleware-mcp")
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let transform = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-universal-transform-input")
        .unwrap();
    let fanout = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-universal-mcp-fanout")
        .unwrap();
    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let tool_name = ToolName::try_from("middleware-probe").unwrap();
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools.clone(),
    );
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &tool_name,
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![(
            transform.name.as_str(),
            empty_middleware_parameters(transform),
        )],
    );
    deployment.registered_tools.clear();
    deployment.tool_bindings.clear();
    deployment.tool_middleware_chains.clear();
    deployment.mcp_imports = vec![McpImport {
        url: format!("http://127.0.0.1:{port}/mcp"),
        auth: None,
        security_scheme: None,
        prefix: None,
        include: None,
        exclude: None,
        version: None,
    }];
    deployment.tool_middleware_configuration.universal = vec![ToolMiddlewareInstallation {
        name: ToolMiddlewareName::try_from(transform.name.as_str()).unwrap(),
        version: Some(transform.version.clone()),
        parameters: NormalizedJsonValue::new(json!({})),
        account: None,
        secret_keys_readable: None,
        secret_keys_revealable: None,
        filesystem_access: ToolFilesystemAccess::Unset,
    }];
    deployment
        .tool_middleware_configuration
        .universal
        .push(ToolMiddlewareInstallation {
            name: ToolMiddlewareName::try_from(fanout.name.as_str()).unwrap(),
            version: Some(fanout.version.clone()),
            parameters: NormalizedJsonValue::new(json!({})),
            account: None,
            secret_keys_readable: None,
            secret_keys_revealable: None,
            filesystem_access: ToolFilesystemAccess::Unset,
        });
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let source = McpImportSource {
        environment_id: context.default_environment_id,
        deployment_revision: 1_u64.try_into().unwrap(),
        import_index: 0,
        upstream_tool_name: String::new(),
    };
    let projected = ProjectedTool::new(
        &json!({
            "name": "upstream-probe",
            "inputSchema": {
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"],
                "additionalProperties": false
            },
            "outputSchema": {
                "type": "object",
                "properties": {"evidence": {"type": "string"}},
                "required": ["evidence"],
                "additionalProperties": false
            }
        }),
        tool_name.as_str(),
        Limits::default(),
    )
    .map_err(anyhow::Error::msg)?;
    environment_state.set_mcp_observation(
        source.clone(),
        Ok(McpImportObservation {
            source: source.clone(),
            protocol_version: golem_mcp_import::transport::PROTOCOL_VERSION.to_string(),
            tools: vec![projected],
            diagnostics: Vec::new(),
        }),
    );
    let mut credential_source = source;
    credential_source.upstream_tool_name = "upstream-probe".to_string();
    environment_state.set_mcp_credential(
        credential_source,
        golem_service_base::clients::registry::McpRuntimeCredential {
            credential: None,
            oauth_grant_generation: None,
        },
    );

    let (checkpoint_port, checkpoint_server, mut checkpoints) =
        start_promise_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "dynamic-mcp-middleware");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([(
                "MIDDLEWARE_PROMISE_CHECKPOINT_PORT".to_string(),
                checkpoint_port.to_string(),
            )]),
            Vec::new(),
        )
        .await?;
    let call_executor = executor.clone();
    let call_component = caller_component.clone();
    let call_agent = agent_id.clone();
    let call = tokio::spawn(async move {
        call_executor
            .invoke_and_await_agent(
                &call_component,
                &call_agent,
                "dynamic_mcp_chain_probe",
                data_value!("acceptance"),
            )
            .await?
            .into_typed::<Vec<String>>()
    });
    let checkpoint = next_promise_checkpoint(&mut checkpoints, "mcp-pending-admitted").await?;
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        pending_started_rx.recv(),
    )
    .await?
    .expect("pending MCP request was observed");
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: checkpoint.oplog_idx,
            },
            vec![],
        )
        .await?;
    let result = call.await??;
    assert_eq!(
        result,
        [
            "leaf(middleware(acceptance)-second)",
            "stdout:middleware(acceptance)-second"
        ],
        "the projected MCP contract must traverse both middleware layers"
    );
    let observed = requests.lock().unwrap().clone();
    assert_eq!(observed.len(), 3);
    assert_eq!(
        observed
            .iter()
            .map(|(request, _)| request["params"]["arguments"]["value"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "middleware(acceptance)-first",
            "middleware(acceptance)-second",
            "middleware(acceptance)-pending"
        ]
    );
    assert_eq!(
        observed
            .iter()
            .map(|(_, key)| key)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3,
        "each underlying start needs a distinct idempotency key"
    );

    drop(server);
    environment_state.clear_mcp_observations();
    environment_state.clear_mcp_credentials();
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        None,
    );
    executor.simulated_crash(&worker_id).await?;
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let _: String = executor
        .invoke_and_await_agent(&caller_component, &agent_id, "replay_probe", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(
        requests.lock().unwrap().len(),
        3,
        "completed Host-leaf replay repeated the MCP effect"
    );
    assert_eq!(environment_state.mcp_observation_requests().len(), 1);
    assert_eq!(environment_state.mcp_credential_requests().len(), 3);

    checkpoint_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn dynamic_mcp_executes_genuine_monomorphic_middleware_and_replays_offline(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use serde_json::{Value, json};
    use std::sync::Mutex;

    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let handler = post({
        let requests = requests.clone();
        move |axum::Json(body): axum::Json<Value>| {
            let requests = requests.clone();
            async move {
                assert_eq!(body["method"], "tools/call");
                let value = body["params"]["arguments"]["value"]
                    .as_str()
                    .expect("projected string argument");
                requests.lock().unwrap().push(body.clone());
                axum::Json(json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                    "result": {
                        "content": [{"type": "text", "text": format!("stdout:{value}")}],
                        "structuredContent": {"evidence": format!("leaf({value})")}
                    }
                }))
            }
        }
    });
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/mcp", handler))
            .await
            .unwrap();
    }));

    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let middleware_component = executor
        .component(
            &context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .name("golem-it:tool-streaming-rust-middleware-monomorphic-mcp")
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let projection = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-monomorphic-mcp-projection")
        .expect("monomorphic MCP middleware export");
    assert!(matches!(
        projection.scope,
        ToolMiddlewareScope::Monomorphic(_)
    ));

    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let tool_name = ToolName::try_from("middleware-probe").unwrap();
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools.clone(),
    );
    deployment.registered_tool_middlewares.insert(
        ToolMiddlewareName::try_from(projection.name.as_str()).unwrap(),
        RegisteredToolMiddleware {
            deployment_revision: deployment.deployment_revision,
            release_id: None,
            definition: projection.clone(),
            provision: ToolProvisionConfig::default(),
            source: ToolMiddlewareSource::Component {
                component_id: middleware_component.id,
                component_revision: middleware_component.revision,
                component_name: ComponentName(
                    "golem-it:tool-streaming-rust-middleware".to_string(),
                ),
            },
            owner_account_id: context.account_id,
            owner_account_email: AccountEmail::new("middleware@golem"),
            metadata_version: "0.1.0".to_string(),
            metadata_digest: Default::default(),
        },
    );
    deployment.registered_tools.clear();
    deployment.tool_bindings.clear();
    deployment.tool_middleware_chains.clear();
    deployment.mcp_imports = vec![McpImport {
        url: format!("http://127.0.0.1:{port}/mcp"),
        auth: None,
        security_scheme: None,
        prefix: None,
        include: None,
        exclude: None,
        version: None,
    }];
    deployment
        .tool_middleware_configuration
        .agent_bindings
        .entry(agent_type.clone())
        .or_default()
        .insert(
            tool_name.clone(),
            ToolBindingInput {
                version: None,
                parameters: NormalizedJsonValue::new(json!({})),
                account: None,
                config_keys_readable: ConfigKeyScope::All,
                secret_keys_readable: SecretKeyScope::All,
                secret_keys_revealable: SecretKeyScope::All,
                filesystem_access: ToolFilesystemAccess::Unset,
                middleware: Some(vec![ToolMiddlewareInstallation {
                    name: ToolMiddlewareName::try_from(projection.name.as_str()).unwrap(),
                    version: Some(projection.version.clone()),
                    parameters: NormalizedJsonValue::new(json!({})),
                    account: None,
                    secret_keys_readable: None,
                    secret_keys_revealable: None,
                    filesystem_access: ToolFilesystemAccess::Unset,
                }]),
                middleware_merge_mode: None,
            },
        );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let source = McpImportSource {
        environment_id: context.default_environment_id,
        deployment_revision: 1_u64.try_into().unwrap(),
        import_index: 0,
        upstream_tool_name: String::new(),
    };
    let projected = ProjectedTool::new(
        &json!({
            "name": "upstream-probe",
            "inputSchema": {
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"],
                "additionalProperties": false
            },
            "outputSchema": {
                "type": "object",
                "properties": {"evidence": {"type": "string"}},
                "required": ["evidence"],
                "additionalProperties": false
            }
        }),
        tool_name.as_str(),
        Limits::default(),
    )
    .map_err(anyhow::Error::msg)?;
    environment_state.set_mcp_observation(
        source.clone(),
        Ok(McpImportObservation {
            source: source.clone(),
            protocol_version: golem_mcp_import::transport::PROTOCOL_VERSION.to_string(),
            tools: vec![projected],
            diagnostics: Vec::new(),
        }),
    );
    let mut credential_source = source;
    credential_source.upstream_tool_name = "upstream-probe".to_string();
    environment_state.set_mcp_credential(
        credential_source,
        golem_service_base::clients::registry::McpRuntimeCredential {
            credential: None,
            oauth_grant_generation: None,
        },
    );

    let agent_id = agent_id!("ToolStreamingCaller", "dynamic-mcp-monomorphic");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let result = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "dynamic_mcp_stdout_probe",
            data_value!("acceptance"),
        )
        .await?
        .into_typed::<String>()?;
    assert_eq!(result, "stdout:monomorphic(acceptance)");
    let observed = requests.lock().unwrap().clone();
    assert_eq!(observed.len(), 1);
    assert_eq!(
        observed[0]["params"]["arguments"]["value"],
        "monomorphic(acceptance)"
    );

    drop(server);
    environment_state.clear_mcp_observations();
    environment_state.clear_mcp_credentials();
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        None,
    );
    executor.simulated_crash(&worker_id).await?;
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let _: String = executor
        .invoke_and_await_agent(&caller_component, &agent_id, "replay_probe", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(environment_state.mcp_observation_requests().len(), 1);
    assert_eq!(environment_state.mcp_credential_requests().len(), 1);

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn overlapping_middleware_exposes_public_ancestry_across_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let middleware_component = executor
        .component(
            &context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .name("golem-it:tool-streaming-rust-middleware-overlap")
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let overlapping = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-overlapping")
        .unwrap();
    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let tool_name = ToolName::try_from("middleware-probe").unwrap();
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools.clone(),
    );
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &tool_name,
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![(
            overlapping.name.as_str(),
            empty_middleware_parameters(overlapping),
        )],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-overlap");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let results: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "middleware_probe_modes",
            data_value!("overlap"),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        results,
        [
            "overlapping[leaf(overlap-left(sync-overlap))|leaf(overlap-right(sync-overlap))]",
            "fire-and-forget-admitted",
            "overlapping[leaf(overlap-left(async-overlap))|leaf(overlap-right(async-overlap))]",
        ]
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let starts = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(parameters)
                if parameters.function_name == "golem::entity::invoke" =>
            {
                Some((
                    entry.oplog_index,
                    parameters.parent_start_index,
                    &entry.attribution,
                ))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let roots = starts
        .iter()
        .filter(|(_, _, attribution)| {
            matches!(
                attribution,
                PublicOplogEntryAttribution::Entity(context)
                    if context.invocation.entity.kind == PublicAgentEntityKind::ToolMiddleware
                        && context.invocation.entity.name == "streaming-overlapping"
                        && context.ancestors.is_empty()
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(roots.len(), 3, "one root plan per outer call mode");
    for (root, parent, _) in roots {
        assert!(
            parent.is_some(),
            "root entity invocations are parented to AgentInvocationStarted"
        );
        let children = starts
            .iter()
            .filter(|(_, parent, _)| *parent == Some(*root))
            .collect::<Vec<_>>();
        assert_eq!(children.len(), 2, "overlap must invoke the leaf twice");
        assert!(children.iter().all(|(_, _, attribution)| {
            matches!(
                attribution,
                PublicOplogEntryAttribution::Entity(context)
                    if context.invocation.entity.kind == PublicAgentEntityKind::Tool
                        && context.invocation.entity.name == "middleware-probe"
                        && context.ancestors.len() == 1
                        && context.ancestors[0].entity.kind
                            == PublicAgentEntityKind::ToolMiddleware
                        && context.ancestors[0].entity.name == "streaming-overlapping"
                        && context.ancestors[0].start_index == *root
            )
        }));
    }

    let short_circuit = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-short-circuit")
        .unwrap();
    let mut redeployed = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools,
    );
    install_middleware_chain(
        &mut redeployed,
        &agent_type,
        &tool_name,
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![(
            short_circuit.name.as_str(),
            empty_middleware_parameters(short_circuit),
        )],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(redeployed),
    );
    executor.simulated_crash(&worker_id).await?;
    let after_redeployment: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "middleware_probe_modes",
            data_value!("redeployed"),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        after_redeployment,
        [
            "short(sync-redeployed)",
            "fire-and-forget-admitted",
            "short(async-redeployed)",
        ]
    );
    let replayed_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let replayed_overlap_roots = replayed_oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.attribution,
                PublicOplogEntryAttribution::Entity(context)
                    if context.invocation.entity.kind == PublicAgentEntityKind::ToolMiddleware
                        && context.invocation.entity.name == "streaming-overlapping"
                        && context.ancestors.is_empty()
            ) && matches!(
                &entry.entry,
                PublicOplogEntry::Start(parameters)
                    if parameters.function_name == "golem::entity::invoke"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(replayed_overlap_roots.len(), 3);
    for root in replayed_overlap_roots {
        let descendants = replayed_oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.attribution,
                    PublicOplogEntryAttribution::Entity(context)
                        if context.invocation.entity.kind == PublicAgentEntityKind::Tool
                            && context.invocation.entity.name == "middleware-probe"
                            && context.ancestors.len() == 1
                            && context.ancestors[0].start_index == root.oplog_index
                ) && matches!(
                    &entry.entry,
                    PublicOplogEntry::Start(parameters)
                        if parameters.function_name == "golem::entity::invoke"
                )
            })
            .count();
        assert_eq!(descendants, 2);
    }
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn successful_middleware_parent_waits_for_admitted_dropped_child_observer(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoint_arrivals) =
        start_crash_checkpoint_server().await;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let middleware_component = executor
        .component(
            &context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .name("golem-it:tool-streaming-rust-middleware-early-return")
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let early = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-early-return")
        .unwrap();
    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let tool_name = ToolName::try_from("middleware-probe").unwrap();
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools,
    );
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &tool_name,
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![(early.name.as_str(), empty_middleware_parameters(early))],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-early-return");
    executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;

    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "middleware_probe_once",
        data_value!("parent"),
    );
    tokio::pin!(invocation);
    let child = tokio::select! {
        result = invocation.as_mut() => {
            panic!("successful parent completed before child admission: {result:?}")
        }
        child = next_crash_checkpoint(&mut checkpoint_arrivals, "middleware-early-child") => {
            child?
        }
    };
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), invocation.as_mut())
            .await
            .is_err(),
        "successful parent completed while its admitted child effect was still blocked"
    );
    child
        .release
        .send(())
        .expect("release admitted middleware child");
    let result: String = invocation.await?.into_typed()?;
    assert_eq!(result, "early-return(parent)");

    checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn typed_tool_output_stream_waits_for_producer_and_is_durable_through_nonidentity_middleware(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = || TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides()).await?;
    let (provider_checkpoint_port, provider_checkpoint_server, mut provider_checkpoints) =
        start_promise_checkpoint_server().await;
    let (caller_checkpoint_port, caller_checkpoint_server, mut caller_checkpoints) =
        start_promise_checkpoint_server().await;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let middleware_component = executor
        .component(
            &context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .name("golem-it:tool-streaming-rust-middleware-typed-output")
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let tool_name = ToolName::try_from("typed-output-stream").unwrap();
    let plain_deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools.clone(),
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(plain_deployment.clone()),
    );
    let env = HashMap::from([
        (
            "PROVIDER_PROMISE_CHECKPOINT_PORT".to_string(),
            provider_checkpoint_port.to_string(),
        ),
        (
            "CALLER_PROMISE_CHECKPOINT_PORT".to_string(),
            caller_checkpoint_port.to_string(),
        ),
    ]);
    let direct_id = agent_id!("ToolStreamingCaller", "typed-output-direct");
    let direct_worker = executor
        .start_agent_with(
            &caller_component.id,
            direct_id.clone(),
            env.clone(),
            Vec::new(),
        )
        .await?;
    let _output = executor.capture_output(&direct_worker).await?;
    let direct_executor = executor.clone();
    let direct_component = caller_component.clone();
    let direct_invocation_id = direct_id.clone();
    let direct_call = tokio::spawn(async move {
        direct_executor
            .invoke_and_await_agent(
                &direct_component,
                &direct_invocation_id,
                "consume_typed_output",
                data_value!(false, "direct"),
            )
            .await
    });
    let provider =
        next_promise_checkpoint(&mut provider_checkpoints, "typed-output-after-first").await?;
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            caller_checkpoints.recv()
        )
        .await
        .is_err(),
        "the caller must not receive the typed stream before its producer completes"
    );
    assert!(
        !direct_call.is_finished(),
        "the invocation result must remain hidden while the producer is gated"
    );
    {
        let oplog = executor
            .get_oplog(&direct_worker, OplogIndex::INITIAL)
            .await?;
        let entity_starts = oplog
            .iter()
            .filter_map(|entry| matches!(&entry.entry, PublicOplogEntry::Start(parameters) if parameters.function_name == "golem::entity::invoke").then_some(entry.oplog_index))
            .collect::<Vec<_>>();
        assert!(
            !entity_starts.is_empty()
                && oplog.iter().all(|entry| !matches!(&entry.entry, PublicOplogEntry::End(parameters) if entity_starts.contains(&parameters.start_index))),
            "the tool entity must remain unterminated while its producer gate is blocked"
        );
    }
    executor
        .complete_promise(
            &PromiseId {
                agent_id: direct_worker.clone(),
                oplog_idx: provider.oplog_idx,
            },
            Vec::new(),
        )
        .await?;
    let caller = next_promise_checkpoint(
        &mut caller_checkpoints,
        "caller-consumed-first-typed-output",
    )
    .await?;
    executor
        .complete_promise(
            &PromiseId {
                agent_id: direct_worker.clone(),
                oplog_idx: caller.oplog_idx,
            },
            Vec::new(),
        )
        .await?;
    let direct = direct_call.await??;
    let direct: Vec<TypedOutputEvidence> = direct.into_typed()?;
    let expected = vec![
        TypedOutputEvidence {
            label: "direct-first".to_string(),
            ordinal: 11,
        },
        TypedOutputEvidence {
            label: "direct-second".to_string(),
            ordinal: 29,
        },
        TypedOutputEvidence {
            label: "direct-third".to_string(),
            ordinal: 47,
        },
    ];
    assert_eq!(direct, expected);

    let universal = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-universal-pass-through")
        .unwrap();
    let projection = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-typed-output-projection")
        .unwrap();
    let mut decorated_deployment = plain_deployment;
    install_middleware_chain(
        &mut decorated_deployment,
        &agent_type,
        &tool_name,
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![
            (
                universal.name.as_str(),
                empty_middleware_parameters(universal),
            ),
            (
                projection.name.as_str(),
                empty_middleware_parameters(projection),
            ),
        ],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(decorated_deployment),
    );
    let decorated_id = agent_id!("ToolStreamingCaller", "typed-output-decorated");
    let decorated_worker = executor
        .start_agent_with(&caller_component.id, decorated_id.clone(), env, Vec::new())
        .await?;
    let key = IdempotencyKey::fresh();
    let invocation_executor = executor.clone();
    let invocation_component = caller_component.clone();
    let invocation_id = decorated_id.clone();
    let invocation_key = key.clone();
    let invocation = tokio::spawn(async move {
        invocation_executor
            .invoke_and_await_agent_with_key(
                &invocation_component,
                &invocation_id,
                &invocation_key,
                "consume_typed_output",
                data_value!(true, "decorated"),
            )
            .await
    });
    let provider_promise =
        next_promise_checkpoint(&mut provider_checkpoints, "typed-output-after-first").await?;
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            caller_checkpoints.recv()
        )
        .await
        .is_err(),
        "projected output must remain hidden until the inner producer completes"
    );
    assert!(!invocation.is_finished());
    let blocked_oplog = executor
        .get_oplog(&decorated_worker, OplogIndex::INITIAL)
        .await?;
    let blocked_entity_starts = blocked_oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(parameters)
                if parameters.function_name == "golem::entity::invoke" =>
            {
                Some((entry.oplog_index, parameters.parent_start_index))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(blocked_entity_starts.len(), 3);
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &decorated_worker);
    let active = executor
        .active_entity_metadata(&owned_agent_id)
        .await
        .expect("streaming caller is active");
    for (start, parent) in &blocked_entity_starts {
        if parent.is_some_and(|parent| {
            blocked_entity_starts
                .iter()
                .any(|(start, _)| *start == parent)
        }) {
            assert!(
                blocked_oplog.iter().all(|entry| {
                    !matches!(&entry.entry, PublicOplogEntry::End(parameters) if parameters.start_index == *start)
                        && !matches!(&entry.entry, PublicOplogEntry::Cancelled(parameters) if parameters.start_index == *start)
                }),
                "entities still producing output must have no terminal after only their first item"
            );
        }
        assert!(
            active
                .tool_operations
                .operations
                .iter()
                .any(|operation| operation.start_index == Some(*start)),
            "every accepted operation must remain registered until its children settle"
        );
    }
    invocation.abort();
    let _ = invocation.await;
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides()).await?;
    let resumed_executor = executor.clone();
    let resumed_component = caller_component.clone();
    let resumed_id = decorated_id.clone();
    let resumed_key = key.clone();
    let resumed_call = tokio::spawn(async move {
        resumed_executor
            .invoke_and_await_agent_with_key(
                &resumed_component,
                &resumed_id,
                &resumed_key,
                "consume_typed_output",
                data_value!(true, "decorated"),
            )
            .await
    });
    executor
        .complete_promise(
            &PromiseId {
                agent_id: decorated_worker.clone(),
                oplog_idx: provider_promise.oplog_idx,
            },
            Vec::new(),
        )
        .await?;
    let caller_promise = next_promise_checkpoint(
        &mut caller_checkpoints,
        "caller-consumed-first-typed-output",
    )
    .await?;
    executor
        .complete_promise(
            &PromiseId {
                agent_id: decorated_worker.clone(),
                oplog_idx: caller_promise.oplog_idx,
            },
            Vec::new(),
        )
        .await?;
    let resumed = resumed_call.await??;
    let decorated: Vec<TypedOutputEvidence> = resumed.into_typed()?;
    assert_eq!(
        decorated,
        expected
            .into_iter()
            .map(|item| TypedOutputEvidence {
                label: item.label.replace("direct", "decorated"),
                ordinal: item.ordinal,
            })
            .collect::<Vec<_>>()
    );
    let oplog = executor
        .get_oplog(&decorated_worker, OplogIndex::INITIAL)
        .await?;
    let entity_start_indices = oplog
        .iter()
        .filter_map(|entry| matches!(&entry.entry, PublicOplogEntry::Start(parameters) if parameters.function_name == "golem::entity::invoke").then_some(entry.oplog_index))
        .collect::<Vec<_>>();
    let entity_ends = oplog
        .iter()
        .filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(parameters) if entity_start_indices.contains(&parameters.start_index)))
        .count();
    assert_eq!(
        entity_start_indices.len(),
        3,
        "universal middleware, typed middleware, and provider execute once each"
    );
    assert_eq!(
        entity_ends, 3,
        "universal middleware, typed middleware, and provider terminate once each"
    );
    assert!(provider_checkpoints.try_recv().is_err());
    assert!(caller_checkpoints.try_recv().is_err());
    provider_checkpoint_server.abort();
    caller_checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn typed_tool_output_tcp_atomic_checkpoint_survives_executor_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = || TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides()).await?;
    let (
        provider_checkpoint_port,
        provider_checkpoint_gate_port,
        provider_checkpoint_server,
        mut provider_checkpoints,
    ) = start_crash_checkpoint_server().await;
    let (
        caller_checkpoint_port,
        caller_checkpoint_gate_port,
        caller_checkpoint_server,
        mut caller_checkpoints,
    ) = start_crash_checkpoint_server().await;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let middleware_component = executor
        .component(
            &context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .name("golem-it:tool-streaming-rust-middleware-typed-output-tcp-restart")
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let universal = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-universal-pass-through")
        .unwrap();
    let projection = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-typed-output-projection")
        .unwrap();
    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let tool_name = ToolName::try_from("typed-output-stream").unwrap();
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools,
    );
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &tool_name,
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![
            (
                universal.name.as_str(),
                empty_middleware_parameters(universal),
            ),
            (
                projection.name.as_str(),
                empty_middleware_parameters(projection),
            ),
        ],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let agent_id = agent_id!("ToolStreamingCaller", "typed-output-tcp-restart");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "PROVIDER_CRASH_CHECKPOINT_PORT".to_string(),
                    provider_checkpoint_port.to_string(),
                ),
                (
                    "PROVIDER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    provider_checkpoint_gate_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_PORT".to_string(),
                    caller_checkpoint_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    caller_checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let key = IdempotencyKey::fresh();
    let invocation_executor = executor.clone();
    let invocation_component = caller_component.clone();
    let invocation_id = agent_id.clone();
    let invocation_key = key.clone();
    let invocation = tokio::spawn(async move {
        invocation_executor
            .invoke_and_await_agent_with_key(
                &invocation_component,
                &invocation_id,
                &invocation_key,
                "consume_typed_output",
                data_value!(true, "tcp-restart"),
            )
            .await
    });
    let provider_gate =
        next_crash_checkpoint(&mut provider_checkpoints, "typed-output-after-first").await?;
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            caller_checkpoints.recv()
        )
        .await
        .is_err(),
        "the typed stream must not reach the caller before producer completion"
    );
    assert!(!invocation.is_finished());
    let before_crash = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let entity_starts = before_crash
        .iter()
        .filter_map(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Start(parameters) if parameters.function_name == "golem::entity::invoke")
                .then_some(entry.oplog_index)
        })
        .collect::<Vec<_>>();
    assert_eq!(entity_starts.len(), 3);
    assert!(
        before_crash.iter().all(|entry| {
            !matches!(&entry.entry, PublicOplogEntry::End(parameters) if entity_starts.contains(&parameters.start_index))
        }),
        "no entity result may become visible before the producer finishes"
    );
    assert!(
        before_crash.iter().all(|entry| {
            !matches!(&entry.entry, PublicOplogEntry::Start(parameters) if parameters.function_name == "golem::tool::internal::observe-results")
        }),
        "typed stream reconstruction must not use a separate result-observation call"
    );

    invocation.abort();
    let _ = invocation.await;
    drop(provider_gate.release);
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides()).await?;
    let resumed_executor = executor.clone();
    let resumed_component = caller_component.clone();
    let resumed_id = agent_id.clone();
    let resumed_key = key.clone();
    let resumed = tokio::spawn(async move {
        resumed_executor
            .invoke_and_await_agent_with_key(
                &resumed_component,
                &resumed_id,
                &resumed_key,
                "consume_typed_output",
                data_value!(true, "tcp-restart"),
            )
            .await
    });
    let replayed_provider_gate =
        next_crash_checkpoint(&mut provider_checkpoints, "typed-output-after-first").await?;
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            caller_checkpoints.recv()
        )
        .await
        .is_err(),
        "the replayed typed stream must not reach the caller before producer completion"
    );
    replayed_provider_gate
        .release
        .send(())
        .expect("release replayed provider TCP atomic gate");
    let replayed_caller_gate = next_crash_checkpoint(
        &mut caller_checkpoints,
        "caller-consumed-first-typed-output",
    )
    .await?;
    replayed_caller_gate
        .release
        .send(())
        .expect("release replayed caller TCP atomic gate");
    let result: Vec<TypedOutputEvidence> = resumed.await??.into_typed()?;
    assert_eq!(
        result,
        vec![
            TypedOutputEvidence {
                label: "tcp-restart-first".to_string(),
                ordinal: 11,
            },
            TypedOutputEvidence {
                label: "tcp-restart-second".to_string(),
                ordinal: 29,
            },
            TypedOutputEvidence {
                label: "tcp-restart-third".to_string(),
                ordinal: 47,
            },
        ]
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let entity_ends = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::End(parameters) if entity_starts.contains(&parameters.start_index))
        })
        .count();
    assert_eq!(
        entity_ends, 3,
        "every middleware and provider layer settles once"
    );
    assert!(oplog.iter().all(|entry| {
        !matches!(&entry.entry, PublicOplogEntry::Start(parameters) if parameters.function_name == "golem::tool::internal::observe-results")
    }));
    assert!(provider_checkpoints.try_recv().is_err());
    assert!(caller_checkpoints.try_recv().is_err());
    provider_checkpoint_server.abort();
    caller_checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn typed_tool_input_stream_is_durable_through_universal_and_nonidentity_middleware(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = || TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides()).await?;
    let (provider_port, provider_server, mut provider_checkpoints) =
        start_promise_checkpoint_server().await;
    let (caller_port, caller_server, mut caller_checkpoints) =
        start_promise_checkpoint_server().await;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let middleware_component = executor
        .component(
            &context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .name("golem-it:tool-streaming-rust-middleware-typed-input")
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let tool_name = ToolName::try_from("typed-input-stream").unwrap();
    let plain_deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools,
    );
    let universal = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-universal-pass-through")
        .unwrap();
    let projection = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-typed-input-projection")
        .unwrap();
    let mut decorated_deployment = plain_deployment.clone();
    install_middleware_chain(
        &mut decorated_deployment,
        &agent_type,
        &tool_name,
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![
            (
                universal.name.as_str(),
                empty_middleware_parameters(universal),
            ),
            (
                projection.name.as_str(),
                empty_middleware_parameters(projection),
            ),
        ],
    );
    let env = HashMap::from([
        (
            "PROVIDER_PROMISE_CHECKPOINT_PORT".to_string(),
            provider_port.to_string(),
        ),
        (
            "CALLER_PROMISE_CHECKPOINT_PORT".to_string(),
            caller_port.to_string(),
        ),
    ]);
    let mut invocations = Vec::new();
    let mut promises = Vec::new();
    let mut workers = Vec::new();
    for (decorated, name, deployment) in [
        (false, "typed-input-direct", plain_deployment),
        (true, "typed-input-decorated", decorated_deployment),
    ] {
        environment_state.set_tool_deployment(
            context.default_environment_id,
            caller_component.id,
            caller_component.revision,
            Some(deployment),
        );
        let agent_id = agent_id!("ToolStreamingCaller", name);
        let worker_id = executor
            .start_agent_with(
                &caller_component.id,
                agent_id.clone(),
                env.clone(),
                Vec::new(),
            )
            .await?;
        let key = IdempotencyKey::fresh();
        let invocation_executor = executor.clone();
        let invocation_component = caller_component.clone();
        let invocation_id = agent_id.clone();
        let invocation_key = key.clone();
        let invocation = tokio::spawn(async move {
            invocation_executor
                .invoke_and_await_agent_with_key(
                    &invocation_component,
                    &invocation_id,
                    &invocation_key,
                    "produce_typed_input",
                    data_value!(decorated),
                )
                .await
        });
        let caller_promise =
            next_promise_checkpoint(&mut caller_checkpoints, "typed-input-caller-produced-first")
                .await?;
        wait_for_promise_checkpoint_to_await(&executor, &worker_id, &caller_promise).await?;
        let provider_promise = next_promise_checkpoint(
            &mut provider_checkpoints,
            "typed-input-provider-consumed-first",
        )
        .await?;
        wait_for_promise_checkpoint_to_await(&executor, &worker_id, &provider_promise).await?;
        assert!(
            !invocation.is_finished(),
            "input drain completed before the caller producer was released"
        );
        invocations.push(invocation);
        promises.push((worker_id.clone(), caller_promise, provider_promise));
        workers.push((decorated, agent_id, worker_id, key));
    }
    for invocation in invocations {
        invocation.abort();
        let _ = invocation.await;
    }
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides()).await?;
    for (worker_id, caller_promise, provider_promise) in promises {
        executor
            .complete_promise(
                &PromiseId {
                    agent_id: worker_id.clone(),
                    oplog_idx: caller_promise.oplog_idx,
                },
                Vec::new(),
            )
            .await?;
        executor
            .complete_promise(
                &PromiseId {
                    agent_id: worker_id,
                    oplog_idx: provider_promise.oplog_idx,
                },
                Vec::new(),
            )
            .await?;
    }
    for (decorated, agent_id, worker_id, key) in workers {
        let evidence: Vec<TypedInputEvidence> = executor
            .invoke_and_await_agent_with_key(
                &caller_component,
                &agent_id,
                &key,
                "produce_typed_input",
                data_value!(decorated),
            )
            .await?
            .into_typed()?;
        let expected = if decorated {
            vec![
                ("jade-first", 83),
                ("jade-second", 131),
                ("jade-third", 197),
            ]
        } else {
            vec![
                ("amber-first", 17),
                ("amber-second", 43),
                ("amber-third", 71),
            ]
        };
        assert_eq!(
            evidence,
            expected
                .into_iter()
                .map(|(label, ordinal)| TypedInputEvidence {
                    label: label.to_string(),
                    ordinal,
                })
                .collect::<Vec<_>>()
        );
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let starts = oplog
            .iter()
            .filter_map(|entry| {
                matches!(&entry.entry, PublicOplogEntry::Start(parameters) if parameters.function_name == "golem::entity::invoke")
                    .then_some(entry.oplog_index)
            })
            .collect::<Vec<_>>();
        let ends = oplog
            .iter()
            .filter(|entry| {
                matches!(&entry.entry, PublicOplogEntry::End(parameters) if starts.contains(&parameters.start_index))
            })
            .count();
        assert_eq!(starts.len(), if decorated { 3 } else { 1 });
        assert_eq!(ends, starts.len());
    }
    if let Ok(unexpected) = provider_checkpoints.try_recv() {
        panic!("unexpected provider checkpoint after reconstruction: {unexpected:?}");
    }
    assert!(caller_checkpoints.try_recv().is_err());
    provider_server.abort();
    caller_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn concurrent_tool_attempt_identity_survives_reordered_admission_and_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(ReorderedToolActivationService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let (
        provider_checkpoint_port,
        provider_checkpoint_gate_port,
        provider_checkpoint_server,
        mut provider_checkpoint_arrivals,
    ) = start_crash_checkpoint_server().await;
    let (
        caller_checkpoint_port,
        caller_checkpoint_gate_port,
        caller_checkpoint_server,
        mut caller_checkpoint_arrivals,
    ) = start_crash_checkpoint_server().await;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "attempt-identity-replay");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "PROVIDER_CRASH_CHECKPOINT_PORT".to_string(),
                    provider_checkpoint_port.to_string(),
                ),
                (
                    "PROVIDER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    provider_checkpoint_gate_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_PORT".to_string(),
                    caller_checkpoint_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    caller_checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);

    let call = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "concurrent_attempt_identity_replay",
        data_value!(),
    );
    let crash_and_replay = async {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            environment_state.wait_for_first_call_to_block(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("first activation lookup did not block"))?;
        let original_provider = next_crash_checkpoint(
            &mut provider_checkpoint_arrivals,
            "attempt-identity-accepted",
        )
        .await?;
        original_provider
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("original accepted tool checkpoint was dropped"))?;
        wait_for_active_tool_operations(&executor, &owned_agent_id, 0).await?;
        environment_state.release_first_call();

        let original = next_crash_checkpoint(
            &mut caller_checkpoint_arrivals,
            "concurrent-attempt-identities",
        )
        .await?;
        assert_eq!(environment_state.activation_calls(), 2);

        executor.simulated_crash(&worker_id).await?;
        drop(original.release);

        assert_eq!(
            environment_state.activation_calls(),
            2,
            "replay must claim both attempts without repeating admission"
        );
        let replayed = next_crash_checkpoint(
            &mut caller_checkpoint_arrivals,
            "concurrent-attempt-identities",
        )
        .await?;
        replayed
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("replayed attempt-identity checkpoint was dropped"))?;
        Ok::<_, anyhow::Error>(())
    };
    let (result, ()) = tokio::try_join!(call, crash_and_replay)?;
    let outcomes: Vec<String> = result.into_typed()?;
    assert_eq!(outcomes, ["rejected", "accepted"]);
    assert_eq!(environment_state.activation_calls(), 2);
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let accepted_start = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == "golem::entity::invoke" => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("second attempt retained its accepted Start");
    let rejected_start = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params)
                if params.function_name == "golem::tool::internal::invocation-rejected" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("first attempt retained its rejected Start");
    assert!(
        accepted_start < rejected_start,
        "the later accepted attempt must be durably ordered before the earlier rejected attempt"
    );
    assert!(
        executor
            .active_entity_metadata(&owned_agent_id)
            .await
            .is_none_or(|active| active.tool_operations.operations.is_empty())
    );

    executor.delete_worker(&worker_id).await?;
    provider_checkpoint_server.abort();
    caller_checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn native_entity_vetoes_owner_suspension_until_cancelled(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let mut streaming = native_streaming_tool_metadata();
    streaming.commands.nodes[0].name = "native-streaming".to_string();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            native_tool_metadata: Some(streaming.clone()),
            configure: Some(Arc::new(|config| {
                config.suspend.wait_suspend_check_interval = Duration::from_millis(100);
            })),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let mut state = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        "ToolStreamingCaller",
        metadata.tools,
    );
    let native = native_deployment_state(
        context.account_id,
        "ToolStreamingCaller",
        streaming,
        native_test_tool_metadata(),
    );
    state.registered_tools.extend(native.registered_tools);
    for (owner, bindings) in native.tool_bindings {
        state
            .tool_bindings
            .entry(owner)
            .or_default()
            .extend(bindings);
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(state),
    );
    let agent = agent_id!("ToolStreamingCaller", "native-veto-long-clock");
    let worker = executor
        .start_agent(&caller_component.id, agent.clone())
        .await?;
    let gate = executor
        .invoke_and_await_agent(&caller_component, &agent, "native_veto_gate", data_value!())
        .await?
        .into_return_value()
        .expect("promise gate");
    let SchemaValue::Record { fields } = &gate else {
        panic!("promise record")
    };
    let SchemaValue::U64(index) = fields[1] else {
        panic!("promise oplog index")
    };
    let loads = executor.instance_load_count(&worker);
    let effects = executor.native_test_effect_count();
    let owned = OwnedAgentId::new(context.default_environment_id, &worker);
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent,
        "native_veto_with_long_clock",
        crate::raw_params(vec![gate]),
    );
    tokio::pin!(invocation);
    tokio::select! {
        result = &mut invocation => panic!("native gate finished early: {result:?}"),
        started = tokio::time::timeout(Duration::from_secs(10), async {
            while executor.native_test_effect_count() == effects {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }) => { started?; }
    }
    tokio::select! {
        result = &mut invocation => panic!("native gate finished early: {result:?}"),
        _ = async {
            let until = tokio::time::Instant::now() + Duration::from_secs(3);
            while tokio::time::Instant::now() < until {
                tokio::time::sleep(Duration::from_millis(100)).await;
                assert!(executor.worker_is_loaded(&owned).await, "executing native must veto unloading");
                assert_eq!(executor.instance_load_count(&worker), loads);
                assert!(!executor.get_oplog(&worker, OplogIndex::INITIAL).await.unwrap()
                    .iter().any(|entry| matches!(entry.entry, PublicOplogEntry::Suspend(_))),
                    "executing native must veto automatic suspension");
            }
        } => {}
    }
    assert_eq!(executor.native_test_effect_count(), effects + 1);
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker.clone(),
                oplog_idx: OplogIndex::from_u64(index),
            },
            vec![],
        )
        .await?;
    tokio::select! {
        result = &mut invocation => panic!("timer finished before unloading: {result:?}"),
        unloaded = tokio::time::timeout(Duration::from_secs(10), async {
            executor.wait_for_status(&worker, AgentStatus::Suspended, Duration::from_secs(8)).await?;
            while executor.worker_is_loaded(&owned).await {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok::<(), anyhow::Error>(())
        }) => { unloaded??; }
    }
    let result = tokio::time::timeout(Duration::from_secs(45), invocation)
        .await??
        .into_typed::<Vec<String>>()?;
    assert_eq!(result, ["cancelled", "20", "settled"]);
    assert!(executor.instance_load_count(&worker) > loads);
    assert_eq!(
        executor.native_test_effect_count(),
        effects + 1,
        "cancelled native must not repeat live effects during replay"
    );
    let follow_up = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent,
            "clock_races_through_tool_entity",
            data_value!(),
        )
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(follow_up, ["2", "2", "timer", "flag", "settled"]);
    assert_eq!(executor.native_test_effect_count(), effects + 1);
    executor.delete_worker(&worker).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn native_tool_config_uses_owner_binding_without_host_privilege(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::agent_config::CanonicalAgentConfigPath;
    use golem_common::model::worker::AgentConfigEntryDto;

    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let mut streaming = native_streaming_tool_metadata();
    streaming.commands.nodes[0].name = "native-streaming".to_string();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            native_tool_metadata: Some(streaming.clone()),
            ..Default::default()
        },
    )
    .await?;
    let configured = |allowed: &str, denied: &str| {
        vec![
            AgentConfigEntryDto {
                path: vec!["allowed".to_string()],
                value: serde_json::json!(allowed).into(),
            },
            AgentConfigEntryDto {
                path: vec!["denied".to_string()],
                value: serde_json::json!(denied).into(),
            },
        ]
    };
    let narrowed_component = executor
        .component_dep(&context.default_environment_id, caller)
        .unique()
        .name("native-config-narrowed")
        .with_agent_config(
            "ToolStreamingCaller",
            configured("narrowed-allowed", "narrowed-denied"),
        )
        .store()
        .await?;
    let empty_component = executor
        .component_dep(&context.default_environment_id, caller)
        .unique()
        .name("native-config-empty")
        .with_agent_config(
            "ToolStreamingCaller",
            configured("empty-allowed", "empty-denied"),
        )
        .store()
        .await?;
    let owner = ToolBindingOwner::AgentType {
        agent_type_name: AgentTypeName("ToolStreamingCaller".to_string()),
    };
    let tool_name = ToolName::try_from("native-streaming").unwrap();
    for (component, scope) in [
        (
            &narrowed_component,
            ConfigKeyScope::Keys(BTreeSet::from([CanonicalAgentConfigPath(vec![
                "allowed".to_string(),
            ])])),
        ),
        (&empty_component, ConfigKeyScope::Keys(BTreeSet::new())),
    ] {
        let mut deployment = native_deployment_state(
            context.account_id,
            "ToolStreamingCaller",
            streaming.clone(),
            native_test_tool_metadata(),
        );
        deployment
            .tool_bindings
            .get_mut(&owner)
            .unwrap()
            .get_mut(&tool_name)
            .unwrap()
            .config_keys_readable = scope;
        environment_state.set_tool_deployment(
            context.default_environment_id,
            component.id,
            component.revision,
            Some(deployment),
        );
    }

    let narrowed = agent_id!("ToolStreamingCaller", "native-config-narrowed");
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &narrowed_component,
                &narrowed,
                "native_config",
                data_value!("allowed")
            )
            .await?
            .into_typed::<String>()?,
        "narrowed-allowed"
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(
                &narrowed_component,
                &narrowed,
                "native_config",
                data_value!("denied")
            )
            .await?
            .into_typed::<String>()?,
        "denied"
    );
    let empty = agent_id!("ToolStreamingCaller", "native-config-empty");
    for key in ["allowed", "denied"] {
        assert_eq!(
            executor
                .invoke_and_await_agent(&empty_component, &empty, "native_config", data_value!(key))
                .await?
                .into_typed::<String>()?,
            "denied"
        );
    }
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn native_tool_runs_all_modes_streams_cancellation_overlap_and_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let mut streaming = native_streaming_tool_metadata();
    streaming.commands.nodes[0].name = "native-streaming".to_string();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            native_tool_metadata: Some(streaming.clone()),
            ..Default::default()
        },
    )
    .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(native_deployment_state(
            context.account_id,
            "ToolStreamingCaller",
            streaming.clone(),
            native_test_tool_metadata(),
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "native-runtime");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let helper_effects_before = executor.native_test_helper_effect_count();
    let evidence: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "native_modes_stream_cancel_overlap",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        &evidence[..6],
        [
            "sync",
            "fire-and-forget",
            "async",
            "stream",
            "cancel",
            "overlap"
        ]
    );
    assert_eq!(evidence[6], "5");
    let helper_effects = executor.native_test_helper_effect_count();
    assert_eq!(helper_effects, helper_effects_before + 1);

    let isolated_context = TestContext::new(last_unique_id);
    let isolated_environment_state = Arc::new(TestEnvironmentStateService::default());
    let isolated_executor = start_with_overrides(
        deps,
        &isolated_context,
        TestExecutorOverrides {
            environment_state_service: Some(isolated_environment_state.clone()),
            native_tool_metadata: Some(streaming.clone()),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(isolated_executor.native_test_helper_effect_count(), 0);
    let isolated_caller_component = isolated_executor
        .component_dep(&isolated_context.default_environment_id, caller)
        .store()
        .await?;
    isolated_environment_state.set_tool_deployment(
        isolated_context.default_environment_id,
        isolated_caller_component.id,
        isolated_caller_component.revision,
        Some(native_deployment_state(
            isolated_context.account_id,
            "ToolStreamingCaller",
            streaming,
            native_test_tool_metadata(),
        )),
    );
    isolated_executor
        .invoke_and_await_agent(
            &isolated_caller_component,
            &agent_id!("ToolStreamingCaller", "isolated-native-runtime"),
            "native_modes_stream_cancel_overlap",
            data_value!(),
        )
        .await?;
    assert_eq!(isolated_executor.native_test_helper_effect_count(), 1);
    assert_eq!(executor.native_test_helper_effect_count(), helper_effects);

    executor.simulated_crash(&worker_id).await?;
    executor.resume(&worker_id, true).await?;
    let count: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "native_effect_count",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        count, "5",
        "completed replay must not repeat native effects"
    );
    assert_eq!(executor.native_test_helper_effect_count(), helper_effects);
    assert!(environment_state.tool_deployment_calls() >= 5);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn rust_generated_client_streams_live_and_handles_edges(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let (http_port, http_server, mut first_http_uploads, mut complete_http_uploads) =
        start_gated_http_server().await;
    let (trap_once_port, trap_once_server, _trap_attempts) = start_trap_attempt_server().await;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "rust-live");
    let mut env = HashMap::new();
    env.insert("HTTP_GATE_PORT".to_string(), http_port.to_string());
    env.insert("TRAP_ONCE_PORT".to_string(), trap_once_port.to_string());
    let worker_id = executor
        .start_agent_with(&caller_component.id, agent_id.clone(), env, Vec::new())
        .await?;

    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let principal_context: Vec<String> = executor
        .invoke_and_await_agent_as_principal(
            &caller_component,
            &agent_id,
            Principal::GolemUser(GolemUserPrincipal {
                account_id: AccountId::new(),
            }),
            "principal_context",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        principal_context,
        ["golem-user", "golem-user", "golem-user"]
    );

    let marker_result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        executor.invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "marker_before_eof",
            data_value!(b"first".to_vec(), b"second".to_vec()),
        ),
    )
    .await;
    let marker: StreamEvidence = match marker_result {
        Ok(result) => result?.into_typed()?,
        Err(_) => {
            let active = executor.active_entity_metadata(&owned_agent_id).await;
            anyhow::bail!("marker-before-EOF call timed out; active metadata: {active:#?}")
        }
    };
    assert_evidence(&marker, b"marker:firstsecond", 2, 11);

    let alternating_chunks = (0..128_u16)
        .map(|index| vec![(index % 251) as u8; 4096])
        .collect::<Vec<_>>();
    let alternating_output = alternating_chunks.concat();
    let alternating: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "alternating_echo",
            data_value!(128_u32, 4096_u32),
        )
        .await?
        .into_typed()?;
    assert_evidence(
        &alternating,
        &alternating_output,
        128,
        alternating_output.len() as u64,
    );

    let binary_input = vec![0, 1, 255, 128, 0, 13, 10];
    let echo: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!("echo", binary_input.clone(), 2_u32),
        )
        .await?
        .into_typed()?;
    assert_evidence(&echo, &binary_input, 4, binary_input.len() as u64);

    let empty: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!("empty", Vec::<u8>::new(), 1_u32),
        )
        .await?
        .into_typed()?;
    assert_evidence(&empty, b"", 0, 0);

    let binary: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!("binary", Vec::<u8>::new(), 1_u32),
        )
        .await?
        .into_typed()?;
    assert_evidence(&binary, &[0, 255, 1, 128, 0, 13, 10, 254], 0, 0);

    let fragmented: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!("fragmented", Vec::<u8>::new(), 1_u32),
        )
        .await?
        .into_typed()?;
    assert_evidence(&fragmented, &[b'f', b'r', b'a', b'g', 0, 255], 0, 0);

    let partial_error: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "result_before_stdout",
            data_value!("declared-error"),
        )
        .await?
        .into_typed()?;
    assert_eq!(partial_error.output, b"marker:");
    assert!(partial_error.completion.contains("Declared"));

    let concurrent: Vec<Vec<u8>> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "two_live_calls",
            data_value!(b"left".to_vec(), b"right".to_vec()),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        concurrent,
        [b"marker:left".to_vec(), b"marker:right".to_vec()]
    );

    let left_first = b"left-first\0".to_vec();
    let left_rest = vec![255, b'l', b'e', b'f', b't'];
    let right_first = b"right-first\0".to_vec();
    let right_rest = vec![128, b'r', b'i', b'g', b'h', b't'];
    let http: Vec<StreamEvidence> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "two_http_calls",
            data_value!(
                left_first.clone(),
                left_rest.clone(),
                right_first.clone(),
                right_rest.clone()
            ),
        )
        .await?
        .into_typed()?;
    let left_body = [left_first.clone(), left_rest.clone()].concat();
    let right_body = [right_first.clone(), right_rest.clone()].concat();
    let left_output = [b"http-left:".as_slice(), left_body.as_slice()].concat();
    let right_output = [b"http-right:".as_slice(), right_body.as_slice()].concat();
    assert_evidence(&http[0], &left_output, 2, left_body.len() as u64);
    assert_evidence(&http[1], &right_output, 2, right_body.len() as u64);

    let mut initial_uploads = BTreeMap::new();
    let mut complete_uploads = BTreeMap::new();
    for _ in 0..2 {
        let (tag, bytes) =
            tokio::time::timeout(std::time::Duration::from_secs(5), first_http_uploads.recv())
                .await
                .expect("initial gated HTTP upload checkpoint timed out")
                .expect("initial gated HTTP upload channel closed");
        initial_uploads.insert(tag, bytes);

        let (tag, bytes) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            complete_http_uploads.recv(),
        )
        .await
        .expect("completed gated HTTP upload checkpoint timed out")
        .expect("completed gated HTTP upload channel closed");
        complete_uploads.insert(tag, bytes);
    }
    assert_eq!(
        initial_uploads,
        BTreeMap::from([
            ("left".to_string(), left_first),
            ("right".to_string(), right_first),
        ])
    );
    assert_eq!(
        complete_uploads,
        BTreeMap::from([
            ("left".to_string(), left_body),
            ("right".to_string(), right_body),
        ])
    );

    let capable_input = b"staged-capable-input".to_vec();
    let capable: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!("/capable.bin", capable_input.clone()),
        )
        .await?
        .into_typed()?;
    assert_evidence(&capable, &capable_input, 1, capable_input.len() as u64);
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/capable.bin")
            .await?,
        capable_input
    );

    let stdout_only_capable_input = b"stdout-only-capable".to_vec();
    let started_contracts: Vec<Vec<u8>> = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        executor.invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "started_invocation_contracts",
            data_value!(
                "/stdout-only-capable.bin",
                stdout_only_capable_input.clone()
            ),
        ),
    )
    .await
    .expect("started invocation contract checks timed out")?
    .into_typed()?;
    assert_eq!(
        started_contracts,
        [
            Vec::new(),
            vec![b'f', b'r', b'a', b'g', 0, 255],
            b"cached-result".to_vec(),
            stdout_only_capable_input.clone(),
        ]
    );
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/stdout-only-capable.bin")
            .await?,
        stdout_only_capable_input
    );

    let raw_agent_id = agent_id!("ToolStreamingCaller", "rust-raw-modes");
    let raw_worker_id = executor
        .start_agent(&caller_component.id, raw_agent_id.clone())
        .await?;
    let raw_modes: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &raw_agent_id,
            "raw_modes_and_handles",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        raw_modes,
        ["invoke-and-await", "invoke", "async-invoke-and-await",]
    );

    let raw_handles: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &raw_agent_id,
            "raw_handle_lifecycles",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        raw_handles,
        [
            "out-of-order-get",
            "explicit-cancel",
            "result-detach",
            "stdout-detach",
            "stdout-operation-resume",
        ]
    );

    let observer_detach: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &raw_agent_id,
            "raw_observer_detach_and_fire_open",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        observer_detach,
        ["invoke-open-stdin", "invoke-and-await-observer-detach"]
    );
    let raw_oplog = executor
        .get_oplog(&raw_worker_id, OplogIndex::INITIAL)
        .await?;
    let raw_entity_starts = raw_oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::entity::invoke"
            )
        })
        .count();
    assert!(raw_entity_starts >= 9, "{raw_oplog:#?}");
    for start in raw_oplog.iter().filter_map(|entry| match &entry.entry {
        PublicOplogEntry::Start(params) if params.function_name == "golem::entity::invoke" => {
            Some(entry.oplog_index)
        }
        _ => None,
    }) {
        assert!(
            raw_oplog.iter().any(|entry| {
                matches!(&entry.entry, PublicOplogEntry::End(params) if params.start_index == start)
                    || matches!(&entry.entry, PublicOplogEntry::Cancelled(params) if params.start_index == start)
            }),
            "raw lifecycle entity Start {start} was not settled"
        );
    }
    executor.simulated_crash(&raw_worker_id).await?;
    let _: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &raw_agent_id,
            "replay_probe",
            data_value!(),
        )
        .await?
        .into_typed()?;
    let replayed_raw_oplog = executor
        .get_oplog(&raw_worker_id, OplogIndex::INITIAL)
        .await?;
    assert_eq!(
        replayed_raw_oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::Start(params)
                        if params.function_name == "golem::entity::invoke"
                )
            })
            .count(),
        raw_entity_starts,
        "replay after cancellation and observer/output detach repeated a tool effect"
    );
    if let Some(active) = executor
        .active_entity_metadata(&OwnedAgentId::new(
            context.default_environment_id,
            &raw_worker_id,
        ))
        .await
    {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }
    executor.delete_worker(&raw_worker_id).await?;

    eprintln!("starting stdout_drop_preserves_sibling");
    let stdout_drop_result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        executor.invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "stdout_drop_preserves_sibling",
            data_value!(),
        ),
    )
    .await;
    let stdout_drop: Vec<String> = match stdout_drop_result {
        Ok(result) => result?.into_typed()?,
        Err(_) => {
            let active = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                executor.active_entity_metadata(&owned_agent_id),
            )
            .await;
            let oplog = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                executor.get_oplog(&worker_id, OplogIndex::INITIAL),
            )
            .await;
            let oplog_tail = oplog.as_ref().ok().and_then(|result| {
                result
                    .as_ref()
                    .ok()
                    .map(|entries| entries.iter().rev().take(20).rev().collect::<Vec<_>>())
            });
            let oplog_error = match &oplog {
                Ok(Ok(_)) => None,
                Ok(Err(error)) => Some(format!("{error:#}")),
                Err(_) => Some("oplog read timed out".to_string()),
            };
            anyhow::bail!(
                "stdout_drop_preserves_sibling timed out for {owned_agent_id}; active metadata: {active:#?}; committed oplog tail: {oplog_tail:#?}; oplog error: {oplog_error:#?}"
            )
        }
    };
    eprintln!("completed stdout_drop_preserves_sibling");
    assert_eq!(stdout_drop, ["blocked-writer-woke", "sibling-completed"]);

    let edge_lifecycles: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "edge_lifecycles",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        edge_lifecycles,
        [
            "large-collect",
            "early-stdin-close",
            "early-stdout-close",
            "source-failure",
            "no-stream",
            "optional-streams",
            "pre-dispatch-rejection",
            "unused-stdout",
        ]
    );

    let nested: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!("nested", b"nested-input".to_vec(), 2_u32),
        )
        .await?
        .into_typed()?;
    assert_eq!(nested.output, b"marker:nested-input");
    assert!(nested.chunks_read > 0);
    assert_eq!(nested.bytes_read, 19);
    assert!(!nested.output_closed);
    assert_eq!(nested.completion, "ok");

    let nested_capable_input = b"nested-capable-input".to_vec();
    let nested_capable_result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        executor.invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!(
                "nested-capable-parent-end",
                nested_capable_input.clone(),
                2_u32
            ),
        ),
    )
    .await;
    let nested_capable: StreamEvidence = match nested_capable_result {
        Ok(result) => result?.into_typed()?,
        Err(_) => {
            let active = executor.active_entity_metadata(&owned_agent_id).await;
            anyhow::bail!("nested capable parent-end call timed out; active metadata: {active:#?}")
        }
    };
    assert_evidence(&nested_capable, b"nested-capable-started", 0, 0);
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/nested-capable-parent-end.bin")
            .await?,
        nested_capable_input
    );
    let nested_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let entity_starts = nested_oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == "golem::entity::invoke" => {
                Some((entry.oplog_index, params.parent_start_index))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        entity_starts.iter().any(|(_, parent)| {
            parent.is_some_and(|parent| entity_starts.iter().any(|(start, _)| *start == parent))
        }),
        "a nested tool Start must retain another entity Start as its oplog parent"
    );

    let capable_modes: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "capable_modes_and_cohorts",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        capable_modes,
        [
            "synchronous",
            "reverse-synchronous",
            "reverse-result-cohort",
            "parent-end-async",
            "parent-end-no-body",
            "parent-end-fire-and-forget",
        ]
    );
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/capable-order.log")
            .await?,
        b"R1R2S1S2P1P2".as_slice()
    );
    for (path, expected) in [
        ("/capable-sync.bin", b"sync".as_slice()),
        ("/capable-r1.bin", b"reverse-first".as_slice()),
        ("/capable-r2.bin", b"reverse-second".as_slice()),
        ("/capable-s1.bin", b"first".as_slice()),
        ("/capable-s2.bin", b"second".as_slice()),
        ("/capable-parent-async.bin", b"parent-async".as_slice()),
        ("/capable-parent-fire.bin", b"parent-fire".as_slice()),
    ] {
        assert_eq!(
            executor.get_file_contents(&worker_id, path).await?,
            expected
        );
    }
    assert!(
        executor
            .get_file_contents(&worker_id, "/capable-parent-no-body.bin")
            .await
            .is_err(),
        "cancelled no-body parent cohort member must never execute"
    );

    let nested_lane_input = b"nested-lane-inheritance".to_vec();
    let nested_lane: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!(
                "nested-capable:/capable-nested-outer.bin",
                nested_lane_input.clone()
            ),
        )
        .await?
        .into_typed()?;
    assert_evidence(
        &nested_lane,
        &nested_lane_input,
        1,
        nested_lane_input.len() as u64,
    );
    for path in ["/capable-nested-inner.bin", "/capable-nested-outer.bin"] {
        assert_eq!(
            executor.get_file_contents(&worker_id, path).await?,
            nested_lane_input
        );
    }
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/capable-order.log")
            .await?,
        b"R1R2S1S2P1P2NO".as_slice()
    );

    let trap_once_oplog_start = executor.oplog_max_index(&worker_id).await?;
    let trap_once_input = b"durable-capable-effect".to_vec();
    let trap_once: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!("trap-once:/capable-trap-once.bin", trap_once_input.clone()),
        )
        .await?
        .into_typed()?;
    assert_evidence(
        &trap_once,
        &trap_once_input,
        1,
        trap_once_input.len() as u64,
    );
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/capable-trap-once.bin")
            .await?,
        trap_once_input
    );
    let trap_once_oplog = executor
        .get_oplog(&worker_id, trap_once_oplog_start)
        .await?;
    let trap_once_entity_starts = trap_once_oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == "golem::entity::invoke" => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        trap_once_entity_starts.len(),
        1,
        "trap recovery must retain the original entity Start"
    );
    let durable_effects = trap_once_oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::api::generate_idempotency-key"
                        && params.parent_start_index == Some(trap_once_entity_starts[0])
            )
        })
        .count();
    assert_eq!(
        durable_effects, 1,
        "trap recovery must consume the recorded nested durable effect exactly once"
    );

    for (name, method, calls) in [
        ("interrupt-stdin", "hold_open_stdin", 3_u32),
        ("interrupt-stdout", "hold_unread_stdout", 1_u32),
    ] {
        let held_agent_id = agent_id!("ToolStreamingCaller", name);
        let held_worker_id = executor
            .start_agent(&caller_component.id, held_agent_id.clone())
            .await?;
        let held_owned_agent_id =
            OwnedAgentId::new(context.default_environment_id, &held_worker_id);
        executor
            .invoke_agent(
                &caller_component,
                &held_agent_id,
                method,
                data_value!(calls),
            )
            .await?;
        wait_for_active_tool_operations(&executor, &held_owned_agent_id, calls as usize).await?;
        if method == "hold_unread_stdout" {
            let stdout = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                loop {
                    if let Some(stdout) = executor
                        .active_entity_metadata(&held_owned_agent_id)
                        .await
                        .and_then(|active| active.tool_operations.operations.into_iter().next())
                        .and_then(|operation| operation.stdout)
                        && stdout.buffered_bytes == 16 * 1024 * 1024
                    {
                        break stdout;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .map_err(|_| {
                anyhow::anyhow!("await-result-before-read never reached bounded backpressure")
            })?;
            assert_eq!(stdout.accepted_bytes, 16 * 1024 * 1024);
            assert_eq!(stdout.delivered_bytes, 0);
            assert!(!stdout.terminal_selected);
        }
        executor.interrupt(&held_worker_id).await?;
        if let Some(active) = executor.active_entity_metadata(&held_owned_agent_id).await {
            assert!(active.tool_operations.operations.is_empty());
            assert!(active.lane.holder.is_none());
            assert_eq!(active.lane.active_invocation_count, 0);
            assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
        }
        executor.delete_worker(&held_worker_id).await?;
    }

    if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }
    http_server.abort();
    trap_once_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn rust_tool_trap_retries_owner_without_replay_overflow(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.retry = RetryConfig {
                    max_attempts: 2,
                    min_delay: std::time::Duration::from_millis(1),
                    max_delay: std::time::Duration::from_millis(1),
                    multiplier: 1.0,
                    max_jitter_factor: None,
                };
            })),
            ..Default::default()
        },
    )
    .await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "rust-trap");
    let result = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!("trap", Vec::<u8>::new(), 1_u32),
        )
        .await;
    let error = result.expect_err("the tool trap must fail the owner invocation");
    assert!(
        error
            .to_string()
            .contains("deterministic streaming tool trap"),
        "the original tool trap must survive owner recovery: {error:?}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn guest_trap_fences_a_blocked_sibling_and_drains_the_owner_group(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.retry = RetryConfig {
                    max_attempts: 1,
                    min_delay: std::time::Duration::from_millis(1),
                    max_delay: std::time::Duration::from_millis(1),
                    multiplier: 1.0,
                    max_jitter_factor: None,
                };
            })),
            ..Default::default()
        },
    )
    .await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    // Observe Store context destruction without re-entering a poisoned guest event loop.
    let healthy_agent = agent_id!("ToolStreamingCaller", "settlement-eof-control");
    let healthy_worker = executor
        .start_agent(&caller_component.id, healthy_agent.clone())
        .await?;
    let mut healthy_probe = executor.probe_entity_store_disposal(&healthy_worker);
    let healthy: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &healthy_agent,
            "marker_before_eof",
            data_value!(b"first".to_vec(), b"second".to_vec()),
        )
        .await?
        .into_typed()?;
    assert_evidence(&healthy, b"marker:firstsecond", 2, 11);
    let healthy_start =
        tokio::time::timeout(std::time::Duration::from_secs(30), healthy_probe.recv())
            .await?
            .expect("healthy Store context destruction receipt");
    let healthy_oplog = executor
        .get_oplog(&healthy_worker, OplogIndex::INITIAL)
        .await?;
    assert!(healthy_oplog.iter().any(|entry| {
        entry.oplog_index == healthy_start
            && matches!(&entry.entry, PublicOplogEntry::Start(params)
                if params.function_name == "golem::entity::invoke")
    }));
    assert!(healthy_oplog.iter().any(|entry| {
        matches!(&entry.entry, PublicOplogEntry::End(params)
            if params.start_index == healthy_start)
    }));
    eprintln!("EOF control Start {healthy_start}: Store context destroyed");

    let agent_id = agent_id!("ToolStreamingCaller", "trap-with-blocked-sibling");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let mut disposal_probe = executor.probe_entity_store_disposal(&worker_id);
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        executor.invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "trap_with_blocked_sibling",
            data_value!(),
        ),
    )
    .await;
    let result = match result {
        Ok(result) => result,
        Err(_) => {
            let active = executor.active_entity_metadata(&owned_agent_id).await;
            anyhow::bail!(
                "guest trap did not drain its blocked sibling; active metadata: {active:#?}"
            );
        }
    };
    let error = result.expect_err("the exact guest trap must fail the owner invocation");
    assert!(
        error
            .to_string()
            .contains("deterministic streaming tool trap"),
        "the original guest-trap provenance must survive owner-group fencing: {error:?}"
    );

    // Failure notification must follow destruction of both original contexts, not just fencing.
    let mut destroyed = Vec::new();
    for _ in 0..2 {
        let start = disposal_probe
            .try_recv()
            .expect("entity Store context must be destroyed before owner failure is returned");
        eprintln!(
            "trap scenario Start {start}: Store context destroyed before failure notification"
        );
        destroyed.push(start);
    }
    destroyed.sort();
    assert!(
        disposal_probe.try_recv().is_err(),
        "duplicate Store receipt"
    );

    let cleanup = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if executor
                .active_entity_metadata(&owned_agent_id)
                .await
                .is_none_or(|active| {
                    active.tool_operations.operations.is_empty()
                        && active.lane.holder.is_none()
                        && active.lane.active_invocation_count == 0
                        && active.slots.iter().all(|slot| slot.invocations.is_empty())
                })
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    if cleanup.is_err() {
        let active = executor.active_entity_metadata(&owned_agent_id).await;
        anyhow::bail!(
            "timed out waiting for guest-trap owner cleanup; active metadata: {active:#?}"
        );
    }
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let invocation_start = oplog
        .iter()
        .rev()
        .find_map(|entry| {
            matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_))
                .then_some(entry.oplog_index)
        })
        .expect("failed invocation has an AgentInvocationStarted entry");
    let entity_starts = oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(params)
                    if params.parent_start_index == Some(invocation_start)
                        && params.function_name == "golem::entity::invoke"
            )
        })
        .map(|entry| entry.oplog_index)
        .collect::<Vec<_>>();
    assert_eq!(
        entity_starts.len(),
        2,
        "the trapped operation and its already-running blocked sibling must both be durable"
    );
    assert!(
        oplog.iter().all(|entry| {
            !matches!(&entry.entry, PublicOplogEntry::End(params)
            if entity_starts.contains(&params.start_index))
                && !matches!(&entry.entry, PublicOplogEntry::Cancelled(params)
                if entity_starts.contains(&params.start_index))
        }),
        "owner fencing must not fabricate entity terminals"
    );
    assert_eq!(destroyed, entity_starts);
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| {
                entry.oplog_index > invocation_start
                    && matches!(entry.entry, PublicOplogEntry::Error(_))
            })
            .count(),
        1,
        "the owner must classify the original trap exactly once"
    );

    Ok(())
}

async fn run_owner_trap_sibling_lifecycle_case(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    provider: &PrecompiledComponent,
    caller: &PrecompiledComponent,
    lifecycle: &str,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.retry = RetryConfig {
                    max_attempts: 1,
                    min_delay: std::time::Duration::from_millis(1),
                    max_delay: std::time::Duration::from_millis(1),
                    multiplier: 1.0,
                    max_jitter_factor: None,
                };
            })),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", format!("trap-{lifecycle}"));
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let owner = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let mut disposals = executor.probe_entity_store_disposal(&worker_id);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        executor.invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "trap_with_sibling_lifecycle",
            data_value!(lifecycle),
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("{lifecycle} waiter did not wake after owner trap"))?;
    let error = result.expect_err("the exact guest trap must fail the owner invocation");
    assert!(
        error
            .to_string()
            .contains("deterministic streaming tool trap"),
        "the original trap must reach the {lifecycle} waiter: {error:?}"
    );

    let mut destroyed = Vec::new();
    for _ in 0..2 {
        destroyed.push(
            disposals
                .try_recv()
                .expect("both sibling Stores are destroyed before failure notification"),
        );
    }
    destroyed.sort();
    assert!(disposals.try_recv().is_err());
    if let Some(active) = executor.active_entity_metadata(&owner).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let invocation_start = oplog
        .iter()
        .rfind(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        .expect("failed invocation has an AgentInvocationStarted entry")
        .oplog_index;
    let entity_starts = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start)
                if start.parent_start_index == Some(invocation_start)
                    && start.function_name == "golem::entity::invoke" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        entity_starts.len(),
        2,
        "the sibling and trap are both admitted"
    );
    let mut sorted_starts = entity_starts.clone();
    sorted_starts.sort();
    assert_eq!(destroyed, sorted_starts);
    assert!(
        oplog.iter().all(|entry| {
            !matches!(&entry.entry, PublicOplogEntry::End(end) if entity_starts.contains(&end.start_index))
                && !matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if entity_starts.contains(&cancelled.start_index))
        }),
        "owner fencing must not fabricate normal terminals for {lifecycle}"
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| entry.oplog_index > invocation_start
                && matches!(entry.entry, PublicOplogEntry::Error(_)))
            .count(),
        1
    );

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let settled = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        settled
            .iter()
            .filter(|entry| {
                matches!(&entry.entry, PublicOplogEntry::Start(start)
                    if start.parent_start_index == Some(invocation_start)
                        && start.function_name == "golem::entity::invoke")
            })
            .count(),
        2,
        "the owner fence must prevent post-termination dispatch"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn guest_trap_retires_a_detached_child_before_waking_the_owner_waiter(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_owner_trap_sibling_lifecycle_case(last_unique_id, deps, provider, caller, "detached-child")
        .await
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn guest_trap_wakes_an_observed_sibling_future_and_retires_its_store(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_owner_trap_sibling_lifecycle_case(last_unique_id, deps, provider, caller, "observed-future")
        .await
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn detached_and_fire_and_forget_traps_fail_the_owner_without_entity_terminals(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.retry = RetryConfig {
                    max_attempts: 1,
                    min_delay: std::time::Duration::from_millis(1),
                    max_delay: std::time::Duration::from_millis(1),
                    multiplier: 1.0,
                    max_jitter_factor: None,
                };
            })),
            ..Default::default()
        },
    )
    .await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    for (name, method) in [
        ("trap-with-waiter", "collect"),
        ("trap-after-result-drop", "drop_trapping_result"),
        ("trap-fire-and-forget", "fire_and_forget_trap"),
    ] {
        let agent_id = agent_id!("ToolStreamingCaller", name);
        let worker_id = executor
            .start_agent(&caller_component.id, agent_id.clone())
            .await?;
        let result = if method == "collect" {
            executor
                .invoke_and_await_agent(
                    &caller_component,
                    &agent_id,
                    method,
                    data_value!("trap", Vec::<u8>::new(), 1_u32),
                )
                .await
        } else {
            executor
                .invoke_and_await_agent(&caller_component, &agent_id, method, data_value!())
                .await
        };
        let error = result.expect_err("a tool trap must fail its launching owner invocation");
        assert!(
            error
                .to_string()
                .contains("deterministic streaming tool trap"),
            "the original classified trap must reach the owner boundary for {method}: {error:?}"
        );

        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let invocation_start = oplog
            .iter()
            .rev()
            .find_map(|entry| {
                matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_))
                    .then_some(entry.oplog_index)
            })
            .expect("failed invocation has an AgentInvocationStarted entry");
        assert!(
            oplog.iter().all(|entry| {
                entry.oplog_index <= invocation_start
                    || !matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_))
            }),
            "{method} must not commit AgentInvocationFinished after a detached tool trap"
        );
        let entity_starts = oplog
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.parent_start_index == Some(invocation_start)
                        && params.function_name == "golem::entity::invoke" =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            entity_starts.len(),
            1,
            "{method} must retain exactly one accepted entity Start"
        );
        assert!(
            oplog.iter().all(|entry| {
                !matches!(
                    &entry.entry,
                    PublicOplogEntry::End(params)
                        if entity_starts.contains(&params.start_index)
                )
            }),
            "{method} must not append an ordinary entity terminal after a tool trap"
        );
        assert_eq!(
            oplog
                .iter()
                .filter(|entry| {
                    entry.oplog_index > invocation_start
                        && matches!(entry.entry, PublicOplogEntry::Error(_))
                })
                .count(),
            1,
            "{method} must classify and record the original owner trap exactly once"
        );
    }

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn capable_streams_enforce_completion_limits_without_leaks(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.limits.max_tool_attachment_bytes = 64;
            })),
            ..Default::default()
        },
    )
    .await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "capable-limits");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);

    let exact_input = vec![b'i'; 64];
    let exact: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!("stdout-exact:/capable-exact.bin", exact_input.clone()),
        )
        .await?
        .into_typed()?;
    assert_evidence(&exact, &[b'x'; 64], 1, 64);
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/capable-exact.bin")
            .await?,
        exact_input
    );

    let stdout_over: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!("stdout-over:/capable-stdout-over.bin", vec![b'o']),
        )
        .await?
        .into_typed()?;
    assert!(
        stdout_over.completion.contains("ResourceExhausted"),
        "stdout overflow must stay operation-fatal even when the provider catches the write error: {stdout_over:#?}"
    );

    let stdin_over: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!("/capable-stdin-over.bin", vec![b'i'; 65]),
        )
        .await?
        .into_typed()?;
    assert!(
        stdin_over.completion.contains("ResourceExhausted"),
        "stdin overflow must settle without launching a sidecar: {stdin_over:#?}"
    );

    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoint_arrivals) =
        start_crash_checkpoint_server().await;
    let overflow_agent = agent_id!("ToolStreamingCaller", "capable-overflow-replay");
    let overflow_worker = executor
        .start_agent_with(
            &caller_component.id,
            overflow_agent.clone(),
            HashMap::from([
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let overflow_owner = OwnedAgentId::new(context.default_environment_id, &overflow_worker);
    executor
        .invoke_agent(
            &caller_component,
            &overflow_agent,
            "hold_capable_overflow_terminal",
            data_value!(vec![b'i'; 65]),
        )
        .await?;
    let original_gate = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        checkpoint_arrivals.recv(),
    )
    .await
    .expect("original capable overflow terminal gate timed out")
    .expect("checkpoint server stopped");
    assert_eq!(original_gate.name, "capable-stdin-overflow-terminal");
    let original_start = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(active) = executor.active_entity_metadata(&overflow_owner).await
                && active.reached_oplog_marker.is_some()
                && active.tool_operations.operations.is_empty()
                && matches!(active.lane.holder, Some(OwnerInvocationId::Agent(_)))
                && active.lane.active_invocation_count == 1
                && active.slots.iter().all(|slot| slot.invocations.is_empty())
            {
                let oplog = executor
                    .get_oplog(&overflow_worker, OplogIndex::INITIAL)
                    .await
                    .expect("read capable overflow oplog");
                if let Some(start) = oplog.iter().find_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(params)
                        if params.function_name == "golem::entity::invoke" =>
                    {
                        Some(entry.oplog_index)
                    }
                    _ => None,
                }) {
                    break start;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("capable overflow did not settle without a sidecar");

    executor.simulated_crash(&overflow_worker).await?;
    drop(original_gate.release);
    let replayed_gate = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        checkpoint_arrivals.recv(),
    )
    .await
    .expect("replayed capable overflow terminal gate timed out")
    .expect("checkpoint server stopped");
    assert_eq!(replayed_gate.name, "capable-stdin-overflow-terminal");
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(active) = executor.active_entity_metadata(&overflow_owner).await
                && active.reached_oplog_marker.is_some()
                && active.tool_operations.operations.is_empty()
                && matches!(active.lane.holder, Some(OwnerInvocationId::Agent(_)))
                && active.lane.active_invocation_count == 1
                && active.slots.iter().all(|slot| slot.invocations.is_empty())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("replayed capable overflow created a body or lane registration");
    let overflow_oplog = executor
        .get_oplog(&overflow_worker, OplogIndex::INITIAL)
        .await?;
    assert_eq!(
        overflow_oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::Start(params)
                        if params.function_name == "golem::entity::invoke"
                )
            })
            .count(),
        1,
        "capable overflow replay must reuse its one no-body Start"
    );
    assert_eq!(
        overflow_oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::End(params) if params.start_index == original_start
                )
            })
            .count(),
        1,
        "capable overflow replay must retain exactly one no-body terminal"
    );
    replayed_gate
        .release
        .send(())
        .expect("release replayed capable overflow gate");
    assert!(
        executor
            .get_file_contents(&overflow_worker, "/must-not-run-after-overflow.bin")
            .await
            .is_err(),
        "replayed capable stdin overflow must not run a body"
    );
    checkpoint_server.abort();

    if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn completed_tool_replay_bypasses_current_attachment_memory_pressure(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("large_dynamic_memory")] large_dynamic_memory: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const SYSTEM_MEMORY_BYTES: u64 = 128 * 1024 * 1024;
    const ATTACHMENT_BYTES: usize = 2 * 1024 * 1024;
    const DESIRED_REPLAY_HEADROOM_BYTES: u64 = 3 * 1024 * 1024;

    let _attachment_pressure_test_permit = ATTACHMENT_PRESSURE_TEST_PERMIT
        .acquire()
        .await
        .expect("attachment-pressure test semaphore is not closed");
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.limits.max_tool_attachment_bytes = 2 * ATTACHMENT_BYTES;
                config.memory.system_memory_override = Some(SYSTEM_MEMORY_BYTES);
                config.memory.worker_memory_ratio = 1.0;
                config.memory.component_size_coefficient = 0.0;
                config.memory.acquire_retry_delay = std::time::Duration::from_millis(25);
            })),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let memory_component = executor
        .component_dep(&context.default_environment_id, large_dynamic_memory)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoint_arrivals) =
        start_crash_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "completed-replay-memory-pressure");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let input = vec![b'i'; ATTACHMENT_BYTES];
    executor
        .invoke_agent(
            &caller_component,
            &agent_id,
            "hold_completed_attachment_reconstruction_under_pressure",
            data_value!("/completed-under-pressure.bin", ATTACHMENT_BYTES as u64),
        )
        .await?;
    let original_checkpoint = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        checkpoint_arrivals.recv(),
    )
    .await
    .expect("original completed-attachment checkpoint timed out")
    .expect("checkpoint server stopped");
    assert_eq!(original_checkpoint.name, "completed-attachment-pressure");
    let target_memory = executor.worker_memory_requirement(&owned_agent_id).await?;

    let pressure_agent = agent_id!("LargeDynamicMemoryAgent", "attachment-replay-pressure");
    let pressure_worker = executor
        .start_agent(&memory_component.id, pressure_agent.clone())
        .await?;
    let pressure_owned = OwnedAgentId::new(context.default_environment_id, &pressure_worker);
    let initial_pressure_memory = executor.worker_memory_requirement(&pressure_owned).await?;
    let growth_budget = SYSTEM_MEMORY_BYTES
        .checked_sub(target_memory + initial_pressure_memory + DESIRED_REPLAY_HEADROOM_BYTES)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "initial workers leave no room for calibrated pressure: target={target_memory}, pressure={initial_pressure_memory}"
            )
        })?;
    let growth_mib = growth_budget / (1024 * 1024);
    anyhow::ensure!(growth_mib > 0, "calibrated pressure growth is empty");
    executor
        .invoke_agent(
            &memory_component,
            &pressure_agent,
            "run_with_memory_and_work",
            data_value!(growth_mib, 120_000_u64),
        )
        .await?;
    let pressure_memory = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let memory = executor
                .worker_memory_requirement(&pressure_owned)
                .await
                .expect("read pressure worker memory");
            if memory + target_memory + 2 * ATTACHMENT_BYTES as u64 > SYSTEM_MEMORY_BYTES {
                break memory;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pressure worker did not consume attachment headroom");
    assert!(
        pressure_memory + target_memory <= SYSTEM_MEMORY_BYTES,
        "pressure must still leave room to restart the owner: pressure={pressure_memory}, target={target_memory}, pool={SYSTEM_MEMORY_BYTES}"
    );
    assert!(
        pressure_memory + target_memory + ATTACHMENT_BYTES as u64 <= SYSTEM_MEMORY_BYTES,
        "either output must fit independently so combined dual-output attachment admission is decisive"
    );

    let mut reconstruction = executor.gate_next_completed_entity_reconstruction(&worker_id);
    executor.simulated_crash(&worker_id).await?;
    drop(original_checkpoint.release);
    tokio::time::timeout(std::time::Duration::from_secs(30), reconstruction.entered())
        .await
        .expect("completed tool body did not reexecute under attachment memory pressure");
    assert!(!executor.owner_replay_is_live(&owned_agent_id).await?);
    reconstruction.release();

    let replayed_checkpoint = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        checkpoint_arrivals.recv(),
    )
    .await
    .expect("replayed completed-attachment checkpoint timed out")
    .expect("checkpoint server stopped");
    assert_eq!(replayed_checkpoint.name, "completed-attachment-pressure");
    replayed_checkpoint
        .release
        .send(())
        .expect("release replayed completed-attachment checkpoint");
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let metadata = executor.get_worker_metadata(&worker_id).await?;
            if metadata.status == AgentStatus::Idle && metadata.pending_invocation_count == 0 {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("replayed owner invocation did not settle"))??;
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/completed-under-pressure.bin")
            .await?,
        input,
        "completed replay must preserve the body-reconstructed filesystem state"
    );
    checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn incomplete_tool_replay_persists_attachment_upgrade_rejection(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("large_dynamic_memory")] large_dynamic_memory: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const SYSTEM_MEMORY_BYTES: u64 = 128 * 1024 * 1024;
    const ATTACHMENT_BYTES: u64 = 8 * 1024 * 1024;
    const DESIRED_REPLAY_HEADROOM_BYTES: u64 = 12 * 1024 * 1024;

    let _attachment_pressure_test_permit = ATTACHMENT_PRESSURE_TEST_PERMIT
        .acquire()
        .await
        .expect("attachment-pressure test semaphore is not closed");
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.limits.max_tool_attachment_bytes = 2 * ATTACHMENT_BYTES as usize;
                config.memory.system_memory_override = Some(SYSTEM_MEMORY_BYTES);
                config.memory.worker_memory_ratio = 1.0;
                config.memory.component_size_coefficient = 0.0;
                config.memory.acquire_retry_delay = std::time::Duration::from_millis(25);
            })),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let memory_component = executor
        .component_dep(&context.default_environment_id, large_dynamic_memory)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoint_arrivals) =
        start_crash_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "incomplete-upgrade-rejection");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);

    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "reject_incomplete_attachment_upgrade_under_pressure",
        data_value!(),
    );
    let prepare_replay = async {
        let original_checkpoint = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("original incomplete-attachment checkpoint timed out")
        .expect("checkpoint server stopped");
        assert_eq!(
            original_checkpoint.name,
            "after-dual-output-before-terminal"
        );
        let operation = executor
            .active_entity_metadata(&owned_agent_id)
            .await
            .and_then(|active| active.tool_operations.operations.into_iter().next())
            .expect("incomplete tool operation is active at the crash checkpoint");
        let original_start = operation.start_index.expect("durable tool Start index");
        let stdout = operation.stdout.expect("incomplete tool stdout metadata");
        assert_eq!(stdout.buffered_bytes as u64, ATTACHMENT_BYTES);
        assert_eq!(stdout.delivered_bytes, 0);
        let stderr = operation.stderr.expect("incomplete tool stderr metadata");
        assert_eq!(stderr.buffered_bytes as u64, ATTACHMENT_BYTES);
        assert_eq!(stderr.delivered_bytes, 0);
        let reconstructed_entity_memory = executor
            .active_entity_metadata(&owned_agent_id)
            .await
            .expect("incomplete tool entity remains active at the crash checkpoint")
            .slots
            .into_iter()
            .flat_map(|slot| slot.invocations)
            .map(|invocation| invocation.linear_memory_bytes)
            .sum::<u64>();
        anyhow::ensure!(
            reconstructed_entity_memory > 0,
            "incomplete tool entity has no reconstructed linear-memory charge"
        );

        let mut reconstructed_body = executor.gate_next_entity_body_start(&worker_id);
        executor.simulated_crash(&worker_id).await?;
        drop(original_checkpoint.release);
        executor.resume(&worker_id, true).await?;
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            reconstructed_body.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("reconstructed entity body start gate was not reached"))?;

        let target_memory = executor.worker_memory_requirement(&owned_agent_id).await?;
        let restart_memory = target_memory
            .checked_add(reconstructed_entity_memory)
            .ok_or_else(|| anyhow::anyhow!("reconstructed memory requirement overflow"))?;
        let pressure_agent = agent_id!("LargeDynamicMemoryAgent", "incomplete-attachment-pressure");
        let pressure_worker = executor
            .start_agent(&memory_component.id, pressure_agent.clone())
            .await?;
        let pressure_owned = OwnedAgentId::new(context.default_environment_id, &pressure_worker);
        let initial_pressure_memory = executor.worker_memory_requirement(&pressure_owned).await?;
        let growth_budget = SYSTEM_MEMORY_BYTES
            .checked_sub(restart_memory + initial_pressure_memory + DESIRED_REPLAY_HEADROOM_BYTES)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "initial workers leave no room for calibrated pressure: owner={target_memory}, entity={reconstructed_entity_memory}, pressure={initial_pressure_memory}"
                )
            })?;
        let growth_mib = growth_budget / (1024 * 1024);
        anyhow::ensure!(growth_mib > 0, "calibrated pressure growth is empty");
        executor
            .invoke_agent(
                &memory_component,
                &pressure_agent,
                "run_with_memory_and_work",
                data_value!(growth_mib, 120_000_u64),
            )
            .await?;
        let pressure_memory = {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
            let mut maximum_observed = 0;
            loop {
                let memory = executor
                    .worker_memory_requirement(&pressure_owned)
                    .await
                    .expect("read pressure worker memory");
                maximum_observed = maximum_observed.max(memory);
                if memory + restart_memory + 2 * ATTACHMENT_BYTES > SYSTEM_MEMORY_BYTES {
                    break memory;
                }
                anyhow::ensure!(
                    tokio::time::Instant::now() < deadline,
                    "pressure worker did not consume attachment-upgrade headroom: owner={target_memory}, entity={reconstructed_entity_memory}, initial_pressure={initial_pressure_memory}, requested_growth_mib={growth_mib}, maximum_pressure={maximum_observed}, attachment={ATTACHMENT_BYTES}, pool={SYSTEM_MEMORY_BYTES}"
                );
                tokio::task::yield_now().await;
            }
        };
        assert!(
            pressure_memory + restart_memory <= SYSTEM_MEMORY_BYTES,
            "pressure must fit beside the reconstructed owner and entity: pressure={pressure_memory}, owner={target_memory}, entity={reconstructed_entity_memory}, pool={SYSTEM_MEMORY_BYTES}"
        );
        assert!(
            pressure_memory + restart_memory + ATTACHMENT_BYTES <= SYSTEM_MEMORY_BYTES,
            "either output upgrade must fit independently so combined dual-output attachment admission is decisive"
        );

        reconstructed_body.release();
        Ok::<_, anyhow::Error>(original_start)
    };
    let (result, original_start) = tokio::join!(invocation, prepare_replay);
    let evidence = result?.into_typed::<Vec<String>>()?;
    let original_start = original_start?;
    assert_eq!(
        evidence,
        vec![
            "resource-exhausted",
            "stdout-resource-exhausted",
            "stderr-resource-exhausted"
        ]
    );
    let durable_file = executor
        .get_file_contents(&worker_id, "/incomplete-attachment-upgrade-rejected")
        .await?;
    assert!(
        durable_file.iter().all(|byte| *byte == b'i')
            && durable_file.len() as u64 == ATTACHMENT_BYTES,
        "dual-pressure provider body effect must be preserved"
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
        "attachment admission rejection must be an ordinary durable tool outcome"
    );
    let (terminal_index, terminal_response) = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(params) if params.start_index == original_start => params
                .response
                .clone()
                .map(|response| (entry.oplog_index, response)),
            _ => None,
        })
        .expect("incomplete attachment upgrade rejection has one durable terminal");
    let (jump_index, jump) = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Jump(params) if entry.oplog_index < terminal_index => {
                Some((entry.oplog_index, params.jump.clone()))
            }
            _ => None,
        })
        .expect("incomplete atomic tail is jumped before the outer tool terminal");
    assert!(
        jump.start > original_start,
        "the recovery Jump must preserve the outer tool Start"
    );
    assert!(
        jump.end < jump_index,
        "the recovery Jump must follow the abandoned atomic interval"
    );
    assert!(
        jump.start <= jump.end,
        "the recovery Jump must identify a non-empty abandoned atomic interval"
    );
    let terminal = SerializableToolOperationTerminal::from_value(terminal_response.value())?;
    assert_eq!(
        terminal.body_execution,
        SerializableEntityBodyExecution::Executed
    );
    assert!(matches!(
        terminal.result,
        Err(SerializableToolRpcError::ResourceExhausted(_))
    ));
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::Start(params)
                        if params.function_name == "golem::entity::invoke"
                )
            })
            .count(),
        1
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::End(params) if params.start_index == original_start
                )
            })
            .count(),
        1
    );
    if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }

    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            checkpoint_arrivals.recv()
        )
        .await
        .is_err(),
        "rejected live repair must not reach the provider's live checkpoint"
    );
    executor.simulated_crash(&worker_id).await?;
    executor.resume(&worker_id, true).await?;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let metadata = executor.get_worker_metadata(&worker_id).await?;
            if metadata.status == AgentStatus::Idle && metadata.pending_invocation_count == 0 {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("stable rejection replay did not settle"))??;
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            checkpoint_arrivals.recv()
        )
        .await
        .is_err(),
        "recorded rejection must reconstruct the body without repeating its live checkpoint"
    );
    let replayed_file = executor
        .get_file_contents(&worker_id, "/incomplete-attachment-upgrade-rejected")
        .await?;
    assert!(
        replayed_file.iter().all(|byte| *byte == b'i')
            && replayed_file.len() as u64 == ATTACHMENT_BYTES,
        "completed rejection replay must reconstruct the executed provider body prefix"
    );
    let replayed_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        replayed_oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::End(params) if params.start_index == original_start
                )
            })
            .count(),
        1,
        "replay must retain the original attachment rejection terminal"
    );
    checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn capable_stdout_poll_does_not_launch_before_result_await(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "capable-stdout-before-result");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    executor
        .invoke_agent(
            &caller_component,
            &agent_id,
            "hold_capable_stdout_before_result",
            data_value!("/must-not-start-before-result.bin", b"staged".to_vec()),
        )
        .await?;

    let operation = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(operation) = executor
                .active_entity_metadata(&owned_agent_id)
                .await
                .and_then(|active| active.tool_operations.operations.into_iter().next())
                && operation.admission == ToolBodyAdmissionMetadata::Ready
            {
                break operation;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("capable operation did not finish eager input staging"))?;
    assert_eq!(operation.lane, ToolOperationLaneMetadata::None);
    assert_eq!(operation.attachment_count, 2);

    executor.interrupt(&worker_id).await?;
    if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn capable_admission_publication_and_clean_stdout_trap_preserve_boundaries(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.retry = RetryConfig {
                    max_attempts: 0,
                    min_delay: std::time::Duration::from_millis(1),
                    max_delay: std::time::Duration::from_millis(1),
                    multiplier: 1.0,
                    max_jitter_factor: None,
                };
            })),
            ..Default::default()
        },
    )
    .await?;
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoint_arrivals) =
        start_crash_checkpoint_server().await;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );
    let env = HashMap::from([
        (
            "CRASH_CHECKPOINT_PORT".to_string(),
            checkpoint_port.to_string(),
        ),
        (
            "CRASH_CHECKPOINT_GATE_PORT".to_string(),
            checkpoint_gate_port.to_string(),
        ),
    ]);

    let sync_agent = agent_id!("ToolStreamingCaller", "sync-capable-staging-boundary");
    let sync_worker = executor
        .start_agent_with(
            &caller_component.id,
            sync_agent.clone(),
            env.clone(),
            Vec::new(),
        )
        .await?;
    let sync_owner = OwnedAgentId::new(context.default_environment_id, &sync_worker);
    let sync_input = b"sync-staged".to_vec();
    let sync_call = executor.invoke_and_await_agent(
        &caller_component,
        &sync_agent,
        "hold_synchronous_capable_staging",
        data_value!(sync_input.clone()),
    );
    let sync_gates = async {
        let staging_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("synchronous capable staging gate timed out")
        .expect("checkpoint server stopped");
        assert_eq!(staging_gate.name, "sync-capable-staging");
        let operation = executor
            .active_entity_metadata(&sync_owner)
            .await
            .and_then(|active| active.tool_operations.operations.into_iter().next())
            .expect("synchronous capable operation is eagerly staged");
        assert_eq!(operation.admission, ToolBodyAdmissionMetadata::Staging);
        assert_eq!(operation.lane, ToolOperationLaneMetadata::None);
        let stdin = operation.stdin.expect("synchronous capable stdin metadata");
        assert_eq!(stdin.accepted_bytes, sync_input.len() as u64);
        assert!(!stdin.terminal_selected);
        staging_gate
            .release
            .send(())
            .expect("release synchronous capable staging gate");

        let body_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("synchronous capable body gate timed out")
        .expect("checkpoint server stopped");
        assert_eq!(body_gate.name, "capable-body");
        let operation = executor
            .active_entity_metadata(&sync_owner)
            .await
            .and_then(|active| active.tool_operations.operations.into_iter().next())
            .expect("synchronous capable body is active");
        assert_eq!(operation.admission, ToolBodyAdmissionMetadata::Running);
        assert_eq!(operation.lane, ToolOperationLaneMetadata::Granted);
        let stdout = operation
            .stdout
            .expect("synchronous capable stdout metadata");
        assert_eq!(stdout.mode, ToolAttachmentModeMetadata::CompletionStaged);
        assert_eq!(stdout.delivered_bytes, 0);
        body_gate
            .release
            .send(())
            .expect("release synchronous capable body gate");
    };
    let (sync_result, ()) = tokio::join!(sync_call, sync_gates);
    sync_result?;
    assert_eq!(
        executor
            .get_file_contents(&sync_worker, "/sync-capable-staging.bin")
            .await?,
        sync_input
    );

    let fire_agent = agent_id!("ToolStreamingCaller", "fire-capable-staging-boundary");
    let fire_worker = executor
        .start_agent_with(
            &caller_component.id,
            fire_agent.clone(),
            env.clone(),
            Vec::new(),
        )
        .await?;
    let fire_owner = OwnedAgentId::new(context.default_environment_id, &fire_worker);
    let fire_input = b"fire-staged".to_vec();
    let fire_call = executor.invoke_and_await_agent(
        &caller_component,
        &fire_agent,
        "hold_fire_and_forget_capable_staging",
        data_value!(fire_input.clone()),
    );
    let fire_gates = async {
        let staging_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("fire-and-forget capable staging gate timed out")
        .expect("checkpoint server stopped");
        assert_eq!(staging_gate.name, "fire-capable-staging");
        let operation = executor
            .active_entity_metadata(&fire_owner)
            .await
            .and_then(|active| active.tool_operations.operations.into_iter().next())
            .expect("fire-and-forget capable operation is eagerly staged");
        assert_eq!(operation.admission, ToolBodyAdmissionMetadata::Staging);
        assert_eq!(operation.lane, ToolOperationLaneMetadata::None);
        staging_gate
            .release
            .send(())
            .expect("release fire-and-forget staging gate");

        let parent_open_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("fire-and-forget parent-open gate timed out")
        .expect("checkpoint server stopped");
        assert_eq!(parent_open_gate.name, "fire-capable-ready-parent-open");
        let operation = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if let Some(operation) = executor
                    .active_entity_metadata(&fire_owner)
                    .await
                    .and_then(|active| active.tool_operations.operations.into_iter().next())
                    && operation.admission == ToolBodyAdmissionMetadata::Ready
                {
                    break operation;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect(
            "fire-and-forget capable operation did not become ready while parent remained open",
        );
        assert_eq!(operation.admission, ToolBodyAdmissionMetadata::Ready);
        assert_eq!(operation.lane, ToolOperationLaneMetadata::None);
        parent_open_gate
            .release
            .send(())
            .expect("release fire-and-forget parent-open gate");

        let body_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("fire-and-forget capable body gate timed out")
        .expect("checkpoint server stopped");
        assert_eq!(body_gate.name, "capable-body");
        let operation = executor
            .active_entity_metadata(&fire_owner)
            .await
            .and_then(|active| active.tool_operations.operations.into_iter().next())
            .expect("fire-and-forget capable body starts after parent end");
        assert_eq!(operation.admission, ToolBodyAdmissionMetadata::Running);
        assert_eq!(operation.lane, ToolOperationLaneMetadata::Granted);
        body_gate
            .release
            .send(())
            .expect("release fire-and-forget capable body gate");
    };
    let (fire_result, ()) = tokio::join!(fire_call, fire_gates);
    fire_result?;
    assert_eq!(
        executor
            .get_file_contents(&fire_worker, "/fire-capable-staging.bin")
            .await?,
        fire_input
    );

    let publication_agent = agent_id!("ToolStreamingCaller", "capable-publication-boundary");
    let publication_worker = executor
        .start_agent_with(
            &caller_component.id,
            publication_agent.clone(),
            env.clone(),
            Vec::new(),
        )
        .await?;
    let publication_owner = OwnedAgentId::new(context.default_environment_id, &publication_worker);
    let publication_input = b"publication-staged".to_vec();
    let publication_call = executor.invoke_and_await_agent(
        &caller_component,
        &publication_agent,
        "hold_capable_publication_checkpoint",
        data_value!("/capable-publication.bin", publication_input.clone()),
    );
    let publication_gates = async {
        let provider_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("capable provider publication gate timed out")
        .expect("checkpoint server stopped");
        assert_eq!(provider_gate.name, "provider-capable-before-publication");

        let operation = executor
            .active_entity_metadata(&publication_owner)
            .await
            .and_then(|active| active.tool_operations.operations.into_iter().next())
            .expect("capable operation is active before publication");
        assert_eq!(operation.admission, ToolBodyAdmissionMetadata::Running);
        assert_eq!(operation.lane, ToolOperationLaneMetadata::Granted);
        let stdout = operation.stdout.expect("capable stdout metadata");
        assert_eq!(stdout.mode, ToolAttachmentModeMetadata::CompletionStaged);
        assert_eq!(stdout.accepted_bytes, publication_input.len() as u64);
        assert_eq!(stdout.delivered_bytes, 0);
        assert_eq!(stdout.buffered_bytes, publication_input.len());
        assert!(!stdout.terminal_selected);
        assert!(
            checkpoint_arrivals.try_recv().is_err(),
            "caller must not observe capable stdout before lane return"
        );

        provider_gate
            .release
            .send(())
            .expect("release capable provider publication gate");
        let caller_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .expect("caller publication observation gate timed out")
        .expect("checkpoint server stopped");
        assert_eq!(caller_gate.name, "caller-observed-capable-publication");
        let active = executor
            .active_entity_metadata(&publication_owner)
            .await
            .expect("owner remains active while caller holds result observation");
        assert!(matches!(
            active.lane.holder,
            Some(OwnerInvocationId::Agent(_))
        ));
        assert_eq!(active.lane.active_invocation_count, 1);
        caller_gate
            .release
            .send(())
            .expect("release caller publication gate");
    };
    let (publication_result, ()) = tokio::join!(publication_call, publication_gates);
    publication_result?;
    assert_eq!(
        executor
            .get_file_contents(&publication_worker, "/capable-publication.bin")
            .await?,
        publication_input
    );
    assert_eq!(
        executor
            .get_file_contents(&publication_worker, "/capable-publication-observed.bin")
            .await?,
        b"observed".as_slice()
    );

    let (
        provider_checkpoint_port,
        provider_checkpoint_gate_port,
        provider_checkpoint_server,
        mut provider_checkpoint_arrivals,
    ) = start_crash_checkpoint_server().await;
    let (
        caller_checkpoint_port,
        caller_checkpoint_gate_port,
        caller_checkpoint_server,
        mut caller_checkpoint_arrivals,
    ) = start_crash_checkpoint_server().await;
    let trap_env = HashMap::from([
        (
            "PROVIDER_CRASH_CHECKPOINT_PORT".to_string(),
            provider_checkpoint_port.to_string(),
        ),
        (
            "PROVIDER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
            provider_checkpoint_gate_port.to_string(),
        ),
        (
            "CALLER_CRASH_CHECKPOINT_PORT".to_string(),
            caller_checkpoint_port.to_string(),
        ),
        (
            "CALLER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
            caller_checkpoint_gate_port.to_string(),
        ),
    ]);
    let trap_agent = agent_id!("ToolStreamingCaller", "clean-stdout-before-trap");
    let trap_worker = executor
        .start_agent_with(
            &caller_component.id,
            trap_agent.clone(),
            trap_env,
            Vec::new(),
        )
        .await?;
    let trap_owner = OwnedAgentId::new(context.default_environment_id, &trap_worker);
    let trap_call = executor.invoke_and_await_agent(
        &caller_component,
        &trap_agent,
        "clean_stdout_then_trap",
        data_value!(),
    );
    let trap_gates = async {
        let provider_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            provider_checkpoint_arrivals.recv(),
        )
        .await
        .expect("provider clean-stdout gate timed out")
        .expect("provider checkpoint server stopped");
        assert_eq!(provider_gate.name, "provider-clean-stdout-before-trap");
        let caller_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            caller_checkpoint_arrivals.recv(),
        )
        .await
        .expect("caller clean-stdout gate timed out")
        .expect("caller checkpoint server stopped");
        assert_eq!(caller_gate.name, "caller-observed-clean-stdout");

        let operation = executor
            .active_entity_metadata(&trap_owner)
            .await
            .and_then(|active| active.tool_operations.operations.into_iter().next())
            .expect("trapping operation remains active at clean stdout EOF");
        let stdout = operation.stdout.expect("trapping stdout metadata");
        assert_eq!(stdout.mode, ToolAttachmentModeMetadata::Live);
        assert!(stdout.terminal_selected);
        assert_eq!(stdout.accepted_bytes, b"marker:".len() as u64);
        assert_eq!(stdout.delivered_bytes, b"marker:".len() as u64);

        caller_gate
            .release
            .send(())
            .expect("release caller clean-stdout gate");
        provider_gate
            .release
            .send(())
            .expect("release provider clean-stdout gate");
    };
    let (trap_result, ()) = tokio::join!(trap_call, trap_gates);
    let trap_error = trap_result.expect_err("non-retryable provider trap must fail the owner");
    assert!(
        trap_error
            .to_string()
            .contains("deterministic streaming tool trap after clean stdout"),
        "original non-retryable trap must reach the owner: {trap_error:?}"
    );
    let trap_oplog = executor
        .get_oplog(&trap_worker, OplogIndex::INITIAL)
        .await?;
    assert_eq!(
        trap_oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::Error(_)))
            .count(),
        1,
        "non-retryable trap is recorded exactly once"
    );
    if let Some(active) = executor.active_entity_metadata(&trap_owner).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }

    checkpoint_server.abort();
    provider_checkpoint_server.abort();
    caller_checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn incomplete_entity_atomic_rollback_recovers_dependent_sibling(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let (provider_port, provider_gate_port, _provider_server, mut provider_arrivals) =
        start_crash_checkpoint_server().await;
    let (trap_port, trap_server, trap_attempts) = start_trap_attempt_server().await;
    let _trap_server = tokio_util::task::AbortOnDropHandle::new(trap_server);

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "entity-ownership-rollback");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                ("TRAP_ONCE_PORT".to_string(), trap_port.to_string()),
                (
                    "PROVIDER_CRASH_CHECKPOINT_PORT".to_string(),
                    provider_port.to_string(),
                ),
                (
                    "PROVIDER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    provider_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let invocation = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        executor.invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "dependent_sibling_after_unjournaled_stdout",
            data_value!(),
        ),
    );
    let recover = async {
        let provider_checkpoint =
            tokio::time::timeout(std::time::Duration::from_secs(30), provider_arrivals.recv())
                .await
                .map_err(|_| anyhow::anyhow!("provider changing-stdout checkpoint timed out"))?
                .ok_or_else(|| anyhow::anyhow!("provider checkpoint server stopped"))?;
        assert_eq!(provider_checkpoint.name, "provider-changing-stdout");
        let before_crash = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                executor.commit_oplog(&worker_id).await?;
                let entries = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                let sibling = entries
                    .iter()
                    .filter_map(|entry| match &entry.entry {
                        PublicOplogEntry::Start(params)
                            if params.function_name == "golem::entity::invoke" =>
                        {
                            Some(entry.oplog_index)
                        }
                        _ => None,
                    })
                    .nth(1);
                if sibling.is_some_and(|start| {
                    entries.iter().any(|entry| matches!(
                    &entry.entry, PublicOplogEntry::End(params) if params.start_index == start
                ))
                }) {
                    break anyhow::Ok(entries);
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("dependent sibling did not complete"))??;
        let entity_starts = before_crash
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::entity::invoke" =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(entity_starts.len(), 2, "outer and dependent sibling Starts");
        assert!(before_crash.iter().all(|entry| !matches!(
            &entry.entry,
            PublicOplogEntry::End(params) if params.start_index == entity_starts[0]
        )));
        before_crash
            .iter()
            .find(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::End(params) if params.start_index == entity_starts[1]
                )
            })
            .expect("dependent sibling End is durable");
        assert!(
            before_crash
                .iter()
                .filter(|entry| matches!(entry.entry, PublicOplogEntry::BeginAtomicRegion(_)))
                .count()
                > before_crash
                    .iter()
                    .filter(|entry| matches!(entry.entry, PublicOplogEntry::EndAtomicRegion(_)))
                    .count(),
            "the provider's outer atomic region must still be incomplete"
        );

        executor.simulated_crash(&worker_id).await?;
        Ok::<_, anyhow::Error>(entity_starts)
    };
    let (result, recovery) = tokio::join!(invocation, recover);
    let original_starts = recovery?;
    let replayed = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    if result.is_err() {
        assert!(
            replayed
                .iter()
                .any(|entry| matches!(entry.entry, PublicOplogEntry::Jump(_))),
            "incomplete entity recovery must record its rollback Jump"
        );
        assert!(replayed.iter().any(|entry| matches!(
            &entry.entry,
            PublicOplogEntry::End(params) if params.start_index == original_starts[1]
        )));
        anyhow::bail!(
            "recovery did not settle with the original dependent sibling Start/End at {} retained; external attempt reads={}, expected recovery to reread `second` after the original caller completed the sibling from `first`",
            original_starts[1],
            trap_attempts.load(Ordering::SeqCst)
        );
    }
    let evidence: Vec<String> = result.expect("timeout handled above")?.into_typed()?;
    assert_eq!(evidence, ["second", "no-stream:second"]);
    assert_eq!(trap_attempts.load(Ordering::SeqCst), 2);

    drop(executor);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state),
            ..Default::default()
        },
    )
    .await?;
    let reconstructed: String = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        executor.invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "replay_probe",
            data_value!(),
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("second reconstruction did not settle"))??
    .into_typed()?;
    assert_eq!(reconstructed, "replayed");
    assert_eq!(trap_attempts.load(Ordering::SeqCst), 2);

    let original_sibling_end = replayed
        .iter()
        .find(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::End(params) if params.start_index == original_starts[1]
            )
        })
        .unwrap()
        .oplog_index;
    assert!(
        replayed.iter().any(|entry| matches!(
            &entry.entry,
            PublicOplogEntry::Jump(params)
                if params.jump.contains(original_starts[1])
                    && params.jump.contains(original_sibling_end)
        )),
        "the old sibling's complete history must be logically deleted, not merely left unclaimed"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn active_stream_crash_replays_pinned_activation_with_fresh_attachments(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides).await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "active-stream-crash");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    executor
        .invoke_agent(
            &caller_component,
            &agent_id,
            "hold_open_stdin",
            data_value!(1_u32),
        )
        .await?;
    let original = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(operation) = executor
                .active_entity_metadata(&owned_agent_id)
                .await
                .and_then(|active| active.tool_operations.operations.into_iter().next())
                && operation.start_index.is_some()
                && operation.attachment_count == 2
            {
                break operation;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for the durable tool Start"))?;
    let original_start = original.start_index.expect("durable tool Start index");
    assert_eq!(original.attachment_count, 2);

    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        None,
    );
    executor.simulated_crash(&worker_id).await?;
    executor.resume(&worker_id, true).await?;
    let replayed = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(operation) = executor
                .active_entity_metadata(&owned_agent_id)
                .await
                .and_then(|active| active.tool_operations.operations.into_iter().next())
                && operation.start_index.is_some()
                && operation.attachment_count == 2
            {
                break operation;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for the replayed durable tool Start"))?;
    assert_eq!(replayed.start_index, Some(original_start));
    assert_eq!(replayed.attachment_count, 2);

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
        "lifecycle recovery must not poison the incomplete entity Start with a durable Error"
    );
    let serialized = serde_json::to_string(&oplog)?;
    for forbidden in ["attachment-id", "endpoint-id", "resource-key"] {
        assert!(
            !serialized.contains(forbidden),
            "transient stream identity `{forbidden}` must not enter the owner oplog"
        );
    }

    executor.interrupt(&worker_id).await?;
    if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn suspended_restart_replays_completed_tool_without_semantic_retry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.suspend.suspend_after = std::time::Duration::from_millis(100);
            })),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        "ToolStreamingCaller",
        metadata.tools,
    );
    for bindings in deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.filesystem_access = ToolFilesystemAccess::Allowed;
        }
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let (promise_port, promise_server, mut promise_checkpoints) =
        start_promise_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "suspended-restart-completed-tool");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([(
                "CALLER_PROMISE_CHECKPOINT_PORT".to_string(),
                promise_port.to_string(),
            )]),
            Vec::new(),
        )
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let key = IdempotencyKey::fresh();
    let expected = b"distinctive-suspended-restart".to_vec();
    let invocation = {
        let executor = executor.clone();
        let caller_component = caller_component.clone();
        let agent_id = agent_id.clone();
        let key = key.clone();
        let expected = expected.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(
                    &caller_component,
                    &agent_id,
                    &key,
                    "completed_capable_tool_then_promise",
                    data_value!("/suspended-restart.bin", expected),
                )
                .await
        })
    };

    let checkpoint =
        next_promise_checkpoint(&mut promise_checkpoints, "completed-capable-tool").await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Suspended,
            std::time::Duration::from_secs(30),
        )
        .await?;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while executor.worker_is_loaded(&owned_agent_id).await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("suspended owner did not finish unloading"))?;

    executor.simulated_crash(&worker_id).await?;
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: checkpoint.oplog_idx,
            },
            Vec::new(),
        )
        .await?;
    let result: Vec<u8> = tokio::time::timeout(std::time::Duration::from_secs(30), invocation)
        .await
        .map_err(|_| {
            anyhow::anyhow!("original invocation did not finish after suspended restart")
        })???
        .into_typed()?;
    assert_eq!(result, expected);
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/suspended-restart.bin")
            .await?,
        expected
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
        "lifecycle restart must not charge a semantic retry"
    );
    assert!(
        oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Interrupted(_))),
        "simulated crash must not manufacture a terminal interruption"
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(started)
                    if matches!(
                        &started.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.method_name.replace('-', "_")
                                == "completed_capable_tool_then_promise"
                    )
            ))
            .count(),
        1,
        "restart must retain the original logical invocation"
    );
    assert_eq!(
        executor.get_worker_metadata(&worker_id).await?.retry_count,
        0
    );
    let follow_up: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "read_owner_file",
            data_value!("/suspended-restart.bin"),
        )
        .await?
        .into_typed()?;
    assert_eq!(follow_up.into_bytes(), expected);

    let (drain_checkpoint_port, drain_gate_port, drain_server, mut drain_checkpoints) =
        start_crash_checkpoint_server().await;
    let drain_agent_id = agent_id!("ToolStreamingCaller", "restart-drains-live-entity");
    let drain_worker_id = executor
        .start_agent_with(
            &caller_component.id,
            drain_agent_id.clone(),
            HashMap::from([
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    drain_checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    drain_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owned_drain_agent = OwnedAgentId::new(context.default_environment_id, &drain_worker_id);
    let drain_key = IdempotencyKey::fresh();
    let drain_invocation = tokio::spawn({
        let executor = executor.clone();
        let caller_component = caller_component.clone();
        let drain_agent_id = drain_agent_id.clone();
        let drain_key = drain_key.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(
                    &caller_component,
                    &drain_agent_id,
                    &drain_key,
                    "collect_capable",
                    data_value!("hold-body:/restart-drain.bin", b"drain-me".to_vec()),
                )
                .await
        }
    });
    let drain_checkpoint = next_crash_checkpoint(&mut drain_checkpoints, "capable-body").await?;
    let held_operation = executor
        .active_entity_metadata(&owned_drain_agent)
        .await
        .and_then(|metadata| metadata.tool_operations.operations.into_iter().next())
        .ok_or_else(|| anyhow::anyhow!("live tool operation was not registered"))?;
    assert_eq!(held_operation.admission, ToolBodyAdmissionMetadata::Running);
    let drain_worker = executor.active_agent(&owned_drain_agent).await.unwrap();
    let restart = tokio::spawn(async move {
        let worker = drain_worker.primary();
        worker
            .set_interrupting(golem_service_base::error::worker_executor::InterruptKind::Restart)
            .await?;
        worker.join_accepted_stops_for_test().await
    });
    tokio::time::timeout(std::time::Duration::from_secs(30), restart)
        .await
        .map_err(|_| anyhow::anyhow!("restart did not cancel the blocked live entity body"))???;
    assert!(!drain_invocation.is_finished());
    let replay_checkpoint = next_crash_checkpoint(&mut drain_checkpoints, "capable-body").await?;
    drop(drain_checkpoint.release);
    replay_checkpoint
        .release
        .send(())
        .map_err(|_| anyhow::anyhow!("replayed tool checkpoint dropped before release"))?;
    let drained: StreamEvidence =
        tokio::time::timeout(std::time::Duration::from_secs(30), drain_invocation)
            .await
            .map_err(|_| anyhow::anyhow!("tool invocation did not recover after restart"))???
            .into_typed()?;
    assert_eq!(drained.chunks_read, 1);
    assert_eq!(drained.bytes_read, 8);
    assert_eq!(
        executor
            .get_file_contents(&drain_worker_id, "/restart-drain.bin")
            .await?,
        b"drain-me".as_slice()
    );
    let drain_oplog = executor
        .get_oplog(&drain_worker_id, OplogIndex::INITIAL)
        .await?;
    assert!(
        drain_oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
        "live-body restart must not charge a semantic retry"
    );
    assert!(
        drain_oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Interrupted(_))),
        "live-body restart must not manufacture a terminal interruption"
    );
    assert_eq!(
        drain_oplog
            .iter()
            .filter(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(started)
                    if matches!(
                        &started.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.method_name.replace('-', "_") == "collect_capable"
                                && method.idempotency_key == drain_key
                    )
            ))
            .count(),
        1,
        "live-body restart must retain the original keyed logical invocation"
    );
    assert_eq!(
        executor
            .get_worker_metadata(&drain_worker_id)
            .await?
            .retry_count,
        0
    );
    let drain_follow_up: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &drain_agent_id,
            "read_owner_file",
            data_value!("/restart-drain.bin"),
        )
        .await?
        .into_typed()?;
    assert_eq!(drain_follow_up.as_bytes(), b"drain-me");
    drain_server.abort();
    promise_server.abort();
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompletedReconstructionExclusiveCase {
    Success,
    Divergence,
    ExecutorShutdownDuringBodyValidation,
    CrashReplaySupervisorWindow,
    EntityCustomRootCrashReplay,
}

/// Crashes the agent while its original invocation is held at the success gate, and releases the
/// gate as a restart, so the next start reconstructs the invocation from the oplog.
async fn crash_at_held_invocation_success(
    executor: &TestWorkerExecutor,
    worker_id: &golem_common::model::AgentId,
    original_success: AgentInvocationSuccessGateHandle,
) -> anyhow::Result<()> {
    let (crash, ()) = tokio::join!(executor.simulated_crash(worker_id), async {
        original_success.abort_as_restart();
    });
    crash?;
    drop(original_success);
    Ok(())
}

/// Checks the recovered oplog of an invocation whose completed tool call was reconstructed: the
/// invocation started once and finished once, the entity `Start` settled exactly once before the
/// finish, every `Start` the invocation recorded has a terminal, and no positional `Start` or
/// `End` follows the finish.
fn assert_replayed_reconstruction_invocation_settled(
    oplog: &[PublicOplogEntryWithIndex],
    method_name: &str,
    entity_start: OplogIndex,
) {
    let started = oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(started)
                    if matches!(
                        &started.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.method_name.replace('-', "_") == method_name
                    )
            )
        })
        .map(|entry| entry.oplog_index)
        .collect::<Vec<_>>();
    assert_eq!(started.len(), 1, "{method_name} must start exactly once");
    let started = started[0];
    let finished = oplog
        .iter()
        .filter(|entry| {
            entry.oplog_index > started
                && matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_))
        })
        .map(|entry| entry.oplog_index)
        .collect::<Vec<_>>();
    assert_eq!(finished.len(), 1, "{method_name} must finish exactly once");
    let finished = finished[0];
    let terminals_of = |start: OplogIndex| {
        oplog
            .iter()
            .filter(|entry| match &entry.entry {
                PublicOplogEntry::End(end) => end.start_index == start,
                PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == start,
                _ => false,
            })
            .map(|entry| entry.oplog_index)
            .collect::<Vec<_>>()
    };
    let entity_terminals = terminals_of(entity_start);
    assert_eq!(
        entity_terminals.len(),
        1,
        "the entity Start {entity_start} must settle exactly once"
    );
    assert!(entity_terminals[0] < finished);
    for entry in oplog
        .iter()
        .filter(|entry| entry.oplog_index > started && entry.oplog_index < finished)
    {
        if matches!(entry.entry, PublicOplogEntry::Start(_)) {
            let terminals = terminals_of(entry.oplog_index);
            assert_eq!(
                terminals.len(),
                1,
                "Start {} of {method_name} must have exactly one terminal",
                entry.oplog_index
            );
            assert!(terminals[0] < finished);
        }
    }
    assert!(
        oplog.iter().all(|entry| entry.oplog_index < finished
            || !matches!(
                entry.entry,
                PublicOplogEntry::Start(_) | PublicOplogEntry::End(_)
            )),
        "no positional Start or End may follow the finish of {method_name}"
    );
}

/// Waits until the replayed clock claim parks on the completed reconstruction whose supervisor the
/// test holds: the clock `Start` was never recorded, and the body's entries are still at the
/// cursor head.
async fn wait_for_clock_claim_blocked_on_held_reconstruction(
    executor: &TestWorkerExecutor,
    owned_agent_id: &OwnedAgentId,
) -> anyhow::Result<()> {
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        executor.wait_for_replay_claim_blocked_on_active_body(owned_agent_id),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!("the replayed clock claim did not wait for the held reconstruction")
    })?
}

async fn run_completed_reconstruction_exclusive_p2_case(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    provider: &PrecompiledComponent,
    caller: &PrecompiledComponent,
    case: CompletedReconstructionExclusiveCase,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.limits.max_tool_attachment_bytes = 64;
            })),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    let deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        "ToolStreamingCaller",
        metadata.tools,
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment.clone()),
    );

    let case_name = match case {
        CompletedReconstructionExclusiveCase::Success => "exclusive-p2-success",
        CompletedReconstructionExclusiveCase::Divergence => "exclusive-p2-divergence",
        CompletedReconstructionExclusiveCase::ExecutorShutdownDuringBodyValidation => {
            "exclusive-p2-body-validation-shutdown"
        }
        CompletedReconstructionExclusiveCase::CrashReplaySupervisorWindow => {
            "exclusive-p2-crash-replay-window"
        }
        CompletedReconstructionExclusiveCase::EntityCustomRootCrashReplay => {
            "exclusive-p2-entity-custom-root"
        }
    };
    let method = match case {
        CompletedReconstructionExclusiveCase::EntityCustomRootCrashReplay => {
            "hold_entity_custom_root_reconstruction_before_exclusive_clock"
        }
        _ => "hold_completed_reconstruction_before_exclusive_clock",
    };
    let agent_id = agent_id!("ToolStreamingCaller", case_name);
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let metadata = executor.get_worker_metadata(&worker_id).await?;
            if metadata.status == AgentStatus::Idle
                && metadata.pending_invocation_count == 0
                && metadata.last_oplog_index > OplogIndex::INITIAL
            {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for caller initialization"))??;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let mut original_success = executor.gate_next_agent_invocation_success(&worker_id);
    executor
        .skip_next_wall_clock_now_durability(&owned_agent_id)
        .await?;
    let invocation =
        executor.invoke_and_await_agent(&caller_component, &agent_id, method, data_value!());
    tokio::pin!(invocation);

    let validate_recovery = async {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            original_success.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("original agent invocation did not reach success gate"))?;
        wait_for_active_tool_operations(&executor, &owned_agent_id, 0).await?;
        let reconstruction_start =
            wait_for_completed_entity_terminal(&executor, &worker_id).await?;
        if case == CompletedReconstructionExclusiveCase::Divergence {
            let updated_component = executor
                .update_component(&caller_component.id, caller.wasm_name.as_str())
                .await?;
            environment_state.set_tool_deployment(
                context.default_environment_id,
                caller_component.id,
                updated_component.revision,
                Some(deployment.clone()),
            );
            executor
                .auto_update_worker(&worker_id, updated_component.revision, true)
                .await?;
        }

        if matches!(
            case,
            CompletedReconstructionExclusiveCase::CrashReplaySupervisorWindow
                | CompletedReconstructionExclusiveCase::EntityCustomRootCrashReplay
        ) {
            let mut supervisor = executor.gate_next_completed_reconstruction_supervisor(&worker_id);
            crash_at_held_invocation_success(&executor, &worker_id, original_success).await?;
            let start =
                tokio::time::timeout(std::time::Duration::from_secs(30), supervisor.entered())
                    .await
                    .map_err(|_| {
                        anyhow::anyhow!("crash-replay reconstruction supervisor was not reached")
                    })?;
            assert_eq!(start, reconstruction_start);
            wait_for_clock_claim_blocked_on_held_reconstruction(&executor, &owned_agent_id).await?;
            supervisor.release();
            return Ok::<_, anyhow::Error>(reconstruction_start);
        }
        let mut reconstruction_body =
            executor.gate_next_completed_entity_reconstruction(&worker_id);
        if case == CompletedReconstructionExclusiveCase::Divergence {
            executor.diverge_next_completed_entity_reconstruction(&worker_id);
        }
        let mut replayed_claim = executor.gate_next_entity_reconstruction_claim(&worker_id);
        crash_at_held_invocation_success(&executor, &worker_id, original_success).await?;
        let claimed_start =
            tokio::time::timeout(std::time::Duration::from_secs(30), replayed_claim.entered())
                .await
                .map_err(|_| anyhow::anyhow!("replayed reconstruction claim was not reached"))?;
        assert_eq!(claimed_start, reconstruction_start);
        let mut replayed_clock = executor.gate_next_wall_clock_now(&owned_agent_id).await?;
        replayed_claim.release();
        tokio::time::timeout(std::time::Duration::from_secs(30), replayed_clock.entered())
            .await
            .map_err(|_| anyhow::anyhow!("replayed clock gate was not reached"))?;
        let mut replayed_success = (case == CompletedReconstructionExclusiveCase::Success)
            .then(|| executor.gate_next_agent_invocation_success(&worker_id));

        match case {
            CompletedReconstructionExclusiveCase::Success => {
                let replayed_success = replayed_success.as_mut().unwrap();
                tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    reconstruction_body.entered(),
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!("completed reconstruction did not reach body validation gate")
                })?;
                replayed_clock.release();
                wait_for_owner_replay_settling(&executor, &owned_agent_id).await?;
                assert!(!executor.owner_replay_is_live(&owned_agent_id).await?);
                assert!(
                    tokio::time::timeout(
                        std::time::Duration::from_millis(250),
                        replayed_success.entered()
                    )
                    .await
                    .is_err(),
                    "exclusive P2 clock call finished before completed reconstruction validation"
                );
                reconstruction_body.release();
                tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    replayed_success.entered(),
                )
                .await
                .map_err(|_| anyhow::anyhow!("replayed agent invocation did not finish"))?;
                replayed_success.release();
            }
            CompletedReconstructionExclusiveCase::Divergence => {
                tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    reconstruction_body.entered(),
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!("divergent reconstruction did not reach body validation gate")
                })?;
                replayed_clock.release();
                wait_for_owner_replay_settling(&executor, &owned_agent_id).await?;
                assert!(!executor.owner_replay_is_live(&owned_agent_id).await?);
                let settling_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                assert!(
                    settling_oplog
                        .iter()
                        .all(|entry| !matches!(entry.entry, PublicOplogEntry::SuccessfulUpdate(_))),
                    "ReplayFinished finalized the pending update before reconstruction validation"
                );
                let owner_failure = executor.wait_for_tool_owner_failure(&owned_agent_id);
                tokio::pin!(owner_failure);
                assert!(matches!(
                    futures::poll!(owner_failure.as_mut()),
                    std::task::Poll::Pending
                ));
                // The replay on the source revision that follows the failed update: hold the
                // supervisor of its completed reconstruction after the claim and before it drains
                // the recorded terminal, so the exclusive clock call claims in that window.
                let mut source_claim =
                    executor.gate_next_completed_reconstruction_supervisor(&worker_id);
                reconstruction_body.release();
                let owner_failure =
                    tokio::time::timeout(std::time::Duration::from_secs(30), owner_failure)
                        .await
                        .map_err(|_| {
                            anyhow::anyhow!("divergence did not select an owner failure")
                        })??;
                assert!(owner_failure.owner_failure_selected);
                assert_eq!(
                    owner_failure.owner_failure,
                    Some(ToolOwnerFailureMetadata::Infrastructure)
                );
                let failed_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                assert!(
                    failed_oplog
                        .iter()
                        .all(|entry| !matches!(entry.entry, PublicOplogEntry::SuccessfulUpdate(_))),
                    "divergent reconstruction permitted ReplayFinished update finalization"
                );
                let source_start = tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    source_claim.entered(),
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!("source-revision reconstruction supervisor was not reached")
                })?;
                assert_eq!(source_start, reconstruction_start);
                wait_for_clock_claim_blocked_on_held_reconstruction(&executor, &owned_agent_id)
                    .await?;
                source_claim.release();
            }
            CompletedReconstructionExclusiveCase::CrashReplaySupervisorWindow
            | CompletedReconstructionExclusiveCase::EntityCustomRootCrashReplay => unreachable!(),
            CompletedReconstructionExclusiveCase::ExecutorShutdownDuringBodyValidation => {
                tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    reconstruction_body.entered(),
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!("completed reconstruction did not reach body validation gate")
                })?;
                replayed_clock.release();
                executor.shutdown_and_wait_for_invocation_loops().await?;
            }
        }
        Ok::<_, anyhow::Error>(reconstruction_start)
    };

    let (invocation_result, validation_result) = tokio::join!(
        tokio::time::timeout(std::time::Duration::from_secs(60), &mut invocation),
        validate_recovery
    );
    let reconstruction_start = validation_result?;
    let invocation_result = invocation_result
        .map_err(|_| anyhow::anyhow!("exclusive-P2 reconstruction invocation timed out"))?;
    match case {
        CompletedReconstructionExclusiveCase::Success
        | CompletedReconstructionExclusiveCase::CrashReplaySupervisorWindow => {
            invocation_result?;
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            assert_replayed_reconstruction_invocation_settled(&oplog, method, reconstruction_start);
        }
        CompletedReconstructionExclusiveCase::EntityCustomRootCrashReplay => {
            invocation_result?;
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            assert_replayed_reconstruction_invocation_settled(&oplog, method, reconstruction_start);
            let custom_roots = oplog
                .iter()
                .filter_map(|entry| match &entry.entry {
                    PublicOplogEntry::Start(params)
                        if params.function_name == "golem-it::entity-custom-root" =>
                    {
                        Some(params.parent_start_index)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                custom_roots,
                vec![Some(reconstruction_start)],
                "the body's root custom invocation must record the entity Start as its parent"
            );
        }
        CompletedReconstructionExclusiveCase::Divergence => {
            invocation_result?;
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            assert_replayed_reconstruction_invocation_settled(&oplog, method, reconstruction_start);
            let failed_updates = oplog
                .iter()
                .filter_map(|entry| match &entry.entry {
                    PublicOplogEntry::FailedUpdate(failed) => Some(failed),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(failed_updates.len(), 1, "{failed_updates:?}");
            assert!(
                failed_updates[0]
                    .details
                    .as_deref()
                    .is_some_and(|details| details.starts_with("UPDATE_REPLAY_FAILED")),
                "{:?}",
                failed_updates[0].details
            );
            assert!(
                oplog.iter().all(|entry| !matches!(
                    entry.entry,
                    PublicOplogEntry::Error(_) | PublicOplogEntry::SuccessfulUpdate(_)
                )),
                "the divergent update replay wrote an invocation error or applied the update"
            );
            let metadata = executor.get_worker_metadata(&worker_id).await?;
            assert_eq!(metadata.component_revision, caller_component.revision);
        }
        CompletedReconstructionExclusiveCase::ExecutorShutdownDuringBodyValidation => {
            assert!(
                invocation_result.is_err(),
                "an executor shutdown during body validation must fail the owner invocation"
            );
        }
    }
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn completed_reconstruction_settles_while_exclusive_p2_waits(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_completed_reconstruction_exclusive_p2_case(
        last_unique_id,
        deps,
        provider,
        caller,
        CompletedReconstructionExclusiveCase::Success,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn crash_replay_clock_claim_waits_for_entity_custom_root_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_completed_reconstruction_exclusive_p2_case(
        last_unique_id,
        deps,
        provider,
        caller,
        CompletedReconstructionExclusiveCase::EntityCustomRootCrashReplay,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn crash_replay_clock_claim_waits_for_completed_reconstruction_supervisor(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_completed_reconstruction_exclusive_p2_case(
        last_unique_id,
        deps,
        provider,
        caller,
        CompletedReconstructionExclusiveCase::CrashReplaySupervisorWindow,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn completed_reconstruction_divergence_fails_automatic_update_and_runs_on_source_revision(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_completed_reconstruction_exclusive_p2_case(
        last_unique_id,
        deps,
        provider,
        caller,
        CompletedReconstructionExclusiveCase::Divergence,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn executor_shutdown_cancels_completed_reconstruction_during_body_validation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_completed_reconstruction_exclusive_p2_case(
        last_unique_id,
        deps,
        provider,
        caller,
        CompletedReconstructionExclusiveCase::ExecutorShutdownDuringBodyValidation,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn completed_reconstruction_claim_blocks_concurrent_replay_to_live(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let (
        provider_checkpoint_port,
        provider_checkpoint_gate_port,
        provider_checkpoint_server,
        mut provider_checkpoints,
    ) = start_crash_checkpoint_server().await;
    let (
        caller_checkpoint_port,
        caller_checkpoint_gate_port,
        caller_checkpoint_server,
        mut caller_checkpoints,
    ) = start_crash_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "completed-reconstruction-barrier");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "PROVIDER_CRASH_CHECKPOINT_PORT".to_string(),
                    provider_checkpoint_port.to_string(),
                ),
                (
                    "PROVIDER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    provider_checkpoint_gate_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_PORT".to_string(),
                    caller_checkpoint_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    caller_checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);

    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "hold_completed_reconstruction_barrier",
        data_value!(),
    );
    let crash_and_validate = async {
        let original_body =
            next_crash_checkpoint(&mut provider_checkpoints, "historical-reconstruction-body")
                .await?;
        wait_for_active_tool_operations(&executor, &owned_agent_id, 1).await?;
        original_body
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("original reconstruction body gate was dropped"))?;
        wait_for_active_tool_operations(&executor, &owned_agent_id, 0).await?;
        let original_live =
            next_crash_checkpoint(&mut caller_checkpoints, "reconstruction-live-effect").await?;
        let mut reconstruction_claim = executor.gate_next_entity_reconstruction_claim(&worker_id);
        let mut reconstruction_body =
            executor.gate_next_completed_entity_reconstruction(&worker_id);
        executor.simulated_crash(&worker_id).await?;
        drop(original_live.release);
        let reconstruction_start = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            reconstruction_claim.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("historical reconstruction claim was not reached"))?;
        let barrier = executor
            .drain_terminal_clamp_then_reconstruction_barrier(&owned_agent_id, reconstruction_start)
            .await?;
        tokio::pin!(barrier);
        assert!(
            matches!(futures::poll!(barrier.as_mut()), std::task::Poll::Pending),
            "the primary replay-to-live barrier ignored the atomically registered reconstruction claim"
        );

        reconstruction_claim.release();
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            reconstruction_body.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("completed reconstruction body did not settle"))?;
        assert!(
            matches!(futures::poll!(barrier.as_mut()), std::task::Poll::Pending),
            "the primary replay-to-live barrier was released before body validation"
        );
        reconstruction_body.release();
        tokio::time::timeout(std::time::Duration::from_secs(30), barrier)
            .await
            .map_err(|_| anyhow::anyhow!("validated reconstruction did not release the barrier"))?;
        let replayed_live = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            caller_checkpoints.recv(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("replayed live effect remained blocked"))?
        .ok_or_else(|| anyhow::anyhow!("caller checkpoint server stopped"))?;
        assert_eq!(replayed_live.name, original_live.name);
        replayed_live
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("replayed live-effect gate was dropped"))?;
        Ok::<_, anyhow::Error>(())
    };
    let _ = tokio::try_join!(invocation, crash_and_validate)?;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::Start(params)
                        if params.function_name == "golem::entity::invoke"
                )
            })
            .count(),
        1
    );
    provider_checkpoint_server.abort();
    caller_checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn incomplete_custom_durability_waits_for_completed_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let (
        caller_checkpoint_port,
        caller_checkpoint_gate_port,
        caller_checkpoint_server,
        mut caller_checkpoints,
    ) = start_crash_checkpoint_server().await;
    let agent_id = agent_id!(
        "ToolStreamingCaller",
        "incomplete-custom-reconstruction-barrier"
    );
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "CALLER_CRASH_CHECKPOINT_PORT".to_string(),
                    caller_checkpoint_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    caller_checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let mut original_body = executor.gate_next_live_entity_body_completion(&worker_id, "streaming");

    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "hold_completed_reconstruction_before_incomplete_custom",
        data_value!(),
    );
    let crash_and_validate = async {
        tokio::time::timeout(std::time::Duration::from_secs(30), original_body.entered())
            .await
            .map_err(|_| anyhow::anyhow!("original entity body did not complete"))?;
        let original_before_custom = next_crash_checkpoint(
            &mut caller_checkpoints,
            "before-reconstruction-custom-effect",
        )
        .await?;
        original_body.release();
        wait_for_active_tool_operations(&executor, &owned_agent_id, 0).await?;
        executor.commit_oplog(&worker_id).await?;
        let entity_start = wait_for_completed_entity_terminal(&executor, &worker_id).await?;
        original_before_custom
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("original custom-start gate was dropped"))?;
        let original_custom =
            next_crash_checkpoint(&mut caller_checkpoints, "reconstruction-custom-effect").await?;
        let original_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let custom_start = original_oplog
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem-it::reconstruction-barrier-custom-effect" =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("recorded custom durability Start was not found"))?;
        let entity_terminal = original_oplog
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::End(params) if params.start_index == entity_start => {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("recorded entity terminal was not found"))?;
        assert!(
            entity_terminal < custom_start,
            "completed entity terminal must precede the abandoned custom suffix"
        );
        assert!(!original_oplog.iter().any(|entry| {
            matches!(&entry.entry, PublicOplogEntry::End(params) if params.start_index == custom_start)
                || matches!(&entry.entry, PublicOplogEntry::Cancelled(params) if params.start_index == custom_start)
        }));
        let mut reconstruction_claim = executor.gate_next_entity_reconstruction_claim(&worker_id);
        let mut reconstruction_body =
            executor.gate_next_completed_entity_reconstruction(&worker_id);
        // Keep the original gate unresolved while recovery starts so the custom call cannot gain
        // a terminal during teardown.
        executor.simulated_crash(&worker_id).await?;
        let reconstruction_start = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            reconstruction_claim.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("historical reconstruction claim was not reached"))?;
        assert_eq!(reconstruction_start, entity_start);
        executor
            .drain_reconstruction_terminal(&owned_agent_id, reconstruction_start)
            .await?;
        reconstruction_claim.release();
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            reconstruction_body.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("completed reconstruction body did not settle"))?;
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            executor.clamp_after_claim(&owned_agent_id, custom_start),
        )
        .await
        .map_err(|_| anyhow::anyhow!("custom durability did not reach replay-to-live"))??;
        wait_for_owner_replay_settling(&executor, &owned_agent_id).await?;
        assert!(!executor.owner_replay_is_live(&owned_agent_id).await?);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(250),
                caller_checkpoints.recv()
            )
            .await
            .is_err(),
            "the incomplete custom invocation bypassed the primary reconstruction barrier"
        );
        reconstruction_body.release();
        let replayed_custom = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            caller_checkpoints.recv(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("replayed custom effect remained blocked"))?
        .ok_or_else(|| anyhow::anyhow!("caller checkpoint server stopped"))?;
        assert_eq!(replayed_custom.name, original_custom.name);
        replayed_custom
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("replayed custom-effect gate was dropped"))?;
        drop(original_custom.release);
        Ok::<_, anyhow::Error>(())
    };
    let _ = tokio::try_join!(invocation, crash_and_validate)?;

    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/reconstruction-custom-order.log")
            .await?,
        b"C".as_slice(),
        "the repaired custom effect must commit exactly once after body validation"
    );
    caller_checkpoint_server.abort();
    Ok(())
}

/// Like [`incomplete_custom_durability_waits_for_completed_reconstruction`], but the completed
/// entity body and the caller Store record interleaved positional entries: the provider body is
/// held at its own crash checkpoint (inside the entity Store) while the caller Store performs a
/// checkpoint HTTP request and an atomic TCP gate before its incomplete custom durability. On
/// recovery, the entity's historical reconstruction runs in its own Store and the caller's
/// positional replay must not consume the entity-owned entries — nor may the entity
/// reconstruction consume the caller's — before the custom call reaches the reconstruction
/// barrier.
#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn incomplete_custom_durability_waits_for_overlapping_completed_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let (
        provider_checkpoint_port,
        provider_checkpoint_gate_port,
        provider_checkpoint_server,
        mut provider_checkpoints,
    ) = start_crash_checkpoint_server().await;
    let (
        caller_checkpoint_port,
        caller_checkpoint_gate_port,
        caller_checkpoint_server,
        mut caller_checkpoints,
    ) = start_crash_checkpoint_server().await;
    let agent_id = agent_id!(
        "ToolStreamingCaller",
        "incomplete-custom-reconstruction-overlap"
    );
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "PROVIDER_CRASH_CHECKPOINT_PORT".to_string(),
                    provider_checkpoint_port.to_string(),
                ),
                (
                    "PROVIDER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    provider_checkpoint_gate_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_PORT".to_string(),
                    caller_checkpoint_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    caller_checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);

    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "hold_completed_reconstruction_overlapping_custom",
        data_value!(),
    );
    let crash_and_validate = async {
        // Both Stores reach their checkpoints while the other is still open, so the entity body's
        // remaining entries and the caller's checkpoint entries interleave in the shared oplog.
        let original_body =
            next_crash_checkpoint(&mut provider_checkpoints, "historical-reconstruction-body")
                .await?;
        let original_before_custom = next_crash_checkpoint(
            &mut caller_checkpoints,
            "before-reconstruction-custom-effect",
        )
        .await?;
        wait_for_active_tool_operations(&executor, &owned_agent_id, 1).await?;
        original_body
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("original reconstruction body gate was dropped"))?;
        wait_for_active_tool_operations(&executor, &owned_agent_id, 0).await?;
        original_before_custom
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("original custom-start gate was dropped"))?;
        let original_custom =
            next_crash_checkpoint(&mut caller_checkpoints, "reconstruction-custom-effect").await?;
        let entity_start = wait_for_completed_entity_terminal(&executor, &worker_id).await?;
        let original_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let custom_start = original_oplog
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem-it::reconstruction-barrier-custom-effect" =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("recorded custom durability Start was not found"))?;
        assert!(!original_oplog.iter().any(|entry| {
            matches!(&entry.entry, PublicOplogEntry::End(params) if params.start_index == custom_start)
                || matches!(&entry.entry, PublicOplogEntry::Cancelled(params) if params.start_index == custom_start)
        }));
        let mut reconstruction_claim = executor.gate_next_entity_reconstruction_claim(&worker_id);
        let mut reconstruction_body =
            executor.gate_next_completed_entity_reconstruction(&worker_id);
        executor.simulated_crash(&worker_id).await?;
        let reconstruction_start = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            reconstruction_claim.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("historical reconstruction claim was not reached"))?;
        assert_eq!(reconstruction_start, entity_start);
        executor
            .drain_reconstruction_terminal(&owned_agent_id, reconstruction_start)
            .await?;
        reconstruction_claim.release();
        if tokio::time::timeout(
            std::time::Duration::from_secs(30),
            reconstruction_body.entered(),
        )
        .await
        .is_err()
        {
            for entry in executor.get_oplog(&worker_id, OplogIndex::INITIAL).await? {
                eprintln!("RECONSTRUCTION_FAILURE_OPLOG {entry:?}");
            }
            return Err(anyhow::anyhow!(
                "completed reconstruction body did not settle"
            ));
        }
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            executor.clamp_after_claim(&owned_agent_id, custom_start),
        )
        .await
        .map_err(|_| anyhow::anyhow!("custom durability did not reach replay-to-live"))??;
        wait_for_owner_replay_settling(&executor, &owned_agent_id).await?;
        assert!(!executor.owner_replay_is_live(&owned_agent_id).await?);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(250),
                caller_checkpoints.recv()
            )
            .await
            .is_err(),
            "the incomplete custom invocation bypassed the primary reconstruction barrier"
        );
        reconstruction_body.release();
        let replayed_custom = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            caller_checkpoints.recv(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("replayed custom effect remained blocked"))?
        .ok_or_else(|| anyhow::anyhow!("caller checkpoint server stopped"))?;
        assert_eq!(replayed_custom.name, original_custom.name);
        replayed_custom
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("replayed custom-effect gate was dropped"))?;
        drop(original_custom.release);
        Ok::<_, anyhow::Error>(())
    };
    let _ = tokio::try_join!(invocation, crash_and_validate)?;

    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/reconstruction-custom-order.log")
            .await?,
        b"C".as_slice(),
        "the repaired custom effect must commit exactly once after body validation"
    );
    assert!(
        provider_checkpoints.try_recv().is_err(),
        "the completed entity body must not re-execute during historical reconstruction"
    );
    provider_checkpoint_server.abort();
    caller_checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn recorded_monotonic_clock_replays_across_incomplete_stream_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_clocked_stream_recovery(last_unique_id, deps, provider, caller, false).await
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn incomplete_monotonic_clock_recovers_after_completed_stream(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    run_clocked_stream_recovery(last_unique_id, deps, provider, caller, true).await
}

async fn run_clocked_stream_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    provider: &PrecompiledComponent,
    caller: &PrecompiledComponent,
    incomplete_clock: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoint_arrivals) =
        start_crash_checkpoint_server().await;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "clocked-incomplete-stream");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let first: Vec<u8> = (0..64).collect();
    let second: Vec<u8> = (192..255).collect();
    let expected = [first.as_slice(), second.as_slice()].concat();
    let mut original_body_start =
        (!incomplete_clock).then(|| executor.gate_next_entity_body_start(&worker_id));
    let path = if incomplete_clock {
        "hold-body:/clocked-stream.bin"
    } else {
        "/clocked-stream.bin"
    };
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "clocked_capable_checkpoint",
        data_value!(path, first, second),
    );
    tokio::pin!(invocation);

    let original_checkpoint = tokio::select! {
        checkpoint = async {
            if let Some(gate) = original_body_start.as_mut() {
                gate.entered().await;
                Ok(None)
            } else {
                checkpoint_arrivals.recv().await.map(Some)
                    .ok_or_else(|| anyhow::anyhow!("crash checkpoint server stopped"))
            }
        } => checkpoint?,
        result = &mut invocation => {
            anyhow::bail!("clocked invocation finished before checkpoint: {result:?}");
        }
        () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
            anyhow::bail!("clocked tool did not reach its original body checkpoint");
        }
    };
    if let Some(checkpoint) = &original_checkpoint {
        assert_eq!(checkpoint.name, "capable-body");
    }
    executor.commit_oplog(&worker_id).await?;
    let original_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let clock_starts: Vec<_> = original_oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == "monotonic_clock::now" => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect();
    #[derive(FromSchema)]
    struct ClockTimestamp {
        nanos: u64,
    }
    let clocks: Vec<u64> = original_oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::End(params) if clock_starts.contains(&params.start_index) => Some(
                ClockTimestamp::from_value(
                    params
                        .response
                        .as_ref()
                        .expect("clock End response")
                        .value(),
                )
                .map(|time| time.nanos),
            ),
            _ => None,
        })
        .collect::<Result<_, _>>()?;
    assert!(
        clocks.len() >= 2,
        "normal clock calls must be recorded before the tool"
    );
    let recorded_elapsed = clocks[clocks.len() - 1].saturating_sub(clocks[clocks.len() - 2]);
    let incomplete_start = if incomplete_clock {
        let owner = OwnedAgentId::new(context.default_environment_id, &worker_id);
        let mut clock = executor.gate_next_monotonic_clock_start(&owner).await?;
        original_checkpoint
            .expect("capable body checkpoint")
            .release
            .send(())
            .expect("release original tool body");
        tokio::time::timeout(std::time::Duration::from_secs(30), clock.entered())
            .await
            .map_err(|_| {
                anyhow::anyhow!("clock did not persist its Start after the completed tool")
            })?;
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let start = oplog
            .iter()
            .rev()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name == "monotonic_clock::now" =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .expect("persisted clock Start");
        assert!(oplog.iter().all(|entry| !matches!(&entry.entry, PublicOplogEntry::End(params) if params.start_index == start)));
        wait_for_completed_entity_terminal(&executor, &worker_id).await?;
        clock.abort_as_restart();
        Some(start)
    } else {
        executor.simulated_crash(&worker_id).await?;
        drop(original_body_start);
        None
    };

    let evidence: ClockedStreamEvidence =
        tokio::time::timeout(std::time::Duration::from_secs(30), &mut invocation)
            .await
            .map_err(|_| {
                anyhow::anyhow!("clocked stream recovery deadlocked after checkpoint release")
            })??
            .into_typed()?;
    assert_eq!(evidence.before_tool_nanos, recorded_elapsed);
    assert!(evidence.after_tool_nanos >= evidence.before_tool_nanos);
    let expected_output = if incomplete_clock {
        [b"body-checkpoint".as_slice(), expected.as_slice()].concat()
    } else {
        expected.clone()
    };
    assert_evidence(&evidence.stream, &expected_output, 2, expected.len() as u64);
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/clocked-stream.bin")
            .await?,
        expected
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    if let Some(start) = incomplete_start {
        assert_eq!(oplog.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::End(params) if params.start_index == start)).count(), 1, "incomplete clock must be repaired exactly once");
    }
    assert!(
        oplog
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
        "clock and stream recovery must not write a replay error"
    );
    checkpoint_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("8m")]
async fn deterministic_stream_crash_checkpoint_matrix(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    #[derive(Clone, Copy)]
    enum Checkpoint {
        Incapable {
            name: &'static str,
            checkpoint: &'static str,
            checkpoint_file: Option<&'static str>,
            entity_terminal: bool,
        },
        CapableStaging,
        Capable {
            name: &'static str,
            path: &'static str,
            checkpoint_file: &'static str,
            checkpoint_bytes: &'static [u8],
        },
        CapablePublished {
            path: &'static str,
            checkpoint_file: &'static str,
            checkpoint_bytes: &'static [u8],
        },
    }

    impl Checkpoint {
        fn name(self) -> &'static str {
            match self {
                Self::Incapable { name, .. } | Self::Capable { name, .. } => name,
                Self::CapableStaging => "capable-staging",
                Self::CapablePublished { .. } => "capable-published",
            }
        }

        fn expected_operation(
            self,
        ) -> Option<(ToolBodyAdmissionMetadata, ToolOperationLaneMetadata)> {
            match self {
                Self::Incapable {
                    entity_terminal: false,
                    ..
                } => Some((
                    ToolBodyAdmissionMetadata::Running,
                    ToolOperationLaneMetadata::None,
                )),
                Self::Incapable {
                    entity_terminal: true,
                    ..
                }
                | Self::CapablePublished { .. } => None,
                Self::CapableStaging => Some((
                    ToolBodyAdmissionMetadata::Staging,
                    ToolOperationLaneMetadata::None,
                )),
                Self::Capable { .. } => Some((
                    ToolBodyAdmissionMetadata::Running,
                    ToolOperationLaneMetadata::Granted,
                )),
            }
        }

        fn checkpoint_file(self) -> Option<(&'static str, &'static [u8])> {
            match self {
                Self::Incapable {
                    checkpoint_file: Some(path),
                    ..
                } => Some((path, b"reached")),
                Self::Capable {
                    checkpoint_file,
                    checkpoint_bytes,
                    ..
                }
                | Self::CapablePublished {
                    checkpoint_file,
                    checkpoint_bytes,
                    ..
                } => Some((checkpoint_file, checkpoint_bytes)),
                _ => None,
            }
        }

        fn expects_entity_terminal(self) -> bool {
            matches!(
                self,
                Self::Incapable {
                    entity_terminal: true,
                    ..
                } | Self::CapablePublished { .. }
            )
        }

        fn expects_marker(self) -> bool {
            self.checkpoint_file().is_some()
        }

        fn attachment_progress_ready(self, operation: &ToolOperationMetadata) -> bool {
            let (Some(stdin), Some(stdout)) = (&operation.stdin, &operation.stdout) else {
                return false;
            };
            match self {
                Self::Incapable {
                    checkpoint: "before-input",
                    ..
                } => {
                    stdin.accepted_bytes == 0
                        && stdout.accepted_bytes == 0
                        && !stdin.terminal_selected
                        && !stdout.terminal_selected
                }
                Self::Incapable {
                    checkpoint: "after-input-and-stdout",
                    ..
                } => stdin.accepted_bytes >= 10 && stdout.delivered_bytes >= 17,
                Self::Incapable {
                    checkpoint: "after-eof-before-terminal",
                    ..
                } => {
                    stdin.delivered_bytes >= 10
                        && stdin.terminal_selected
                        && stdout.delivered_bytes >= 12
                        && !stdout.terminal_selected
                }
                Self::Incapable {
                    checkpoint: "after-stdout-terminal",
                    ..
                } => {
                    stdin.delivered_bytes >= 10
                        && stdout.delivered_bytes >= 15
                        && stdout.terminal_selected
                }
                Self::Incapable { .. } => true,
                Self::CapableStaging => stdin.accepted_bytes >= 6,
                Self::Capable {
                    name: "capable-body",
                    ..
                } => {
                    stdin.delivered_bytes >= 12
                        && stdin.terminal_selected
                        && stdout.accepted_bytes >= 15
                }
                Self::Capable {
                    name: "capable-completion",
                    ..
                } => {
                    stdin.delivered_bytes >= 18
                        && stdin.terminal_selected
                        && stdout.accepted_bytes >= 39
                }
                Self::Capable { .. } | Self::CapablePublished { .. } => false,
            }
        }
    }

    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoint_arrivals) =
        start_crash_checkpoint_server().await;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let checkpoints = [
        Checkpoint::Incapable {
            name: "before-input",
            checkpoint: "before-input",
            checkpoint_file: None,
            entity_terminal: false,
        },
        Checkpoint::Incapable {
            name: "after-input-and-stdout",
            checkpoint: "after-input-and-stdout",
            checkpoint_file: Some("/after-input-and-stdout.checkpoint"),
            entity_terminal: false,
        },
        Checkpoint::Incapable {
            name: "after-eof-before-terminal",
            checkpoint: "after-eof-before-terminal",
            checkpoint_file: Some("/after-eof-before-terminal.checkpoint"),
            entity_terminal: false,
        },
        Checkpoint::Incapable {
            name: "after-stdout-terminal",
            checkpoint: "after-stdout-terminal",
            checkpoint_file: Some("/after-stdout-terminal.checkpoint"),
            entity_terminal: false,
        },
        Checkpoint::Incapable {
            name: "after-entity-terminal",
            checkpoint: "after-terminal-before-result",
            checkpoint_file: Some("/after-terminal-before-result.checkpoint"),
            entity_terminal: true,
        },
        Checkpoint::CapableStaging,
        Checkpoint::Capable {
            name: "capable-body",
            path: "hold-body:/capable-body.checkpoint",
            checkpoint_file: "/capable-body.checkpoint",
            checkpoint_bytes: b"capable-body",
        },
        Checkpoint::Capable {
            name: "capable-completion",
            path: "hold-completion:/capable-completion.checkpoint",
            checkpoint_file: "/capable-completion.checkpoint",
            checkpoint_bytes: b"capable-completion:buffered",
        },
        Checkpoint::CapablePublished {
            path: "order:U:/capable-published.bin",
            checkpoint_file: "/capable-order.log",
            checkpoint_bytes: b"U",
        },
    ];
    let checkpoint_filter = std::env::var("GOLEM_TEST_CRASH_CHECKPOINT").ok();

    for checkpoint in checkpoints {
        if checkpoint_filter
            .as_deref()
            .is_some_and(|filter| filter != checkpoint.name())
        {
            continue;
        }
        let agent_id = agent_id!(
            "ToolStreamingCaller",
            format!("crash-{}", checkpoint.name())
        );
        let worker_id = executor
            .start_agent_with(
                &caller_component.id,
                agent_id.clone(),
                HashMap::from([
                    (
                        "CRASH_CHECKPOINT_PORT".to_string(),
                        checkpoint_port.to_string(),
                    ),
                    (
                        "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                        checkpoint_gate_port.to_string(),
                    ),
                ]),
                Vec::new(),
            )
            .await?;
        let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
        match checkpoint {
            Checkpoint::Incapable { checkpoint, .. } => {
                executor
                    .invoke_agent(
                        &caller_component,
                        &agent_id,
                        "hold_incapable_checkpoint",
                        data_value!(checkpoint),
                    )
                    .await?;
            }
            Checkpoint::CapableStaging => {
                executor
                    .invoke_agent(
                        &caller_component,
                        &agent_id,
                        "hold_capable_staging_checkpoint",
                        data_value!(b"staged".to_vec()),
                    )
                    .await?;
            }
            Checkpoint::Capable { path, name, .. } => {
                executor
                    .invoke_agent(
                        &caller_component,
                        &agent_id,
                        "hold_capable_checkpoint",
                        data_value!(path, name.as_bytes().to_vec()),
                    )
                    .await?;
            }
            Checkpoint::CapablePublished { path, .. } => {
                executor
                    .invoke_agent(
                        &caller_component,
                        &agent_id,
                        "hold_capable_published_checkpoint",
                        data_value!(path, checkpoint.name().as_bytes().to_vec()),
                    )
                    .await?;
            }
        }

        let original_checkpoint = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "timed out waiting for original `{}` component gate",
                checkpoint.name()
            )
        })?
        .ok_or_else(|| anyhow::anyhow!("crash checkpoint server stopped"))?;
        assert_eq!(original_checkpoint.name, checkpoint.name());

        let (original_start, original_marker) = if let Some((expected_admission, expected_lane)) =
            checkpoint.expected_operation()
        {
            let (start, marker, admission, lane, attachment_count) =
                tokio::time::timeout(std::time::Duration::from_secs(30), async {
                    loop {
                        if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await
                            && let Some(operation) = active.tool_operations.operations.first()
                            && let Some(start) = operation.start_index
                            && operation.admission == expected_admission
                            && operation.lane == expected_lane
                            && operation.attachment_count == 2
                            && checkpoint.attachment_progress_ready(operation)
                            && (!checkpoint.expects_marker()
                                || active.reached_oplog_marker.is_some())
                            && (expected_admission != ToolBodyAdmissionMetadata::Running
                                || active.slots.iter().any(|slot| {
                                    slot.invocations.iter().any(|invocation| {
                                        invocation.invocation_id.start_index() == start
                                            && invocation.store_attached
                                    })
                                }))
                        {
                            break (
                                start,
                                active.reached_oplog_marker,
                                operation.admission,
                                operation.lane,
                                operation.attachment_count,
                            );
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "timed out waiting for original `{}` crash checkpoint",
                        checkpoint.name()
                    )
                })?;
            assert_eq!(
                admission,
                expected_admission,
                "unexpected admission at `{}`",
                checkpoint.name()
            );
            assert_eq!(
                lane,
                expected_lane,
                "unexpected lane state at `{}`",
                checkpoint.name()
            );
            assert_eq!(
                attachment_count,
                2,
                "both fresh attachments must remain active at `{}`",
                checkpoint.name()
            );
            (start, marker)
        } else {
            let terminal_checkpoint =
                tokio::time::timeout(std::time::Duration::from_secs(30), async {
                    loop {
                        if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await
                            && (!checkpoint.expects_marker()
                                || active.reached_oplog_marker.is_some())
                            && active.tool_operations.operations.is_empty()
                            && active.slots.iter().all(|slot| slot.invocations.is_empty())
                        {
                            let oplog = executor
                                .get_oplog(&worker_id, OplogIndex::INITIAL)
                                .await
                                .expect("read owner oplog at crash checkpoint");
                            let entity_starts = oplog
                                .iter()
                                .filter_map(|entry| match &entry.entry {
                                    PublicOplogEntry::Start(params)
                                        if params.function_name == "golem::entity::invoke" =>
                                    {
                                        Some(entry.oplog_index)
                                    }
                                    _ => None,
                                })
                                .collect::<Vec<_>>();
                            if entity_starts.len() == 1 {
                                break (entity_starts[0], active.reached_oplog_marker);
                            }
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await;
            match terminal_checkpoint {
                Ok(checkpoint) => checkpoint,
                Err(_) => {
                    let active = executor.active_entity_metadata(&owned_agent_id).await;
                    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                    anyhow::bail!(
                        "timed out waiting for original `{}` crash checkpoint; active metadata: {active:#?}; oplog: {oplog:#?}",
                        checkpoint.name()
                    );
                }
            }
        };
        let original_terminal = if checkpoint.expects_entity_terminal() {
            let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
            let terminal = oplog
                .iter()
                .find(|entry| {
                    matches!(
                        &entry.entry,
                        PublicOplogEntry::End(params) if params.start_index == original_start
                    )
                })
                .expect("post-terminal checkpoint must expose its structured entity terminal");
            Some(serde_json::to_value(&terminal.entry)?)
        } else {
            None
        };

        executor.simulated_crash(&worker_id).await?;
        drop(original_checkpoint.release);

        let replayed_checkpoint = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await;
        let replayed_checkpoint = match replayed_checkpoint {
            Ok(Some(checkpoint)) => checkpoint,
            Ok(None) => anyhow::bail!("crash checkpoint server stopped"),
            Err(_) => {
                let active = executor.active_entity_metadata(&owned_agent_id).await;
                let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                anyhow::bail!(
                    "timed out waiting for replayed `{}` component gate; active metadata: {active:#?}; oplog: {oplog:#?}",
                    checkpoint.name()
                );
            }
        };
        assert_eq!(replayed_checkpoint.name, checkpoint.name());

        let replay_ready = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let checkpoint_ready = executor
                    .active_entity_metadata(&owned_agent_id)
                    .await
                    .is_some_and(|active| {
                        let marker_ready = original_marker
                            .is_none_or(|marker| active.reached_oplog_marker == Some(marker));
                        marker_ready
                            && match checkpoint.expected_operation() {
                                Some((admission, lane)) => {
                                    active.tool_operations.operations.first().is_some_and(
                                        |operation| {
                                            operation.start_index == Some(original_start)
                                                && operation.admission == admission
                                                && operation.lane == lane
                                                && operation.attachment_count == 2
                                                && checkpoint.attachment_progress_ready(operation)
                                                && (admission != ToolBodyAdmissionMetadata::Running
                                                    || active.slots.iter().any(|slot| {
                                                        slot.invocations.iter().any(|invocation| {
                                                            invocation.invocation_id.start_index()
                                                                == original_start
                                                                && invocation.store_attached
                                                        })
                                                    }))
                                        },
                                    )
                                }
                                None => active.tool_operations.operations.is_empty(),
                            }
                    });
                if checkpoint_ready {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        if replay_ready.is_err() {
            let active = executor.active_entity_metadata(&owned_agent_id).await;
            anyhow::bail!(
                "timed out waiting for replayed `{}` crash checkpoint; active metadata: {active:#?}",
                checkpoint.name()
            );
        }

        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let entity_starts = oplog
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::entity::invoke" =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            entity_starts,
            vec![original_start],
            "replay must reuse the durable entity Start at `{}`",
            checkpoint.name()
        );
        assert_eq!(
            oplog
                .iter()
                .filter(|entry| {
                    matches!(
                        &entry.entry,
                        PublicOplogEntry::End(params) if params.start_index == original_start
                    )
                })
                .count(),
            usize::from(checkpoint.expects_entity_terminal()),
            "replay must not duplicate the entity terminal at `{}`",
            checkpoint.name()
        );
        if let Some(original_terminal) = original_terminal {
            let replayed_terminal = oplog
                .iter()
                .find(|entry| {
                    matches!(
                        &entry.entry,
                        PublicOplogEntry::End(params) if params.start_index == original_start
                    )
                })
                .expect("replay must retain the structured entity terminal");
            assert_eq!(
                serde_json::to_value(&replayed_terminal.entry)?,
                original_terminal,
                "replay must preserve the exact structured entity terminal"
            );
        }
        assert!(
            oplog
                .iter()
                .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
            "lifecycle replay must not write an owner Error at `{}`",
            checkpoint.name()
        );
        let serialized = serde_json::to_string(&oplog)?;
        for forbidden in ["attachment-id", "endpoint-id", "resource-key"] {
            assert!(
                !serialized.contains(forbidden),
                "transient stream identity `{forbidden}` must not be durable at `{}`",
                checkpoint.name()
            );
        }

        replayed_checkpoint
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("replayed component gate was dropped before release"))?;
        let cleanup = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if executor
                    .active_entity_metadata(&owned_agent_id)
                    .await
                    .is_none_or(|active| {
                        active.tool_operations.operations.is_empty()
                            && active.lane.holder.is_none()
                            && active.lane.active_invocation_count == 0
                            && active.slots.iter().all(|slot| slot.invocations.is_empty())
                    })
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        if cleanup.is_err() {
            let active = executor.active_entity_metadata(&owned_agent_id).await;
            anyhow::bail!(
                "timed out waiting for `{}` checkpoint release cleanup; active metadata: {active:#?}",
                checkpoint.name()
            );
        }
        if let Some((path, expected)) = checkpoint.checkpoint_file() {
            assert_eq!(
                tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    executor.get_file_contents(&worker_id, path),
                )
                .await
                .map_err(|_| anyhow::anyhow!("checkpoint filesystem inspection timed out"))??,
                expected,
                "checkpoint filesystem bytes must be exact after replay at `{}`",
                checkpoint.name()
            );
        }
        executor.delete_worker(&worker_id).await?;
    }

    checkpoint_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("10m")]
async fn dual_output_reconstruction_preserves_independent_history_and_effects(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut arrivals) =
        start_crash_checkpoint_server().await;
    let (effect_port, mut effects, effect_server) =
        middleware_acceptance::start_probe_effect_server().await;

    let cases = [
        (
            "before-either-output",
            b"stdout-last".as_slice(),
            b"stderr-last".as_slice(),
        ),
        (
            "after-stdout-only",
            b"stdout-firststdout-last".as_slice(),
            b"stderr-last".as_slice(),
        ),
        (
            "after-stderr-only",
            b"stdout-last".as_slice(),
            b"stderr-firststderr-last".as_slice(),
        ),
        (
            "after-both-partial",
            b"stdout-firststdout-last".as_slice(),
            b"stderr-firststderr-last".as_slice(),
        ),
        (
            "after-stdout-terminal",
            b"stdout-first".as_slice(),
            b"stderr-last".as_slice(),
        ),
    ];

    for (checkpoint, expected_stdout, expected_stderr) in cases {
        let agent_id = agent_id!(
            "ToolStreamingCaller",
            format!("dual-output-reconstruction-{checkpoint}")
        );
        let worker_id = executor
            .start_agent_with(
                &caller_component.id,
                agent_id.clone(),
                HashMap::from([
                    (
                        "CRASH_CHECKPOINT_PORT".to_string(),
                        checkpoint_port.to_string(),
                    ),
                    (
                        "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                        checkpoint_gate_port.to_string(),
                    ),
                    (
                        "MIDDLEWARE_PROBE_EFFECT_PORT".to_string(),
                        effect_port.to_string(),
                    ),
                ]),
                Vec::new(),
            )
            .await?;
        let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
        let invocation = executor.invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "hold_dual_reconstruction",
            data_value!(checkpoint),
        );
        tokio::pin!(invocation);

        let effect = tokio::select! {
            effect = effects.recv() => effect.expect("dual-output effect server remains available"),
            result = invocation.as_mut() => panic!("dual-output call settled before checkpoint: {result:?}"),
        };
        assert_eq!(effect, format!("dual-reconstruct-{checkpoint}"));
        let original_checkpoint = tokio::select! {
            checkpoint = next_crash_checkpoint(&mut arrivals, checkpoint) => checkpoint?,
            result = invocation.as_mut() => panic!("dual-output call settled before checkpoint: {result:?}"),
        };
        let (start, original_stdout, original_stderr) =
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                loop {
                    if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await
                        && let Some(operation) = active.tool_operations.operations.first()
                        && let Some(start) = operation.start_index
                        && operation.attachment_count == 2
                        && let (Some(stdout), Some(stderr)) = (&operation.stdout, &operation.stderr)
                    {
                        break (start, stdout.clone(), stderr.clone());
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .map_err(|_| {
                anyhow::anyhow!("dual-output operation metadata timed out at {checkpoint}")
            })?;

        executor.simulated_crash(&worker_id).await?;
        drop(original_checkpoint.release);
        let replayed_checkpoint = tokio::select! {
            checkpoint = next_crash_checkpoint(&mut arrivals, checkpoint) => checkpoint?,
            result = invocation.as_mut() => panic!("dual-output replay settled before checkpoint: {result:?}"),
        };
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if executor
                    .active_entity_metadata(&owned_agent_id)
                    .await
                    .and_then(|active| active.tool_operations.operations.first().cloned())
                    .is_some_and(|operation| {
                        operation.start_index == Some(start)
                            && operation.attachment_count == 2
                            && operation.stdout.as_ref() == Some(&original_stdout)
                            && operation.stderr.as_ref() == Some(&original_stderr)
                    })
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("dual-output replay metadata timed out at {checkpoint}"))?;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), effects.recv())
                .await
                .is_err(),
            "replay repeated the external effect at {checkpoint}"
        );
        replayed_checkpoint
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("dual-output replay gate dropped at {checkpoint}"))?;
        let outputs: Vec<Vec<u8>> = invocation.await?.into_typed()?;
        assert_eq!(outputs, [expected_stdout, expected_stderr]);
        executor.delete_worker(&worker_id).await?;
    }

    checkpoint_server.abort();
    effect_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn capable_terminal_lane_return_and_delayed_publication_survive_crash(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoint_arrivals) =
        start_crash_checkpoint_server().await;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "capable-terminal-crash");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let input = b"terminal-before-publication".to_vec();
    let output_path = "/capable-terminal-crash.bin";
    let mut child_completion_gate =
        executor.gate_next_live_entity_body_completion(&worker_id, "streaming");

    let call = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "hold_capable_terminal_checkpoint",
        data_value!(output_path, input.clone()),
    );
    let crash_and_replay = async {
        let original_child_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("original retained child checkpoint timed out"))?
        .ok_or_else(|| anyhow::anyhow!("crash checkpoint server stopped"))?;
        assert_eq!(original_child_gate.name, "capable-terminal-retained-child");
        original_child_gate
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("original retained child gate dropped before release"))?;
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            child_completion_gate.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("original retained child completion gate timed out"))?;
        let mut replayed_child_completion =
            executor.gate_next_incomplete_entity_reconstruction(&worker_id, "streaming");

        let terminal_boundary = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await
                    && let Some(outer) =
                        active.tool_operations.operations.iter().find(|operation| {
                            operation.winner == ToolOperationWinnerMetadata::Ordinary
                        })
                    && let Some(outer_start) = outer.start_index
                    && outer.admission == ToolBodyAdmissionMetadata::Running
                    && outer.lane == ToolOperationLaneMetadata::None
                    && outer.attachment_count == 2
                    && let Some(stdout) = &outer.stdout
                    && stdout.mode == ToolAttachmentModeMetadata::CompletionStaged
                    && stdout.accepted_bytes == input.len() as u64
                    && stdout.delivered_bytes == 0
                    && stdout.buffered_bytes == input.len()
                    && stdout.terminal_selected
                    && matches!(active.lane.holder, Some(OwnerInvocationId::Agent(_)))
                {
                    let oplog = executor
                        .get_oplog(&worker_id, OplogIndex::INITIAL)
                        .await
                        .expect("read oplog at capable terminal boundary");
                    let entity_starts = oplog
                        .iter()
                        .filter_map(|entry| match &entry.entry {
                            PublicOplogEntry::Start(params)
                                if params.function_name == "golem::entity::invoke" =>
                            {
                                Some((entry.oplog_index, params.parent_start_index))
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    let Some(child_start) = entity_starts.iter().find_map(|(start, parent)| {
                        (*parent == Some(outer_start)).then_some(*start)
                    }) else {
                        tokio::task::yield_now().await;
                        continue;
                    };
                    if entity_starts.len() != 2 {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    let Some(terminal) = oplog.iter().find(|entry| {
                        matches!(
                            &entry.entry,
                            PublicOplogEntry::End(params) if params.start_index == outer_start
                        )
                    }) else {
                        tokio::task::yield_now().await;
                        continue;
                    };
                    let durable_effects = oplog
                        .iter()
                        .filter(|entry| {
                            matches!(
                                &entry.entry,
                                PublicOplogEntry::Start(params)
                                    if params.function_name
                                        == "golem::api::generate_idempotency-key"
                                        && params.parent_start_index == Some(child_start)
                            )
                        })
                        .count();
                    let child_has_terminal = oplog.iter().any(|entry| {
                        matches!(
                            &entry.entry,
                            PublicOplogEntry::End(params)
                                if params.start_index == child_start
                        ) || matches!(
                            &entry.entry,
                            PublicOplogEntry::Cancelled(params)
                                if params.start_index == child_start
                        )
                    });
                    if durable_effects == 1 && !child_has_terminal {
                        break (
                            outer_start,
                            child_start,
                            serde_json::to_value(&terminal.entry)
                                .expect("serialize original capable entity terminal"),
                        );
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        let (outer_start, child_start, original_terminal) = match terminal_boundary {
            Ok(boundary) => boundary,
            Err(_) => {
                let active = executor.active_entity_metadata(&owned_agent_id).await;
                let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
                anyhow::bail!(
                    "capable terminal/lane-return/completion-staged boundary timed out; active metadata: {active:#?}; oplog: {oplog:#?}"
                );
            }
        };

        environment_state.set_tool_deployment(
            context.default_environment_id,
            caller_component.id,
            caller_component.revision,
            None,
        );
        executor.simulated_crash(&worker_id).await?;
        drop(child_completion_gate);

        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            replayed_child_completion.entered(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("replayed retained child completion timed out"))?;

        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await
                    && active.tool_operations.operations.iter().any(|operation| {
                        operation.start_index == Some(outer_start)
                            && operation.attachment_count == 2
                            && operation.stdout.as_ref().is_some_and(|stdout| {
                                stdout.mode == ToolAttachmentModeMetadata::CompletionStaged
                                    && stdout.accepted_bytes == input.len() as u64
                                    && stdout.delivered_bytes == 0
                                    && stdout.terminal_selected
                            })
                    })
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("fresh capable replay attachments were not reconstructed"))?;
        replayed_child_completion.release();

        let published_gate = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            checkpoint_arrivals.recv(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("replayed capable publication checkpoint timed out"))?
        .ok_or_else(|| anyhow::anyhow!("crash checkpoint server stopped"))?;
        assert_eq!(published_gate.name, "capable-terminal-published");
        published_gate
            .release
            .send(())
            .map_err(|_| anyhow::anyhow!("published caller gate dropped before release"))?;

        Ok::<_, anyhow::Error>((outer_start, child_start, original_terminal))
    };
    let (_, (outer_start, child_start, original_terminal)) =
        tokio::try_join!(call, crash_and_replay)?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let entity_starts = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == "golem::entity::invoke" => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(entity_starts, vec![outer_start, child_start]);
    for start in [outer_start, child_start] {
        assert_eq!(
            oplog
                .iter()
                .filter(|entry| {
                    matches!(
                        &entry.entry,
                        PublicOplogEntry::End(params) if params.start_index == start
                    )
                })
                .count(),
            1,
            "each pinned entity Start must retain exactly one terminal"
        );
    }
    let replayed_outer_terminal = oplog
        .iter()
        .find(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::End(params) if params.start_index == outer_start
            )
        })
        .expect("outer capable terminal remains in the oplog");
    assert_eq!(
        serde_json::to_value(&replayed_outer_terminal.entry)?,
        original_terminal
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::Start(params)
                        if params.function_name == "golem::api::generate_idempotency-key"
                            && params.parent_start_index == Some(child_start)
                )
            })
            .count(),
        1,
        "recovery must consume the retained child's durable effect exactly once"
    );
    let serialized = serde_json::to_string(&oplog)?;
    for forbidden in ["attachment-id", "endpoint-id", "resource-key"] {
        assert!(!serialized.contains(forbidden));
    }
    assert_eq!(
        executor.get_file_contents(&worker_id, output_path).await?,
        input
    );
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/capable-terminal-published.checkpoint")
            .await?,
        b"reached".as_slice()
    );
    if let Some(active) = executor.active_entity_metadata(&owned_agent_id).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }

    executor.delete_worker(&worker_id).await?;
    checkpoint_server.abort();

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn entity_generated_key_replay_reserves_position_for_incomplete_http_retry(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let (effect_port, effect_server, mut attempts, keys, effects) =
        start_idempotency_effect_server().await;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "entity-idempotency-recovery");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([(
                "IDEMPOTENCY_EFFECT_PORT".to_string(),
                effect_port.to_string(),
            )]),
            Vec::new(),
        )
        .await?;
    let input = b"entity-idempotency".to_vec();
    let call = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "collect_capable",
        data_value!("atomic-idempotency-parent", input.clone()),
    );
    let recover = async {
        let first_key = tokio::time::timeout(std::time::Duration::from_secs(30), attempts.recv())
            .await
            .map_err(|_| anyhow::anyhow!("initial idempotent HTTP effect timed out"))?
            .ok_or_else(|| anyhow::anyhow!("idempotency effect server stopped"))?;

        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        let entity_starts = oplog
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::entity::invoke" =>
                {
                    Some((entry.oplog_index, params.parent_start_index))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let (child_start, Some(parent_start)) = entity_starts
            .iter()
            .find(|(_, parent)| parent.is_some())
            .copied()
            .expect("nested entity was admitted before its HTTP effect")
        else {
            unreachable!()
        };
        assert!(
            oplog
                .iter()
                .any(|entry| matches!(&entry.entry, PublicOplogEntry::EndAtomicRegion(_))),
            "the admitting atomic scope must close before the crash"
        );

        executor.simulated_crash(&worker_id).await?;
        let retry_key = tokio::time::timeout(std::time::Duration::from_secs(30), attempts.recv())
            .await
            .map_err(|_| anyhow::anyhow!("reconstructed HTTP retry timed out"))?
            .ok_or_else(|| anyhow::anyhow!("idempotency effect server stopped before retry"))?;
        assert_eq!(retry_key, first_key, "incomplete HTTP retry changed key");
        Ok::<_, anyhow::Error>((parent_start, child_start))
    };
    let (result, (_parent_start, _child_start)) = tokio::try_join!(call, recover)?;
    let result: StreamEvidence = result.into_typed()?;
    assert_evidence(&result, &input, 1, input.len() as u64);
    let recorded_keys = keys.lock().await;
    assert_eq!(recorded_keys.len(), 2);
    assert_eq!(recorded_keys[0], recorded_keys[1]);
    drop(recorded_keys);
    assert_eq!(
        effects.lock().await.len(),
        1,
        "upstream effect was repeated"
    );

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        oplog
            .iter()
            .any(|entry| matches!(&entry.entry, PublicOplogEntry::EndAtomicRegion(_)))
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::api::generate_idempotency-key"
            ))
            .count(),
        1,
        "completed generated-key call must replay instead of running live"
    );
    executor.delete_worker(&worker_id).await?;
    effect_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn typescript_generated_client_streams_live(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_ts_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_ts_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;

    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-ts-provider",
            "TsToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("TsToolStreamingCaller", "ts-live");
    let evidence: TsStreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "markerBeforeEof",
            data_value!(b"typescript-live".to_vec()),
        )
        .await?
        .into_typed()?;
    assert_eq!(evidence.output, b"ts-marker:typescript-live");
    assert_eq!(evidence.bytes_read, 15);

    let failure: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "typedStdoutFailure",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(failure, "resource-exhausted");

    let declared: TsCompletionEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "declaredErrorCompletion",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(declared.output, b"ts-declared:");
    assert_eq!(declared.stdout_terminal, "finished");
    assert_eq!(declared.result_terminal, "declared-error");

    let dual: TsDualOutputEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "dualOutputDeclaredError",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(dual.stdout, vec![0, 127, 128, 255]);
    assert_eq!(dual.stderr, vec![255, 128, 1, 2]);
    assert_eq!(dual.result_terminal, "declared-error");

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn scala_generated_client_streams_live(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_scala")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;

    let stored_component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let component_path = deps
        .component_directory
        .join(format!("{}.wasm", component.wasm_name));
    let metadata = extract_component_metadata(&component_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        stored_component.id,
        stored_component.revision,
        Some(deployment_state(
            context.account_id,
            stored_component.id,
            stored_component.revision,
            "scala:examples",
            "ScalaToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ScalaToolStreamingCaller", "scala-live");
    let evidence: ScalaStreamEvidence = executor
        .invoke_and_await_agent(
            &stored_component,
            &agent_id,
            "markerBeforeEof",
            data_value!("scala-live"),
        )
        .await?
        .into_typed()?;
    assert_eq!(evidence.output, "scala-marker:scala-live");
    assert_eq!(evidence.bytes_read, 10);

    for (mode, terminal, result) in [
        ("finish", "finished", "done"),
        ("fail", "failed:stream producer failed", "done"),
        ("unit", "finished", "unit"),
        ("plain", "none", "plain"),
    ] {
        let output: ScalaOutputEvidence = executor
            .invoke_and_await_agent(
                &stored_component,
                &agent_id,
                "outputEvidence",
                data_value!(mode),
            )
            .await?
            .into_typed()?;
        assert_eq!(
            output.bytes,
            if mode == "plain" {
                vec![]
            } else {
                vec![0, 127, 128, 255]
            },
            "{mode}"
        );
        assert_eq!(output.terminal, terminal, "{mode}");
        assert_eq!(output.result, result, "{mode}");
    }

    let cleanup: ScalaCleanupEvidence = executor
        .invoke_and_await_agent(
            &stored_component,
            &agent_id,
            "invalidCommandPathCleanup",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(cleanup.error, "invalid-command-path:missing");
    assert!(cleanup.stdin_cancelled);
    assert_eq!(cleanup.stdout_terminal, "failed");

    let declared: ScalaOutputEvidence = executor
        .invoke_and_await_agent(
            &stored_component,
            &agent_id,
            "declaredErrorCompletion",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        declared.bytes,
        b"scala-declared:"
            .iter()
            .map(|byte| i32::from(*byte))
            .collect::<Vec<_>>()
    );
    assert_eq!(declared.terminal, "finished");
    assert_eq!(declared.result, "declared:expected");

    let dual: ScalaDualOutputEvidence = executor
        .invoke_and_await_agent(
            &stored_component,
            &agent_id,
            "dualOutputDeclaredError",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(dual.stdout, vec![0, 127, 255]);
    assert_eq!(dual.stderr, vec![128, 1, 2]);
    assert_eq!(dual.result, "declared:dual-expected");

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn moonbit_generated_client_streams_live(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_moonbit")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;

    let stored_component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let component_path = deps
        .component_directory
        .join(format!("{}.wasm", component.wasm_name));
    let metadata = extract_component_metadata(&component_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        stored_component.id,
        stored_component.revision,
        Some(deployment_state(
            context.account_id,
            stored_component.id,
            stored_component.revision,
            "golem:moonbit-examples",
            "MoonBitToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("MoonBitToolStreamingCaller", "moonbit-live");
    let worker_id = executor
        .start_agent(&stored_component.id, agent_id.clone())
        .await?;
    let owned_agent_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let payload = b"moonbit-live".to_vec();
    let payload_value = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::binary(BinaryRestrictions::default())),
        SchemaValue::Binary(BinaryValuePayload {
            bytes: payload.clone(),
            mime_type: None,
        }),
    );
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        executor.invoke_and_await_agent(
            &stored_component,
            &agent_id,
            "marker_before_eof",
            build_input_record(vec![payload_value])?,
        ),
    )
    .await;
    let bytes_read: u64 = match result {
        Ok(result) => result?.into_typed()?,
        Err(_) => {
            let active = executor.active_entity_metadata(&owned_agent_id).await;
            anyhow::bail!("MoonBit streaming call timed out; active metadata: {active:#?}")
        }
    };
    assert_eq!(bytes_read, payload.len() as u64);

    let explicit_failure: String = executor
        .invoke_and_await_agent(
            &stored_component,
            &agent_id,
            "explicit_stdout_failure",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(explicit_failure, "resource-exhausted:ok");

    let declared: String = executor
        .invoke_and_await_agent(
            &stored_component,
            &agent_id,
            "declared_error_completion",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(declared, "finished:declared:expected");

    let dual: MoonBitDualOutputEvidence = executor
        .invoke_and_await_agent(
            &stored_component,
            &agent_id,
            "dual_output_declared_error",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(dual.stdout, b"moon-out:\x00\xff");
    assert_eq!(dual.stderr, b"moon-err:\x80!");
    assert_eq!(dual.result, "declared:dual-expected");

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn native_external_tool_session_delivers_stdout_before_input_eof(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::worker::{
        ExternalToolInvocation, InputStreamEnd, InputStreamItem, InvocationRequest,
        InvocationStart, ResumeAttach, ResumeOperation, StreamCursor, ToolByteStreamRole,
        input_stream_item, invocation_request, invocation_response, invocation_session_completion,
        invocation_session_result,
    };
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        "ToolStreamingCaller",
        metadata.tools,
    );
    for bindings in deployment.tool_bindings.values_mut() {
        bindings
            .get_mut(&ToolName::try_from("streaming").unwrap())
            .unwrap()
            .filesystem_access = ToolFilesystemAccess::Allowed;
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let worker_id = executor
        .start_agent(
            &caller_component.id,
            agent_id!("ToolStreamingCaller", "native-session"),
        )
        .await?;
    let metadata = executor.get_worker_metadata(&worker_id).await?;
    let key = IdempotencyKey::fresh();
    let input = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(vec![
            golem_common::schema::NamedFieldType {
                name: "mode".to_string(),
                body: SchemaType::string(),
                metadata: Default::default(),
            },
        ])),
        SchemaValue::Record {
            fields: vec![SchemaValue::String("marker-echo".to_string())],
        },
    );
    let (sender, receiver) = tokio::sync::mpsc::channel(8);
    let start_request = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(worker_id.clone().into()),
            idempotency_key: Some(key.clone().into()),
            auth_ctx: Some(executor.auth_ctx().into()),
            principal: Some(
                Principal::GolemUser(GolemUserPrincipal {
                    account_id: context.account_id,
                })
                .into(),
            ),
            environment_id: Some(context.default_environment_id.into()),
            component_owner_account_id: Some(context.account_id.into()),
            expected_callee_fingerprint: Some(metadata.fingerprint.0.into()),
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            external_tool: Some(ExternalToolInvocation {
                tool_name: "streaming".to_string(),
                command_path: vec!["run".to_string()],
                input: Some(input.try_into().map_err(anyhow::Error::msg)?),
                stdin: true,
                stdout: true,
                stderr: false,
                fresh_owner: false,
                expected_deployment_revision: None,
            }),
            ..Default::default()
        })),
    };
    sender.send(start_request.clone()).await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let acceptance = responses
        .message()
        .await?
        .expect("native session acceptance");
    let Some(invocation_response::Response::Accepted(accepted)) = acceptance.response else {
        anyhow::bail!("native session was not accepted: {acceptance:?}");
    };
    let stdin_id = accepted
        .stream_mappings
        .iter()
        .find(|mapping| mapping.tool_byte_stream_role == Some(ToolByteStreamRole::Stdin as i32))
        .expect("stdin role")
        .transport_stream_id;
    let stdout_id = accepted
        .stream_mappings
        .iter()
        .find(|mapping| mapping.tool_byte_stream_role == Some(ToolByteStreamRole::Stdout as i32))
        .expect("stdout role")
        .transport_stream_id;
    assert_ne!(stdin_id, stdout_id);
    let stdin_mapping = accepted
        .stream_mappings
        .iter()
        .find(|mapping| mapping.transport_stream_id == stdin_id)
        .expect("stdin mapping");
    let stdin_stream_id = stdin_mapping.handle.as_ref().unwrap().stream_id;
    let mut output = Vec::new();
    let mut stdout_cursor = None;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while output.len() < b"marker:".len() {
            match responses
                .message()
                .await?
                .expect("stdout before EOF")
                .response
            {
                Some(invocation_response::Response::OutputItem(item)) => {
                    assert_eq!(item.transport_stream_id, stdout_id);
                    stdout_cursor = Some(StreamCursor {
                        stream_id: item.durable_stream_id,
                        last_observed_offset: Some(item.durable_offset),
                    });
                    output.extend(item.packed_u8);
                }
                other => anyhow::bail!("expected stdout before submitting stdin, got {other:?}"),
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    assert_eq!(output, b"marker:");
    let mut changed = start_request.clone();
    let Some(invocation_request::Request::Start(start)) = &mut changed.request else {
        unreachable!()
    };
    start.external_tool.as_mut().unwrap().stdout = false;
    let mut retry = executor
        .client
        .clone()
        .invoke_agent_session(tokio_stream::iter([changed]))
        .await?
        .into_inner();
    let rejected = retry
        .message()
        .await?
        .expect("changed declaration response");
    assert!(
        matches!(rejected.response, Some(invocation_response::Response::Rejected(ref error)) if error.reason == golem_api_grpc::proto::golem::worker::InvocationRejectionReason::IdempotencyConflict as i32),
        "{rejected:?}"
    );

    // Take over while the result is still blocked on stdin, resuming after the marker.
    let Some(invocation_request::Request::Start(start)) = start_request.request else {
        unreachable!()
    };
    let (resumed_sender, receiver) = tokio::sync::mpsc::channel(8);
    resumed_sender
        .send(InvocationRequest {
            request: Some(invocation_request::Request::ResumeAttach(ResumeAttach {
                idempotency_key: start.idempotency_key,
                agent_id: start.agent_id,
                environment_id: start.environment_id,
                attachment_id: accepted.attachment_id,
                attempt_id: Some(uuid::Uuid::new_v4().into()),
                expected_callee_fingerprint: start.expected_callee_fingerprint,
                expected_epoch: accepted.epoch,
                operation: ResumeOperation::Takeover as i32,
                cursors: vec![stdout_cursor.unwrap()],
                auth_ctx: start.auth_ctx,
                principal: start.principal,
            })),
        })
        .await?;
    let mut resumed = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let response = resumed.message().await?.expect("takeover acceptance");
    let Some(invocation_response::Response::Accepted(resumed_accepted)) = response.response else {
        anyhow::bail!("takeover failed: {response:?}");
    };
    assert_eq!(resumed_accepted.epoch, accepted.epoch + 1);
    assert_eq!(
        resumed_accepted.stream_mappings.len(),
        accepted.stream_mappings.len()
    );
    for original in &accepted.stream_mappings {
        let resumed = resumed_accepted
            .stream_mappings
            .iter()
            .find(|mapping| mapping.transport_stream_id == original.transport_stream_id)
            .unwrap();
        assert_eq!(resumed.handle, original.handle);
        assert_eq!(resumed.role, original.role);
        assert_eq!(
            resumed.tool_byte_stream_role,
            original.tool_byte_stream_role
        );
    }
    drop(responses);
    drop(sender);
    let sender = resumed_sender;
    responses = resumed;
    let accepted = resumed_accepted;
    let bytes = vec![0, 255, 17, 128, 9];
    sender
        .send(InvocationRequest {
            request: Some(invocation_request::Request::InputItem(InputStreamItem {
                transport_stream_id: stdin_id,
                sequence: 0,
                payload: Some(input_stream_item::Payload::PackedU8(bytes.clone())),
                durable_stream_id: stdin_stream_id,
                epoch: accepted.epoch,
            })),
        })
        .await?;
    sender
        .send(InvocationRequest {
            request: Some(invocation_request::Request::InputEnd(InputStreamEnd {
                transport_stream_id: stdin_id,
                sequence: bytes.len() as u64,
                durable_stream_id: stdin_stream_id,
                epoch: accepted.epoch,
            })),
        })
        .await?;
    let mut result_seen = false;
    let mut ended = false;
    let mut finished = false;
    while let Some(response) = responses.message().await? {
        match response.response {
            Some(invocation_response::Response::OutputItem(item)) => {
                assert_eq!(item.transport_stream_id, stdout_id);
                output.extend(item.packed_u8);
            }
            Some(invocation_response::Response::OutputEnd(end)) => {
                assert_eq!(end.transport_stream_id, stdout_id);
                assert!(!ended);
                ended = true;
            }
            Some(invocation_response::Response::Result(result)) => {
                let Some(invocation_session_result::Result::ToolResult(result)) = result.result
                else {
                    anyhow::bail!("native session returned a non-tool result");
                };
                let result: golem_common::model::oplog::PublicExternalToolResult =
                    result.try_into().map_err(anyhow::Error::msg)?;
                let golem_common::model::oplog::PublicExternalToolResult::Success(result) = result
                else {
                    anyhow::bail!("tool failed: {result:?}");
                };
                let value = result.result.expect("stream summary");
                let SchemaValue::Record { fields } = value.value() else {
                    panic!("summary must be a record")
                };
                assert_eq!(fields[1], SchemaValue::U64(bytes.len() as u64));
                assert_eq!(fields[2], SchemaValue::Bool(false));
                result_seen = true;
            }
            Some(invocation_response::Response::InputAck(_)) => {}
            Some(invocation_response::Response::Finished(completion)) => {
                assert!(
                    matches!(
                        &completion.outcome,
                        Some(invocation_session_completion::Outcome::Success(_))
                    ),
                    "native external-tool session failed: {completion:?}"
                );
                finished = true;
            }
            other => anyhow::bail!("unexpected native session response: {other:?}"),
        }
    }
    assert_eq!(output, [b"marker:".as_slice(), bytes.as_slice()].concat());
    assert!(result_seen && ended && finished);
    drop(sender);
    drop(responses);
    drop(executor);

    // Replay must reconstruct the capable body from journals with no client socket.
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        None,
    );
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state),
            ..Default::default()
        },
    )
    .await?;
    let (completed_sender, completed_receiver) = tokio::sync::mpsc::channel(1);
    completed_sender
        .send(InvocationRequest {
            request: Some(invocation_request::Request::ResumeAttach(ResumeAttach {
                idempotency_key: accepted.idempotency_key.clone(),
                agent_id: accepted.agent_id.clone(),
                environment_id: accepted.environment_id,
                attachment_id: accepted.attachment_id,
                attempt_id: Some(uuid::Uuid::new_v4().into()),
                expected_callee_fingerprint: accepted.callee_fingerprint,
                expected_epoch: accepted.epoch,
                operation: ResumeOperation::Resume as i32,
                cursors: Vec::new(),
                auth_ctx: Some(executor.auth_ctx().into()),
                principal: Some(
                    Principal::GolemUser(GolemUserPrincipal {
                        account_id: context.account_id,
                    })
                    .into(),
                ),
            })),
        })
        .await?;
    let mut completed = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(completed_receiver))
        .await?
        .into_inner();
    let response = completed
        .message()
        .await?
        .expect("completed session acceptance");
    let Some(invocation_response::Response::Accepted(reopened)) = response.response else {
        anyhow::bail!("completed session resume failed: {response:?}");
    };
    assert_eq!(reopened.stream_mappings.len(), 2);
    for original in &accepted.stream_mappings {
        let mapping = reopened
            .stream_mappings
            .iter()
            .find(|mapping| mapping.transport_stream_id == original.transport_stream_id)
            .expect("same binding after result publication and restart");
        assert_eq!(mapping.handle, original.handle);
        assert_eq!(mapping.role, original.role);
    }
    let mut completed_result = false;
    let mut completed_finished = false;
    while let Some(response) = completed.message().await? {
        match response.response {
            Some(invocation_response::Response::Result(result)) => {
                assert!(matches!(
                    result.result,
                    Some(invocation_session_result::Result::ToolResult(_))
                ));
                completed_result = true;
            }
            Some(invocation_response::Response::Finished(completion)) => {
                assert!(matches!(
                    completion.outcome,
                    Some(invocation_session_completion::Outcome::Success(_))
                ));
                completed_finished = true;
            }
            Some(invocation_response::Response::OutputItem(_))
            | Some(invocation_response::Response::OutputEnd(_)) => {}
            other => anyhow::bail!("unexpected completed session response: {other:?}"),
        }
    }
    assert!(completed_result && completed_finished);
    drop(completed_sender);
    let after: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id!("ToolStreamingCaller", "native-session"),
            "record_native_order",
            data_value!("R"),
        )
        .await?
        .into_typed()?;
    assert_eq!(after, "R");
    Ok(())
}

async fn exercise_filesystem_tools(
    executor: &TestWorkerExecutor,
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    environment_state: &TestEnvironmentStateService,
    caller_component: &golem_common::model::component::ComponentDto,
    provider: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let definitions = metadata
        .tools
        .iter()
        .map(|definition| {
            assert!(definition.requires_filesystem);
            let name = definition
                .name()
                .expect("filesystem tool has a root command");
            (ToolName::try_from(name).unwrap(), definition.clone())
        })
        .collect::<BTreeMap<_, _>>();
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem:filesystem-tools",
        "ToolStreamingCaller",
        metadata.tools,
    );
    for bindings in deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.filesystem_access = ToolFilesystemAccess::Allowed;
        }
    }
    let mut denied_deployment = deployment.clone();
    for bindings in denied_deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.filesystem_access = ToolFilesystemAccess::Denied;
        }
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "filesystem-tools");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let fingerprint = executor.get_worker_metadata(&worker_id).await?.fingerprint;
    let principal = Principal::GolemUser(GolemUserPrincipal {
        account_id: context.account_id,
    });
    let path = "workspace/filesystem-tools/notes.txt".to_string();

    let write = invoke_filesystem_tool_success(
        executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definitions,
        "write-file",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String(path.clone()),
            ),
            (
                "content",
                SchemaType::string(),
                SchemaValue::String("one\r\ntwo\nthree".to_string()),
            ),
            (
                "create-parent-directories",
                SchemaType::bool(),
                SchemaValue::Bool(true),
            ),
        ]),
    )
    .await?;
    assert_eq!(
        write,
        SchemaValue::Record {
            fields: vec![SchemaValue::Enum { case: 0 }, SchemaValue::U64(14)]
        }
    );

    let read = invoke_filesystem_tool_success(
        executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definitions,
        "read-file",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String(path.clone()),
            ),
            {
                let (schema, value) = filesystem_optional_line(Some(2));
                ("start-line", schema, value)
            },
            {
                let (schema, value) = filesystem_optional_line(None);
                ("end-line", schema, value)
            },
            {
                let (schema, value) = filesystem_cursor_none();
                ("cursor", schema, value)
            },
        ]),
    )
    .await?;
    assert_eq!(
        read,
        SchemaValue::Record {
            fields: vec![
                SchemaValue::String("two\nthree".to_string()),
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::U64(2))),
                },
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::U64(3))),
                },
                SchemaValue::Option { inner: None },
            ]
        }
    );

    let edit = invoke_filesystem_tool_success(
        executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definitions,
        "edit-file",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String(path.clone()),
            ),
            (
                "old-text",
                SchemaType::string(),
                SchemaValue::String("two\n".to_string()),
            ),
            (
                "new-text",
                SchemaType::string(),
                SchemaValue::String("TWO\r\n".to_string()),
            ),
        ]),
    )
    .await?;
    assert_eq!(
        edit,
        SchemaValue::Record {
            fields: vec![
                SchemaValue::U64(1),
                SchemaValue::U64(14),
                SchemaValue::U64(15)
            ]
        }
    );

    let full_read = invoke_filesystem_tool_success(
        executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definitions,
        "read-file",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String(path.clone()),
            ),
            {
                let (schema, value) = filesystem_optional_line(None);
                ("start-line", schema, value)
            },
            {
                let (schema, value) = filesystem_optional_line(None);
                ("end-line", schema, value)
            },
            {
                let (schema, value) = filesystem_cursor_none();
                ("cursor", schema, value)
            },
        ]),
    )
    .await?;
    assert_eq!(
        full_read,
        SchemaValue::Record {
            fields: vec![
                SchemaValue::String("one\r\nTWO\r\nthree".to_string()),
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::U64(1))),
                },
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::U64(3))),
                },
                SchemaValue::Option { inner: None },
            ]
        }
    );

    let root = "workspace/filesystem-tools".to_string();
    let ls = invoke_filesystem_tool_success(
        executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definitions,
        "ls",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String(root.clone()),
            ),
            (
                "max-depth",
                SchemaType::option(SchemaType::u32()),
                SchemaValue::Option { inner: None },
            ),
            (
                "glob",
                SchemaType::option(SchemaType::string()),
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::String("*.txt".to_string()))),
                },
            ),
            (
                "limit",
                SchemaType::option(SchemaType::u32()),
                SchemaValue::Option { inner: None },
            ),
            (
                "cursor",
                SchemaType::option(SchemaType::string()),
                SchemaValue::Option { inner: None },
            ),
        ]),
    )
    .await?;
    assert_eq!(
        ls,
        SchemaValue::Record {
            fields: vec![
                SchemaValue::List {
                    elements: vec![SchemaValue::Record {
                        fields: vec![
                            SchemaValue::String(path.clone()),
                            SchemaValue::Variant(VariantValuePayload {
                                case: 0,
                                payload: Some(Box::new(SchemaValue::U64(15))),
                            }),
                        ],
                    }],
                },
                SchemaValue::List {
                    elements: Vec::new(),
                },
                SchemaValue::Option { inner: None },
            ],
        }
    );

    let grep = invoke_filesystem_tool_success(
        executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definitions,
        "grep",
        filesystem_tool_input(vec![
            ("path", SchemaType::string(), SchemaValue::String(root)),
            (
                "pattern",
                SchemaType::string(),
                SchemaValue::String("T.O".to_string()),
            ),
            (
                "mode",
                SchemaType::option(SchemaType::string()),
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::Enum { case: 1 })),
                },
            ),
            (
                "max-depth",
                SchemaType::option(SchemaType::u32()),
                SchemaValue::Option { inner: None },
            ),
            (
                "limit",
                SchemaType::option(SchemaType::u32()),
                SchemaValue::Option { inner: None },
            ),
            (
                "cursor",
                SchemaType::option(SchemaType::string()),
                SchemaValue::Option { inner: None },
            ),
            (
                "include-globs",
                SchemaType::list(SchemaType::string()),
                SchemaValue::List {
                    elements: vec![SchemaValue::String("*.txt".to_string())],
                },
            ),
            (
                "exclude-globs",
                SchemaType::list(SchemaType::string()),
                SchemaValue::List {
                    elements: Vec::new(),
                },
            ),
            (
                "case-insensitive",
                SchemaType::bool(),
                SchemaValue::Bool(false),
            ),
        ]),
    )
    .await?;
    assert_eq!(
        grep,
        SchemaValue::Record {
            fields: vec![
                SchemaValue::List {
                    elements: vec![SchemaValue::Record {
                        fields: vec![
                            SchemaValue::String(path.clone()),
                            SchemaValue::U64(2),
                            SchemaValue::String("TWO".to_string()),
                            SchemaValue::Bool(false),
                        ],
                    }],
                },
                SchemaValue::List {
                    elements: Vec::new(),
                },
                SchemaValue::Option { inner: None },
            ],
        }
    );

    let bounded_stream_probe: Vec<u64> = executor
        .invoke_and_await_agent(
            caller_component,
            &agent_id,
            "probe_bounded_wasi_stream",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(bounded_stream_probe, vec![4, 0, 4]);

    executor
        .invoke_and_await_agent(
            caller_component,
            &agent_id,
            "prepare_filesystem_limit_fixtures",
            data_value!(),
        )
        .await?;
    let oversized_directory = invoke_filesystem_tool_success(
        executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definitions,
        "ls",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String("workspace/filesystem-tools/oversized-directory".to_string()),
            ),
            (
                "max-depth",
                SchemaType::option(SchemaType::u32()),
                SchemaValue::Option { inner: None },
            ),
            (
                "glob",
                SchemaType::option(SchemaType::string()),
                SchemaValue::Option { inner: None },
            ),
            (
                "limit",
                SchemaType::option(SchemaType::u32()),
                SchemaValue::Option { inner: None },
            ),
            (
                "cursor",
                SchemaType::option(SchemaType::string()),
                SchemaValue::Option { inner: None },
            ),
        ]),
    )
    .await?;
    let SchemaValue::Record { fields } = oversized_directory else {
        anyhow::bail!("oversized-directory ls returned a non-record result");
    };
    let [
        SchemaValue::List { elements: entries },
        SchemaValue::List {
            elements: diagnostics,
        },
        _,
    ] = fields.as_slice()
    else {
        anyhow::bail!("oversized-directory ls returned an unexpected result shape");
    };
    assert!(entries.is_empty());
    assert!(matches!(
        diagnostics.as_slice(),
        [SchemaValue::Record { fields }]
            if matches!(fields.get(1), Some(SchemaValue::Enum { case: 4 }))
    ));

    let bounded_read = invoke_filesystem_tool_success(
        executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definitions,
        "grep",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String("workspace/filesystem-tools/bounded-read.txt".to_string()),
            ),
            (
                "pattern",
                SchemaType::string(),
                SchemaValue::String("not-present".to_string()),
            ),
            (
                "mode",
                SchemaType::option(SchemaType::string()),
                SchemaValue::Option { inner: None },
            ),
            (
                "max-depth",
                SchemaType::option(SchemaType::u32()),
                SchemaValue::Option { inner: None },
            ),
            (
                "limit",
                SchemaType::option(SchemaType::u32()),
                SchemaValue::Option { inner: None },
            ),
            (
                "cursor",
                SchemaType::option(SchemaType::string()),
                SchemaValue::Option { inner: None },
            ),
            (
                "include-globs",
                SchemaType::list(SchemaType::string()),
                SchemaValue::List {
                    elements: Vec::new(),
                },
            ),
            (
                "exclude-globs",
                SchemaType::list(SchemaType::string()),
                SchemaValue::List {
                    elements: Vec::new(),
                },
            ),
            (
                "case-insensitive",
                SchemaType::bool(),
                SchemaValue::Bool(false),
            ),
        ]),
    )
    .await?;
    assert!(matches!(
        bounded_read,
        SchemaValue::Record { fields }
            if matches!(fields.as_slice(), [
                SchemaValue::List { elements: matches },
                SchemaValue::List { elements: diagnostics },
                SchemaValue::Option { inner: Some(_) },
            ] if matches.is_empty() && diagnostics.is_empty())
    ));
    let bounded_grep_pages: Vec<u64> = executor
        .invoke_and_await_agent(
            caller_component,
            &agent_id,
            "filesystem_limit_roundtrip",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(bounded_grep_pages, vec![262_144, 4_097, 4, 0]);

    let stale = invoke_filesystem_tool(
        executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definitions,
        "edit-file",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String(path.clone()),
            ),
            (
                "old-text",
                SchemaType::string(),
                SchemaValue::String("outdated".to_string()),
            ),
            (
                "new-text",
                SchemaType::string(),
                SchemaValue::String("replacement".to_string()),
            ),
        ]),
    )
    .await?;
    assert_filesystem_tool_error(stale, "stale-edit")?;

    let replace = invoke_filesystem_tool_success(
        executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definitions,
        "write-file",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String(path.clone()),
            ),
            (
                "content",
                SchemaType::string(),
                SchemaValue::String("same same".to_string()),
            ),
            (
                "create-parent-directories",
                SchemaType::bool(),
                SchemaValue::Bool(false),
            ),
        ]),
    )
    .await?;
    assert_eq!(
        replace,
        SchemaValue::Record {
            fields: vec![SchemaValue::Enum { case: 1 }, SchemaValue::U64(9)]
        }
    );

    let ambiguous = invoke_filesystem_tool(
        executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definitions,
        "edit-file",
        filesystem_tool_input(vec![
            ("path", SchemaType::string(), SchemaValue::String(path)),
            (
                "old-text",
                SchemaType::string(),
                SchemaValue::String("same".to_string()),
            ),
            (
                "new-text",
                SchemaType::string(),
                SchemaValue::String("different".to_string()),
            ),
        ]),
    )
    .await?;
    assert_filesystem_tool_error(ambiguous, "ambiguous-edit")?;

    // "aa" occurs twice in "aaa" (at offsets 0 and 1), so the documented
    // unambiguous-edit contract requires rejecting this edit.
    let overlap_path = "workspace/filesystem-tools/overlap.txt".to_string();
    invoke_filesystem_tool_success(
        executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definitions,
        "write-file",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String(overlap_path.clone()),
            ),
            (
                "content",
                SchemaType::string(),
                SchemaValue::String("aaa".to_string()),
            ),
            (
                "create-parent-directories",
                SchemaType::bool(),
                SchemaValue::Bool(true),
            ),
        ]),
    )
    .await?;
    let overlapping = invoke_filesystem_tool(
        executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definitions,
        "edit-file",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String(overlap_path),
            ),
            (
                "old-text",
                SchemaType::string(),
                SchemaValue::String("aa".to_string()),
            ),
            (
                "new-text",
                SchemaType::string(),
                SchemaValue::String("x".to_string()),
            ),
        ]),
    )
    .await?;
    assert_filesystem_tool_error(overlapping, "ambiguous-edit")?;

    let traversal = invoke_filesystem_tool(
        executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definitions,
        "read-file",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String("../outside.txt".to_string()),
            ),
            {
                let (schema, value) = filesystem_optional_line(None);
                ("start-line", schema, value)
            },
            {
                let (schema, value) = filesystem_optional_line(None);
                ("end-line", schema, value)
            },
            {
                let (schema, value) = filesystem_cursor_none();
                ("cursor", schema, value)
            },
        ]),
    )
    .await?;
    assert_filesystem_tool_error(traversal, "unsafe-path")?;

    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(denied_deployment),
    );
    let denied_worker = executor
        .start_agent(
            &caller_component.id,
            agent_id!("ToolStreamingCaller", "filesystem-tools-denied"),
        )
        .await?;
    let denied_fingerprint = executor
        .get_worker_metadata(&denied_worker)
        .await?
        .fingerprint;
    for tool_name in ["ls", "grep"] {
        let input = if tool_name == "ls" {
            filesystem_tool_input(vec![
                (
                    "path",
                    SchemaType::string(),
                    SchemaValue::String("workspace".to_string()),
                ),
                (
                    "max-depth",
                    SchemaType::option(SchemaType::u32()),
                    SchemaValue::Option { inner: None },
                ),
                (
                    "glob",
                    SchemaType::option(SchemaType::string()),
                    SchemaValue::Option { inner: None },
                ),
                (
                    "limit",
                    SchemaType::option(SchemaType::u32()),
                    SchemaValue::Option { inner: None },
                ),
                (
                    "cursor",
                    SchemaType::option(SchemaType::string()),
                    SchemaValue::Option { inner: None },
                ),
            ])
        } else {
            filesystem_tool_input(vec![
                (
                    "path",
                    SchemaType::string(),
                    SchemaValue::String("workspace".to_string()),
                ),
                (
                    "pattern",
                    SchemaType::string(),
                    SchemaValue::String("text".to_string()),
                ),
                (
                    "mode",
                    SchemaType::option(SchemaType::string()),
                    SchemaValue::Option { inner: None },
                ),
                (
                    "max-depth",
                    SchemaType::option(SchemaType::u32()),
                    SchemaValue::Option { inner: None },
                ),
                (
                    "limit",
                    SchemaType::option(SchemaType::u32()),
                    SchemaValue::Option { inner: None },
                ),
                (
                    "cursor",
                    SchemaType::option(SchemaType::string()),
                    SchemaValue::Option { inner: None },
                ),
                (
                    "include-globs",
                    SchemaType::list(SchemaType::string()),
                    SchemaValue::List {
                        elements: Vec::new(),
                    },
                ),
                (
                    "exclude-globs",
                    SchemaType::list(SchemaType::string()),
                    SchemaValue::List {
                        elements: Vec::new(),
                    },
                ),
                (
                    "case-insensitive",
                    SchemaType::bool(),
                    SchemaValue::Bool(false),
                ),
            ])
        };
        let denied = invoke_filesystem_tool(
            executor,
            &denied_worker,
            denied_fingerprint,
            principal.clone(),
            &definitions,
            tool_name,
            input,
        )
        .await
        .expect_err("filesystem-disabled tool activation must fail");
        assert!(
            denied.to_string().contains("filesystemAccess is denied"),
            "filesystem-disabled '{tool_name}' failed for the wrong reason: {denied:?}"
        );
    }
    Ok(())
}

async fn exercise_guest_invoked_filesystem_tools(
    executor: &TestWorkerExecutor,
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    environment_state: &TestEnvironmentStateService,
    caller_component: &golem_common::model::component::ComponentDto,
    provider: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem:filesystem-tools",
        "ToolStreamingCaller",
        metadata.tools,
    );
    for bindings in deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.filesystem_access = ToolFilesystemAccess::Allowed;
        }
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let evidence: Vec<String> = executor
        .invoke_and_await_agent(
            caller_component,
            &agent_id!("ToolStreamingCaller", "filesystem-tools-guest"),
            "filesystem_tool_roundtrip",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        evidence,
        vec![
            "created",
            "14",
            "two\nthree",
            "2",
            "3",
            "none",
            "1",
            "14",
            "15",
            "workspace/guest-filesystem-tools/notes.txt:file:15:pages=5",
            "workspace/guest-filesystem-tools/notes.txt:2:TWO:matches=2:diagnostics=1:pages=3",
        ]
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn builtin_filesystem_tools_have_expected_behavior(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("filesystem_tools")] filesystem_tools: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;

    exercise_filesystem_tools(
        &executor,
        deps,
        &context,
        &environment_state,
        &caller_component,
        filesystem_tools,
    )
    .await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("10m")]
async fn builtin_javascript_and_typescript_tools_run_in_sidecars(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("javascript_tools")] javascript_tools: &PrecompiledComponent,
    #[tagged_as("typescript_tools")] typescript_tools: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    exercise_javascript_and_typescript_tools(
        last_unique_id,
        deps,
        caller,
        javascript_tools,
        typescript_tools,
        false,
    )
    .await
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn builtin_node_recursive_rm_removes_typescript_output(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("javascript_tools")] javascript_tools: &PrecompiledComponent,
    #[tagged_as("typescript_tools")] typescript_tools: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let mut deployment: Option<ToolDeploymentState> = None;
    for (provider, package) in [
        (javascript_tools, "golem:javascript-tools"),
        (typescript_tools, "golem:typescript-tools"),
    ] {
        let component = executor
            .component_dep(&context.default_environment_id, provider)
            .store()
            .await?;
        let metadata = extract_component_metadata(
            &deps
                .component_directory
                .join(format!("{}.wasm", provider.wasm_name)),
            false,
            true,
        )
        .await?;
        let provider_deployment = deployment_state(
            context.account_id,
            component.id,
            component.revision,
            package,
            "ToolStreamingCaller",
            metadata.tools,
        );
        if let Some(deployment) = &mut deployment {
            deployment
                .registered_tools
                .extend(provider_deployment.registered_tools);
            for (owner, bindings) in provider_deployment.tool_bindings {
                deployment
                    .tool_bindings
                    .entry(owner)
                    .or_default()
                    .extend(bindings);
            }
        } else {
            deployment = Some(provider_deployment);
        }
    }
    let mut deployment = deployment.unwrap();
    for bindings in deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.filesystem_access = ToolFilesystemAccess::Allowed;
        }
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let mut failures = Vec::new();
    for (agent_name, remove) in [
        (
            "recursive-rm-sync",
            "fs.rmSync('candidate', { recursive: true, force: true }); verify();",
        ),
        (
            "recursive-rm-promises",
            "await require('node:fs/promises').rm('candidate', { recursive: true, force: true }); verify();",
        ),
    ] {
        let setup = invoke_cli_tool(
            &executor,
            &caller_component,
            agent_name,
            "node",
            "/workspace",
            vec![
                "-e",
                "const fs = require('node:fs'); fs.mkdirSync('src/nested', { recursive: true }); fs.writeFileSync('src/main.ts', 'export const main: number = 17;\\n'); fs.writeFileSync('src/nested/value.ts', 'export const value: number = 42;\\n');",
            ],
        )
        .await?;
        assert_eq!(setup.exit_code, 0, "setup: {setup:?}");

        let compilation = invoke_cli_tool(
            &executor,
            &caller_component,
            agent_name,
            "tsc",
            "/workspace",
            vec![
                "--pretty",
                "false",
                "--target",
                "es2022",
                "--module",
                "es2022",
                "--rootDir",
                "src",
                "--outDir",
                "candidate",
                "--noEmitOnError",
                "src/main.ts",
                "src/nested/value.ts",
            ],
        )
        .await?;
        assert_eq!(compilation.exit_code, 0, "compilation: {compilation:?}");

        let script = format!(
            r#"
const fs = require('node:fs');
const assert = require('node:assert/strict');
assert.equal(fs.readFileSync('candidate/main.js', 'utf8'), 'export const main = 17;\n');
assert.equal(fs.readFileSync('candidate/nested/value.js', 'utf8'), 'export const value = 42;\n');
console.log('compiled-output-readable');
function verify() {{
    assert.equal(fs.existsSync('candidate'), false);
    assert.equal(fs.readFileSync('src/nested/value.ts', 'utf8'), 'export const value: number = 42;\n');
    console.log('candidate-removed-source-preserved');
}}
{remove}
"#
        );
        let removal = invoke_cli_tool(
            &executor,
            &caller_component,
            agent_name,
            "node",
            "/workspace",
            vec!["-e", &script],
        )
        .await?;
        assert!(
            removal.stdout.starts_with(b"compiled-output-readable\n"),
            "output must be readable before removal: {removal:?}"
        );
        if removal.exit_code != 0
            || removal.stdout != b"compiled-output-readable\ncandidate-removed-source-preserved\n"
            || !removal.stderr.is_empty()
        {
            failures.push(format!(
                "{agent_name}: exit {}; stdout: {}; stderr: {}",
                removal.exit_code,
                String::from_utf8_lossy(&removal.stdout),
                String::from_utf8_lossy(&removal.stderr),
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn typescript_guest_recursive_rm_removes_tool_created_output(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_ts_caller")] caller: &PrecompiledComponent,
    #[tagged_as("typescript_tools")] typescript_tools: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider = executor
        .component_dep(&context.default_environment_id, typescript_tools)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", typescript_tools.wasm_name)),
        false,
        true,
    )
    .await?;
    let mut deployment = deployment_state(
        context.account_id,
        provider.id,
        provider.revision,
        "golem:typescript-tools",
        "TsToolStreamingCaller",
        metadata.tools,
    );
    for bindings in deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.filesystem_access = ToolFilesystemAccess::Allowed;
        }
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let mut failures = Vec::new();
    for promises in [false, true] {
        let result = executor
            .invoke_and_await_agent(
                &caller_component,
                &agent_id!("TsToolStreamingCaller", format!("guest-rm-{promises}")),
                "recursiveRmToolOutput",
                data_value!(promises),
            )
            .await?
            .into_typed::<Result<String, String>>()?;
        if result != Ok("removed-output-source-preserved-recompiled".to_string()) {
            failures.push(format!("promises={promises}: {result:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

#[test]
#[ignore = "GOL-714: completed JavaScript tool calls do not reconstruct deterministically"]
#[tracing::instrument]
#[timeout("10m")]
async fn builtin_javascript_and_typescript_tools_reconstruct_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("javascript_tools")] javascript_tools: &PrecompiledComponent,
    #[tagged_as("typescript_tools")] typescript_tools: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    exercise_javascript_and_typescript_tools(
        last_unique_id,
        deps,
        caller,
        javascript_tools,
        typescript_tools,
        true,
    )
    .await
}

async fn exercise_javascript_and_typescript_tools(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    caller: &PrecompiledComponent,
    javascript_tools: &PrecompiledComponent,
    typescript_tools: &PrecompiledComponent,
    verify_reconstruction: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;

    for (provider, package, tool, version, stdout) in [
        (javascript_tools, "golem:javascript-tools", "node", "", ""),
        (
            javascript_tools,
            "golem:javascript-tools",
            "npm",
            "10.9.9",
            "10.9.9\n",
        ),
        (
            javascript_tools,
            "golem:javascript-tools",
            "npx",
            "10.9.9",
            "10.9.9\n",
        ),
        (
            typescript_tools,
            "golem:typescript-tools",
            "tsc",
            "5.9.2",
            "Version 5.9.2\n",
        ),
    ] {
        invoke_cli_tool_version(
            &executor,
            deps,
            &context,
            &environment_state,
            &caller_component,
            provider,
            package,
            tool,
            version,
            stdout,
        )
        .await?;
    }

    let javascript_component = executor
        .component_dep(&context.default_environment_id, javascript_tools)
        .store()
        .await?;
    let javascript_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", javascript_tools.wasm_name)),
        false,
        true,
    )
    .await?;
    let typescript_component = executor
        .component_dep(&context.default_environment_id, typescript_tools)
        .store()
        .await?;
    let typescript_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", typescript_tools.wasm_name)),
        false,
        true,
    )
    .await?;
    let mut deployment = deployment_state(
        context.account_id,
        javascript_component.id,
        javascript_component.revision,
        "golem:javascript-tools",
        "ToolStreamingCaller",
        javascript_metadata.tools,
    );
    let typescript_deployment = deployment_state(
        context.account_id,
        typescript_component.id,
        typescript_component.revision,
        "golem:typescript-tools",
        "ToolStreamingCaller",
        typescript_metadata.tools,
    );
    deployment
        .registered_tools
        .extend(typescript_deployment.registered_tools);
    for (owner, bindings) in typescript_deployment.tool_bindings {
        deployment
            .tool_bindings
            .entry(owner)
            .or_default()
            .extend(bindings);
    }
    for bindings in deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.filesystem_access = ToolFilesystemAccess::Allowed;
        }
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let delayed_output = invoke_cli_tool(
        &executor,
        &caller_component,
        "node-delayed-output",
        "node",
        "/workspace",
        vec!["-e", "setTimeout(() => console.log('late'), 20)"],
    )
    .await?;
    assert_eq!(delayed_output.exit_code, 0);
    assert_eq!(delayed_output.stdout, b"late\n");
    assert!(delayed_output.stderr.is_empty());

    let immediate_exit = invoke_cli_tool(
        &executor,
        &caller_component,
        "node-immediate-exit",
        "node",
        "/workspace",
        vec!["-e", "process.exit(7); console.log('unexpected')"],
    )
    .await?;
    assert_eq!(immediate_exit.exit_code, 7);
    assert!(immediate_exit.stdout.is_empty());
    assert!(immediate_exit.stderr.is_empty());

    let behavior_agent = "js-ts-behavior";
    let fixture_source = r##"
const fs = require('node:fs');
fs.mkdirSync('local-pkg', { recursive: true });
fs.writeFileSync('package.json', JSON.stringify({
  name: 'builtin-tools-fixture',
  version: '1.0.0',
  private: true,
  scripts: { probe: "node -e \"console.log('npm-script-ok')\"" }
}));
fs.writeFileSync('local-pkg/package.json', JSON.stringify({
  name: 'local-tool',
  version: '1.0.0',
  bin: { 'local-tool': 'cli.js' }
}));
fs.writeFileSync(
  'local-pkg/cli.js',
  "#!/usr/bin/env node\nconsole.log('npx:' + process.argv.slice(2).join(','));\n",
  { mode: 0o755 }
);
fs.writeFileSync('valid.ts', 'const value: number = 42;\nconsole.log(value);\n');
fs.writeFileSync('invalid.ts', 'const value: number = "wrong";\n');
"##;
    let fixture = invoke_cli_tool(
        &executor,
        &caller_component,
        behavior_agent,
        "node",
        "/workspace",
        vec!["-e", fixture_source],
    )
    .await?;
    assert_eq!(
        fixture.exit_code,
        0,
        "fixture stdout: {}; stderr: {}",
        String::from_utf8_lossy(&fixture.stdout),
        String::from_utf8_lossy(&fixture.stderr)
    );
    assert!(fixture.stdout.is_empty());
    assert!(fixture.stderr.is_empty());

    let npm_install = invoke_cli_tool(
        &executor,
        &caller_component,
        behavior_agent,
        "npm",
        "/workspace",
        vec![
            "install",
            "./local-pkg",
            "--offline",
            "--ignore-scripts",
            "--no-package-lock",
            "--no-audit",
            "--no-fund",
        ],
    )
    .await?;
    assert_eq!(
        npm_install.exit_code,
        0,
        "npm install stderr: {}",
        String::from_utf8_lossy(&npm_install.stderr)
    );

    let npm_script = invoke_cli_tool(
        &executor,
        &caller_component,
        behavior_agent,
        "npm",
        "/workspace",
        vec!["run", "probe"],
    )
    .await?;
    assert_eq!(npm_script.exit_code, 0);
    assert!(String::from_utf8(npm_script.stdout)?.contains("npm-script-ok"));
    assert!(npm_script.stderr.is_empty());

    let npx = invoke_cli_tool(
        &executor,
        &caller_component,
        behavior_agent,
        "npx",
        "/workspace",
        vec!["--no-install", "local-tool", "one", "two"],
    )
    .await?;
    assert_eq!(npx.exit_code, 0);
    assert_eq!(npx.stdout, b"npx:one,two\n");
    assert!(npx.stderr.is_empty());

    let tsc_success = invoke_cli_tool(
        &executor,
        &caller_component,
        behavior_agent,
        "tsc",
        "/workspace",
        vec![
            "--pretty", "false", "--target", "es2022", "--module", "commonjs", "--outDir", "dist",
            "valid.ts",
        ],
    )
    .await?;
    assert_eq!(
        tsc_success.exit_code,
        0,
        "tsc stderr: {}",
        String::from_utf8_lossy(&tsc_success.stderr)
    );
    assert!(tsc_success.stdout.is_empty());

    let tsc_failure = invoke_cli_tool(
        &executor,
        &caller_component,
        behavior_agent,
        "tsc",
        "/workspace",
        vec!["--pretty", "false", "--noEmit", "invalid.ts"],
    )
    .await?;
    assert_ne!(tsc_failure.exit_code, 0);
    assert!(String::from_utf8(tsc_failure.stdout)?.contains("error TS2322"));

    if verify_reconstruction {
        drop(executor);
        let executor = start_with_overrides(deps, &context, overrides).await?;
        let reconstructed = invoke_cli_tool(
            &executor,
            &caller_component,
            behavior_agent,
            "node",
            "/workspace",
            vec![
                "-e",
                "const fs = require('node:fs'); console.log(fs.readFileSync('dist/valid.js', 'utf8').includes('const value = 42'))",
            ],
        )
        .await?;
        assert_eq!(reconstructed.exit_code, 0);
        assert_eq!(reconstructed.stdout, b"true\n");
        assert!(reconstructed.stderr.is_empty());
    }
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn builtin_git_tool_persists_local_workflow_across_invocations_and_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("filesystem_tools")] filesystem_tools: &PrecompiledComponent,
    #[tagged_as("git_tool")] git_tool: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = || TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides()).await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let filesystem_component = executor
        .component_dep(&context.default_environment_id, filesystem_tools)
        .store()
        .await?;
    let git_component = executor
        .component_dep(&context.default_environment_id, git_tool)
        .store()
        .await?;
    let filesystem_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", filesystem_tools.wasm_name)),
        false,
        true,
    )
    .await?;
    let git_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", git_tool.wasm_name)),
        false,
        true,
    )
    .await?;
    let git_definition = git_metadata
        .tools
        .iter()
        .find(|definition| definition.name() == Some("git"))
        .expect("git component exports the git tool")
        .clone();
    let filesystem_definitions = filesystem_metadata
        .tools
        .iter()
        .map(|definition| {
            (
                ToolName::try_from(definition.name().unwrap()).unwrap(),
                definition.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut deployment = deployment_state(
        context.account_id,
        git_component.id,
        git_component.revision,
        "golem:git-tool",
        "ToolStreamingCaller",
        git_metadata.tools,
    );
    let filesystem_deployment = deployment_state(
        context.account_id,
        filesystem_component.id,
        filesystem_component.revision,
        "golem:filesystem-tools",
        "ToolStreamingCaller",
        filesystem_metadata.tools,
    );
    deployment
        .registered_tools
        .extend(filesystem_deployment.registered_tools);
    for (owner, bindings) in filesystem_deployment.tool_bindings {
        deployment
            .tool_bindings
            .entry(owner)
            .or_default()
            .extend(bindings);
    }
    for bindings in deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.filesystem_access = ToolFilesystemAccess::Allowed;
        }
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "git-tool-lifecycle");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let fingerprint = executor.get_worker_metadata(&worker_id).await?.fingerprint;
    let principal = Principal::GolemUser(GolemUserPrincipal {
        account_id: context.account_id,
    });
    let cwd = string_list(&["/workspace/repo"]);

    invoke_git_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &git_definition,
        "init",
        BTreeMap::from([
            ("working-directory", string_list(&[])),
            ("directory", optional_string(Some("workspace/repo"))),
            ("initial-branch", optional_string(Some("main"))),
        ]),
    )
    .await?;
    for (key, value) in [
        ("user.name", "Integration Test"),
        ("user.email", "integration@example.com"),
    ] {
        invoke_git_tool_success(
            &executor,
            &worker_id,
            fingerprint,
            principal.clone(),
            &git_definition,
            "config",
            BTreeMap::from([
                ("working-directory", cwd.clone()),
                ("key", SchemaValue::String(key.to_string())),
                ("value", optional_string(Some(value))),
                ("local", SchemaValue::Bool(true)),
            ]),
        )
        .await?;
    }
    invoke_filesystem_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &filesystem_definitions,
        "write-file",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String("workspace/repo/hello.txt".to_string()),
            ),
            (
                "content",
                SchemaType::string(),
                SchemaValue::String("hello\n".to_string()),
            ),
            (
                "create-parent-directories",
                SchemaType::bool(),
                SchemaValue::Bool(false),
            ),
        ]),
    )
    .await?;
    invoke_git_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &git_definition,
        "add",
        BTreeMap::from([
            ("working-directory", cwd.clone()),
            ("paths", string_list(&["hello.txt"])),
            ("all", SchemaValue::Bool(false)),
            ("update", SchemaValue::Bool(false)),
        ]),
    )
    .await?;
    let commit = invoke_git_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &git_definition,
        "commit",
        BTreeMap::from([
            ("working-directory", cwd.clone()),
            ("message", string_list(&["initial"])),
            ("author", optional_string(None)),
            ("allow-empty", SchemaValue::Bool(false)),
        ]),
    )
    .await?;
    let Some(SchemaValue::Record {
        fields: commit_fields,
    }) = commit
    else {
        anyhow::bail!("git commit returned an unexpected result")
    };
    let SchemaValue::String(commit_oid) = &commit_fields[0] else {
        anyhow::bail!("git commit did not return an object id")
    };
    assert_eq!(commit_oid.len(), 40);

    invoke_filesystem_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &filesystem_definitions,
        "write-file",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String("workspace/repo/hello.txt".to_string()),
            ),
            (
                "content",
                SchemaType::string(),
                SchemaValue::String("index\n".to_string()),
            ),
            (
                "create-parent-directories",
                SchemaType::bool(),
                SchemaValue::Bool(false),
            ),
        ]),
    )
    .await?;
    invoke_git_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &git_definition,
        "add",
        BTreeMap::from([
            ("working-directory", cwd.clone()),
            ("paths", string_list(&["hello.txt"])),
            ("all", SchemaValue::Bool(false)),
            ("update", SchemaValue::Bool(false)),
        ]),
    )
    .await?;
    invoke_filesystem_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &filesystem_definitions,
        "write-file",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String("workspace/repo/hello.txt".to_string()),
            ),
            (
                "content",
                SchemaType::string(),
                SchemaValue::String("worktree\n".to_string()),
            ),
            (
                "create-parent-directories",
                SchemaType::bool(),
                SchemaValue::Bool(false),
            ),
        ]),
    )
    .await?;

    let index_before_restart = executor
        .get_file_contents(&worker_id, "/workspace/repo/.git/index")
        .await?;

    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides()).await?;
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/workspace/repo/.git/index")
            .await?,
        index_before_restart,
        "completed replay must preserve the exact staged index"
    );
    let config = invoke_git_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &git_definition,
        "config",
        BTreeMap::from([
            ("working-directory", cwd.clone()),
            ("key", SchemaValue::String("user.name".to_string())),
            ("value", optional_string(None)),
            ("local", SchemaValue::Bool(true)),
        ]),
    )
    .await?;
    let Some(SchemaValue::Record {
        fields: config_fields,
    }) = config
    else {
        anyhow::bail!("git config returned an unexpected result")
    };
    assert_eq!(
        config_fields[0],
        SchemaValue::String("user.name".to_string())
    );
    assert_eq!(config_fields[1], optional_string(Some("Integration Test")));
    assert_eq!(config_fields[2], SchemaValue::Bool(false));

    let status = invoke_git_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &git_definition,
        "status",
        BTreeMap::from([
            ("working-directory", cwd.clone()),
            ("paths", string_list(&[])),
            ("short", SchemaValue::Bool(false)),
            ("porcelain", SchemaValue::String("v1".to_string())),
            ("null", SchemaValue::Bool(false)),
        ]),
    )
    .await?;
    let Some(SchemaValue::Record {
        fields: status_fields,
    }) = status
    else {
        anyhow::bail!("git status returned an unexpected result")
    };
    let SchemaValue::List { elements } = &status_fields[0] else {
        anyhow::bail!("git status did not return an entry list")
    };
    assert_eq!(elements.len(), 1);
    let SchemaValue::Record {
        fields: status_entry_fields,
    } = &elements[0]
    else {
        anyhow::bail!("git status did not return a status entry")
    };
    assert_eq!(
        status_entry_fields[0],
        SchemaValue::String("hello.txt".to_string())
    );
    assert_eq!(
        status_entry_fields[3],
        SchemaValue::String("MM".to_string())
    );
    let head: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "read_owner_file",
            data_value!("/workspace/repo/.git/HEAD"),
        )
        .await?
        .into_typed()?;
    assert_eq!(head, "ref: refs/heads/main\n");
    let branch_tip: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "read_owner_file",
            data_value!("/workspace/repo/.git/refs/heads/main"),
        )
        .await?
        .into_typed()?;
    assert_eq!(branch_tip.trim(), commit_oid);

    let log = invoke_git_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &git_definition,
        "log",
        BTreeMap::from([
            ("working-directory", cwd.clone()),
            ("ref", optional_string(None)),
            ("max-count", SchemaValue::F64(1.0)),
            ("oneline", SchemaValue::Bool(true)),
        ]),
    )
    .await?;
    let Some(SchemaValue::Record { fields: log_fields }) = log else {
        anyhow::bail!("git log returned an unexpected result")
    };
    assert_eq!(
        log_fields[1],
        SchemaValue::String(format!("{} initial\n", &commit_oid[..7]))
    );

    let cached_diff = invoke_git_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &git_definition,
        "diff",
        BTreeMap::from([
            ("working-directory", cwd.clone()),
            ("from", optional_string(None)),
            ("to", optional_string(None)),
            ("paths", string_list(&["hello.txt"])),
            ("unified", SchemaValue::F64(3.0)),
            ("cached", SchemaValue::Bool(true)),
            ("name-only", SchemaValue::Bool(false)),
            ("stat", SchemaValue::Bool(false)),
        ]),
    )
    .await?;
    let Some(SchemaValue::Record {
        fields: cached_diff_fields,
    }) = cached_diff
    else {
        anyhow::bail!("git diff --cached returned an unexpected result")
    };
    let SchemaValue::String(cached_patch) = &cached_diff_fields[0] else {
        anyhow::bail!("git diff --cached did not return a patch")
    };
    assert!(cached_patch.contains("-hello\n+index"));

    let diff = invoke_git_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &git_definition,
        "diff",
        BTreeMap::from([
            ("working-directory", cwd.clone()),
            ("from", optional_string(None)),
            ("to", optional_string(None)),
            ("paths", string_list(&["hello.txt"])),
            ("unified", SchemaValue::F64(3.0)),
            ("cached", SchemaValue::Bool(false)),
            ("name-only", SchemaValue::Bool(false)),
            ("stat", SchemaValue::Bool(false)),
        ]),
    )
    .await?;
    let Some(SchemaValue::Record {
        fields: diff_fields,
    }) = diff
    else {
        anyhow::bail!("git diff returned an unexpected result")
    };
    let SchemaValue::String(patch) = &diff_fields[0] else {
        anyhow::bail!("git diff did not return a patch")
    };
    assert!(patch.contains("-index\n+worktree"));

    invoke_git_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &git_definition,
        "checkout",
        BTreeMap::from([
            ("working-directory", cwd.clone()),
            ("new-branch", optional_string(Some("persisted-state"))),
            ("detach", SchemaValue::Bool(false)),
            ("ref", optional_string(None)),
            ("paths", string_list(&[])),
        ]),
    )
    .await?;
    let feature_head: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "read_owner_file",
            data_value!("/workspace/repo/.git/HEAD"),
        )
        .await?
        .into_typed()?;
    assert_eq!(feature_head, "ref: refs/heads/persisted-state\n");
    let preserved_worktree: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "read_owner_file",
            data_value!("/workspace/repo/hello.txt"),
        )
        .await?
        .into_typed()?;
    assert_eq!(preserved_worktree, "worktree\n");

    invoke_git_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &git_definition,
        "checkout",
        BTreeMap::from([
            ("working-directory", cwd.clone()),
            ("new-branch", optional_string(None)),
            ("detach", SchemaValue::Bool(false)),
            ("ref", optional_string(None)),
            ("paths", string_list(&["hello.txt"])),
        ]),
    )
    .await?;
    let restored: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "read_owner_file",
            data_value!("/workspace/repo/hello.txt"),
        )
        .await?
        .into_typed()?;
    assert_eq!(restored, "index\n");

    invoke_git_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &git_definition,
        "checkout",
        BTreeMap::from([
            ("working-directory", cwd.clone()),
            ("new-branch", optional_string(None)),
            ("detach", SchemaValue::Bool(false)),
            ("ref", optional_string(Some("main"))),
            ("paths", string_list(&[])),
        ]),
    )
    .await?;
    let main_head: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "read_owner_file",
            data_value!("/workspace/repo/.git/HEAD"),
        )
        .await?
        .into_typed()?;
    assert_eq!(main_head, "ref: refs/heads/main\n");
    Ok(())
}

struct GitToolHarness {
    context: TestContext,
    environment_state: Arc<TestEnvironmentStateService>,
    executor: TestWorkerExecutor,
    caller_component_id: golem_common::model::component::ComponentId,
    worker_id: golem_common::model::AgentId,
    fingerprint: golem_common::model::AgentFingerprint,
    principal: Principal,
    git_definition: golem_common::schema::tool::Tool,
    filesystem_definitions: BTreeMap<ToolName, golem_common::schema::tool::Tool>,
}

async fn start_git_tool_harness(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    caller: &PrecompiledComponent,
    filesystem_tools: &PrecompiledComponent,
    git_tool: &PrecompiledComponent,
    instance_name: &str,
) -> anyhow::Result<GitToolHarness> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let filesystem_component = executor
        .component_dep(&context.default_environment_id, filesystem_tools)
        .store()
        .await?;
    let git_component = executor
        .component_dep(&context.default_environment_id, git_tool)
        .store()
        .await?;
    let filesystem_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", filesystem_tools.wasm_name)),
        false,
        true,
    )
    .await?;
    let git_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", git_tool.wasm_name)),
        false,
        true,
    )
    .await?;
    let git_definition = git_metadata
        .tools
        .iter()
        .find(|definition| definition.name() == Some("git"))
        .expect("git component exports the git tool")
        .clone();
    let filesystem_definitions = filesystem_metadata
        .tools
        .iter()
        .map(|definition| {
            (
                ToolName::try_from(definition.name().unwrap()).unwrap(),
                definition.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut deployment = deployment_state(
        context.account_id,
        git_component.id,
        git_component.revision,
        "golem:git-tool",
        "ToolStreamingCaller",
        git_metadata.tools,
    );
    let filesystem_deployment = deployment_state(
        context.account_id,
        filesystem_component.id,
        filesystem_component.revision,
        "golem:filesystem-tools",
        "ToolStreamingCaller",
        filesystem_metadata.tools,
    );
    deployment
        .registered_tools
        .extend(filesystem_deployment.registered_tools);
    for (owner, bindings) in filesystem_deployment.tool_bindings {
        deployment
            .tool_bindings
            .entry(owner)
            .or_default()
            .extend(bindings);
    }
    for bindings in deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.filesystem_access = ToolFilesystemAccess::Allowed;
        }
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let agent_id = agent_id!("ToolStreamingCaller", instance_name);
    let worker_id = executor.start_agent(&caller_component.id, agent_id).await?;
    let fingerprint = executor.get_worker_metadata(&worker_id).await?.fingerprint;
    let principal = Principal::GolemUser(GolemUserPrincipal {
        account_id: context.account_id,
    });
    Ok(GitToolHarness {
        context,
        environment_state,
        executor,
        caller_component_id: caller_component.id,
        worker_id,
        fingerprint,
        principal,
        git_definition,
        filesystem_definitions,
    })
}

async fn initialize_git_harness(harness: &GitToolHarness, branch: &str) -> anyhow::Result<()> {
    invoke_git_tool_success(
        &harness.executor,
        &harness.worker_id,
        harness.fingerprint,
        harness.principal.clone(),
        &harness.git_definition,
        "init",
        BTreeMap::from([
            ("working-directory", string_list(&[])),
            ("directory", optional_string(Some("workspace/repo"))),
            ("initial-branch", optional_string(Some(branch))),
        ]),
    )
    .await?;
    for (key, value) in [
        ("user.name", "Recovery Test"),
        ("user.email", "recovery@example.com"),
    ] {
        invoke_git_tool_success(
            &harness.executor,
            &harness.worker_id,
            harness.fingerprint,
            harness.principal.clone(),
            &harness.git_definition,
            "config",
            BTreeMap::from([
                ("working-directory", string_list(&["/workspace/repo"])),
                ("key", SchemaValue::String(key.to_string())),
                ("value", optional_string(Some(value))),
                ("local", SchemaValue::Bool(true)),
            ]),
        )
        .await?;
    }
    Ok(())
}

fn git_commit_oid(value: Option<SchemaValue>) -> anyhow::Result<String> {
    let Some(SchemaValue::Record { fields }) = value else {
        anyhow::bail!("git commit returned an unexpected result")
    };
    let Some(SchemaValue::String(oid)) = fields.first() else {
        anyhow::bail!("git commit did not return an object id")
    };
    Ok(oid.clone())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn builtin_git_tool_serializes_filesystem_calls_and_cancels_queued_mutation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("filesystem_tools")] filesystem_tools: &PrecompiledComponent,
    #[tagged_as("git_tool")] git_tool: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let harness = start_git_tool_harness(
        last_unique_id,
        deps,
        caller,
        filesystem_tools,
        git_tool,
        "git-tool-concurrency",
    )
    .await?;
    initialize_git_harness(&harness, "main").await?;
    let cwd = string_list(&["/workspace/repo"]);
    let cancelled_key = IdempotencyKey::fresh();

    {
        let mut active_git_gate = harness
            .executor
            .gate_next_entity_body_start(&harness.worker_id);
        let active_git = invoke_git_tool_success(
            &harness.executor,
            &harness.worker_id,
            harness.fingerprint,
            harness.principal.clone(),
            &harness.git_definition,
            "status",
            BTreeMap::from([
                ("working-directory", cwd.clone()),
                ("paths", string_list(&[])),
                ("short", SchemaValue::Bool(true)),
                ("porcelain", SchemaValue::String("v1".to_string())),
                ("null", SchemaValue::Bool(false)),
            ]),
        );
        tokio::pin!(active_git);
        tokio::select! {
            () = active_git_gate.entered() => {}
            result = &mut active_git => anyhow::bail!("gated git status settled before entering its body: {result:?}"),
        }

        let filesystem_write = invoke_filesystem_tool_success(
            &harness.executor,
            &harness.worker_id,
            harness.fingerprint,
            harness.principal.clone(),
            &harness.filesystem_definitions,
            "write-file",
            filesystem_tool_input(vec![
                (
                    "path",
                    SchemaType::string(),
                    SchemaValue::String("workspace/repo/serialized.txt".to_string()),
                ),
                (
                    "content",
                    SchemaType::string(),
                    SchemaValue::String("serialized\n".to_string()),
                ),
                (
                    "create-parent-directories",
                    SchemaType::bool(),
                    SchemaValue::Bool(false),
                ),
            ]),
        );
        tokio::pin!(filesystem_write);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), &mut filesystem_write)
                .await
                .is_err(),
            "filesystem tool must wait behind a filesystem-capable Git body on the same owner"
        );

        let cancelled_config = invoke_git_tool_success_with_key(
            &harness.executor,
            &harness.worker_id,
            harness.fingerprint,
            cancelled_key.clone(),
            harness.principal.clone(),
            &harness.git_definition,
            "config",
            BTreeMap::from([
                ("working-directory", cwd.clone()),
                ("key", SchemaValue::String("user.name".to_string())),
                ("value", optional_string(Some("Cancelled Name"))),
                ("local", SchemaValue::Bool(true)),
            ]),
        );
        tokio::pin!(cancelled_config);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), &mut cancelled_config)
                .await
                .is_err(),
            "queued Git config must remain pending behind the active Git body"
        );
        assert!(
            harness
                .executor
                .cancel_invocation(&harness.worker_id, &cancelled_key)
                .await?,
            "queued Git invocation must be cancellable"
        );
        active_git_gate.release();
        active_git.await?;
        filesystem_write.await?;
    }

    let oplog = harness
        .executor
        .get_oplog(&harness.worker_id, OplogIndex::INITIAL)
        .await?;
    assert!(oplog.iter().any(|entry| {
        matches!(
            &entry.entry,
            PublicOplogEntry::CancelPendingInvocation(params)
                if params.idempotency_key == cancelled_key
        )
    }));
    assert_eq!(
        harness
            .executor
            .get_file_contents(&harness.worker_id, "/workspace/repo/serialized.txt")
            .await?
            .as_ref(),
        b"serialized\n"
    );
    let config = invoke_git_tool_success(
        &harness.executor,
        &harness.worker_id,
        harness.fingerprint,
        harness.principal,
        &harness.git_definition,
        "config",
        BTreeMap::from([
            ("working-directory", cwd),
            ("key", SchemaValue::String("user.name".to_string())),
            ("value", optional_string(None)),
            ("local", SchemaValue::Bool(true)),
        ]),
    )
    .await?;
    let Some(SchemaValue::Record { fields }) = config else {
        anyhow::bail!("git config after cancellation returned an unexpected result")
    };
    assert_eq!(fields[1], optional_string(Some("Recovery Test")));
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn builtin_git_tool_commit_recovers_after_body_before_terminal_and_owners_are_isolated(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("filesystem_tools")] filesystem_tools: &PrecompiledComponent,
    #[tagged_as("git_tool")] git_tool: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let harness = start_git_tool_harness(
        last_unique_id,
        deps,
        caller,
        filesystem_tools,
        git_tool,
        "git-tool-crash",
    )
    .await?;
    initialize_git_harness(&harness, "main").await?;
    let cwd = string_list(&["/workspace/repo"]);

    let baseline_oid = git_commit_oid(
        invoke_git_tool_success(
            &harness.executor,
            &harness.worker_id,
            harness.fingerprint,
            harness.principal.clone(),
            &harness.git_definition,
            "commit",
            BTreeMap::from([
                ("working-directory", cwd.clone()),
                ("message", string_list(&["baseline"])),
                ("author", optional_string(None)),
                ("allow-empty", SchemaValue::Bool(true)),
            ]),
        )
        .await?,
    )?;
    invoke_filesystem_tool_success(
        &harness.executor,
        &harness.worker_id,
        harness.fingerprint,
        harness.principal.clone(),
        &harness.filesystem_definitions,
        "write-file",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String("workspace/repo/recovered.txt".to_string()),
            ),
            (
                "content",
                SchemaType::string(),
                SchemaValue::String("recovered\n".to_string()),
            ),
            (
                "create-parent-directories",
                SchemaType::bool(),
                SchemaValue::Bool(false),
            ),
        ]),
    )
    .await?;
    invoke_git_tool_success(
        &harness.executor,
        &harness.worker_id,
        harness.fingerprint,
        harness.principal.clone(),
        &harness.git_definition,
        "add",
        BTreeMap::from([
            ("working-directory", cwd.clone()),
            ("paths", string_list(&["recovered.txt"])),
            ("all", SchemaValue::Bool(false)),
            ("update", SchemaValue::Bool(false)),
        ]),
    )
    .await?;

    let mut completion = harness
        .executor
        .gate_next_live_entity_body_completion(&harness.worker_id, "git");
    let key = IdempotencyKey::fresh();
    {
        let mut commit = Box::pin(invoke_git_tool_success_with_key(
            &harness.executor,
            &harness.worker_id,
            harness.fingerprint,
            key.clone(),
            harness.principal.clone(),
            &harness.git_definition,
            "commit",
            BTreeMap::from([
                ("working-directory", cwd.clone()),
                ("message", string_list(&["survives terminal crash"])),
                ("author", optional_string(None)),
                ("allow-empty", SchemaValue::Bool(false)),
            ]),
        ));
        tokio::select! {
            () = completion.entered() => {}
            result = &mut commit => anyhow::bail!("git commit settled before its completion checkpoint: {result:?}"),
        }
        harness.executor.commit_oplog(&harness.worker_id).await?;
    }
    let oplog = harness
        .executor
        .get_oplog(&harness.worker_id, OplogIndex::INITIAL)
        .await?;
    let entity_start = oplog
        .iter()
        .rev()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == "golem::entity::invoke" => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("Git commit has an entity Start");
    assert!(oplog.iter().all(|entry| {
        !matches!(&entry.entry, PublicOplogEntry::End(params) if params.start_index == entity_start)
            && !matches!(&entry.entry, PublicOplogEntry::Cancelled(params) if params.start_index == entity_start)
    }));

    harness
        .executor
        .shutdown_and_wait_for_invocation_loops()
        .await?;
    drop(completion);
    let GitToolHarness {
        context,
        environment_state,
        executor,
        caller_component_id,
        worker_id,
        fingerprint,
        principal,
        git_definition,
        filesystem_definitions,
    } = harness;
    drop(executor);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state),
            ..Default::default()
        },
    )
    .await?;
    let recovered_oid = git_commit_oid(
        tokio::time::timeout(
            std::time::Duration::from_secs(60),
            invoke_git_tool_success_with_key(
                &executor,
                &worker_id,
                fingerprint,
                key,
                principal.clone(),
                &git_definition,
                "commit",
                BTreeMap::from([
                    ("working-directory", cwd.clone()),
                    ("message", string_list(&["survives terminal crash"])),
                    ("author", optional_string(None)),
                    ("allow-empty", SchemaValue::Bool(false)),
                ]),
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Git commit recovery timed out"))??,
    )?;
    let recovered_ref = String::from_utf8(
        executor
            .get_file_contents(&worker_id, "/workspace/repo/.git/refs/heads/main")
            .await?
            .to_vec(),
    )?;
    assert_eq!(recovered_oid, recovered_ref.trim());
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/workspace/repo/recovered.txt")
            .await?
            .as_ref(),
        b"recovered\n"
    );
    let log = invoke_git_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &git_definition,
        "log",
        BTreeMap::from([
            ("working-directory", cwd.clone()),
            ("ref", optional_string(None)),
            ("max-count", SchemaValue::F64(2.0)),
            ("oneline", SchemaValue::Bool(true)),
        ]),
    )
    .await?;
    let Some(SchemaValue::Record { fields: log_fields }) = log else {
        anyhow::bail!("git log returned an unexpected result")
    };
    let SchemaValue::List { elements } = &log_fields[0] else {
        anyhow::bail!("git log did not return commits")
    };
    assert_eq!(elements.len(), 2);
    let commit_oids = elements
        .iter()
        .map(|entry| match entry {
            SchemaValue::Record { fields } => match &fields[0] {
                SchemaValue::String(oid) => Ok(oid.as_str()),
                _ => anyhow::bail!("log entry did not contain an object id"),
            },
            _ => anyhow::bail!("log entry was not a record"),
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    assert_eq!(commit_oids, [recovered_oid.as_str(), baseline_oid.as_str()]);

    let isolated_id = agent_id!("ToolStreamingCaller", "git-tool-other-owner");
    let isolated_worker = executor
        .start_agent(&caller_component_id, isolated_id)
        .await?;
    let isolated_fingerprint = executor
        .get_worker_metadata(&isolated_worker)
        .await?
        .fingerprint;
    invoke_git_tool_success(
        &executor,
        &isolated_worker,
        isolated_fingerprint,
        principal.clone(),
        &git_definition,
        "init",
        BTreeMap::from([
            ("working-directory", string_list(&[])),
            ("directory", optional_string(Some("workspace/repo"))),
            ("initial-branch", optional_string(Some("isolated"))),
        ]),
    )
    .await?;
    invoke_filesystem_tool_success(
        &executor,
        &isolated_worker,
        isolated_fingerprint,
        principal,
        &filesystem_definitions,
        "write-file",
        filesystem_tool_input(vec![
            (
                "path",
                SchemaType::string(),
                SchemaValue::String("workspace/repo/isolated.txt".to_string()),
            ),
            (
                "content",
                SchemaType::string(),
                SchemaValue::String("isolated\n".to_string()),
            ),
            (
                "create-parent-directories",
                SchemaType::bool(),
                SchemaValue::Bool(false),
            ),
        ]),
    )
    .await?;
    assert!(
        executor
            .get_file_contents(&worker_id, "/workspace/repo/isolated.txt")
            .await
            .is_err()
    );
    assert!(
        executor
            .get_file_contents(&isolated_worker, "/workspace/repo/recovered.txt")
            .await
            .is_err()
    );
    Ok(())
}

#[test]
fn builtin_git_tool_runtime_provides_wasi_http(
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("git_tool")] git_tool: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let engine = Engine::new(&create_wasmtime_config_without_fs_cache())?;
    let component = Component::from_file(
        &engine,
        deps.component_directory
            .join(format!("{}.wasm", git_tool.wasm_name)),
    )?;
    let imports = component
        .component_type()
        .imports(&engine)
        .map(|(name, _)| name.to_string())
        .collect::<Vec<_>>();

    assert!(
        imports.iter().any(|name| name.starts_with("wasi:http/")),
        "Git tool must import wasi:http for all future network transport; imports: {imports:?}"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn git_network_probe_uses_isomorphic_git_web_transport(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("git_network_probe")] probe: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    fn pkt_line(payload: &str) -> String {
        format!("{:04x}{payload}", payload.len() + 4)
    }

    let oid = "0123456789abcdef0123456789abcdef01234567";
    let advertisement = format!(
        "{}0000{}0000",
        pkt_line("# service=git-upload-pack\n"),
        pkt_line(&format!(
            "{oid} refs/heads/main\0symref=HEAD:refs/heads/main agent=golem-test\n"
        ))
    );
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    let server = tokio::spawn(async move {
        let route = Router::new().route(
            "/repo.git/info/refs",
            get(move |request: Request| {
                let advertisement = advertisement.clone();
                async move {
                    assert_eq!(request.uri().query(), Some("service=git-upload-pack"));
                    Response::builder()
                        .header(
                            "content-type",
                            "application/x-git-upload-pack-advertisement",
                        )
                        .body(Body::from(advertisement))
                        .unwrap()
                }
            }),
        );
        axum::serve(listener, route).await.unwrap();
    });

    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, TestExecutorOverrides::default()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, probe)
        .store()
        .await?;
    let agent_id = agent_id!("GitNetworkProbe", "web-transport");
    let worker_id = executor
        .start_agent(&component.id, agent_id.clone())
        .await?;
    let refs: Vec<String> = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "discover",
            data_value!(format!("http://localhost:{port}/repo.git")),
        )
        .await?
        .into_typed()?;
    assert_eq!(refs, vec![format!("refs/heads/main {oid}")]);

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    for function_name in ["http::client::send", "http::types::response::consume-body"] {
        let starts = oplog
            .iter()
            .filter(|entry| {
                matches!(&entry.entry, PublicOplogEntry::Start(params) if params.function_name == function_name)
            })
            .collect::<Vec<_>>();
        assert_eq!(starts.len(), 1, "expected one durable {function_name} call");
        assert!(oplog.iter().any(|entry| {
            matches!(&entry.entry, PublicOplogEntry::End(params) if params.start_index == starts[0].oplog_index)
        }));
    }
    assert!(oplog.iter().all(|entry| {
        !matches!(&entry.entry, PublicOplogEntry::Start(params) if params.function_name.starts_with("sockets::"))
    }));
    server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn filesystem_tools_work_through_guest_invocation(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("filesystem_tools")] filesystem_tools: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;

    exercise_guest_invoked_filesystem_tools(
        &executor,
        deps,
        &context,
        &environment_state,
        &caller_component,
        filesystem_tools,
    )
    .await?;
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn clock_races_complete_through_tool_entity_without_suspending_owner(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent = agent_id!("ToolStreamingCaller", "clock-race-tool-owner");
    let worker = executor
        .start_agent(&caller_component.id, agent.clone())
        .await?;
    let boundary = executor.oplog_max_index(&worker).await?;
    let result = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent,
            "clock_races_through_tool_entity",
            data_value!(),
        )
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(result, ["2", "2", "timer", "flag", "settled"]);

    let interval = executor.get_oplog(&worker, boundary).await?;
    assert!(
        interval
            .iter()
            .filter(|entry| entry.oplog_index > boundary)
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Suspend(_))),
        "tool/entity clock-race interval unexpectedly contained a Suspend entry"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn long_clock_through_tool_entity_suspends_and_reconstructs_owner(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.suspend.wait_suspend_check_interval = Duration::from_millis(100);
            })),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent = agent_id!("ToolStreamingCaller", "long-clock-tool-owner");
    let worker = executor
        .start_agent(&caller_component.id, agent.clone())
        .await?;
    let loads = executor.instance_load_count(&worker);
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent,
        "long_clock_through_tool_entity",
        data_value!(),
    );
    tokio::pin!(invocation);
    tokio::select! {
        result = &mut invocation => panic!("tool timer finished before suspension: {result:?}"),
        unloaded = tokio::time::timeout(Duration::from_secs(15), async {
            executor.wait_for_status(&worker, AgentStatus::Suspended, Duration::from_secs(10)).await?;
            while executor.worker_is_loaded(&OwnedAgentId::new(context.default_environment_id, &worker)).await {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok::<(), anyhow::Error>(())
        }) => { unloaded??; }
    }
    let result = tokio::time::timeout(Duration::from_secs(45), invocation)
        .await??
        .into_typed::<Vec<String>>()?;
    assert_eq!(result, ["20", "settled"]);
    assert!(executor.instance_load_count(&worker) > loads);
    let follow_up = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent,
            "clock_races_through_tool_entity",
            data_value!(),
        )
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(follow_up, ["2", "2", "timer", "flag", "settled"]);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn aggregate_clocks_through_tool_entities_suspend_after_short_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            configure: Some(Arc::new(|config| {
                config.suspend.wait_suspend_check_interval = Duration::from_millis(100);
            })),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );
    let agent = agent_id!("ToolStreamingCaller", "aggregate-clock-tool-owner");
    let worker = executor
        .start_agent(&caller_component.id, agent.clone())
        .await?;
    let loads = executor.instance_load_count(&worker);
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent,
        "aggregate_clocks_through_tool_entities",
        data_value!(),
    );
    tokio::pin!(invocation);
    tokio::select! {
        result = &mut invocation => panic!("aggregate finished before suspension: {result:?}"),
        unloaded = tokio::time::timeout(Duration::from_secs(15), async {
            executor.wait_for_status(&worker, AgentStatus::Suspended, Duration::from_secs(10)).await?;
            while executor.worker_is_loaded(&OwnedAgentId::new(context.default_environment_id, &worker)).await {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok::<(), anyhow::Error>(())
        }) => { unloaded??; }
    }
    let parked = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    let entity_starts: Vec<_> = parked
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(params) if params.function_name == "golem::entity::invoke" => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect();
    assert_eq!(entity_starts.len(), 2);
    assert_eq!(
        parked
            .iter()
            .filter(|entry| matches!(&entry.entry,
                PublicOplogEntry::End(params) if entity_starts.contains(&params.start_index)
            ))
            .count(),
        1,
        "the short producer must finish before the remaining aggregate wait unloads"
    );
    let result = tokio::time::timeout(Duration::from_secs(45), invocation)
        .await??
        .into_typed::<Vec<String>>()?;
    assert_eq!(result, ["20", "2", "settled"]);
    assert!(executor.instance_load_count(&worker) > loads);
    let follow_up = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent,
            "clock_races_through_tool_entity",
            data_value!(),
        )
        .await?
        .into_typed::<Vec<String>>()?;
    assert_eq!(follow_up, ["2", "2", "timer", "flag", "settled"]);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn builtin_web_fetch_has_expected_behavior_and_replays_after_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("web_fetch")] web_fetch: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, web_fetch)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", web_fetch.wasm_name)),
        false,
        true,
    )
    .await?;
    let definition = metadata
        .tools
        .iter()
        .find(|definition| definition.name() == Some("web-fetch"))
        .cloned()
        .expect("web-fetch metadata is present");
    assert!(!definition.requires_filesystem);
    for command in &definition.commands.nodes {
        if let Some(body) = &command.body {
            assert!(
                body.annotations
                    .as_ref()
                    .expect("web-fetch command body has annotations")
                    .open_world
            );
        }
    }
    let command_index = definition
        .command_index_by_path(&[])
        .expect("web-fetch root command is present");
    let command_body = definition.commands.nodes[command_index]
        .body
        .as_ref()
        .expect("web-fetch root command has a body");
    let conversion_option = command_body
        .options
        .iter()
        .find(|option| option.long == "convert-html-to-text")
        .expect("web-fetch exports the convert-html-to-text option");
    assert!(!conversion_option.required);
    assert!(matches!(
        conversion_option.shape,
        OptionShape::Scalar(SchemaType::Bool { .. })
    ));
    let input_schema = definition.canonical_input_record_schema(command_index)?;
    let SchemaType::Record { fields, .. } = &input_schema.root else {
        anyhow::bail!("web-fetch canonical input is not a record")
    };
    let conversion_field = fields
        .iter()
        .find(|field| field.name == "convert-html-to-text")
        .expect("web-fetch canonical input contains convert-html-to-text");
    assert_eq!(
        conversion_field.body,
        SchemaType::option(SchemaType::bool())
    );
    let deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem:web-fetch",
        "ToolStreamingCaller",
        metadata.tools,
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "web-fetch");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let fingerprint = executor.get_worker_metadata(&worker_id).await?.fingerprint;
    let principal = Principal::GolemUser(GolemUserPrincipal {
        account_id: context.account_id,
    });
    let WebFetchHttpServers {
        source_port,
        target_port,
        source_server,
        source_requests,
        target_requests,
        target_server,
        bounded_body_gate,
        timeout_started,
        timeout_cancelled,
        interrupted_started,
        interrupted_requests,
    } = start_web_fetch_http_servers().await;
    let source_url = |path: &str| format!("http://127.0.0.1:{source_port}{path}");

    let html = expect_web_fetch_success(
        invoke_web_fetch(
            &executor,
            &worker_id,
            fingerprint,
            principal.clone(),
            &definition,
            IdempotencyKey::fresh(),
            web_fetch_input(source_url("/html"), None, None, None, None),
        )
        .await?,
    )?;
    assert_eq!(html.0, source_url("/html"));
    assert_eq!(html.1, 200);
    assert_eq!(html.2.as_deref(), Some("text/html; charset=utf-8"));
    assert_eq!(
        html.3,
        "<html><body><h1>Fetch title</h1><script>hidden()</script><p>Readable body.</p></body></html>"
    );
    assert!(!html.4);

    let converted_html = expect_web_fetch_success(
        invoke_web_fetch(
            &executor,
            &worker_id,
            fingerprint,
            principal.clone(),
            &definition,
            IdempotencyKey::fresh(),
            web_fetch_input(source_url("/html"), None, None, None, Some(true)),
        )
        .await?,
    )?;
    assert!(
        converted_html.3.contains("Fetch title"),
        "{}",
        converted_html.3
    );
    assert!(
        converted_html.3.contains("Readable body."),
        "{}",
        converted_html.3
    );
    assert!(!converted_html.3.contains("hidden"), "{}", converted_html.3);
    assert!(!converted_html.3.contains("<h1>"), "{}", converted_html.3);
    assert!(!converted_html.3.contains("<p>"), "{}", converted_html.3);
    assert!(!converted_html.4);

    let missing = expect_web_fetch_success(
        invoke_web_fetch(
            &executor,
            &worker_id,
            fingerprint,
            principal.clone(),
            &definition,
            IdempotencyKey::fresh(),
            web_fetch_input(source_url("/missing"), None, None, None, None),
        )
        .await?,
    )?;
    assert_eq!(missing.1, 404);
    assert_eq!(missing.3, "missing body");

    let relative = expect_web_fetch_success(
        invoke_web_fetch(
            &executor,
            &worker_id,
            fingerprint,
            principal.clone(),
            &definition,
            IdempotencyKey::fresh(),
            web_fetch_input(source_url("/relative"), None, None, None, None),
        )
        .await?,
    )?;
    assert_eq!(relative.0, source_url("/relative-final"));
    assert_eq!(relative.3, "relative target");

    let cross_host = expect_web_fetch_success(
        invoke_web_fetch(
            &executor,
            &worker_id,
            fingerprint,
            principal.clone(),
            &definition,
            IdempotencyKey::fresh(),
            web_fetch_input(source_url("/cross-host"), None, None, None, None),
        )
        .await?,
    )?;
    assert_eq!(
        cross_host.0,
        format!("http://127.0.0.1:{target_port}/cross-host-final")
    );
    assert_eq!(cross_host.3, "cross-host target");
    assert_eq!(target_requests.load(Ordering::SeqCst), 1);

    let streaming = expect_web_fetch_success(
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            invoke_web_fetch(
                &executor,
                &worker_id,
                fingerprint,
                principal.clone(),
                &definition,
                IdempotencyKey::fresh(),
                web_fetch_input(source_url("/streaming"), None, Some(5), None, None),
            ),
        )
        .await
        .expect("bounded fetch must return before the server finishes the body")?,
    )?;
    bounded_body_gate.notify_waiters();
    assert_eq!(streaming.3, "abcde");
    assert!(streaming.4);

    for (path, expected) in [
        ("/invalid-utf8", "invalid-text-encoding"),
        ("/binary", "unsupported-content-type"),
        ("/cycle", "unsafe-redirect"),
    ] {
        let result = invoke_web_fetch(
            &executor,
            &worker_id,
            fingerprint,
            principal.clone(),
            &definition,
            IdempotencyKey::fresh(),
            web_fetch_input(source_url(path), None, None, None, None),
        )
        .await?;
        assert_web_fetch_error(result, expected)?;
    }

    let no_redirects = invoke_web_fetch(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definition,
        IdempotencyKey::fresh(),
        web_fetch_input(source_url("/relative"), None, None, Some(0), None),
    )
    .await?;
    assert_web_fetch_error(no_redirects, "redirect-limit")?;

    let invalid_limit = invoke_web_fetch(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definition,
        IdempotencyKey::fresh(),
        web_fetch_input(
            source_url("/html"),
            None,
            Some(5 * 1024 * 1024 + 1),
            None,
            None,
        ),
    )
    .await?;
    assert_web_fetch_error(invalid_limit, "invalid-safety-limit")?;

    let timed_out = {
        let timed_out = invoke_web_fetch(
            &executor,
            &worker_id,
            fingerprint,
            principal.clone(),
            &definition,
            IdempotencyKey::fresh(),
            web_fetch_input(source_url("/slow-stream"), Some(500), None, None, None),
        );
        let (started, result) = tokio::join!(
            tokio::time::timeout(std::time::Duration::from_secs(2), timeout_started),
            tokio::time::timeout(std::time::Duration::from_secs(2), timed_out),
        );
        started
            .expect("slow response body did not start")
            .map_err(|_| anyhow::anyhow!("slow response producer stopped before starting"))?;
        result.expect("web-fetch total deadline did not stop an active response stream")?
    };
    assert_web_fetch_error(timed_out, "timeout")?;
    tokio::time::timeout(std::time::Duration::from_secs(2), timeout_cancelled)
        .await
        .expect("timed-out web-fetch did not cancel the response body")
        .map_err(|_| anyhow::anyhow!("slow response producer stopped without cancellation"))?;

    let header_timeout = invoke_web_fetch(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definition,
        IdempotencyKey::fresh(),
        web_fetch_input(source_url("/slow-headers"), Some(300), None, None, None),
    )
    .await?;
    assert_web_fetch_error(header_timeout, "timeout")?;

    let replay_key = IdempotencyKey::fresh();
    let replay_input = web_fetch_input(source_url("/cross-host"), None, None, None, None);
    let first = expect_web_fetch_success(
        invoke_web_fetch(
            &executor,
            &worker_id,
            fingerprint,
            principal.clone(),
            &definition,
            replay_key.clone(),
            replay_input.clone(),
        )
        .await?,
    )?;
    let target_requests_before_restart = target_requests.load(Ordering::SeqCst);
    assert_eq!(target_requests_before_restart, 2);

    let interrupted_key = IdempotencyKey::fresh();
    let interrupted_input =
        web_fetch_input(source_url("/interrupted"), Some(10_000), None, None, None);
    let interrupted = invoke_web_fetch(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definition,
        interrupted_key.clone(),
        interrupted_input.clone(),
    );
    tokio::pin!(interrupted);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::select! {
            started = interrupted_started => started
                .map_err(|_| anyhow::anyhow!("interrupted response producer stopped before starting")),
            result = &mut interrupted => anyhow::bail!(
                "web-fetch completed before the worker interruption: {result:?}"
            ),
        }
    })
    .await
    .expect("interrupted response body did not start")?;
    assert_eq!(interrupted_requests.load(Ordering::SeqCst), 1);
    let source_requests_before_restart = source_requests.load(Ordering::SeqCst);

    executor.simulated_crash(&worker_id).await?;
    let interrupted_result = expect_web_fetch_success(
        tokio::time::timeout(std::time::Duration::from_secs(10), &mut interrupted)
            .await
            .expect("interrupted web-fetch did not recover after the simulated crash")?,
    )?;
    assert_eq!(interrupted_result.3, "retried after interruption");
    assert_eq!(
        interrupted_requests.load(Ordering::SeqCst),
        2,
        "an interrupted incomplete fetch must retry its GET during recovery"
    );
    assert_eq!(
        source_requests.load(Ordering::SeqCst),
        source_requests_before_restart + 1,
        "component reconstruction must replay completed source requests without repeating HTTP"
    );
    assert_eq!(
        target_requests.load(Ordering::SeqCst),
        target_requests_before_restart,
        "component reconstruction must replay completed redirected requests without repeating HTTP"
    );
    let replayed_interrupted = expect_web_fetch_success(
        invoke_web_fetch(
            &executor,
            &worker_id,
            fingerprint,
            principal.clone(),
            &definition,
            interrupted_key,
            interrupted_input,
        )
        .await?,
    )?;
    assert_eq!(replayed_interrupted, interrupted_result);

    let replayed = expect_web_fetch_success(
        invoke_web_fetch(
            &executor,
            &worker_id,
            fingerprint,
            principal,
            &definition,
            replay_key,
            replay_input,
        )
        .await?,
    )?;
    assert_eq!(replayed, first);
    assert_eq!(
        target_requests.load(Ordering::SeqCst),
        target_requests_before_restart,
        "completed fetch must replay after worker restart without repeating HTTP"
    );
    assert_eq!(
        interrupted_requests.load(Ordering::SeqCst),
        2,
        "retrying a recovered incomplete fetch must not repeat its GET"
    );
    assert_eq!(
        source_requests.load(Ordering::SeqCst),
        source_requests_before_restart + 1,
        "retrying recovered and completed fetches must not repeat HTTP"
    );

    executor.shutdown_and_wait_for_invocation_loops().await?;
    source_server.abort();
    target_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn native_external_tool_scalar_admission(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let (native_order_port, _native_order_server, native_order_requests) =
        start_native_order_http_server().await;
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        "ToolStreamingCaller",
        metadata.tools,
    );
    for bindings in deployment.tool_bindings.values_mut() {
        bindings
            .get_mut(&ToolName::try_from("streaming").unwrap())
            .unwrap()
            .filesystem_access = ToolFilesystemAccess::Allowed;
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment.clone()),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "native-external-tool");
    let env = HashMap::from([(
        "NATIVE_ORDER_HTTP_PORT".to_string(),
        native_order_port.to_string(),
    )]);
    let worker_id = executor
        .start_agent_with(&caller_component.id, agent_id.clone(), env, Vec::new())
        .await?;
    executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "record_native_order",
            data_value!("A"),
        )
        .await?;
    let fingerprint = executor.get_worker_metadata(&worker_id).await?.fingerprint;
    let principal = Principal::GolemUser(GolemUserPrincipal {
        account_id: context.account_id,
    });
    let key = IdempotencyKey::fresh();
    let input = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(vec![
            golem_common::schema::NamedFieldType {
                name: "value".to_string(),
                body: SchemaType::string(),
                metadata: Default::default(),
            },
        ])),
        SchemaValue::Record {
            fields: vec![SchemaValue::String("native-order".to_string())],
        },
    );

    let missing_id = golem_common::model::AgentId {
        component_id: caller_component.id,
        agent_id: agent_id!("ToolStreamingCaller", "missing-native-owner").to_string(),
    };
    executor
        .invoke_external_tool(
            &missing_id,
            golem_common::model::AgentFingerprint(uuid::Uuid::new_v4()),
            IdempotencyKey::fresh(),
            ToolName::try_from("streaming").unwrap(),
            vec!["no-stream".to_string()],
            input.clone(),
            InvocationContextStack::fresh(),
            principal.clone(),
            None,
        )
        .await
        .expect_err("native admission must not create a missing owner");
    assert!(
        executor.get_worker_metadata(&missing_id).await.is_err(),
        "failed native admission created a durable owner"
    );

    let wrong_fingerprint = golem_common::model::AgentFingerprint(uuid::Uuid::new_v4());
    let error = executor
        .invoke_external_tool(
            &worker_id,
            wrong_fingerprint,
            IdempotencyKey::fresh(),
            ToolName::try_from("streaming").unwrap(),
            vec!["no-stream".to_string()],
            input.clone(),
            InvocationContextStack::fresh(),
            principal.clone(),
            None,
        )
        .await
        .expect_err("wrong owner fingerprint must be rejected");
    assert!(error.to_string().contains("fingerprint does not match"));

    let output = executor
        .invoke_external_tool(
            &worker_id,
            fingerprint,
            key.clone(),
            ToolName::try_from("streaming").unwrap(),
            vec!["no-stream".to_string()],
            input.clone(),
            InvocationContextStack::fresh(),
            principal.clone(),
            None,
        )
        .await?;
    let AgentInvocationResult::ExternalTool { result: Ok(result) } = output.result else {
        anyhow::bail!("expected successful scalar external tool result, got {output:?}");
    };
    let Some(SchemaValue::String(scalar)) = result.result.map(|value| value.into_parts().1) else {
        anyhow::bail!("expected string scalar tool result");
    };
    assert_eq!(scalar, "no-stream:native-order");
    assert_eq!(native_order_requests.load(Ordering::SeqCst), 1);

    executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "record_native_order",
            data_value!("B"),
        )
        .await?;
    let order: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "read_owner_file",
            data_value!("/native-tool-order.log"),
        )
        .await?
        .into_typed()?;
    assert_eq!(order, "ATB");

    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        None,
    );
    let cached = executor
        .invoke_external_tool(
            &worker_id,
            fingerprint,
            key.clone(),
            ToolName::try_from("streaming").unwrap(),
            vec!["no-stream".to_string()],
            input.clone(),
            InvocationContextStack::fresh(),
            principal.clone(),
            None,
        )
        .await?;
    let AgentInvocationResult::ExternalTool { result: Ok(result) } = cached.result else {
        anyhow::bail!("expected cached scalar external tool result, got {cached:?}");
    };
    let Some(SchemaValue::String(scalar)) = result.result.map(|value| value.into_parts().1) else {
        anyhow::bail!("expected cached string scalar tool result");
    };
    assert_eq!(scalar, "no-stream:native-order");
    assert_eq!(
        native_order_requests.load(Ordering::SeqCst),
        1,
        "completed replay must not repeat the external HTTP effect"
    );

    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;

    // This is intentionally the first ingress after restart: native admission must cold-load an
    // existing owner, and the completed tool effect must replay rather than execute a second time.
    let restarted = executor
        .invoke_external_tool(
            &worker_id,
            fingerprint,
            key,
            ToolName::try_from("streaming").unwrap(),
            vec!["no-stream".to_string()],
            input.clone(),
            InvocationContextStack::fresh(),
            principal,
            None,
        )
        .await?;
    let AgentInvocationResult::ExternalTool { result: Ok(result) } = restarted.result else {
        anyhow::bail!("expected restarted scalar external tool result, got {restarted:?}");
    };
    let Some(SchemaValue::String(scalar)) = result.result.map(|value| value.into_parts().1) else {
        anyhow::bail!("expected restarted string scalar tool result");
    };
    assert_eq!(scalar, "no-stream:native-order");
    assert_eq!(
        native_order_requests.load(Ordering::SeqCst),
        1,
        "completed replay after executor restart must not repeat the external HTTP effect"
    );

    let reconstructed_order: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "read_owner_file",
            data_value!("/native-tool-order.log"),
        )
        .await?
        .into_typed()?;
    assert_eq!(reconstructed_order, "ATB");
    assert_eq!(
        reconstructed_order.matches('T').count(),
        1,
        "completed replay must not duplicate the tool side effect"
    );
    assert_eq!(
        native_order_requests.load(Ordering::SeqCst),
        1,
        "owner reconstruction with the deployment removed must replay the recorded HTTP result instead of running the effect live"
    );

    // Warm the read-only cache: once populated, a cache hit adds no invocation entries.
    let cache_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let before = executor.oplog_max_index(&worker_id).await?;
        let _: String = executor
            .invoke_and_await_agent(
                &caller_component,
                &agent_id,
                "read_owner_file",
                data_value!("/native-tool-order.log"),
            )
            .await?
            .into_typed()?;
        if executor.oplog_max_index(&worker_id).await? == before {
            break;
        }
        anyhow::ensure!(
            std::time::Instant::now() < cache_deadline,
            "read_owner_file did not become cacheable"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let error_input = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(vec![
            golem_common::schema::NamedFieldType {
                name: "value".to_string(),
                body: SchemaType::string(),
                metadata: Default::default(),
            },
        ])),
        SchemaValue::Record {
            fields: vec![SchemaValue::String("native-error".to_string())],
        },
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let failed_mutation = executor
        .invoke_external_tool(
            &worker_id,
            fingerprint,
            IdempotencyKey::fresh(),
            ToolName::try_from("streaming").unwrap(),
            vec!["no-stream".to_string()],
            error_input,
            InvocationContextStack::fresh(),
            Principal::GolemUser(GolemUserPrincipal {
                account_id: context.account_id,
            }),
            None,
        )
        .await?;
    assert!(matches!(
        failed_mutation.result,
        AgentInvocationResult::ExternalTool { result: Err(_) }
    ));

    let before_read = executor.oplog_max_index(&worker_id).await?;
    let final_order: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "read_owner_file",
            data_value!("/native-tool-order.log"),
        )
        .await?
        .into_typed()?;
    assert_eq!(final_order, "ATBE");
    assert!(
        executor.oplog_max_index(&worker_id).await? > before_read,
        "a mutating tool error must invalidate the read-only cache"
    );

    let fresh = executor
        .invoke_external_tool(
            &worker_id,
            fingerprint,
            IdempotencyKey::fresh(),
            ToolName::try_from("streaming").unwrap(),
            vec!["no-stream".to_string()],
            input,
            InvocationContextStack::fresh(),
            Principal::GolemUser(GolemUserPrincipal {
                account_id: context.account_id,
            }),
            None,
        )
        .await?;
    assert!(matches!(
        fresh.result,
        AgentInvocationResult::ExternalTool { result: Ok(_) }
    ));
    assert_eq!(native_order_requests.load(Ordering::SeqCst), 2);

    Ok(())
}

struct BaselineOwnerComponentService {
    inner: Arc<dyn golem_worker_executor::services::component::ComponentService>,
    owner_loads: Arc<AtomicUsize>,
    http_port: u16,
}

#[async_trait::async_trait]
impl golem_worker_executor::services::component::ComponentService
    for BaselineOwnerComponentService
{
    async fn get(
        &self,
        engine: &wasmtime::Engine,
        id: golem_common::model::component::ComponentId,
        revision: ComponentRevision,
    ) -> Result<
        (
            wasmtime::component::Component,
            golem_service_base::model::component::Component,
        ),
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        let metadata = self.inner.get_metadata(id, Some(revision)).await?;
        if metadata.component_name.0 == "virtual-native-policy" {
            self.owner_loads.fetch_add(1, Ordering::SeqCst);
        }
        let (component, _) = self.inner.get(engine, id, revision).await?;
        Ok((component, self.get_metadata(id, Some(revision)).await?))
    }

    async fn get_metadata(
        &self,
        id: golem_common::model::component::ComponentId,
        revision: Option<ComponentRevision>,
    ) -> Result<
        golem_service_base::model::component::Component,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        use golem_common::model::component_metadata::ComponentProvisionConfig;
        let mut component = self.inner.get_metadata(id, revision).await?;
        if component.component_name.0 == "virtual-native-policy" {
            let permissions = component
                .metadata
                .agent_type_provision_configs()
                .get(&AgentTypeName("ToolStreamingCaller".to_string()))
                .unwrap()
                .initial_permissions
                .clone();
            component.metadata = component.metadata.with_component_config(
                Default::default(),
                ComponentProvisionConfig {
                    initial_permissions: permissions,
                    env: BTreeMap::from([
                        (
                            "NATIVE_ORDER_HTTP_PORT".to_string(),
                            self.http_port.to_string(),
                        ),
                        ("FORBID_AGENT_CONSTRUCTION".to_string(), "true".to_string()),
                    ]),
                    ..Default::default()
                },
            );
        }
        Ok(component)
    }

    async fn resolve_component(
        &self,
        reference: String,
        environment: golem_common::model::environment::EnvironmentId,
        application: golem_common::model::application::ApplicationId,
        account: AccountId,
    ) -> Result<
        Option<golem_common::model::component::ComponentId>,
        golem_service_base::error::worker_executor::WorkerExecutorError,
    > {
        self.inner
            .resolve_component(reference, environment, application, account)
            .await
    }

    async fn all_cached_metadata(&self) -> Vec<golem_service_base::model::component::Component> {
        self.inner.all_cached_metadata().await
    }

    async fn invalidate_all_metadata_for_environment(
        &self,
        environment: golem_common::model::environment::EnvironmentId,
    ) {
        self.inner
            .invalidate_all_metadata_for_environment(environment)
            .await
    }
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn ephemeral_external_tool_owner_converges_and_uses_component_baseline(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let (http_port, _server, effects) = start_native_order_http_server().await;
    let owner_loads = Arc::new(AtomicUsize::new(0));
    let loads = owner_loads.clone();
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.oplog.default_snapshotting =
                golem_worker_executor::services::golem_config::SnapshotPolicy::EveryNInvocation {
                    count: 1,
                };
        })),
        environment_state_service: Some(environment_state.clone()),
        wrap_component_service: Some(Arc::new(move |inner| {
            Arc::new(BaselineOwnerComponentService {
                inner,
                owner_loads: loads.clone(),
                http_port,
            })
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .name("virtual-native-policy")
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        "ToolStreamingCaller",
        metadata.tools,
    );
    let agent_owner = ToolBindingOwner::AgentType {
        agent_type_name: AgentTypeName("ToolStreamingCaller".to_string()),
    };
    let mut bindings = deployment
        .tool_bindings
        .remove(&agent_owner)
        .expect("caller bindings");
    let baseline_owner = ToolBindingOwner::ComponentBaseline {
        component_id: caller_component.id,
    };
    for binding in bindings.values_mut() {
        binding.owner = baseline_owner.clone();
    }
    bindings
        .get_mut(&ToolName::try_from("streaming").unwrap())
        .unwrap()
        .filesystem_access = ToolFilesystemAccess::Allowed;
    deployment.tool_bindings.insert(baseline_owner, bindings);
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let principal = Principal::GolemUser(GolemUserPrincipal {
        account_id: context.account_id,
    });
    let key = IdempotencyKey::fresh();
    let invocation_context = InvocationContextStack::fresh();
    let (first, second) = tokio::join!(
        executor.get_or_add_ephemeral_external_tool(
            caller_component.id,
            context.default_environment_id,
            &key,
            &invocation_context,
            principal.clone(),
        ),
        executor.get_or_add_ephemeral_external_tool(
            caller_component.id,
            context.default_environment_id,
            &key,
            &invocation_context,
            principal.clone(),
        )
    );
    let first = first?;
    let second = second?;
    assert!(Arc::ptr_eq(&first, &second));
    let owner_id = first.agent_id();
    assert_eq!(
        owner_id.agent_id,
        OwnerKind::external_tool_instance_name(&key)
    );
    let owner_metadata = first.get_latest_worker_metadata().await;
    assert_eq!(owner_metadata.owner_kind, OwnerKind::EphemeralExternalTool);
    assert_eq!(owner_metadata.agent_mode, AgentMode::Ephemeral);
    assert!(owner_metadata.last_known_status.total_linear_memory_size > 0);
    assert_eq!(
        owner_metadata.fingerprint,
        second.get_latest_worker_metadata().await.fingerprint
    );

    let input = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(vec![
            golem_common::schema::NamedFieldType {
                name: "value".to_string(),
                body: SchemaType::string(),
                metadata: Default::default(),
            },
        ])),
        SchemaValue::Record {
            fields: vec![SchemaValue::String("native-order".to_string())],
        },
    );
    let invoke = || {
        executor.invoke_external_tool(
            &owner_id,
            owner_metadata.fingerprint,
            key.clone(),
            ToolName::try_from("streaming").unwrap(),
            vec!["no-stream".to_string()],
            input.clone(),
            InvocationContextStack::fresh(),
            principal.clone(),
            None,
        )
    };
    let mut initializer = first
        .owner_execution()
        .test_gate_next_monotonic_clock_start();
    let check_initialization = async {
        initializer.entered().await;
        let oplog = executor.get_oplog(&owner_id, OplogIndex::INITIAL).await?;
        assert_eq!(
            oplog
                .iter()
                .filter(|entry| matches!(entry.entry, PublicOplogEntry::PendingAgentInvocation(_)))
                .count(),
            1
        );
        assert!(
            !oplog
                .iter()
                .any(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        );
        assert_eq!(effects.load(Ordering::SeqCst), 0);
        initializer.release();
        Ok::<_, golem_service_base::error::worker_executor::WorkerExecutorError>(())
    };
    let (output, retry, ()) = tokio::try_join!(invoke(), invoke(), check_initialization)?;
    assert_eq!(output, retry);
    let AgentInvocationResult::ExternalTool { result: Ok(result) } = output.result else {
        anyhow::bail!("expected successful baseline external tool result, got {output:?}");
    };
    let Some(SchemaValue::String(value)) = result.result.map(|value| value.into_parts().1) else {
        anyhow::bail!("expected baseline external tool string result");
    };
    assert_eq!(value, "no-stream:native-order");
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(owner_loads.load(Ordering::SeqCst), 1);
    assert!(
        first.memory_requirement().await?
            >= owner_metadata.last_known_status.total_linear_memory_size
    );
    assert_eq!(
        first.resident_component_charge_requirement().await,
        (
            caller_component.id,
            caller_component.revision,
            owner_metadata.last_known_status.component_size
        ),
    );
    let oplog = executor.get_oplog(&owner_id, OplogIndex::INITIAL).await?;
    assert!(oplog.iter().take_while(|entry| !matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        .any(|entry| matches!(&entry.entry, PublicOplogEntry::Start(params) if params.function_name == "monotonic_clock::now")),
        "the real component's core initializer must run before tool dispatch");
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Snapshot(_)))
    );
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
            .count(),
        1
    );

    let interrupted_key = IdempotencyKey::fresh();
    let interrupted = executor
        .get_or_add_ephemeral_external_tool(
            caller_component.id,
            context.default_environment_id,
            &interrupted_key,
            &InvocationContextStack::fresh(),
            principal.clone(),
        )
        .await?;
    let interrupted_id = interrupted.agent_id();
    let interrupted_fingerprint = interrupted.get_latest_worker_metadata().await.fingerprint;
    let mut interrupted_gate = executor.gate_next_entity_body_start(&interrupted_id);
    let interrupted_call = || {
        executor.invoke_external_tool(
            &interrupted_id,
            interrupted_fingerprint,
            interrupted_key.clone(),
            ToolName::try_from("streaming").unwrap(),
            vec!["no-stream".to_string()],
            input.clone(),
            InvocationContextStack::fresh(),
            principal.clone(),
            None,
        )
    };
    let crash = async {
        interrupted_gate.entered().await;
        executor.simulated_crash(&interrupted_id).await
    };
    let (interrupted_result, crash_result) =
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            tokio::join!(interrupted_call(), crash)
        })
        .await
        .expect("losing an external owner's Store must terminate its accepted invocation");
    crash_result?;
    interrupted_result.expect_err("ephemeral execution cannot restart after losing its Store");
    interrupted_call()
        .await
        .expect_err("same-key retry must not restart the lost external owner");
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(owner_loads.load(Ordering::SeqCst), 2);
    executor.delete_worker(&interrupted_id).await?;
    drop(interrupted_gate);
    drop(interrupted);

    let startup_key = IdempotencyKey::fresh();
    let startup = executor
        .get_or_add_ephemeral_external_tool(
            caller_component.id,
            context.default_environment_id,
            &startup_key,
            &InvocationContextStack::fresh(),
            principal.clone(),
        )
        .await?;
    let startup_id = startup.agent_id();
    let startup_fingerprint = startup.get_latest_worker_metadata().await.fingerprint;
    let mut initializer = startup
        .owner_execution()
        .test_gate_next_monotonic_clock_start();
    let startup_call = || {
        executor.invoke_external_tool(
            &startup_id,
            startup_fingerprint,
            startup_key.clone(),
            ToolName::try_from("streaming").unwrap(),
            vec!["no-stream".to_string()],
            input.clone(),
            InvocationContextStack::fresh(),
            principal.clone(),
            None,
        )
    };
    let interrupt_initialization = async {
        initializer.entered().await;
        initializer.abort_as_restart();
    };
    let (startup_result, ()) = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        tokio::join!(startup_call(), interrupt_initialization)
    })
    .await
    .expect("interrupted core initialization must terminate the accepted tool call");
    startup_result.expect_err("core initialization must not restart an external owner");
    startup_call().await.expect_err(
        "same-key retry must not recreate the component after interrupted initialization",
    );
    assert_eq!(owner_loads.load(Ordering::SeqCst), 3);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    let startup_oplog = executor.get_oplog(&startup_id, OplogIndex::INITIAL).await?;
    assert!(
        !startup_oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
    );
    assert!(
        startup_oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Interrupted(_)))
    );
    executor.delete_worker(&startup_id).await?;
    drop(startup);

    let fresh_key = IdempotencyKey::fresh();
    let fresh = executor
        .get_or_add_ephemeral_external_tool(
            caller_component.id,
            context.default_environment_id,
            &fresh_key,
            &InvocationContextStack::fresh(),
            principal.clone(),
        )
        .await?;
    assert_ne!(fresh.agent_id(), owner_id);

    // Agent exports exist, but an external owner must not construct or update any agent type.
    first
        .enqueue_manual_update(caller_component.revision)
        .await
        .expect_err("virtual owners cannot update a primary guest");
    first
        .clone()
        .invoke(golem_common::model::AgentInvocation::ManualUpdate {
            target_revision: caller_component.revision,
        })
        .await
        .err()
        .expect("regular ingress cannot execute a primary guest on a virtual owner");

    let lost_id = fresh.agent_id();
    let lost_fingerprint = fresh.get_latest_worker_metadata().await.fingerprint;
    let mut body_gate = executor.gate_next_entity_body_start(&lost_id);
    {
        let pending = executor.invoke_external_tool(
            &lost_id,
            lost_fingerprint,
            fresh_key.clone(),
            ToolName::try_from("streaming").unwrap(),
            vec!["no-stream".to_string()],
            input.clone(),
            InvocationContextStack::fresh(),
            principal.clone(),
            None,
        );
        tokio::pin!(pending);
        tokio::select! {
            () = body_gate.entered() => {},
            result = &mut pending => anyhow::bail!("invocation ended before body gate: {result:?}"),
        }
        let update_error = executor
            .auto_update_worker(&lost_id, ComponentRevision::new(1000).unwrap(), false)
            .await
            .expect_err("automatic update must not interrupt a virtual owner's invocation");
        assert!(
            update_error
                .to_string()
                .contains("update is not supported for an external tool owner"),
            "unexpected update rejection: {update_error}"
        );
        assert!(
            fresh
                .get_latest_worker_metadata()
                .await
                .last_known_status
                .pending_updates
                .is_empty()
        );
        executor.commit_oplog(&lost_id).await?;
    }
    drop(first);
    drop(second);
    drop(fresh);
    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let observed = executor
        .invoke_external_tool(
            &owner_id,
            owner_metadata.fingerprint,
            key,
            ToolName::try_from("streaming").unwrap(),
            vec!["no-stream".to_string()],
            input.clone(),
            InvocationContextStack::fresh(),
            principal.clone(),
            None,
        )
        .await?;
    assert!(matches!(
        observed.result,
        AgentInvocationResult::ExternalTool { result: Ok(_) }
    ));
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(owner_loads.load(Ordering::SeqCst), 4);

    let failed = executor
        .invoke_external_tool(
            &lost_id,
            lost_fingerprint,
            fresh_key,
            ToolName::try_from("streaming").unwrap(),
            vec!["no-stream".to_string()],
            input,
            InvocationContextStack::fresh(),
            principal,
            None,
        )
        .await
        .expect_err("accepted ephemeral execution must not restart after executor loss");
    assert!(failed.to_string().contains("ephemeral"), "{failed}");
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(owner_loads.load(Ordering::SeqCst), 4);
    executor.delete_worker(&lost_id).await?;
    executor.delete_worker(&owner_id).await?;
    drop(body_gate);

    Ok(())
}

#[test]
#[timeout("2m")]
async fn native_tool_secret_permissions_and_completed_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::agent_secret::{
        AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
    };
    use golem_common::model::oplog::payload::types::SerializableToolError;
    use golem_common::schema::{NamedFieldType, SecretValuePayload};
    use golem_service_base::model::agent_secret::AgentSecret;

    for denied in [false, true] {
        let context = TestContext::new(last_unique_id);
        let environment_state = Arc::new(TestEnvironmentStateService::default());
        let secret_id = AgentSecretId::new();
        let path = vec!["nativeSecret".to_string()];
        environment_state.set_agent_secret(AgentSecret {
            id: secret_id,
            environment_id: context.default_environment_id,
            path: CanonicalAgentSecretPath(path.clone()),
            revision: AgentSecretRevision::INITIAL,
            secret_type: SchemaGraph::anonymous(SchemaType::string()),
            secret_value: Some(SchemaValue::String("not-revealed".to_string())),
        });
        let overrides = TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        };
        let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
        let provider_component = executor
            .component_dep(&context.default_environment_id, provider)
            .store()
            .await?;
        let caller_component = executor
            .component_dep(&context.default_environment_id, caller)
            .update_agent_provision_config("ToolStreamingCaller", |config| {
                if denied {
                    config.initial_permissions.lower_bound.negative.push(
                        golem_common::model::card::parse_polymorphic_permission(
                            "secret(?env) @ * : hold : nativeSecret",
                        )
                        .unwrap(),
                    );
                }
            })
            .store()
            .await?;
        let metadata = extract_component_metadata(
            &deps
                .component_directory
                .join(format!("{}.wasm", provider.wasm_name)),
            false,
            true,
        )
        .await?;
        environment_state.set_tool_deployment(
            context.default_environment_id,
            caller_component.id,
            caller_component.revision,
            Some(deployment_state(
                context.account_id,
                provider_component.id,
                provider_component.revision,
                "golem-it:tool-streaming-rust-provider",
                "ToolStreamingCaller",
                metadata.tools,
            )),
        );
        let agent = agent_id!("ToolStreamingCaller", "native-secret");
        let worker_id = executor
            .start_agent(&caller_component.id, agent.clone())
            .await?;
        let fingerprint = executor.get_worker_metadata(&worker_id).await?.fingerprint;
        // This is trusted executor ingress: the reference names a provisioned, versioned secret,
        // rather than accepting a capability supplied as public JSON.
        let secret = SchemaValue::Secret(SecretValuePayload {
            secret_id: secret_id.0,
            config_key: Some(path),
            version: 0,
            resolved_at: chrono::Utc::now(),
            category: None,
        });
        for fail in [false, true] {
            let input = TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::record(vec![
                    NamedFieldType {
                        name: "value".to_string(),
                        body: SchemaType::secret(Default::default()),
                        metadata: Default::default(),
                    },
                    NamedFieldType {
                        name: "fail".to_string(),
                        body: SchemaType::bool(),
                        metadata: Default::default(),
                    },
                ])),
                SchemaValue::Record {
                    fields: vec![secret.clone(), SchemaValue::Bool(fail)],
                },
            );
            let output = executor
                .invoke_external_tool(
                    &worker_id,
                    fingerprint,
                    IdempotencyKey::fresh(),
                    ToolName::try_from("streaming").unwrap(),
                    vec!["echo-secret".to_string()],
                    input,
                    InvocationContextStack::fresh(),
                    Principal::GolemUser(GolemUserPrincipal {
                        account_id: context.account_id,
                    }),
                    None,
                )
                .await;
            if denied {
                assert!(matches!(output, Err(golem_service_base::error::worker_executor::WorkerExecutorError::PermissionDenied { .. })));
                break;
            }
            match output?.result {
                AgentInvocationResult::ExternalTool { result: Ok(result) } if !fail => {
                    assert_eq!(result.result.unwrap().value(), &secret);
                }
                AgentInvocationResult::ExternalTool {
                    result: Err(SerializableToolRpcError::RemoteToolError(error)),
                } if fail => assert!(matches!(
                    error.as_ref(),
                    SerializableToolError::CustomError(_)
                )),
                other => anyhow::bail!(
                    "unexpected secret tool result: {:?}",
                    other.redacted_debug()
                ),
            }
        }
        let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        if denied {
            assert!(!oplog.iter().any(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::Start(params)
                    if params.function_name == "golem::entity::invoke"
                        || params.function_name == "golem::tool::internal::response-secret-hold-admission"
            )));
            continue;
        }
        assert_eq!(oplog.iter().filter(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::Start(params)
            if params.function_name == "golem::tool::internal::response-secret-hold-admission"
    )).count(), 2);

        environment_state.set_tool_deployment(
            context.default_environment_id,
            caller_component.id,
            caller_component.revision,
            None,
        );
        drop(executor);
        let executor = start_with_overrides(deps, &context, overrides).await?;
        let after: String = executor
            .invoke_and_await_agent(
                &caller_component,
                &agent,
                "record_native_order",
                data_value!("R"),
            )
            .await?
            .into_typed()?;
        assert_eq!(after, "R");
        let replayed = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
        assert_eq!(replayed.iter().filter(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::Start(params)
            if params.function_name == "golem::tool::internal::response-secret-hold-admission"
    )).count(), 2);
        assert_eq!(environment_state.agent_secret_revision_calls(), 0);
    }
    Ok(())
}
