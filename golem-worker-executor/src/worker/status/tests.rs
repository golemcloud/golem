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

use crate::model::ExecutionStatus;
use crate::services::component::ComponentService;
use crate::services::golem_config::GolemConfig;
use crate::services::oplog::{Oplog, OplogService};
use crate::services::{HasComponentService, HasConfig, HasOplogService};
use crate::worker::status::{
    calculate_last_known_status, calculate_last_known_status_for_existing_worker,
    calculate_last_known_status_with_checkpoint_reader, calculate_oplog_processor_checkpoints,
    calculate_total_linear_memory_size, hydrate_initial_pending_evidence, try_fold_status_from,
};
use async_trait::async_trait;
use golem_common::base_model::OplogIndex;
use golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::base_model::oplog::{CardInstallFailure, QueuedCardEvent};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentMode, Principal};
use golem_common::model::application::ApplicationId;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::durable_stream::{
    AttachmentId, AttemptId, PersistedStreamInvocationDescriptorV1, StartAttemptDescriptorV1,
    StreamInvocationIdV1, StreamSessionAttachedRecordV1, StreamSessionPreparedRecordV1,
    StreamSessionRecordV1,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::{InvocationContextStack, TraceId};
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    AgentError, DurableFunctionType, HostRequest, HostRequestNoInput, HostResponse, OplogEntry,
    OplogPayload, PayloadId, RawOplogPayload, UpdateDescription,
};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::{
    AgentId, AgentInvocation, AgentInvocationPayload, AgentInvocationResult, AgentMetadata,
    AgentStatus, AgentStatusRecord, FailedUpdateRecord, IdempotencyKey,
    OplogProcessorCheckpointState, OwnedAgentId, PendingInvocationRef, PendingUpdateKind,
    PendingUpdateRef, ReceivedCardTransferState, RetryConfig, RetryPolicyState, ScanCursor,
    SuccessfulUpdateRecord, Timestamp,
};
use golem_common::read_only_lock;
use golem_common::schema::IntoTypedSchemaValue;
use golem_common::schema::SchemaValue;
use golem_service_base::error::worker_executor::WorkerExecutorError;

#[test]
async fn invalid_initial_pending_bounds_do_not_read_the_referent() {
    let test_case = TestCase::builder(0).build();
    let key = IdempotencyKey::fresh();
    let session_key = StreamInvocationIdV1 {
        callee_environment_id: test_case.owned_agent_id.environment_id,
        callee: test_case.owned_agent_id.agent_id().clone(),
        callee_fingerprint: golem_common::model::AgentFingerprint(uuid::Uuid::new_v4()),
        idempotency_key: key.clone(),
    };
    let attempt = AttemptId::fresh();
    for pending in [
        OplogIndex::NONE,
        OplogIndex::from_u64(10),
        OplogIndex::from_u64(13),
    ] {
        let mut baseline = AgentStatusRecord::default();
        baseline.durable_stream_sessions.insert(
            key.clone(),
            golem_common::model::DurableStreamSessionStatus {
                first_prepared: Some(OplogIndex::from_u64(10)),
                prepared: Some(OplogIndex::from_u64(10)),
                session_key: Some(session_key.clone()),
                prepared_attempt_id: Some(attempt),
                ..Default::default()
            },
        );
        let attached = StreamSessionRecordV1::Attached(StreamSessionAttachedRecordV1 {
            format_version: 1,
            session_key: session_key.clone(),
            attachment_id: AttachmentId::primary(
                session_key.callee_environment_id,
                &session_key.callee,
                &key,
            )
            .unwrap(),
            attempt_id: attempt,
            epoch: 1,
            pending_invocation_oplog_index: pending,
        });
        let entries = BTreeMap::from([(
            OplogIndex::from_u64(13),
            OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(attached)),
            },
        )]);
        test_case.read_starts.lock().unwrap().clear();
        hydrate_initial_pending_evidence(
            &test_case,
            &test_case.owned_agent_id,
            AgentMode::Durable,
            &mut baseline,
            &entries,
        )
        .await
        .unwrap();
        assert!(test_case.read_starts.lock().unwrap().is_empty());
        assert!(
            baseline
                .durable_stream_sessions
                .get(&key)
                .unwrap()
                .lifecycle_error
                .is_some()
        );
    }
}
use golem_service_base::model::component::Component;
use pretty_assertions::assert_eq;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use test_r::test;
use uuid::Uuid;

fn update_status_with_new_entries(
    agent_mode: AgentMode,
    last_known: AgentStatusRecord,
    new_entries: BTreeMap<OplogIndex, OplogEntry>,
    default_retry_policy: &RetryConfig,
) -> Option<AgentStatusRecord> {
    super::update_status_with_new_entries(agent_mode, last_known, new_entries, default_retry_policy)
        .unwrap()
}

#[test]
fn cancellation_obligations_survive_status_checkpoint_without_local_prepared() {
    use golem_common::model::durable_stream::{
        StreamCancelReasonV1, StreamCancelRoleV1, StreamConsumerCancelAppliedRecordV1,
        StreamConsumerCancelIntentRecordV1,
    };
    let intent = StreamConsumerCancelIntentRecordV1 {
        format_version: 1,
        session_key: StreamInvocationIdV1 {
            callee_environment_id: EnvironmentId::new(),
            callee: AgentId {
                component_id: ComponentId::new(),
                agent_id: "remote".into(),
            },
            callee_fingerprint: golem_common::model::AgentFingerprint(Uuid::new_v4()),
            idempotency_key: IdempotencyKey::fresh(),
        },
        stream_id: golem_common::model::StreamId(Uuid::new_v4()),
        epoch: 7,
        role: StreamCancelRoleV1::OutputConsumer,
        reason: StreamCancelReasonV1::Cancelled,
        details: Some("cancel requested".into()),
    };
    let entry = |record| OplogEntry::StreamSession {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: None,
        record: OplogPayload::Inline(Box::new(record)),
    };
    let fold = |status, index, record| {
        update_status_with_new_entries(
            AgentMode::Durable,
            status,
            BTreeMap::from([(OplogIndex::from_u64(index), entry(record))]),
            &RetryConfig::default(),
        )
        .unwrap()
    };
    let pending = fold(
        AgentStatusRecord::default(),
        2,
        StreamSessionRecordV1::ConsumerCancelIntent(intent.clone()),
    );
    assert!(pending.durable_stream_sessions.iter().next().is_none());
    assert_eq!(
        pending.pending_durable_stream_cancellations,
        HashSet::from([intent.clone()])
    );
    let bytes = golem_common::serialization::serialize(&pending).unwrap();
    let pending: AgentStatusRecord = golem_common::serialization::deserialize(&bytes).unwrap();
    let mut wrong = intent.clone();
    wrong.epoch = 8;
    let pending = fold(
        pending,
        3,
        StreamSessionRecordV1::ConsumerCancelApplied(StreamConsumerCancelAppliedRecordV1 {
            format_version: 1,
            intent: wrong,
        }),
    );
    assert_eq!(
        pending.pending_durable_stream_cancellations,
        HashSet::from([intent.clone()])
    );
    let applied = fold(
        pending,
        4,
        StreamSessionRecordV1::ConsumerCancelApplied(StreamConsumerCancelAppliedRecordV1 {
            format_version: 1,
            intent,
        }),
    );
    assert!(applied.pending_durable_stream_cancellations.is_empty());
    assert!(applied.durable_stream_sessions.iter().next().is_none());
}

#[test]
fn successful_update_resets_total_linear_memory_size() {
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(2),
            OplogEntry::successful_update(
                ComponentRevision::new(2).unwrap(),
                100,
                Some(32),
                HashSet::new(),
            ),
        ),
        (OplogIndex::from_u64(3), OplogEntry::grow_memory(8)),
    ]);

    assert_eq!(
        calculate_total_linear_memory_size(64, &DeletedRegions::new(), &entries),
        40
    );
}

/// Why `commit_and_update_state` still compares the folded record against the previous one
/// instead of taking "the commit produced entries" as the answer: the fold's `finalize` step
/// prunes oplog-processor checkpoints that are neither active nor in-flight, and it runs
/// whether or not there were entries. So an empty fold can still change the record, and
/// skipping it would drop that change.
#[test]
fn empty_fold_is_not_the_identity() {
    let retry = RetryConfig::default();
    let grant = EnvironmentPluginGrantId::new();
    let baseline = AgentStatusRecord {
        oplog_idx: OplogIndex::from_u64(10),
        active_plugins: HashSet::new(),
        oplog_processor_checkpoints: HashMap::from([(
            grant,
            OplogProcessorCheckpointState {
                target_agent_id: None,
                confirmed_up_to: OplogIndex::from_u64(5),
                sending_up_to: OplogIndex::from_u64(5),
                last_batch_start: OplogIndex::from_u64(5),
            },
        )]),
        ..AgentStatusRecord::default()
    };

    let folded = update_status_with_new_entries(
        AgentMode::Durable,
        baseline.clone(),
        BTreeMap::new(),
        &retry,
    )
    .unwrap();

    assert!(folded.oplog_processor_checkpoints.is_empty());
    assert_ne!(folded, baseline);
}

/// The other half: any entry moves `oplog_idx`, so a non-empty fold always differs.
#[test]
fn any_entry_moves_the_oplog_index() {
    let retry = RetryConfig::default();
    let baseline = AgentStatusRecord {
        oplog_idx: OplogIndex::from_u64(10),
        ..AgentStatusRecord::default()
    };

    let folded = update_status_with_new_entries(
        AgentMode::Durable,
        baseline.clone(),
        BTreeMap::from([(OplogIndex::from_u64(11), OplogEntry::grow_memory(8))]),
        &retry,
    )
    .unwrap();

    assert_eq!(folded.oplog_idx, OplogIndex::from_u64(11));
    assert_ne!(folded, baseline);
}

#[test]
async fn empty() {
    let test_case = TestCase::builder(0).build();

    run_test_case(test_case).await;
}

#[test]
async fn semantic_retry_state_keeps_status_retrying_after_legacy_retry_limit() {
    let idempotency_key = IdempotencyKey::fresh();
    let retry_from = OplogIndex::from_u64(2);
    let retry_state = RetryPolicyState::Counter(56);
    let test_case = TestCase::builder(0)
        .agent_invocation_started("a", vec![], idempotency_key.clone())
        .add(
            OplogEntry::error(
                None,
                AgentError::TransientError("transient".to_string()),
                retry_from,
                false,
                Some(retry_state.clone()),
            ),
            move |mut status| {
                status.status = AgentStatus::Retrying;
                status.current_retry_state.insert(retry_from, retry_state);
                status
                    .invocation_results
                    .insert(idempotency_key, status.oplog_idx);
                status
            },
        )
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn exhausted_semantic_retry_state_marks_status_failed() {
    let idempotency_key = IdempotencyKey::fresh();
    let retry_from = OplogIndex::from_u64(2);
    let retry_state = RetryPolicyState::AndThen {
        left: Box::new(RetryPolicyState::Counter(3)),
        right: Box::new(RetryPolicyState::Terminal),
        on_right: true,
    };
    let test_case = TestCase::builder(0)
        .agent_invocation_started("a", vec![], idempotency_key.clone())
        .add(
            OplogEntry::error(
                None,
                AgentError::TransientError("transient".to_string()),
                retry_from,
                false,
                Some(retry_state.clone()),
            ),
            move |mut status| {
                status.status = AgentStatus::Failed;
                status.current_retry_state.insert(retry_from, retry_state);
                status
                    .invocation_results
                    .insert(idempotency_key, status.oplog_idx);
                status
            },
        )
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn invocation_results() {
    let k1 = IdempotencyKey::fresh();
    let k2 = IdempotencyKey::fresh();

    let test_case = TestCase::builder(0)
        .agent_invocation_started("a", vec![], k1.clone())
        .grow_memory(10)
        .grow_memory(100)
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        )
        .agent_invocation_started("b", vec![], k2.clone())
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k2,
            ComponentRevision::INITIAL,
        )
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn invocation_results_with_jump() {
    let k1 = IdempotencyKey::fresh();
    let k2 = IdempotencyKey::fresh();

    let test_case = TestCase::builder(0)
        .agent_invocation_started("a", vec![], k1.clone())
        .grow_memory(10)
        .grow_memory(100)
        .jump(OplogIndex::from_u64(2))
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        )
        .agent_invocation_started("b", vec![], k2.clone())
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k2,
            ComponentRevision::INITIAL,
        )
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn invocation_results_with_revert() {
    let k1 = IdempotencyKey::fresh();
    let k2 = IdempotencyKey::fresh();

    let test_case = TestCase::builder(0)
        .agent_invocation_started("a", vec![], k1.clone())
        .grow_memory(10)
        .grow_memory(100)
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        )
        .agent_invocation_started("b", vec![], k2.clone())
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k2,
            ComponentRevision::INITIAL,
        )
        .revert(OplogIndex::from_u64(5))
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn single_auto_update_for_running() {
    let k1 = IdempotencyKey::fresh();
    let update1 = UpdateDescription::Automatic {
        target_revision: ComponentRevision::new(2).unwrap(),
    };

    let test_case = TestCase::builder(1)
        .agent_invocation_started("a", vec![], k1.clone())
        .grow_memory(10)
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .pending_update(&update1, |_| {})
        .successful_update(update1, 2000, &HashSet::new())
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        )
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn auto_update_for_running_with_jump() {
    let k1 = IdempotencyKey::fresh();
    let update1 = UpdateDescription::Automatic {
        target_revision: ComponentRevision::new(2).unwrap(),
    };
    let update2 = UpdateDescription::Automatic {
        target_revision: ComponentRevision::new(3).unwrap(),
    };

    let test_case = TestCase::builder(1)
        .agent_invocation_started("a", vec![], k1.clone())
        .grow_memory(10)
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .pending_update(&update1, |_| {})
        .pending_update(&update2, |_| {})
        .successful_update(update1, 2000, &HashSet::new())
        .jump(OplogIndex::from_u64(4))
        .successful_update(update2, 3000, &HashSet::new())
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        )
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn single_manual_update() {
    let k1 = IdempotencyKey::fresh();
    let k2 = IdempotencyKey::fresh();
    let update1 = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(2).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
    };

    let test_case = TestCase::builder(1)
        .agent_invocation_started("a", vec![], k1.clone())
        .grow_memory(10)
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .pending_invocation(AgentInvocation::ManualUpdate {
            target_revision: ComponentRevision::new(2).unwrap(),
        })
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        )
        .pending_update(&update1, |status| status.total_linear_memory_size = 200)
        .successful_update(update1, 2000, &HashSet::new())
        .agent_invocation_started("c", vec![], k2.clone())
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k2,
            ComponentRevision::INITIAL,
        )
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn single_manual_failed_update() {
    let k1 = IdempotencyKey::fresh();
    let k2 = IdempotencyKey::fresh();
    let update1 = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(2).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
    };

    let test_case = TestCase::builder(1)
        .agent_invocation_started("a", vec![], k1.clone())
        .grow_memory(10)
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .pending_invocation(AgentInvocation::ManualUpdate {
            target_revision: ComponentRevision::new(2).unwrap(),
        })
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        )
        .pending_update(&update1, |_| {})
        .failed_update(update1)
        .agent_invocation_started("c", vec![], k2.clone())
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k2,
            ComponentRevision::INITIAL,
        )
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn single_manual_failed_update_during_snapshot() {
    let k1 = IdempotencyKey::fresh();
    let k2 = IdempotencyKey::fresh();
    let update2 = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(2).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
    };

    let test_case = TestCase::builder(1)
        .agent_invocation_started("a", vec![], k1.clone())
        .grow_memory(10)
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .pending_invocation(AgentInvocation::ManualUpdate {
            target_revision: ComponentRevision::new(2).unwrap(),
        })
        .failed_update(update2)
        .agent_invocation_started("c", vec![], k2.clone())
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k2,
            ComponentRevision::INITIAL,
        )
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn auto_update_for_running_with_jump_and_revert() {
    let k1 = IdempotencyKey::fresh();
    let update1 = UpdateDescription::Automatic {
        target_revision: ComponentRevision::new(2).unwrap(),
    };
    let update2 = UpdateDescription::Automatic {
        target_revision: ComponentRevision::new(3).unwrap(),
    };

    let test_case = TestCase::builder(1)
        .agent_invocation_started("a", vec![], k1.clone())
        .grow_memory(10)
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .pending_update(&update1, |_| {})
        .pending_update(&update2, |_| {})
        .successful_update(update1, 2000, &HashSet::new())
        .jump(OplogIndex::from_u64(4))
        .successful_update(update2, 3000, &HashSet::new())
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        )
        .revert(OplogIndex::from_u64(3))
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn single_manual_update_with_revert() {
    let k1 = IdempotencyKey::fresh();
    let k2 = IdempotencyKey::fresh();
    let update1 = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(2).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
    };

    let test_case = TestCase::builder(1)
        .agent_invocation_started("a", vec![], k1.clone())
        .grow_memory(10)
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .pending_invocation(AgentInvocation::ManualUpdate {
            target_revision: ComponentRevision::new(2).unwrap(),
        })
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        )
        .pending_update(&update1, |_| {})
        .successful_update(update1, 2000, &HashSet::new())
        .agent_invocation_started("c", vec![], k2.clone())
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k2,
            ComponentRevision::INITIAL,
        )
        .revert(OplogIndex::from_u64(4))
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn multiple_manual_updates_with_jump_and_revert() {
    let k1 = IdempotencyKey::fresh();
    let k2 = IdempotencyKey::fresh();
    let update1 = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(2).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
    };
    let update2 = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(2).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
    };

    let test_case = TestCase::builder(1)
        .agent_invocation_started("a", vec![], k1.clone())
        .grow_memory(10)
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .pending_invocation(AgentInvocation::ManualUpdate {
            target_revision: ComponentRevision::new(2).unwrap(),
        })
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        )
        .pending_update(&update1, |_| {})
        .failed_update(update1)
        .agent_invocation_started("c", vec![], k2.clone())
        .pending_invocation(AgentInvocation::ManualUpdate {
            target_revision: ComponentRevision::new(2).unwrap(),
        })
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k2,
            ComponentRevision::INITIAL,
        )
        .pending_update(&update2, |_| {})
        .successful_update(update2, 2000, &HashSet::new())
        .revert(OplogIndex::from_u64(5))
        .build();

    run_test_case(test_case).await;
}

/// Two snapshot-based updates that both succeed: the first update's region
/// has to stay folded into the skipped regions while the second one's
/// override sits on top of it.
#[test]
async fn two_successful_manual_updates() {
    let k1 = IdempotencyKey::fresh();
    let k2 = IdempotencyKey::fresh();
    let update1 = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(2).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
    };
    let update2 = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(3).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
    };

    let test_case = TestCase::builder(1)
        .agent_invocation_started("a", vec![], k1.clone())
        .host_call(
            "b",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(1.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        )
        .pending_update(&update1, |_| {})
        .successful_update(update1, 1000, &HashSet::new())
        // The suffix the second update has to replay.
        .agent_invocation_started("c", vec![], k2.clone())
        .host_call(
            "d",
            HostRequest::NoInput(HostRequestNoInput {}),
            HostResponse::Custom(2.into_typed_schema_value().unwrap()),
            DurableFunctionType::ReadLocal,
        )
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k2,
            ComponentRevision::new(2).unwrap(),
        )
        .pending_update(&update2, |_| {})
        .successful_update(update2, 2000, &HashSet::new())
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn multiple_reverts() {
    let k1 = IdempotencyKey::fresh();
    let k2 = IdempotencyKey::fresh();

    let test_case = TestCase::builder(0)
        .agent_invocation_started("a", vec![], k1.clone())
        .grow_memory(10)
        .grow_memory(100)
        .pending_invocation(AgentInvocation::AgentMethod {
            idempotency_key: k2.clone(),
            method_name: "b".to_string(),
            input: SchemaValue::Record {
                fields: vec![SchemaValue::Bool(true)],
            },
            invocation_context: InvocationContextStack::fresh(),
            principal: Principal::anonymous(),
            scope_card: None,
        })
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1.clone(),
            ComponentRevision::INITIAL,
        )
        .agent_invocation_started("b", vec![], k2.clone())
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k2.clone(),
            ComponentRevision::INITIAL,
        )
        .revert(OplogIndex::from_u64(5))
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        )
        .agent_invocation_started("b", vec![], k2.clone())
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k2,
            ComponentRevision::INITIAL,
        )
        .revert(OplogIndex::from_u64(2))
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn cancel_pending_invocation() {
    let k1 = IdempotencyKey::fresh();
    let k2 = IdempotencyKey::fresh();

    let test_case = TestCase::builder(0)
        .pending_invocation(AgentInvocation::AgentMethod {
            idempotency_key: k1.clone(),
            method_name: "a".to_string(),
            input: SchemaValue::Record {
                fields: vec![SchemaValue::Bool(true)],
            },
            invocation_context: InvocationContextStack::fresh(),
            principal: Principal::anonymous(),
            scope_card: None,
        })
        .pending_invocation(AgentInvocation::AgentMethod {
            idempotency_key: k2.clone(),
            method_name: "b".to_string(),
            input: SchemaValue::Record { fields: vec![] },
            invocation_context: InvocationContextStack::fresh(),
            principal: Principal::anonymous(),
            scope_card: None,
        })
        .cancel_pending_invocation(k1)
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn permission_denied_rejection_survives_status_reconstruction() {
    let idempotency_key = IdempotencyKey::fresh();
    let test_case = TestCase::builder(0)
        .pending_invocation(AgentInvocation::AgentMethod {
            idempotency_key: idempotency_key.clone(),
            method_name: "denied".to_string(),
            input: SchemaValue::Record { fields: vec![] },
            invocation_context: InvocationContextStack::fresh(),
            principal: Principal::anonymous(),
            scope_card: None,
        })
        .permission_denied_pending_invocation(idempotency_key.clone())
        .build();
    let expected = test_case.entries.last().unwrap().expected_status.clone();
    let pending_baseline = test_case.entries[1].expected_status.clone();

    let from_start = calculate_last_known_status(
        &test_case,
        &test_case.owned_agent_id,
        AgentMode::Durable,
        None,
    )
    .await
    .unwrap()
    .unwrap();
    let from_before_atomic_pair = calculate_last_known_status_for_existing_worker(
        &test_case,
        &test_case.owned_agent_id,
        AgentMode::Durable,
        Some(pending_baseline),
    )
    .await
    .unwrap();

    assert_eq!(from_start, expected);
    assert_eq!(from_before_atomic_pair, expected);
    assert_eq!(expected.status, AgentStatus::Idle);
    assert!(expected.pending_invocations.is_empty());
    assert_eq!(expected.current_idempotency_key, None);
    assert_eq!(
        expected.invocation_results.get(&idempotency_key),
        Some(&expected.oplog_idx)
    );
}

#[test]
async fn snapshot_tracking() {
    let k1 = IdempotencyKey::fresh();

    let test_case = TestCase::builder(0)
        .agent_invocation_started("a", vec![], k1.clone())
        .grow_memory(10)
        .snapshot()
        .grow_memory(100)
        .snapshot()
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        )
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn successful_auto_update_invalidates_automatic_snapshot() {
    let update = UpdateDescription::Automatic {
        target_revision: ComponentRevision::new(2).unwrap(),
    };

    let test_case = TestCase::builder(1)
        .snapshot()
        .pending_update(&update, |_| {})
        .successful_update(update, 2000, &HashSet::new())
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(final_status.last_automatic_snapshot_index, None);
    assert_eq!(final_status.last_automatic_snapshot_timestamp, None);
    assert_eq!(
        final_status.last_automatic_snapshot_component_revision,
        None
    );
    run_test_case(test_case).await;
}

#[test]
async fn failed_auto_update_keeps_automatic_snapshot() {
    let update = UpdateDescription::Automatic {
        target_revision: ComponentRevision::new(2).unwrap(),
    };

    let test_case = TestCase::builder(1)
        .snapshot()
        .pending_update(&update, |_| {})
        .failed_update(update)
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(
        final_status.last_automatic_snapshot_component_revision,
        Some(ComponentRevision::new(1).unwrap())
    );
    run_test_case(test_case).await;
}

#[test]
async fn snapshot_tracking_with_revert() {
    let k1 = IdempotencyKey::fresh();

    let test_case = TestCase::builder(0)
        .agent_invocation_started("a", vec![], k1.clone())
        .grow_memory(10)
        .snapshot()
        .grow_memory(100)
        .snapshot()
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        )
        .revert(OplogIndex::from_u64(3))
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn non_existing_oplog() {
    let environment_id = EnvironmentId::new();
    let owned_agent_id = OwnedAgentId::new(
        environment_id,
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "test-worker".to_string(),
        },
    );
    let test_case = TestCase {
        owned_agent_id: owned_agent_id.clone(),
        entries: vec![],
        read_starts: Arc::new(std::sync::Mutex::new(Vec::new())),
        raw_payloads: Arc::new(std::sync::Mutex::new(Vec::new())),
    };

    let result = calculate_last_known_status(&test_case, &owned_agent_id, AgentMode::Durable, None)
        .await
        .unwrap();
    assert2::assert!(let None = result);
}

/// Builds an oplog where a clean idle boundary (idx 3) precedes an invocation that later jumps,
/// deleting the region [4, 7]. Returns the test case plus the clean-checkpoint baseline (idx 3),
/// a stale live baseline inside the deleted region (idx 6), and the expected final status.
fn jump_repair_fixture() -> (
    TestCase,
    AgentStatusRecord,
    AgentStatusRecord,
    AgentStatusRecord,
) {
    let k1 = IdempotencyKey::fresh();
    let k2 = IdempotencyKey::fresh();

    let test_case = TestCase::builder(0)
        .agent_invocation_started("a", vec![], k1.clone()) // idx 2
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        ) // idx 3 (clean idle boundary)
        .agent_invocation_started("b", vec![], k2.clone()) // idx 4
        .grow_memory(10) // idx 5
        .grow_memory(100) // idx 6
        .jump(OplogIndex::from_u64(4)) // idx 7, deletes region [4, 7]
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k2,
            ComponentRevision::INITIAL,
        ) // idx 8
        .build();

    let final_expected = test_case.entries.last().unwrap().expected_status.clone();
    let checkpoint = test_case.entries[2].expected_status.clone(); // idx 3, before the deleted region
    let stale_live = test_case.entries[5].expected_status.clone(); // idx 6, inside the deleted region

    (test_case, checkpoint, stale_live, final_expected)
}

#[test]
async fn checkpoint_repair_folds_from_checkpoint_after_jump() {
    let (test_case, checkpoint, stale_live, final_expected) = jump_repair_fixture();

    // The stale live baseline is inside the deleted region, so it cannot be folded forward.
    let direct = try_fold_status_from(
        &test_case,
        &test_case.owned_agent_id,
        AgentMode::Durable,
        stale_live.clone(),
    )
    .await
    .unwrap();
    assert2::assert!(let None = direct);

    test_case.read_starts.lock().unwrap().clear();

    let repaired = calculate_last_known_status_with_checkpoint_reader(
        &test_case,
        &test_case.owned_agent_id,
        AgentMode::Durable,
        Some(stale_live),
        || async { Some(checkpoint) },
    )
    .await
    .unwrap();

    assert_eq!(repaired, Some(final_expected));

    let read_starts = test_case.read_starts.lock().unwrap().clone();
    assert!(
        read_starts.contains(&4),
        "expected a fold from the checkpoint baseline (read starting at idx 4), got {read_starts:?}"
    );
    assert!(
        !read_starts.contains(&1),
        "expected no full recompute from idx 1, got {read_starts:?}"
    );
}

#[test]
async fn checkpoint_repair_falls_back_to_full_recompute_without_checkpoint() {
    let (test_case, _checkpoint, stale_live, final_expected) = jump_repair_fixture();

    test_case.read_starts.lock().unwrap().clear();

    let result = calculate_last_known_status_with_checkpoint_reader(
        &test_case,
        &test_case.owned_agent_id,
        AgentMode::Durable,
        Some(stale_live),
        || async { None },
    )
    .await
    .unwrap();

    assert_eq!(result, Some(final_expected));

    let read_starts = test_case.read_starts.lock().unwrap().clone();
    assert!(
        read_starts.contains(&1),
        "expected a full recompute from idx 1, got {read_starts:?}"
    );
}

#[test]
async fn full_recompute_reads_each_oplog_chunk_twice() {
    let (test_case, _checkpoint, _stale_live, final_expected) = jump_repair_fixture();

    let result = calculate_last_known_status(
        &test_case,
        &test_case.owned_agent_id,
        AgentMode::Durable,
        None,
    )
    .await
    .unwrap();

    assert_eq!(result, Some(final_expected));
    assert_eq!(
        *test_case.read_starts.lock().unwrap(),
        vec![1, 3, 5, 7, 1, 3, 5, 7]
    );
}

#[test]
async fn checkpoint_repair_falls_back_to_full_recompute_when_checkpoint_unusable() {
    let (test_case, _checkpoint, stale_live, final_expected) = jump_repair_fixture();

    // A checkpoint that itself falls inside the deleted region (idx 5) cannot be folded forward
    // either, so we must fall back to a full recompute.
    let unusable_checkpoint = test_case.entries[4].expected_status.clone(); // idx 5

    test_case.read_starts.lock().unwrap().clear();

    let result = calculate_last_known_status_with_checkpoint_reader(
        &test_case,
        &test_case.owned_agent_id,
        AgentMode::Durable,
        Some(stale_live),
        || async { Some(unusable_checkpoint) },
    )
    .await
    .unwrap();

    assert_eq!(result, Some(final_expected));

    let read_starts = test_case.read_starts.lock().unwrap().clone();
    assert!(
        read_starts.contains(&1),
        "expected a full recompute from idx 1, got {read_starts:?}"
    );
}

struct TestCaseBuilder {
    entries: Vec<TestEntry>,
    previous_status_record: AgentStatusRecord,
    owned_agent_id: OwnedAgentId,
}

impl TestCaseBuilder {
    pub fn new(
        account_id: AccountId,
        owned_agent_id: OwnedAgentId,
        component_revision: ComponentRevision,
    ) -> Self {
        let status = AgentStatusRecord {
            component_revision,
            component_revision_for_replay: component_revision,
            component_size: 100,
            total_linear_memory_size: 200,
            oplog_idx: OplogIndex::INITIAL,
            ..Default::default()
        };
        TestCaseBuilder {
            entries: vec![TestEntry {
                oplog_entry: OplogEntry::create(
                    owned_agent_id.agent_id(),
                    AgentMode::Durable,
                    component_revision,
                    vec![],
                    owned_agent_id.environment_id(),
                    account_id,
                    None,
                    100,
                    200,
                    HashSet::new(),
                    Vec::new(),
                    None,
                    Uuid::new_v4(),
                ),
                expected_status: status.clone(),
            }],
            previous_status_record: status,
            owned_agent_id,
        }
    }

    pub fn add(
        mut self,
        entry: OplogEntry,
        update: impl FnOnce(AgentStatusRecord) -> AgentStatusRecord,
    ) -> Self {
        self.previous_status_record.oplog_idx = self.previous_status_record.oplog_idx.next();
        self.previous_status_record = update(self.previous_status_record);
        self.entries.push(TestEntry {
            oplog_entry: entry,
            expected_status: self.previous_status_record.clone(),
        });
        self
    }

    pub fn agent_invocation_started(
        self,
        function_name: &str,
        request: Vec<SchemaValue>,
        idempotency_key: IdempotencyKey,
    ) -> Self {
        let payload = AgentInvocationPayload::AgentMethod {
            method_name: function_name.to_string(),
            input: SchemaValue::Record { fields: request },
            principal: Principal::anonymous(),
            scope_card: None,
        };
        self.add(
            OplogEntry::AgentInvocationStarted {
                timestamp: Timestamp::now_utc(),
                idempotency_key: idempotency_key.clone(),
                payload: OplogPayload::Inline(Box::new(payload)),
                trace_id: TraceId::generate(),
                trace_states: vec![],
                invocation_context: vec![],
                wallet_pin: None,
            },
            move |mut status| {
                status.current_idempotency_key = Some(idempotency_key);
                status.status = AgentStatus::Running;
                if !status.pending_invocations.is_empty() {
                    status.pending_invocations.pop();
                }
                status
            },
        )
    }

    pub fn agent_invocation_finished(
        self,
        result: AgentInvocationResult,
        idempotency_key: IdempotencyKey,
        component_revision: ComponentRevision,
    ) -> Self {
        self.add(
            OplogEntry::AgentInvocationFinished {
                timestamp: Timestamp::now_utc(),
                result: OplogPayload::Inline(Box::new(result)),
                method_name: None,
                consumed_fuel: 0,
                component_revision,
            },
            move |mut status| {
                status
                    .invocation_results
                    .insert(idempotency_key, status.oplog_idx);
                status.current_idempotency_key = None;
                status.status = AgentStatus::Idle;
                status
            },
        )
    }

    pub fn host_call(
        self,
        name: &str,
        i: HostRequest,
        o: HostResponse,
        func_type: DurableFunctionType,
    ) -> Self {
        let start_index = OplogIndex::from_u64(self.entries.len() as u64 + 1);
        self.add(
            OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                function_name: HostFunctionName::Custom(name.to_string()),
                invocation_id: None,
                observational_owner: None,
                request: Some(OplogPayload::Inline(Box::new(i))),
                durable_function_type: func_type,
            },
            |status| status,
        )
        .add(
            OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index,
                response: Some(OplogPayload::Inline(Box::new(o))),
                forced_commit: false,
            },
            |status| status,
        )
    }

    pub fn grow_memory(self, delta: u64) -> Self {
        self.add(
            OplogEntry::GrowMemory {
                timestamp: Timestamp::now_utc(),
                delta,
            },
            |mut status| {
                status.total_linear_memory_size += delta;
                status
            },
        )
    }

    pub fn snapshot(self) -> Self {
        let oplog_idx = OplogIndex::from_u64(self.entries.len() as u64 + 1);
        let timestamp = Timestamp::now_utc().rounded();
        self.add(
            OplogEntry::Snapshot {
                timestamp,
                data: OplogPayload::Inline(Box::new(vec![])),
                mime_type: "application/octet-stream".to_string(),
                active_cards: Vec::new(),
                wallet_generation: 0,
            },
            move |mut status| {
                status.last_automatic_snapshot_index = Some(oplog_idx);
                status.last_automatic_snapshot_timestamp = Some(timestamp);
                status.last_automatic_snapshot_component_revision = Some(status.component_revision);
                status
            },
        )
    }

    pub fn jump(self, target: OplogIndex) -> Self {
        let current = OplogIndex::from_u64(self.entries.len() as u64 + 1);
        let region = OplogRegion {
            start: target,
            end: current,
        };
        let old_status = self.entries[u64::from(target) as usize - 1]
            .expected_status
            .clone();
        self.add(OplogEntry::jump(None, region.clone()), move |mut status| {
            status.status = old_status.status;
            status.component_revision = old_status.component_revision;
            status.current_idempotency_key = old_status.current_idempotency_key;
            status.total_linear_memory_size = old_status.total_linear_memory_size;
            status.component_size = old_status.component_size;
            status.owned_resources = old_status.owned_resources;
            status.skipped_regions.add(region);
            status
        })
    }

    pub fn revert(self, target: OplogIndex) -> Self {
        let current = OplogIndex::from_u64(self.entries.len() as u64 + 1);
        let region = OplogRegion {
            start: target.next(),
            end: current,
        };

        let old_status = self.entries[u64::from(target) as usize - 1]
            .expected_status
            .clone();
        self.add(OplogEntry::revert(region.clone()), move |mut status| {
            let revert_generation = status
                .invocation_results
                .revert_generation()
                .wrapping_add(1);
            status.active_plugins = old_status.active_plugins;

            status.skipped_regions = old_status.skipped_regions;
            status.skipped_regions.add(region.clone());
            status.deleted_regions.add(region);

            status.status = old_status.status;
            status.component_revision = old_status.component_revision;
            status.current_idempotency_key = old_status.current_idempotency_key;
            status.total_linear_memory_size = old_status.total_linear_memory_size;
            status.component_size = old_status.component_size;
            status.owned_resources = old_status.owned_resources;
            status.successful_updates = old_status.successful_updates;
            status.failed_updates = old_status.failed_updates;
            status.invocation_results = old_status.invocation_results;
            status
                .invocation_results
                .set_revert_generation(revert_generation);
            status.component_revision_for_replay = old_status.component_revision_for_replay;
            status.last_manual_update_snapshot_index = old_status.last_manual_update_snapshot_index;
            status.last_automatic_snapshot_index = old_status.last_automatic_snapshot_index;
            status.last_automatic_snapshot_timestamp = old_status.last_automatic_snapshot_timestamp;
            status.last_automatic_snapshot_component_revision =
                old_status.last_automatic_snapshot_component_revision;

            status
        })
    }

    pub fn pending_invocation(self, invocation: AgentInvocation) -> Self {
        let (idempotency_key, invocation_payload, invocation_context) =
            invocation.clone().into_parts();
        let entry = OplogEntry::pending_agent_invocation(
            idempotency_key,
            OplogPayload::Inline(Box::new(invocation_payload)),
            invocation_context.trace_id.clone(),
            invocation_context.trace_states.clone(),
            invocation_context.to_oplog_data(),
        )
        .rounded();
        let oplog_idx = OplogIndex::from_u64(self.entries.len() as u64 + 1);
        let ref_idempotency_key = invocation.idempotency_key().cloned();
        let manual_update_target_revision = match &invocation {
            AgentInvocation::ManualUpdate { target_revision } => Some(*target_revision),
            _ => None,
        };
        self.add(entry.clone(), move |mut status| {
            status.pending_invocations.push(PendingInvocationRef {
                timestamp: entry.timestamp(),
                oplog_index: oplog_idx,
                idempotency_key: ref_idempotency_key.clone(),
                manual_update_target_revision,
            });
            status
        })
    }

    pub fn cancel_pending_invocation(self, idempotency_key: IdempotencyKey) -> Self {
        let entry = OplogEntry::cancel_pending_invocation(idempotency_key.clone()).rounded();
        self.add(entry.clone(), move |mut status| {
            status
                .pending_invocations
                .retain(|ti| ti.idempotency_key() != Some(&idempotency_key));
            status.cancelled_idempotency_key = Some(idempotency_key);
            status
        })
    }

    pub fn permission_denied_pending_invocation(self, idempotency_key: IdempotencyKey) -> Self {
        self.cancel_pending_invocation(idempotency_key.clone()).add(
            OplogEntry::error(
                None,
                AgentError::PermissionDenied("permission denied".to_string()),
                OplogIndex::INITIAL,
                false,
                None,
            ),
            move |mut status| {
                status.cancelled_idempotency_key = None;
                status
                    .invocation_results
                    .insert(idempotency_key, status.oplog_idx);
                status.status = AgentStatus::Idle;
                status.current_retry_state.clear();
                status
            },
        )
    }

    pub fn pending_update(
        self,
        update_description: &UpdateDescription,
        extra_status_updates: impl Fn(&mut AgentStatusRecord),
    ) -> Self {
        let entry = OplogEntry::pending_update(update_description.clone()).rounded();
        let oplog_idx = OplogIndex::from_u64(self.entries.len() as u64 + 1);
        self.add(entry.clone(), move |mut status| {
            let kind = match update_description {
                UpdateDescription::Automatic { .. } => PendingUpdateKind::Automatic,
                UpdateDescription::SnapshotBased { .. } => PendingUpdateKind::SnapshotBased,
            };
            status.pending_updates.push_back(PendingUpdateRef {
                timestamp: entry.timestamp(),
                oplog_index: oplog_idx,
                target_revision: *update_description.target_revision(),
                kind,
            });

            if !status.pending_invocations.is_empty() {
                status.pending_invocations.pop();
            }

            if let UpdateDescription::SnapshotBased { .. } = update_description {
                status
                    .skipped_regions
                    .set_override(DeletedRegions::from_regions(vec![
                        OplogRegion::from_index_range(OplogIndex::INITIAL.next()..=oplog_idx),
                    ]));
            }

            extra_status_updates(&mut status);

            status
        })
    }

    pub fn successful_update(
        self,
        update_description: UpdateDescription,
        new_component_size: u64,
        new_active_plugins: &HashSet<EnvironmentPluginGrantId>,
    ) -> Self {
        let old_status = self.entries.first().unwrap().expected_status.clone();
        let entry = OplogEntry::successful_update(
            *update_description.target_revision(),
            new_component_size,
            None,
            new_active_plugins.clone(),
        )
        .rounded();
        self.add(entry.clone(), move |mut status| {
            let applied_update = status.pending_updates.pop_front();
            status.successful_updates.push(SuccessfulUpdateRecord {
                timestamp: entry.timestamp(),
                target_revision: *update_description.target_revision(),
            });
            status.component_size = new_component_size;
            status.component_revision = *update_description.target_revision();
            status.active_plugins = new_active_plugins.clone();
            status.last_automatic_snapshot_index = None;
            status.last_automatic_snapshot_timestamp = None;
            status.last_automatic_snapshot_component_revision = None;

            if status.skipped_regions.is_overridden() {
                status.skipped_regions.merge_override();
                status.total_linear_memory_size = old_status.total_linear_memory_size;
                status.owned_resources = HashMap::new();
            }

            if let UpdateDescription::SnapshotBased {
                target_revision, ..
            } = update_description
            {
                status.component_revision_for_replay = target_revision;
                status.last_manual_update_snapshot_index = applied_update.map(|au| au.oplog_index);
            };

            status
        })
    }

    pub fn failed_update(self, update_description: UpdateDescription) -> Self {
        let entry = OplogEntry::failed_update(
            *update_description.target_revision(),
            Some("details".to_string()),
        )
        .rounded();
        self.add(entry.clone(), move |mut status| {
            status.failed_updates.push(FailedUpdateRecord {
                timestamp: entry.timestamp(),
                target_revision: *update_description.target_revision(),
                details: Some("details".to_string()),
            });
            status.pending_updates.pop_front();

            if status.skipped_regions.is_overridden() {
                status.skipped_regions.drop_override();
            }

            if let UpdateDescription::SnapshotBased {
                target_revision, ..
            } = update_description
            {
                status.pending_invocations.retain(|invocation| {
                    invocation.manual_update_target_revision != Some(target_revision)
                });
            };

            status
        })
    }

    pub fn build(self) -> TestCase {
        TestCase {
            owned_agent_id: self.owned_agent_id,
            entries: self
                .entries
                .into_iter()
                .map(|entry| entry.rounded())
                .collect(),
            read_starts: Arc::new(std::sync::Mutex::new(Vec::new())),
            raw_payloads: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }
}

#[derive(Debug, Clone)]
struct TestEntry {
    oplog_entry: OplogEntry,
    expected_status: AgentStatusRecord,
}

impl TestEntry {
    pub fn rounded(self) -> Self {
        TestEntry {
            oplog_entry: self.oplog_entry.rounded(),
            expected_status: self.expected_status,
        }
    }
}

type RawPayloads = Vec<(PayloadId, Vec<u8>)>;

#[derive(Debug, Clone)]
struct TestCase {
    owned_agent_id: OwnedAgentId,
    entries: Vec<TestEntry>,
    /// Records the `start_idx` of every exact read so tests can assert which baseline
    /// a recompute folded from. Shared across `self.clone()`s handed out by `oplog_service()`.
    read_starts: Arc<std::sync::Mutex<Vec<u64>>>,
    raw_payloads: Arc<std::sync::Mutex<RawPayloads>>,
}

impl TestCase {
    pub fn builder(initial_component_version: u64) -> TestCaseBuilder {
        let environment_id = EnvironmentId::new();
        let account_id = AccountId::new();
        let owned_agent_id = OwnedAgentId::new(
            environment_id,
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "test-worker".to_string(),
            },
        );
        TestCaseBuilder::new(
            account_id,
            owned_agent_id,
            initial_component_version.try_into().unwrap(),
        )
    }
}

impl HasOplogService for TestCase {
    fn oplog_service(&self) -> Arc<dyn OplogService> {
        Arc::new(self.clone())
    }
}

#[async_trait]
impl OplogService for TestCase {
    fn set_stream_session_index(
        &self,
        _: Arc<crate::services::stream_session_index::StreamSessionIndexService>,
    ) {
        unreachable!("status-fold fixture does not open raw oplogs")
    }

    fn stream_session_index(
        &self,
    ) -> Option<Arc<crate::services::stream_session_index::StreamSessionIndexService>> {
        None
    }

    async fn create(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _initial_entry: OplogEntry,
        _initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog + 'static> {
        unreachable!()
    }

    async fn create_fresh(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _initial_entry: OplogEntry,
        _initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog + 'static> {
        unreachable!()
    }

    async fn open(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _last_oplog_index: Option<OplogIndex>,
        _initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog + 'static> {
        unreachable!()
    }

    async fn get_last_index(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
    ) -> OplogIndex {
        OplogIndex::from_u64(self.entries.len() as u64)
    }

    async fn delete(&self, _owned_agent_id: &OwnedAgentId, _agent_mode: AgentMode) {
        unreachable!()
    }

    async fn read_exact(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        let mut result = BTreeMap::new();
        let idx_u64: u64 = idx.into();
        self.read_starts.lock().unwrap().push(idx_u64);
        if n == 0 {
            return result;
        }
        let end = idx_u64
            .checked_add(n - 1)
            .unwrap_or_else(|| panic!("Invalid oplog range starting at {idx} with {n} entries"));
        for i in idx_u64..=end {
            let entry = i
                .checked_sub(1)
                .and_then(|offset| self.entries.get(offset as usize))
                .unwrap_or_else(|| {
                    panic!(
                        "Missing oplog entry in exact range [{idx}..={}]",
                        OplogIndex::from_u64(end)
                    )
                });
            result.insert(OplogIndex::from_u64(i), entry.oplog_entry.clone());
        }
        result
    }

    async fn exists(&self, _owned_agent_id: &OwnedAgentId, _agent_mode: AgentMode) -> bool {
        unreachable!()
    }

    async fn scan_for_component(
        &self,
        _environment_id: &EnvironmentId,
        _component_id: &ComponentId,
        _modes: Option<AgentMode>,
        _cursor: ScanCursor,
        _count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        unreachable!()
    }

    async fn upload_raw_payload(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        unreachable!()
    }

    async fn download_raw_payload(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        payload_id: PayloadId,
        _md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.raw_payloads
            .lock()
            .unwrap()
            .iter()
            .find(|(stored_id, _)| *stored_id == payload_id)
            .map(|(_, bytes)| bytes.clone())
            .ok_or_else(|| format!("missing raw payload {payload_id}"))
    }
}

impl HasConfig for TestCase {
    fn config(&self) -> Arc<GolemConfig> {
        let mut config = GolemConfig {
            retry: RetryConfig::default(),
            ..Default::default()
        };
        config.invocation_results.physical_index_catch_up_chunk_size = 2;
        Arc::new(config)
    }
}

impl HasComponentService for TestCase {
    fn component_service(&self) -> Arc<dyn ComponentService> {
        Arc::new(self.clone())
    }
}

#[async_trait]
impl ComponentService for TestCase {
    async fn get(
        &self,
        _engine: &wasmtime::Engine,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
    ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
        unreachable!()
    }

    async fn get_metadata(
        &self,
        _component_id: ComponentId,
        _forced_revision: Option<ComponentRevision>,
    ) -> Result<Component, WorkerExecutorError> {
        Err(WorkerExecutorError::unknown(
            "no component metadata available in worker status tests",
        ))
    }

    async fn resolve_component(
        &self,
        _component_reference: String,
        _resolving_environment: EnvironmentId,
        _resolving_application: ApplicationId,
        _resolving_account: AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError> {
        unreachable!()
    }

    async fn all_cached_metadata(&self) -> Vec<Component> {
        Vec::new()
    }

    async fn invalidate_all_metadata_for_environment(&self, _environment_id: EnvironmentId) {}
}

async fn run_test_case(test_case: TestCase) {
    let final_expected_status = test_case.entries.last().unwrap().expected_status.clone();

    for idx in 0..=test_case.entries.len() {
        let last_known_status = if idx == 0 {
            None
        } else {
            Some(test_case.entries[idx - 1].expected_status.clone())
        };
        let final_status = calculate_last_known_status_for_existing_worker(
            &test_case,
            &test_case.owned_agent_id,
            AgentMode::Durable,
            last_known_status,
        )
        .await
        .unwrap();

        assert_eq!(
            final_status, final_expected_status,
            "Calculating the last known status from oplog index {idx}"
        )
    }
}

#[test]
async fn cold_recompute_downloads_uncached_external_stream_session_payload() {
    let mut test_case = TestCase::builder(1).build();
    let session_key = StreamInvocationIdV1 {
        callee_environment_id: test_case.owned_agent_id.environment_id,
        callee: test_case.owned_agent_id.agent_id.clone(),
        callee_fingerprint: golem_common::model::AgentFingerprint(Uuid::new_v4()),
        idempotency_key: IdempotencyKey::fresh(),
    };
    let attachment_id = AttachmentId::primary(
        session_key.callee_environment_id,
        &session_key.callee,
        &session_key.idempotency_key,
    )
    .unwrap();
    let record = StreamSessionRecordV1::Prepared(StreamSessionPreparedRecordV1 {
        format_version: 1,
        attempt: StartAttemptDescriptorV1 {
            format_version: 1,
            session_key: session_key.clone(),
            attachment_id,
            expected_callee_fingerprint: session_key.callee_fingerprint,
            attempt_id: AttemptId::fresh(),
            invocation: PersistedStreamInvocationDescriptorV1 {
                format_version: 1,
                session_key: session_key.clone(),
                target_component_revision: ComponentRevision::INITIAL,
                method_name: "large-cold-status".to_string(),
                invocation_value: vec![7; 70 * 1024],
                stream_handles: Vec::new(),
                execution_config: Vec::new(),
                effective_identity: Vec::new(),
            },
            effective_identity: Vec::new(),
            live_join_buffer_events: 1,
        },
        stream_mappings: Vec::new(),
    });
    let bytes = golem_common::serialization::serialize(&record).unwrap();
    assert!(bytes.len() > 64 * 1024);
    let payload_id = PayloadId::new();
    test_case
        .raw_payloads
        .lock()
        .unwrap()
        .push((payload_id.clone(), bytes));
    test_case.entries.push(TestEntry {
        oplog_entry: OplogEntry::StreamSession {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            record: OplogPayload::External {
                payload_id,
                md5_hash: vec![0; 16],
                cached: None,
            },
        },
        expected_status: AgentStatusRecord::default(),
    });

    let status = calculate_last_known_status_for_existing_worker(
        &test_case,
        &test_case.owned_agent_id,
        AgentMode::Durable,
        None,
    )
    .await
    .unwrap();

    assert_eq!(
        status
            .durable_stream_sessions
            .get(&session_key.idempotency_key)
            .unwrap()
            .prepared,
        Some(OplogIndex::from_u64(test_case.entries.len() as u64))
    );
}

// --------------------------------------------------------------------------
// U2: Checkpoint state tracking — calculate_oplog_processor_checkpoints
// --------------------------------------------------------------------------

#[test]
fn checkpoint_latest_per_grant_id_wins() {
    let grant_id = EnvironmentPluginGrantId::new();
    let target = AgentId {
        component_id: ComponentId::new(),
        agent_id: "target-worker".to_string(),
    };
    let active_plugins = HashSet::from([grant_id]);
    let deleted_regions = DeletedRegions::default();

    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::OplogProcessorCheckpoint {
                timestamp: Timestamp::now_utc(),
                plugin_grant_id: grant_id,
                target_agent_id: target.clone(),
                confirmed_up_to: OplogIndex::from_u64(5),
                sending_up_to: OplogIndex::from_u64(10),
                last_batch_start: OplogIndex::NONE,
            },
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::OplogProcessorCheckpoint {
                timestamp: Timestamp::now_utc(),
                plugin_grant_id: grant_id,
                target_agent_id: target.clone(),
                confirmed_up_to: OplogIndex::from_u64(10),
                sending_up_to: OplogIndex::from_u64(20),
                last_batch_start: OplogIndex::NONE,
            },
        ),
    ]);

    let result = calculate_oplog_processor_checkpoints(
        HashMap::new(),
        &active_plugins,
        &deleted_regions,
        &entries,
        true,
    );

    assert_eq!(result.len(), 1);
    let state = result.get(&grant_id).unwrap();
    assert_eq!(state.confirmed_up_to, OplogIndex::from_u64(10));
    assert_eq!(state.sending_up_to, OplogIndex::from_u64(20));
    assert_eq!(state.target_agent_id, Some(target));
}

#[test]
fn checkpoint_different_plugins_tracked_independently() {
    let grant_id_a = EnvironmentPluginGrantId::new();
    let grant_id_b = EnvironmentPluginGrantId::new();
    let target_a = AgentId {
        component_id: ComponentId::new(),
        agent_id: "target-a".to_string(),
    };
    let target_b = AgentId {
        component_id: ComponentId::new(),
        agent_id: "target-b".to_string(),
    };
    let active_plugins = HashSet::from([grant_id_a, grant_id_b]);
    let deleted_regions = DeletedRegions::default();

    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::OplogProcessorCheckpoint {
                timestamp: Timestamp::now_utc(),
                plugin_grant_id: grant_id_a,
                target_agent_id: target_a.clone(),
                confirmed_up_to: OplogIndex::from_u64(5),
                sending_up_to: OplogIndex::from_u64(10),
                last_batch_start: OplogIndex::NONE,
            },
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::OplogProcessorCheckpoint {
                timestamp: Timestamp::now_utc(),
                plugin_grant_id: grant_id_b,
                target_agent_id: target_b.clone(),
                confirmed_up_to: OplogIndex::from_u64(3),
                sending_up_to: OplogIndex::from_u64(7),
                last_batch_start: OplogIndex::NONE,
            },
        ),
    ]);

    let result = calculate_oplog_processor_checkpoints(
        HashMap::new(),
        &active_plugins,
        &deleted_regions,
        &entries,
        true,
    );

    assert_eq!(result.len(), 2);

    let state_a = result.get(&grant_id_a).unwrap();
    assert_eq!(state_a.confirmed_up_to, OplogIndex::from_u64(5));
    assert_eq!(state_a.target_agent_id, Some(target_a));

    let state_b = result.get(&grant_id_b).unwrap();
    assert_eq!(state_b.confirmed_up_to, OplogIndex::from_u64(3));
    assert_eq!(state_b.target_agent_id, Some(target_b));
}

#[test]
fn checkpoint_target_agent_id_preserved() {
    let grant_id = EnvironmentPluginGrantId::new();
    let target = AgentId {
        component_id: ComponentId::new(),
        agent_id: "specific-target".to_string(),
    };
    let active_plugins = HashSet::from([grant_id]);
    let deleted_regions = DeletedRegions::default();

    let entries = BTreeMap::from([(
        OplogIndex::from_u64(1),
        OplogEntry::OplogProcessorCheckpoint {
            timestamp: Timestamp::now_utc(),
            plugin_grant_id: grant_id,
            target_agent_id: target.clone(),
            confirmed_up_to: OplogIndex::from_u64(5),
            sending_up_to: OplogIndex::from_u64(5),
            last_batch_start: OplogIndex::NONE,
        },
    )]);

    let result = calculate_oplog_processor_checkpoints(
        HashMap::new(),
        &active_plugins,
        &deleted_regions,
        &entries,
        true,
    );

    let state = result.get(&grant_id).unwrap();
    assert_eq!(state.target_agent_id, Some(target));
}

// --------------------------------------------------------------------------
// U3: Checkpoint cleanup — deactivated plugins evicted, in-flight retained
// --------------------------------------------------------------------------

#[test]
fn deactivated_plugin_evicted_from_checkpoints() {
    let grant_id = EnvironmentPluginGrantId::new();
    // Plugin is no longer active
    let active_plugins = HashSet::new();
    let deleted_regions = DeletedRegions::default();

    // Pre-seed with a checkpoint that is fully confirmed (not in-flight)
    let initial = HashMap::from([(
        grant_id,
        OplogProcessorCheckpointState {
            target_agent_id: Some(AgentId {
                component_id: ComponentId::new(),
                agent_id: "old-target".to_string(),
            }),
            confirmed_up_to: OplogIndex::from_u64(10),
            sending_up_to: OplogIndex::from_u64(10),
            last_batch_start: OplogIndex::NONE,
        },
    )]);

    let entries = BTreeMap::new();
    let result = calculate_oplog_processor_checkpoints(
        initial,
        &active_plugins,
        &deleted_regions,
        &entries,
        true,
    );

    assert!(
        !result.contains_key(&grant_id),
        "Deactivated plugin with no in-flight batch should be evicted"
    );
}

#[test]
fn deactivated_plugin_retained_when_in_flight() {
    let grant_id = EnvironmentPluginGrantId::new();
    // Plugin is no longer active
    let active_plugins = HashSet::new();
    let deleted_regions = DeletedRegions::default();

    // Pre-seed with a checkpoint that has in-flight batch (sending_up_to > confirmed_up_to)
    let initial = HashMap::from([(
        grant_id,
        OplogProcessorCheckpointState {
            target_agent_id: Some(AgentId {
                component_id: ComponentId::new(),
                agent_id: "target".to_string(),
            }),
            confirmed_up_to: OplogIndex::from_u64(5),
            sending_up_to: OplogIndex::from_u64(15),
            last_batch_start: OplogIndex::NONE,
        },
    )]);

    let entries = BTreeMap::new();
    let result = calculate_oplog_processor_checkpoints(
        initial,
        &active_plugins,
        &deleted_regions,
        &entries,
        true,
    );

    assert!(
        result.contains_key(&grant_id),
        "Deactivated plugin with in-flight batch should be retained"
    );
    let state = result.get(&grant_id).unwrap();
    assert_eq!(state.confirmed_up_to, OplogIndex::from_u64(5));
    assert_eq!(state.sending_up_to, OplogIndex::from_u64(15));
}

#[test]
fn successful_update_drops_old_grant_retains_new() {
    let old_grant = EnvironmentPluginGrantId::new();
    let new_grant = EnvironmentPluginGrantId::new();
    let deleted_regions = DeletedRegions::default();
    // After SuccessfulUpdate, only new_grant is active
    let active_plugins = HashSet::from([new_grant]);

    // Pre-seed old checkpoint
    let initial = HashMap::from([(
        old_grant,
        OplogProcessorCheckpointState {
            target_agent_id: Some(AgentId {
                component_id: ComponentId::new(),
                agent_id: "old".to_string(),
            }),
            confirmed_up_to: OplogIndex::from_u64(10),
            sending_up_to: OplogIndex::from_u64(10),
            last_batch_start: OplogIndex::NONE,
        },
    )]);

    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(11),
            OplogEntry::SuccessfulUpdate {
                timestamp: Timestamp::now_utc(),
                target_revision: ComponentRevision::new(2).unwrap(),
                new_component_size: 200,
                new_total_linear_memory_size: None,
                new_active_plugins: HashSet::from([new_grant]),
            },
        ),
        (
            OplogIndex::from_u64(12),
            OplogEntry::OplogProcessorCheckpoint {
                timestamp: Timestamp::now_utc(),
                plugin_grant_id: new_grant,
                target_agent_id: AgentId {
                    component_id: ComponentId::new(),
                    agent_id: "new-target".to_string(),
                },
                confirmed_up_to: OplogIndex::from_u64(12),
                sending_up_to: OplogIndex::from_u64(12),
                last_batch_start: OplogIndex::NONE,
            },
        ),
    ]);

    let result = calculate_oplog_processor_checkpoints(
        initial,
        &active_plugins,
        &deleted_regions,
        &entries,
        true,
    );

    assert!(
        !result.contains_key(&old_grant),
        "Old grant should be dropped after SuccessfulUpdate with different active set"
    );
    assert!(
        result.contains_key(&new_grant),
        "New grant should be present"
    );
}

// --------------------------------------------------------------------------
// U4: Activation initialization — ActivatePlugin seeds checkpoint
// --------------------------------------------------------------------------

#[test]
fn activate_plugin_initializes_checkpoint() {
    let grant_id = EnvironmentPluginGrantId::new();
    let active_plugins = HashSet::from([grant_id]);
    let deleted_regions = DeletedRegions::default();

    let activation_index = OplogIndex::from_u64(5);
    let entries = BTreeMap::from([(
        activation_index,
        OplogEntry::ActivatePlugin {
            timestamp: Timestamp::now_utc(),
            plugin_grant_id: grant_id,
        },
    )]);

    let result = calculate_oplog_processor_checkpoints(
        HashMap::new(),
        &active_plugins,
        &deleted_regions,
        &entries,
        true,
    );

    assert_eq!(result.len(), 1);
    let state = result.get(&grant_id).unwrap();
    assert_eq!(
        state.confirmed_up_to, activation_index,
        "confirmed_up_to should be set to the activation index"
    );
    assert_eq!(
        state.sending_up_to, activation_index,
        "sending_up_to should be set to the activation index"
    );
    assert_eq!(
        state.target_agent_id, None,
        "target_agent_id should be None for freshly activated plugin"
    );
}

#[test]
fn activate_plugin_does_not_overwrite_existing_checkpoint() {
    let grant_id = EnvironmentPluginGrantId::new();
    let target = AgentId {
        component_id: ComponentId::new(),
        agent_id: "existing-target".to_string(),
    };
    let active_plugins = HashSet::from([grant_id]);
    let deleted_regions = DeletedRegions::default();

    // Pre-seed with an existing checkpoint
    let initial = HashMap::from([(
        grant_id,
        OplogProcessorCheckpointState {
            target_agent_id: Some(target.clone()),
            confirmed_up_to: OplogIndex::from_u64(3),
            sending_up_to: OplogIndex::from_u64(8),
            last_batch_start: OplogIndex::NONE,
        },
    )]);

    let entries = BTreeMap::from([(
        OplogIndex::from_u64(10),
        OplogEntry::ActivatePlugin {
            timestamp: Timestamp::now_utc(),
            plugin_grant_id: grant_id,
        },
    )]);

    let result = calculate_oplog_processor_checkpoints(
        initial,
        &active_plugins,
        &deleted_regions,
        &entries,
        true,
    );

    let state = result.get(&grant_id).unwrap();
    assert_eq!(
        state.target_agent_id,
        Some(target),
        "ActivatePlugin should not overwrite existing checkpoint (or_insert semantics)"
    );
    assert_eq!(state.confirmed_up_to, OplogIndex::from_u64(3));
    assert_eq!(state.sending_up_to, OplogIndex::from_u64(8));
}

#[test]
fn oplog_processor_checkpoint_fold_is_chunk_composable() {
    fn fold(entries: &BTreeMap<OplogIndex, OplogEntry>, chunk_size: usize) -> AgentStatusRecord {
        let mut status = AgentStatusRecord::default();
        let last_index = *entries.keys().next_back().unwrap();
        let all_entries: Vec<_> = entries.iter().collect();
        for chunk in all_entries.chunks(chunk_size) {
            let chunk: BTreeMap<_, _> = chunk
                .iter()
                .map(|(index, entry)| (**index, (**entry).clone()))
                .collect();
            let finalize = chunk.keys().next_back() == Some(&last_index);
            status = super::update_status_with_precomputed_regions(
                AgentMode::Durable,
                status,
                chunk,
                &RetryConfig::default(),
                DeletedRegions::new(),
                DeletedRegions::new(),
                finalize,
            )
            .unwrap();
        }
        status
    }

    let grant_id = EnvironmentPluginGrantId::new();
    let target = AgentId {
        component_id: ComponentId::new(),
        agent_id: "checkpoint-target".to_string(),
    };
    let test_case = TestCase::builder(0).build();
    let mut create = test_case.entries[0].oplog_entry.clone();
    let OplogEntry::Create {
        initial_active_plugins,
        ..
    } = &mut create
    else {
        unreachable!()
    };
    initial_active_plugins.insert(grant_id);

    let entries = BTreeMap::from([
        (OplogIndex::INITIAL, create),
        (
            OplogIndex::from_u64(2),
            OplogEntry::OplogProcessorCheckpoint {
                timestamp: Timestamp::now_utc(),
                plugin_grant_id: grant_id,
                target_agent_id: target.clone(),
                confirmed_up_to: OplogIndex::INITIAL,
                sending_up_to: OplogIndex::from_u64(4),
                last_batch_start: OplogIndex::INITIAL,
            },
        ),
        (
            OplogIndex::from_u64(3),
            OplogEntry::DeactivatePlugin {
                timestamp: Timestamp::now_utc(),
                plugin_grant_id: grant_id,
            },
        ),
        (
            OplogIndex::from_u64(4),
            OplogEntry::OplogProcessorCheckpoint {
                timestamp: Timestamp::now_utc(),
                plugin_grant_id: grant_id,
                target_agent_id: target.clone(),
                confirmed_up_to: OplogIndex::from_u64(4),
                sending_up_to: OplogIndex::from_u64(4),
                last_batch_start: OplogIndex::INITIAL,
            },
        ),
        (
            OplogIndex::from_u64(5),
            OplogEntry::ActivatePlugin {
                timestamp: Timestamp::now_utc(),
                plugin_grant_id: grant_id,
            },
        ),
    ]);

    let unchunked = fold(&entries, entries.len());
    assert_eq!(fold(&entries, 1), unchunked);
    assert_eq!(fold(&entries, 2), unchunked);
    let checkpoint = unchunked
        .oplog_processor_checkpoints
        .get(&grant_id)
        .unwrap();
    assert_eq!(checkpoint.target_agent_id, Some(target));
    assert_eq!(checkpoint.confirmed_up_to, OplogIndex::from_u64(4));
}

#[test]
fn deactivate_then_reactivate_seeds_new_checkpoint() {
    let grant_id = EnvironmentPluginGrantId::new();
    let active_plugins = HashSet::from([grant_id]);
    let deleted_regions = DeletedRegions::default();

    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(3),
            OplogEntry::ActivatePlugin {
                timestamp: Timestamp::now_utc(),
                plugin_grant_id: grant_id,
            },
        ),
        (
            OplogIndex::from_u64(7),
            OplogEntry::DeactivatePlugin {
                timestamp: Timestamp::now_utc(),
                plugin_grant_id: grant_id,
            },
        ),
        (
            OplogIndex::from_u64(10),
            OplogEntry::ActivatePlugin {
                timestamp: Timestamp::now_utc(),
                plugin_grant_id: grant_id,
            },
        ),
    ]);

    let result = calculate_oplog_processor_checkpoints(
        HashMap::new(),
        &active_plugins,
        &deleted_regions,
        &entries,
        true,
    );

    let state = result.get(&grant_id).unwrap();
    assert_eq!(
        state.confirmed_up_to,
        OplogIndex::from_u64(10),
        "After deactivate+reactivate, checkpoint should be seeded at new activation index"
    );
    assert_eq!(state.sending_up_to, OplogIndex::from_u64(10));
    assert_eq!(state.target_agent_id, None);
}

#[test]
fn card_revoked_entry_is_recorded_in_status() {
    let card_id = golem_common::model::card::CardId::new();
    let entries = BTreeMap::from([(
        OplogIndex::from_u64(1),
        OplogEntry::CardRevoked {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            queued_event_index: OplogIndex::from_u64(1),
            card_id,
            wallet_generation: None,
        },
    )]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert!(status.revoked_cards.contains(&card_id));
}

fn test_card(card_id: golem_common::model::card::CardId) -> golem_common::model::card::Card {
    golem_common::model::card::Card {
        card_id,
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: chrono::Utc::now(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    }
}

#[test]
fn card_event_queued_revoke_is_pending() {
    let card_id = golem_common::model::card::CardId::new();
    let entries = BTreeMap::from([(
        OplogIndex::from_u64(1),
        OplogEntry::card_event_queued(None, QueuedCardEvent::revoke(card_id)),
    )]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert_eq!(status.pending_card_events.len(), 1);
    assert_eq!(
        status.pending_card_events[0].oplog_index,
        OplogIndex::from_u64(1)
    );
    assert_eq!(
        status.pending_card_events[0].event,
        QueuedCardEvent::revoke(card_id)
    );
}

#[test]
fn card_revoked_removes_pending_revoke_and_records_revoked_card() {
    let card_id = golem_common::model::card::CardId::new();
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(None, QueuedCardEvent::revoke(card_id)),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_revoked(None, OplogIndex::from_u64(1), card_id, None),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert!(status.pending_card_events.is_empty());
    assert!(status.revoked_cards.contains(&card_id));
}

#[test]
fn card_revoked_cascade_records_every_revoked_card() {
    let first_card_id = golem_common::model::card::CardId::new();
    let second_card_id = golem_common::model::card::CardId::new();
    let entries = BTreeMap::from([(
        OplogIndex::from_u64(1),
        OplogEntry::CardRevokedCascade {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            revoked_card_ids: vec![first_card_id, second_card_id],
            affected_wallets: Vec::new(),
            local_wallet_generation: None,
        },
    )]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert!(status.revoked_cards.contains(&first_card_id));
    assert!(status.revoked_cards.contains(&second_card_id));
}

#[test]
fn installing_reused_card_id_clears_prior_revocation_status() {
    let card_id = golem_common::model::card::CardId::new();
    let revoked = BTreeMap::from([(
        OplogIndex::from_u64(1),
        OplogEntry::CardRevokedCascade {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            revoked_card_ids: vec![card_id],
            affected_wallets: Vec::new(),
            local_wallet_generation: Some(1),
        },
    )]);
    let status_after_revoke = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        revoked,
        &RetryConfig::default(),
    )
    .unwrap();
    assert!(status_after_revoke.revoked_cards.contains(&card_id));

    let installed = BTreeMap::from([(
        OplogIndex::from_u64(2),
        OplogEntry::card_installed(None, None, test_card(card_id).into(), Some(2)),
    )]);
    let status_after_install = update_status_with_new_entries(
        AgentMode::Durable,
        status_after_revoke,
        installed,
        &RetryConfig::default(),
    )
    .unwrap();

    assert!(
        !status_after_install.revoked_cards.contains(&card_id),
        "a successful installation of a new live card incarnation must permit a later revocation to be queued for that card ID"
    );
}

#[test]
fn card_revoked_cascade_completes_every_matching_pending_revoke() {
    let first_card_id = golem_common::model::card::CardId::new();
    let second_card_id = golem_common::model::card::CardId::new();
    let unrelated_card_id = golem_common::model::card::CardId::new();
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(None, QueuedCardEvent::revoke(first_card_id)),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_event_queued(None, QueuedCardEvent::revoke(second_card_id)),
        ),
        (
            OplogIndex::from_u64(3),
            OplogEntry::card_event_queued(None, QueuedCardEvent::revoke(unrelated_card_id)),
        ),
        (
            OplogIndex::from_u64(4),
            OplogEntry::CardRevokedCascade {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                revoked_card_ids: vec![first_card_id, second_card_id],
                affected_wallets: Vec::new(),
                local_wallet_generation: Some(1),
            },
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert_eq!(status.pending_card_events.len(), 1);
    assert_eq!(
        status.pending_card_events[0].event,
        QueuedCardEvent::revoke(unrelated_card_id)
    );
    assert!(status.revoked_cards.contains(&first_card_id));
    assert!(status.revoked_cards.contains(&second_card_id));
}

#[test]
fn reverted_card_revoked_cascade_still_records_revoked_cards() {
    let card_id = golem_common::model::card::CardId::new();
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::CardRevokedCascade {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                revoked_card_ids: vec![card_id],
                affected_wallets: Vec::new(),
                local_wallet_generation: None,
            },
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::revert(OplogRegion {
                start: OplogIndex::from_u64(1),
                end: OplogIndex::from_u64(1),
            }),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert!(status.revoked_cards.contains(&card_id));
}

#[test]
fn card_transfer_confirmed_removes_only_the_matching_pending_transfer() {
    use golem_common::model::card::{AgentCardHolder, CardHolder};

    let transferred_card = test_card(golem_common::model::card::CardId::new());
    let pending_card = test_card(golem_common::model::card::CardId::new());
    let completed_transfer_id = uuid::Uuid::new_v4();
    let pending_transfer_id = uuid::Uuid::new_v4();
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: golem_common::model::AgentId {
            component_id: golem_common::model::component::ComponentId(uuid::Uuid::new_v4()),
            agent_id: "card-transfer-target".to_string(),
        },
    });
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(
                None,
                QueuedCardEvent::transfer_started(
                    completed_transfer_id,
                    transferred_card.clone(),
                    target_holder.clone(),
                ),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_event_queued(
                None,
                QueuedCardEvent::transfer_started(
                    pending_transfer_id,
                    pending_card.clone(),
                    target_holder.clone(),
                ),
            ),
        ),
        (
            OplogIndex::from_u64(3),
            OplogEntry::card_transfer_confirmed(
                None,
                completed_transfer_id,
                transferred_card.card_id,
                transferred_card.card_id,
                target_holder.clone(),
            ),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert_eq!(status.pending_card_events.len(), 1);
    assert_eq!(
        status.pending_card_events[0].oplog_index,
        OplogIndex::from_u64(2)
    );
    assert!(matches!(
        &status.pending_card_events[0].event,
        QueuedCardEvent::TransferStarted(event)
            if event.transfer_id == pending_transfer_id
                && event.card_id == pending_card.card_id
                && event.card.as_ref() == Some(&pending_card.clone().into())
                && event.target_holder == target_holder
    ));
}

#[test]
fn polymorphic_transfer_confirmation_matches_source_and_installed_child_ids() {
    use golem_common::model::card::{AgentCardHolder, CardHolder};

    let source_card_id = golem_common::model::card::CardId::new();
    let installed_child = test_card(golem_common::model::card::CardId::new());
    let transfer_id = uuid::Uuid::new_v4();
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: golem_common::model::AgentId {
            component_id: golem_common::model::component::ComponentId(uuid::Uuid::new_v4()),
            agent_id: "polymorphic-card-transfer-target".to_string(),
        },
    });
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(
                None,
                QueuedCardEvent::transfer_started_with_source(
                    transfer_id,
                    source_card_id,
                    installed_child.clone(),
                    target_holder.clone(),
                ),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_transfer_confirmed(
                None,
                transfer_id,
                source_card_id,
                installed_child.card_id,
                target_holder,
            ),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert!(status.pending_card_events.is_empty());
}

#[test]
fn polymorphic_transfer_confirmation_with_conflicting_source_keeps_pending_intent() {
    use golem_common::model::card::{AgentCardHolder, CardHolder};

    let source_card_id = golem_common::model::card::CardId::new();
    let installed_child = test_card(golem_common::model::card::CardId::new());
    let transfer_id = uuid::Uuid::new_v4();
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: golem_common::model::AgentId {
            component_id: golem_common::model::component::ComponentId(uuid::Uuid::new_v4()),
            agent_id: "polymorphic-card-transfer-target".to_string(),
        },
    });
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(
                None,
                QueuedCardEvent::transfer_started_with_source(
                    transfer_id,
                    source_card_id,
                    installed_child.clone(),
                    target_holder.clone(),
                ),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_transfer_confirmed(
                None,
                transfer_id,
                golem_common::model::card::CardId::new(),
                installed_child.card_id,
                target_holder,
            ),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert_eq!(status.pending_card_events.len(), 1);
}

#[test]
fn received_card_transfer_is_reconstructed_as_a_distinct_target_receipt() {
    let card = test_card(golem_common::model::card::CardId::new());
    let stored_card = golem_common::model::card::StoredCard::from(card.clone());
    let transfer_id = uuid::Uuid::new_v4();
    let source_card_id = golem_common::model::card::CardId::new();
    let entries = BTreeMap::from([(
        OplogIndex::from_u64(1),
        OplogEntry::card_event_queued(
            None,
            QueuedCardEvent::transfer_received(transfer_id, source_card_id, card.clone()),
        ),
    )]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert_eq!(status.pending_card_events.len(), 1);
    assert!(matches!(
        &status.pending_card_events[0].event,
        QueuedCardEvent::TransferReceived(receipt)
            if receipt.transfer_id == transfer_id
                && receipt.source_card_id == Some(source_card_id)
                && receipt.card_id == card.card_id
                && receipt.card.as_ref() == Some(&stored_card)
    ));
    assert!(matches!(
        status.received_card_transfers.get(&transfer_id),
        Some(ReceivedCardTransferState::Received {
            source_card_id: Some(recorded_source_card_id),
            card: recorded_card,
        }) if *recorded_source_card_id == source_card_id && recorded_card == &stored_card
    ));
}

#[test]
fn received_card_transfer_index_refines_legacy_identity_and_detects_conflicts() {
    use golem_common::base_model::oplog::QueuedCardEventTransferReceived;

    let card = test_card(golem_common::model::card::CardId::new());
    let stored_card = golem_common::model::card::StoredCard::from(card);
    let transfer_id = uuid::Uuid::new_v4();
    let source_card_id = golem_common::model::card::CardId::new();
    let legacy_receipt = QueuedCardEvent::TransferReceived(QueuedCardEventTransferReceived {
        transfer_id,
        source_card_id: None,
        card_id: stored_card.card_id(),
        card: Some(stored_card.clone()),
    });

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        BTreeMap::from([(
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(None, legacy_receipt),
        )]),
        &RetryConfig::default(),
    )
    .unwrap();
    assert!(matches!(
        status.received_card_transfers.get(&transfer_id),
        Some(ReceivedCardTransferState::Received {
            source_card_id: None,
            card,
        }) if card == &stored_card
    ));

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        status,
        BTreeMap::from([(
            OplogIndex::from_u64(2),
            OplogEntry::card_event_queued(
                None,
                QueuedCardEvent::transfer_received(
                    transfer_id,
                    source_card_id,
                    stored_card.clone(),
                ),
            ),
        )]),
        &RetryConfig::default(),
    )
    .unwrap();
    assert!(matches!(
        status.received_card_transfers.get(&transfer_id),
        Some(ReceivedCardTransferState::Received {
            source_card_id: Some(recorded_source_card_id),
            card,
        }) if *recorded_source_card_id == source_card_id && card == &stored_card
    ));

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        status,
        BTreeMap::from([(
            OplogIndex::from_u64(3),
            OplogEntry::card_event_queued(
                None,
                QueuedCardEvent::transfer_received(
                    transfer_id,
                    golem_common::model::card::CardId::new(),
                    stored_card,
                ),
            ),
        )]),
        &RetryConfig::default(),
    )
    .unwrap();
    assert_eq!(
        status.received_card_transfers.get(&transfer_id),
        Some(&ReceivedCardTransferState::Conflict)
    );
}

#[test]
fn received_card_transfer_index_is_sticky_across_skipped_and_deleted_regions() {
    let card = test_card(golem_common::model::card::CardId::new());
    let source_card_id = golem_common::model::card::CardId::new();
    let skipped_transfer_id = uuid::Uuid::new_v4();
    let deleted_transfer_id = uuid::Uuid::new_v4();
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(
                None,
                QueuedCardEvent::transfer_received(
                    skipped_transfer_id,
                    source_card_id,
                    card.clone(),
                ),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::jump(
                None,
                OplogRegion {
                    start: OplogIndex::from_u64(1),
                    end: OplogIndex::from_u64(1),
                },
            ),
        ),
        (
            OplogIndex::from_u64(3),
            OplogEntry::card_event_queued(
                None,
                QueuedCardEvent::transfer_received(
                    deleted_transfer_id,
                    source_card_id,
                    card.clone(),
                ),
            ),
        ),
        (
            OplogIndex::from_u64(4),
            OplogEntry::revert(OplogRegion {
                start: OplogIndex::from_u64(3),
                end: OplogIndex::from_u64(3),
            }),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    for transfer_id in [skipped_transfer_id, deleted_transfer_id] {
        assert!(matches!(
            status.received_card_transfers.get(&transfer_id),
            Some(ReceivedCardTransferState::Received {
                source_card_id: Some(recorded_source_card_id),
                card: recorded_card,
            }) if *recorded_source_card_id == source_card_id
                && recorded_card == &card.clone().into()
        ));
    }
}

#[test]
fn target_card_transferred_clears_only_its_matching_receipt() {
    use golem_common::model::card::{AgentCardHolder, CardHolder};

    let card = test_card(golem_common::model::card::CardId::new());
    let source_card_id = golem_common::model::card::CardId::new();
    let transfer_id = uuid::Uuid::new_v4();
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: golem_common::model::AgentId {
            component_id: golem_common::model::component::ComponentId(uuid::Uuid::new_v4()),
            agent_id: "card-transfer-target".to_string(),
        },
    });
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(
                None,
                QueuedCardEvent::transfer_received(transfer_id, source_card_id, card.clone()),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_transferred(
                None,
                transfer_id,
                Some(source_card_id),
                card.card_id,
                target_holder,
                card.into(),
                Some(7),
            ),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert!(status.pending_card_events.is_empty());
}

#[test]
fn legacy_target_card_transferred_clears_its_matching_receipt() {
    use golem_common::model::card::{AgentCardHolder, CardHolder};

    let card = test_card(golem_common::model::card::CardId::new());
    let source_card_id = golem_common::model::card::CardId::new();
    let transfer_id = uuid::Uuid::new_v4();
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: golem_common::model::AgentId {
            component_id: golem_common::model::component::ComponentId(uuid::Uuid::new_v4()),
            agent_id: "card-transfer-target".to_string(),
        },
    });
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(
                None,
                QueuedCardEvent::transfer_received(transfer_id, source_card_id, card.clone()),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_transferred(
                None,
                transfer_id,
                None,
                card.card_id,
                target_holder,
                card.into(),
                None,
            ),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert!(status.pending_card_events.is_empty());
}

#[test]
fn target_card_transferred_with_conflicting_source_keeps_the_receipt() {
    use golem_common::model::card::{AgentCardHolder, CardHolder};

    let card = test_card(golem_common::model::card::CardId::new());
    let source_card_id = golem_common::model::card::CardId::new();
    let transfer_id = uuid::Uuid::new_v4();
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: golem_common::model::AgentId {
            component_id: golem_common::model::component::ComponentId(uuid::Uuid::new_v4()),
            agent_id: "card-transfer-target".to_string(),
        },
    });
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(
                None,
                QueuedCardEvent::transfer_received(transfer_id, source_card_id, card.clone()),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_transferred(
                None,
                transfer_id,
                Some(golem_common::model::card::CardId::new()),
                card.card_id,
                target_holder,
                card.clone().into(),
                Some(7),
            ),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert_eq!(status.pending_card_events.len(), 1);
    assert!(matches!(
        &status.pending_card_events[0].event,
        QueuedCardEvent::TransferReceived(receipt)
            if receipt.transfer_id == transfer_id
                && receipt.source_card_id == Some(source_card_id)
                && receipt.card.as_ref() == Some(&card.into())
    ));
}

#[test]
fn card_transfer_confirmed_with_conflicting_source_card_keeps_pending_transfer() {
    use golem_common::model::card::{AgentCardHolder, CardHolder};

    let pending_card = test_card(golem_common::model::card::CardId::new());
    let conflicting_card = test_card(golem_common::model::card::CardId::new());
    let transfer_id = uuid::Uuid::new_v4();
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: golem_common::model::AgentId {
            component_id: golem_common::model::component::ComponentId(uuid::Uuid::new_v4()),
            agent_id: "card-transfer-target".to_string(),
        },
    });
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(
                None,
                QueuedCardEvent::transfer_started(
                    transfer_id,
                    pending_card.clone(),
                    target_holder.clone(),
                ),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_transfer_confirmed(
                None,
                transfer_id,
                conflicting_card.card_id,
                conflicting_card.card_id,
                target_holder,
            ),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert_eq!(status.pending_card_events.len(), 1);
    assert!(matches!(
        &status.pending_card_events[0].event,
        QueuedCardEvent::TransferStarted(event)
            if event.transfer_id == transfer_id
                && event.card_id == pending_card.card_id
                && event.card.as_ref() == Some(&pending_card.clone().into())
    ));
}

#[test]
fn card_install_failure_does_not_clear_pending_transfer() {
    use golem_common::model::card::{AgentCardHolder, CardHolder};

    let card = test_card(golem_common::model::card::CardId::new());
    let transfer_id = uuid::Uuid::new_v4();
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: golem_common::model::AgentId {
            component_id: golem_common::model::component::ComponentId(uuid::Uuid::new_v4()),
            agent_id: "card-transfer-target".to_string(),
        },
    });
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(
                None,
                QueuedCardEvent::transfer_started(transfer_id, card.clone(), target_holder.clone()),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_install_failed(
                None,
                OplogIndex::from_u64(1),
                card.card_id,
                golem_common::base_model::oplog::CardInstallFailure::NotFound,
            ),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert_eq!(status.pending_card_events.len(), 1);
    assert_eq!(
        status.pending_card_events[0].oplog_index,
        OplogIndex::from_u64(1)
    );
    assert!(matches!(
        &status.pending_card_events[0].event,
        QueuedCardEvent::TransferStarted(event)
            if event.transfer_id == transfer_id
                && event.card_id == card.card_id
                && event.card.as_ref() == Some(&card.clone().into())
                && event.target_holder == target_holder
    ));
}

#[test]
fn target_card_transferred_does_not_clear_source_pending_transfer() {
    use golem_common::model::card::{AgentCardHolder, CardHolder};

    let card = test_card(golem_common::model::card::CardId::new());
    let transfer_id = uuid::Uuid::new_v4();
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: golem_common::model::AgentId {
            component_id: golem_common::model::component::ComponentId(uuid::Uuid::new_v4()),
            agent_id: "card-transfer-target".to_string(),
        },
    });
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(
                None,
                QueuedCardEvent::transfer_started(transfer_id, card.clone(), target_holder.clone()),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_transferred(
                None,
                transfer_id,
                Some(card.card_id),
                card.card_id,
                target_holder,
                card.clone().into(),
                None,
            ),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert_eq!(status.pending_card_events.len(), 1);
    assert!(matches!(
        &status.pending_card_events[0].event,
        QueuedCardEvent::TransferStarted(event)
            if event.transfer_id == transfer_id
                && event.card_id == card.card_id
                && event.card.as_ref() == Some(&card.clone().into())
    ));
}

#[test]
fn reverted_card_transfer_confirmation_still_closes_pending_transfer() {
    use golem_common::model::card::{AgentCardHolder, CardHolder};

    let card = test_card(golem_common::model::card::CardId::new());
    let transfer_id = uuid::Uuid::new_v4();
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: golem_common::model::AgentId {
            component_id: golem_common::model::component::ComponentId(uuid::Uuid::new_v4()),
            agent_id: "card-transfer-target".to_string(),
        },
    });
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(
                None,
                QueuedCardEvent::transfer_started(transfer_id, card.clone(), target_holder.clone()),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_transfer_confirmed(
                None,
                transfer_id,
                card.card_id,
                card.card_id,
                target_holder,
            ),
        ),
        (
            OplogIndex::from_u64(3),
            OplogEntry::revert(OplogRegion {
                start: OplogIndex::from_u64(2),
                end: OplogIndex::from_u64(2),
            }),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert!(status.pending_card_events.is_empty());
}

#[test]
fn card_revoked_removes_only_matching_pending_revoke_index() {
    let card_id = golem_common::model::card::CardId::new();
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(None, QueuedCardEvent::revoke(card_id)),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_event_queued(None, QueuedCardEvent::revoke(card_id)),
        ),
        (
            OplogIndex::from_u64(3),
            OplogEntry::card_revoked(None, OplogIndex::from_u64(1), card_id, None),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert_eq!(status.pending_card_events.len(), 1);
    assert_eq!(
        status.pending_card_events[0].oplog_index,
        OplogIndex::from_u64(2)
    );
    assert!(status.revoked_cards.contains(&card_id));
}

#[test]
fn card_event_queued_install_is_pending() {
    let card_id = golem_common::model::card::CardId::new();
    let card = test_card(card_id);
    let entries = BTreeMap::from([(
        OplogIndex::from_u64(1),
        OplogEntry::card_event_queued(
            Some(OplogIndex::from_u64(42)),
            QueuedCardEvent::install(card.clone()),
        ),
    )]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert_eq!(status.pending_card_events.len(), 1);
    assert_eq!(
        status.pending_card_events[0].entity_parent_start_index,
        Some(OplogIndex::from_u64(42))
    );
    assert_eq!(
        status.pending_card_events[0].event,
        QueuedCardEvent::install(card)
    );
}

#[test]
fn card_installed_removes_pending_install() {
    let card_id = golem_common::model::card::CardId::new();
    let card = test_card(card_id);
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(None, QueuedCardEvent::install(card.clone())),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_installed(None, Some(OplogIndex::from_u64(1)), card.into(), None),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert!(status.pending_card_events.is_empty());
}

#[test]
fn card_install_failed_removes_pending_install() {
    let card_id = golem_common::model::card::CardId::new();
    let card = test_card(card_id);
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(None, QueuedCardEvent::install(card)),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_install_failed(
                None,
                OplogIndex::from_u64(1),
                card_id,
                CardInstallFailure::CardRevoked,
            ),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert!(status.pending_card_events.is_empty());
}

#[test]
fn queued_card_event_survives_deleted_region_without_terminal() {
    let card_id = golem_common::model::card::CardId::new();
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_event_queued(None, QueuedCardEvent::revoke(card_id)),
        ),
        (
            OplogIndex::from_u64(3),
            OplogEntry::revert(OplogRegion {
                start: OplogIndex::from_u64(2),
                end: OplogIndex::from_u64(2),
            }),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert_eq!(status.pending_card_events.len(), 1);
}

#[test]
fn terminal_card_event_in_deleted_region_cleans_pending_event() {
    let card_id = golem_common::model::card::CardId::new();
    let card = test_card(card_id);
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(None, QueuedCardEvent::install(card.clone())),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_installed(None, Some(OplogIndex::from_u64(1)), card.into(), None),
        ),
        (
            OplogIndex::from_u64(3),
            OplogEntry::revert(OplogRegion {
                start: OplogIndex::from_u64(2),
                end: OplogIndex::from_u64(2),
            }),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert!(status.pending_card_events.is_empty());
}

#[test]
fn card_revoked_entry_is_recorded_even_in_deleted_region() {
    let card_id = golem_common::model::card::CardId::new();
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(2),
            OplogEntry::CardRevoked {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                queued_event_index: OplogIndex::from_u64(1),
                card_id,
                wallet_generation: None,
            },
        ),
        (
            OplogIndex::from_u64(3),
            OplogEntry::revert(OplogRegion {
                start: OplogIndex::from_u64(2),
                end: OplogIndex::from_u64(2),
            }),
        ),
    ]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert!(
        status
            .deleted_regions
            .is_in_deleted_region(OplogIndex::from_u64(2))
    );
    assert!(status.revoked_cards.contains(&card_id));
}
