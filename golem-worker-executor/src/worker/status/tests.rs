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
use crate::worker::snapshot_selection::names_in_use;
use crate::worker::status::{RegionFold, fold_regions_from};
use crate::worker::status::{
    StatusOplogReader, calculate_last_known_status,
    calculate_last_known_status_for_existing_worker,
    calculate_last_known_status_with_checkpoint_reader, calculate_latest_worker_status,
    calculate_oplog_processor_checkpoints, calculate_status_with_reader,
    calculate_total_linear_memory_size, fold_committed_status, fold_invocation_result_entries,
    hydrate_initial_pending_evidence, try_fold_status_from, try_fold_status_from_reader,
};
use async_trait::async_trait;
use golem_common::base_model::OplogIndex;
use golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::base_model::oplog::{CardInstallFailure, QueuedCardEvent};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentMode, OwnerKind, Principal};
use golem_common::model::application::ApplicationId;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::durable_stream::{
    AttachmentId, AttemptId, PersistedStreamInvocationDescriptor, StartAttemptDescriptor,
    StreamExportFork, StreamExportForkAdmittedRecord, StreamExportForkCandidate,
    StreamForkCutRecord, StreamId, StreamInvocationId, StreamSessionAttachedRecord,
    StreamSessionExpiryPolicy, StreamSessionPreparedRecord, StreamSessionRecord,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::{InvocationContextStack, TraceId};
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    AgentError, AgentResourceId, DurableFunctionType, FailedSnapshotAssistedUpdateDetails,
    FilesystemSnapshotName, HostRequest, HostRequestNoInput, HostResponse, OplogEntry,
    OplogErrorKind, OplogPayload, PayloadId, RawOplogPayload, SnapshotAssistedUpdateDetails,
    UpdateDescription,
};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocation, AgentInvocationPayload, AgentInvocationResult,
    AgentMetadata, AgentResourceDescription, AgentStatus, AgentStatusRecord, AuthoritativeSnapshot,
    AuthoritativeSnapshotKind, AutomaticSnapshot, FailedUpdateRecord, IdempotencyKey,
    OplogProcessorCheckpointState, OwnedAgentId, PendingInvocationRef, PendingUpdateKind,
    PendingUpdateRef, ReceivedCardTransferState, RetryConfig, RetryPolicyState, ScanCursor,
    SnapshotFiles, SuccessfulUpdateRecord, Timestamp, UsableAutomaticSnapshot,
};
use golem_common::read_only_lock;
use golem_common::resource_runtime::ResourceTypeId;
use golem_common::schema::IntoTypedSchemaValue;
use golem_common::schema::SchemaValue;
use golem_service_base::error::worker_executor::WorkerExecutorError;

fn test_invocation_wallet_pin() -> golem_common::model::card::InvocationWalletPin {
    golem_common::model::card::InvocationWalletPin {
        wallet_token: golem_common::model::card::WalletVersionToken {
            wallet_id_hash: [0; 32],
            generation: 0,
        },
        pinned_card_ids: Vec::new(),
        scope_card_id: None,
    }
}

#[test]
async fn invalid_initial_pending_bounds_do_not_read_the_referent() {
    let test_case = TestCase::builder(0).build();
    let key = IdempotencyKey::fresh();
    let session_key = StreamInvocationId {
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
                session_key: Some(session_key.idempotency_key.clone()),
                prepared_attempt_id: Some(attempt),
                ..Default::default()
            },
        );
        let attached = StreamSessionRecord::Attached(StreamSessionAttachedRecord {
            format_version: 1,
            session_key: session_key.idempotency_key.clone(),
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
                summary: None,
            },
        )]);
        test_case.read_starts.lock().unwrap().clear();
        let reader = StatusOplogReader::new(
            &test_case,
            &test_case.owned_agent_id,
            AgentMode::Durable,
            None,
            OplogIndex::from_u64(13),
        );
        hydrate_initial_pending_evidence(&reader, &mut baseline, &entries)
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

#[test]
async fn fork_regions_use_the_pinned_horizon_before_later_reverts() {
    let test_case = TestCase::builder(0)
        .add(OplogEntry::no_op(None), |status| status)
        .add(
            OplogEntry::jump(
                None,
                OplogRegion {
                    start: OplogIndex::from_u64(2),
                    end: OplogIndex::from_u64(2),
                },
            ),
            |status| status,
        )
        .add(
            OplogEntry::revert(OplogRegion {
                start: OplogIndex::from_u64(3),
                end: OplogIndex::from_u64(3),
            }),
            |status| status,
        )
        .build();
    let before = super::skipped_regions_at(
        &test_case,
        &test_case.owned_agent_id,
        OplogIndex::from_u64(3),
    )
    .await
    .unwrap();
    let after = super::skipped_regions_at(
        &test_case,
        &test_case.owned_agent_id,
        OplogIndex::from_u64(4),
    )
    .await
    .unwrap();
    assert!(before.is_in_deleted_region(OplogIndex::from_u64(2)));
    assert!(!before.is_in_deleted_region(OplogIndex::from_u64(3)));
    assert!(!after.is_in_deleted_region(OplogIndex::from_u64(2)));
    assert!(after.is_in_deleted_region(OplogIndex::from_u64(3)));
}

fn update_status_with_new_entries(
    agent_mode: AgentMode,
    last_known: AgentStatusRecord,
    new_entries: BTreeMap<OplogIndex, OplogEntry>,
    default_retry_policy: &RetryConfig,
) -> Option<AgentStatusRecord> {
    super::update_status_with_new_entries(agent_mode, last_known, new_entries, default_retry_policy)
        .unwrap()
}

/// The region fold of `entries` from `baseline` and its pending updates.
fn fold_regions(
    baseline: &AgentStatusRecord,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> RegionFold {
    fold_regions_from(baseline, baseline.pending_updates.clone(), entries)
}

#[test]
fn cancellation_obligations_survive_status_checkpoint_without_local_prepared() {
    use golem_common::model::durable_stream::{
        LocalStreamId, StreamCancelReason, StreamCancelRole, StreamConsumerCancelAppliedRecord,
        StreamConsumerCancelIntentRecord, StreamRecordReference, StreamRegistrationInvocation,
    };
    let intent = StreamConsumerCancelIntentRecord {
        format_version: 1,
        session_key: StreamRegistrationInvocation::Remote(StreamInvocationId {
            callee_environment_id: EnvironmentId::new(),
            callee: AgentId {
                component_id: ComponentId::new(),
                agent_id: "remote".into(),
            },
            callee_fingerprint: golem_common::model::AgentFingerprint(Uuid::new_v4()),
            idempotency_key: IdempotencyKey::fresh(),
        }),
        consumer_invocation: IdempotencyKey::new("consumer".into()),
        source: StreamRecordReference::Local(LocalStreamId(OplogIndex::from_u64(21))),
        epoch: 7,
        role: StreamCancelRole::OutputConsumer,
        reason: StreamCancelReason::Cancelled,
        details: Some("cancel requested".into()),
    };
    let entry = |record| OplogEntry::StreamSession {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: None,
        record: OplogPayload::Inline(Box::new(record)),
        summary: None,
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
        StreamSessionRecord::ConsumerCancelIntent(intent.clone()),
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
        StreamSessionRecord::ConsumerCancelApplied(StreamConsumerCancelAppliedRecord {
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
        StreamSessionRecord::ConsumerCancelApplied(StreamConsumerCancelAppliedRecord {
            format_version: 1,
            intent,
        }),
    );
    assert!(applied.pending_durable_stream_cancellations.is_empty());
    assert!(applied.durable_stream_sessions.iter().next().is_none());
}

#[test]
fn stream_status_discards_reverted_sessions_and_cancellation_receipts_but_keeps_jumps() {
    use crate::services::worker_fork::lineage::tests::prepared;
    use golem_common::model::durable_stream::{
        LocalStreamId, StreamCancelReason, StreamCancelRole, StreamConsumerCancelAppliedRecord,
        StreamConsumerCancelIntentRecord, StreamRecordReference,
    };
    let first = crate::durable_host::durable_stream::tests::identity().invocation;
    let mut second = first.clone();
    second.idempotency_key = IdempotencyKey::fresh();
    let intent = StreamConsumerCancelIntentRecord {
        format_version: 1,
        session_key: golem_common::model::durable_stream::StreamRegistrationInvocation::Remote(
            first.clone(),
        ),
        consumer_invocation: IdempotencyKey::new("consumer".into()),
        source: StreamRecordReference::Local(LocalStreamId(OplogIndex::from_u64(21))),
        epoch: 7,
        role: StreamCancelRole::OutputConsumer,
        reason: StreamCancelReason::Cancelled,
        details: None,
    };
    let mut removed_intent = intent.clone();
    removed_intent.session_key =
        golem_common::model::durable_stream::StreamRegistrationInvocation::Remote(second.clone());
    removed_intent.source = StreamRecordReference::Local(LocalStreamId(OplogIndex::from_u64(22)));
    let entry = |record| OplogEntry::StreamSession {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: None,
        record: OplogPayload::Inline(Box::new(record)),
        summary: None,
    };
    for revert in [false, true] {
        let mut entries: BTreeMap<_, _> = [
            StreamSessionRecord::Prepared(prepared(&first)),
            StreamSessionRecord::ConsumerCancelIntent(intent.clone()),
            StreamSessionRecord::Prepared(prepared(&second)),
            StreamSessionRecord::ConsumerCancelApplied(StreamConsumerCancelAppliedRecord {
                intent: intent.clone(),
                format_version: 1,
            }),
            StreamSessionRecord::ConsumerCancelIntent(removed_intent.clone()),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, record)| (OplogIndex::from_u64(i as u64 + 2), entry(record)))
        .collect();
        let region = OplogRegion {
            start: OplogIndex::from_u64(4),
            end: OplogIndex::from_u64(6),
        };
        entries.insert(
            OplogIndex::from_u64(7),
            if revert {
                OplogEntry::revert(region)
            } else {
                OplogEntry::jump(None, region)
            },
        );
        let status = update_status_with_new_entries(
            AgentMode::Durable,
            AgentStatusRecord::default(),
            entries,
            &RetryConfig::default(),
        )
        .unwrap();
        assert!(
            status
                .durable_stream_sessions
                .get(&first.idempotency_key)
                .is_some()
        );
        assert_eq!(
            status
                .durable_stream_sessions
                .get(&second.idempotency_key)
                .is_none(),
            revert
        );
        assert_eq!(
            status.pending_durable_stream_cancellations,
            HashSet::from([if revert {
                intent.clone()
            } else {
                removed_intent.clone()
            }]),
        );
    }
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
                None,
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

fn export_owner_fingerprint() -> AgentFingerprint {
    AgentFingerprint(uuid::Uuid::from_u128(1))
}

fn export_admission_baseline() -> AgentStatusRecord {
    AgentStatusRecord {
        export_fork_admissions: golem_common::model::ExportForkAdmissions {
            owner_fingerprint: Some(export_owner_fingerprint()),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn export_fork_admission(target: AgentId, session: &str, updated_millis: u64) -> OplogEntry {
    let source = AgentId {
        component_id: target.component_id,
        agent_id: "source".into(),
    };
    OplogEntry::StreamSession {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: None,
        record: OplogPayload::Inline(Box::new(StreamSessionRecord::ExportForkAdmitted(
            StreamExportForkAdmittedRecord {
                format_version: 1,
                target: target.clone(),
                request_hash: vec![1; 32],
                candidate: StreamExportForkCandidate {
                    export: StreamExportFork {
                        source,
                        source_environment_id: EnvironmentId::new(),
                        source_fingerprint: export_owner_fingerprint(),
                        source_path: "/source".into(),
                        session: session.into(),
                        slot: "output".into(),
                        expected_method: "run".into(),
                        requested_offset: None,
                        anchor: None,
                        sub_offset: 0,
                        content_type: "application/octet-stream".into(),
                        initial_content_hash: vec![2; 32],
                        closed: false,
                        target_expiry_policy: StreamSessionExpiryPolicy::None,
                    },
                    horizon: OplogIndex::from_u64(20),
                    cut: OplogIndex::from_u64(10),
                    selected: StreamId(uuid::Uuid::new_v4()),
                    source_invocation: StreamInvocationId {
                        callee_environment_id: EnvironmentId::new(),
                        callee: target.clone(),
                        callee_fingerprint: export_owner_fingerprint(),
                        idempotency_key: IdempotencyKey::new(session.to_string()),
                    },
                    target_session_key: IdempotencyKey::new("target-session".into()),
                    expiry_policy: StreamSessionExpiryPolicy::None,
                    expiry_deadline_millis: None,
                    retained_through: None,
                    initial: None,
                },
                updated_millis,
                credit_millis: updated_millis + 100,
            },
        ))),
        summary: None,
    }
}

#[test]
fn export_fork_admission_fold_is_incremental_and_idempotent_per_target() {
    let target = AgentId {
        component_id: ComponentId::new(),
        agent_id: "target".into(),
    };
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(2),
            export_fork_admission(target.clone(), "s", 10),
        ),
        (
            OplogIndex::from_u64(3),
            export_fork_admission(target.clone(), "s", 20),
        ),
    ]);
    let rebuilt = update_status_with_new_entries(
        AgentMode::Durable,
        export_admission_baseline(),
        entries.clone(),
        &RetryConfig::default(),
    )
    .unwrap();
    let first = update_status_with_new_entries(
        AgentMode::Durable,
        export_admission_baseline(),
        BTreeMap::from([(
            OplogIndex::from_u64(2),
            entries[&OplogIndex::from_u64(2)].clone(),
        )]),
        &RetryConfig::default(),
    )
    .unwrap();
    let incremental = update_status_with_new_entries(
        AgentMode::Durable,
        first,
        BTreeMap::from([(
            OplogIndex::from_u64(3),
            entries[&OplogIndex::from_u64(3)].clone(),
        )]),
        &RetryConfig::default(),
    )
    .unwrap();
    assert_eq!(incremental, rebuilt);
    let all = rebuilt.export_fork_admissions;
    assert_eq!(all.session_counts.get("s"), Some(&1));
    assert_eq!(
        all.reservations[&target].oplog_index,
        OplogIndex::from_u64(3)
    );
    assert_eq!(all.credit_millis, Some(120));
}

#[test]
fn export_fork_admission_fold_accepts_serialized_inline_payload() {
    let target = AgentId {
        component_id: ComponentId::new(),
        agent_id: "target".into(),
    };
    let OplogEntry::StreamSession {
        timestamp,
        entity_parent_start_index,
        record: OplogPayload::Inline(record),
        ..
    } = export_fork_admission(target.clone(), "s", 10)
    else {
        unreachable!()
    };
    let bytes = golem_common::serialization::serialize(record.as_ref()).unwrap();
    let entries = BTreeMap::from([(
        OplogIndex::from_u64(2),
        OplogEntry::StreamSession {
            timestamp,
            entity_parent_start_index,
            record: OplogPayload::SerializedInline {
                bytes,
                cached: None,
            },
            summary: None,
        },
    )]);

    let rebuilt = update_status_with_new_entries(
        AgentMode::Durable,
        export_admission_baseline(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert!(
        rebuilt
            .export_fork_admissions
            .reservations
            .contains_key(&target)
    );
    assert_eq!(
        rebuilt.export_fork_admissions.session_counts.get("s"),
        Some(&1)
    );
}

#[test]
fn export_fork_admission_fold_accepts_cached_payloads_and_rejects_missing_external() {
    let target = AgentId {
        component_id: ComponentId::new(),
        agent_id: "target".into(),
    };
    let OplogEntry::StreamSession {
        record: OplogPayload::Inline(record),
        ..
    } = export_fork_admission(target.clone(), "s", 10)
    else {
        unreachable!()
    };
    let bytes = golem_common::serialization::serialize(record.as_ref()).unwrap();

    for payload in [
        OplogPayload::SerializedInline {
            bytes,
            cached: Some(Arc::new(record.as_ref().clone())),
        },
        OplogPayload::External {
            payload_id: PayloadId::new(),
            md5_hash: vec![0; 16],
            cached: Some(Arc::new(record.as_ref().clone())),
        },
    ] {
        let rebuilt = update_status_with_new_entries(
            AgentMode::Durable,
            export_admission_baseline(),
            BTreeMap::from([(
                OplogIndex::from_u64(2),
                OplogEntry::stream_session(None, payload, None),
            )]),
            &RetryConfig::default(),
        )
        .unwrap();
        assert!(
            rebuilt
                .export_fork_admissions
                .reservations
                .contains_key(&target)
        );
    }

    let error = super::update_status_with_new_entries(
        AgentMode::Durable,
        export_admission_baseline(),
        BTreeMap::from([(
            OplogIndex::from_u64(2),
            OplogEntry::stream_session(
                None,
                OplogPayload::External {
                    payload_id: PayloadId::new(),
                    md5_hash: vec![0; 16],
                    cached: None,
                },
                None,
            ),
        )]),
        &RetryConfig::default(),
    )
    .unwrap_err();
    assert_eq!(
        error,
        "durable stream session record payload has not been loaded"
    );
}

#[test]
fn export_fork_admission_fold_distinguishes_atomic_skip_revert_and_new_owner() {
    let target = AgentId {
        component_id: ComponentId::new(),
        agent_id: "target".into(),
    };
    let admitted = export_fork_admission(target.clone(), "s", 10);
    let region = OplogRegion::from_index_range(OplogIndex::from_u64(2)..=OplogIndex::from_u64(2));
    let retained = super::calculate_export_fork_admissions(
        export_admission_baseline().export_fork_admissions,
        &DeletedRegions::new(),
        &BTreeMap::from([
            (OplogIndex::from_u64(2), admitted.clone()),
            (
                OplogIndex::from_u64(3),
                OplogEntry::Jump {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    jump: region.clone(),
                },
            ),
        ]),
    )
    .unwrap();
    assert!(retained.reservations.contains_key(&target));

    let deleted = DeletedRegionsBuilder::from_regions(vec![region]).build();
    let reverted = super::calculate_export_fork_admissions(
        export_admission_baseline().export_fork_admissions,
        &deleted,
        &BTreeMap::from([(OplogIndex::from_u64(2), admitted)]),
    )
    .unwrap();
    assert!(reverted.reservations.is_empty());

    let cut = StreamForkCutRecord {
        format_version: 1,
        request_hash: vec![3; 32],
        creation_fingerprint: golem_common::model::AgentFingerprint(uuid::Uuid::new_v4()),
        export: None,
        cut_index: OplogIndex::from_u64(2),
        revert: None,
        epoch_floor: 1,
        selected_stream_id: None,
        retained_through: None,
    };
    let unchanged = super::calculate_export_fork_admissions(
        retained,
        &DeletedRegions::new(),
        &BTreeMap::from([(
            OplogIndex::from_u64(4),
            OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(StreamSessionRecord::ForkCut(cut))),
                summary: None,
            },
        )]),
    )
    .unwrap();
    assert!(unchanged.reservations.contains_key(&target));
}

#[test]
fn export_fork_admission_fold_ignores_ancestor_after_reverting_before_fork_cut() {
    let target = AgentId {
        component_id: ComponentId::new(),
        agent_id: "ancestor-target".into(),
    };
    let cut = StreamForkCutRecord {
        format_version: 1,
        request_hash: vec![3; 32],
        creation_fingerprint: golem_common::model::AgentFingerprint(uuid::Uuid::new_v4()),
        export: None,
        cut_index: OplogIndex::from_u64(2),
        revert: None,
        epoch_floor: 1,
        selected_stream_id: None,
        retained_through: None,
    };
    let mut create = TestCase::builder(0).build().entries[0].oplog_entry.clone();
    let child_fingerprint = AgentFingerprint(uuid::Uuid::from_u128(2));
    let OplogEntry::Create { parameters, .. } = &mut create else {
        unreachable!()
    };
    parameters.instance_id = child_fingerprint.0;
    let entries = BTreeMap::from([
        (OplogIndex::INITIAL, create),
        (
            OplogIndex::from_u64(2),
            export_fork_admission(target.clone(), "ancestor-session", 10),
        ),
        (
            OplogIndex::from_u64(3),
            OplogEntry::stream_session(
                None,
                OplogPayload::Inline(Box::new(StreamSessionRecord::ForkCut(cut))),
                None,
            ),
        ),
    ]);
    let deleted = DeletedRegionsBuilder::from_regions(vec![OplogRegion::from_index_range(
        OplogIndex::from_u64(3)..=OplogIndex::from_u64(3),
    )])
    .build();

    let rebuilt = super::update_status_with_precomputed_regions(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        entries,
        &RetryConfig::default(),
        regions_without_updates(deleted),
        std::collections::VecDeque::new(),
        true,
    )
    .unwrap();

    assert_eq!(
        rebuilt.export_fork_admissions.owner_fingerprint,
        Some(child_fingerprint)
    );
    assert!(
        !rebuilt
            .export_fork_admissions
            .reservations
            .contains_key(&target)
    );
    assert!(rebuilt.export_fork_admissions.session_counts.is_empty());
}

#[test]
fn export_fork_admission_retry_before_publication_recovers_cut_and_budgets() {
    use crate::services::worker_fork::admission::{self, Admission};

    let target = AgentId {
        component_id: ComponentId::new(),
        agent_id: "first".into(),
    };
    let OplogEntry::StreamSession {
        record: OplogPayload::Inline(template),
        ..
    } = export_fork_admission(target.clone(), "session", 1500)
    else {
        unreachable!()
    };
    let StreamSessionRecord::ExportForkAdmitted(template) = *template else {
        unreachable!()
    };
    let original = template.candidate;
    let accepted = admission::reserve(
        &Default::default(),
        target.clone(),
        vec![1; 32],
        original.clone(),
        1,
        2,
        1500,
    )
    .unwrap();
    assert_eq!(accepted.credit_millis, 1000);
    let entries = BTreeMap::from([(
        OplogIndex::from_u64(21),
        OplogEntry::stream_session(
            None,
            OplogPayload::Inline(Box::new(StreamSessionRecord::ExportForkAdmitted(accepted))),
            None,
        ),
    )]);
    // No target exists yet; reconstruct solely from the source's committed admission.
    let rebuilt = update_status_with_new_entries(
        AgentMode::Durable,
        export_admission_baseline(),
        entries,
        &RetryConfig::default(),
    )
    .unwrap();
    let mut later = original.clone();
    later.cut = OplogIndex::from_u64(30);
    later.horizon = OplogIndex::from_u64(40);
    assert_eq!(
        admission::reserve(
            &rebuilt.export_fork_admissions,
            target.clone(),
            vec![1; 32],
            later.clone(),
            0,
            0,
            1500
        ),
        Err(Admission::Existing {
            oplog_index: OplogIndex::from_u64(21)
        })
    );
    assert_eq!(
        admission::reserve(
            &rebuilt.export_fork_admissions,
            target.clone(),
            vec![2; 32],
            later.clone(),
            10,
            10,
            1500
        ),
        Err(Admission::Conflict)
    );
    let second = AgentId {
        agent_id: "second".into(),
        ..target.clone()
    };
    assert_eq!(
        admission::reserve(
            &rebuilt.export_fork_admissions,
            second.clone(),
            vec![2; 32],
            later.clone(),
            1,
            2,
            1500
        ),
        Err(Admission::LimitReached)
    );
    later.export.session = "other-session".into();
    let second_record = admission::reserve(
        &rebuilt.export_fork_admissions,
        second,
        vec![2; 32],
        later.clone(),
        1,
        2,
        1500,
    )
    .unwrap();
    assert_eq!(second_record.credit_millis, 0);
    let rebuilt = update_status_with_new_entries(
        AgentMode::Durable,
        rebuilt,
        BTreeMap::from([(
            OplogIndex::from_u64(22),
            OplogEntry::stream_session(
                None,
                OplogPayload::Inline(Box::new(StreamSessionRecord::ExportForkAdmitted(
                    second_record,
                ))),
                None,
            ),
        )]),
        &RetryConfig::default(),
    )
    .unwrap();
    let third = AgentId {
        agent_id: "third".into(),
        ..target
    };
    later.export.session = "third-session".into();
    for now in [1400, 1999] {
        assert_eq!(
            admission::reserve(
                &rebuilt.export_fork_admissions,
                third.clone(),
                vec![3; 32],
                later.clone(),
                1,
                2,
                now
            ),
            Err(Admission::RateLimited {
                retry_after_seconds: 1
            })
        );
    }
    assert_eq!(
        admission::reserve(
            &rebuilt.export_fork_admissions,
            third,
            vec![3; 32],
            later,
            1,
            2,
            2000
        )
        .unwrap()
        .credit_millis,
        0
    );
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
                OplogErrorKind::Invocation,
                AgentError::TransientError("transient".to_string()),
                retry_from,
                false,
                Some(retry_state.clone()),
            ),
            move |mut status| {
                status.status = AgentStatus::Retrying;
                status.last_error_kind = Some(OplogErrorKind::Invocation);
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
                OplogErrorKind::Invocation,
                AgentError::TransientError("transient".to_string()),
                retry_from,
                false,
                Some(retry_state.clone()),
            ),
            move |mut status| {
                status.status = AgentStatus::Failed;
                status.last_error_kind = Some(OplogErrorKind::Invocation);
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
async fn recovery_failure_remains_failed_until_recovery_succeeds() {
    let retry_from = OplogIndex::from_u64(1);
    let test_case = TestCase::builder(0)
        .add(
            OplogEntry::error(
                None,
                OplogErrorKind::Recovery,
                AgentError::InternalError("replay diverged".to_string()),
                retry_from,
                false,
                Some(RetryPolicyState::Terminal),
            ),
            move |mut status| {
                status.status = AgentStatus::Failed;
                status.last_error_kind = Some(OplogErrorKind::Recovery);
                status
                    .current_retry_state
                    .insert(retry_from, RetryPolicyState::Terminal);
                status
            },
        )
        .add(OplogEntry::recovery_succeeded(), |mut status| {
            status.status = AgentStatus::Idle;
            status.last_error_kind = None;
            status
        })
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn infrastructure_recovery_ignores_existing_semantic_retry_count() {
    let retry_from = OplogIndex::from_u64(1);
    let exhausted = RetryPolicyState::Counter(100);
    let test_case = TestCase::builder(0)
        .add(
            OplogEntry::error(
                None,
                OplogErrorKind::Invocation,
                AgentError::TransientError("semantic failure".to_string()),
                retry_from,
                false,
                Some(exhausted.clone()),
            ),
            {
                let exhausted = exhausted.clone();
                move |mut status| {
                    status.status = AgentStatus::Failed;
                    status.last_error_kind = Some(OplogErrorKind::Invocation);
                    status.current_retry_state.insert(retry_from, exhausted);
                    status
                }
            },
        )
        .add(
            OplogEntry::error(
                None,
                OplogErrorKind::Recovery,
                AgentError::Unknown("payload backend unavailable".to_string()),
                retry_from,
                false,
                None,
            ),
            move |mut status| {
                status.status = AgentStatus::Retrying;
                status.last_error_kind = Some(OplogErrorKind::Recovery);
                status.current_retry_state.insert(retry_from, exhausted);
                status
            },
        )
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn incomplete_invocation_replay_does_not_hide_recovery_failure() {
    let retry_from = OplogIndex::from_u64(1);
    let idempotency_key = IdempotencyKey::fresh();
    let test_case = TestCase::builder(0)
        .add(
            OplogEntry::error(
                None,
                OplogErrorKind::Recovery,
                AgentError::InternalError("replay diverged".to_string()),
                retry_from,
                false,
                Some(RetryPolicyState::Terminal),
            ),
            move |mut status| {
                status.status = AgentStatus::Failed;
                status.last_error_kind = Some(OplogErrorKind::Recovery);
                status
                    .current_retry_state
                    .insert(retry_from, RetryPolicyState::Terminal);
                status
            },
        )
        .add(
            OplogEntry::AgentInvocationStarted {
                timestamp: Timestamp::now_utc(),
                idempotency_key: idempotency_key.clone(),
                payload: OplogPayload::Inline(Box::new(AgentInvocationPayload::AgentMethod {
                    method_name: "a".to_string(),
                    input: SchemaValue::Record { fields: Vec::new() },
                    principal: Principal::anonymous(),
                    scope_card: None,
                })),
                trace_id: TraceId::generate(),
                trace_states: Vec::new(),
                invocation_context: Vec::new(),
                wallet_pin: Box::new(test_invocation_wallet_pin()),
            },
            {
                let idempotency_key = idempotency_key.clone();
                move |mut status| {
                    status.current_idempotency_key = Some(idempotency_key);
                    status
                }
            },
        )
        .add(
            OplogEntry::AgentInvocationFinished {
                timestamp: Timestamp::now_utc(),
                result: OplogPayload::Inline(Box::new(AgentInvocationResult::AgentInitialization)),
                method_name: None,
                consumed_fuel: 0,
                component_revision: ComponentRevision::INITIAL,
            },
            {
                let idempotency_key = idempotency_key.clone();
                move |mut status| {
                    status
                        .invocation_results
                        .insert(idempotency_key, status.oplog_idx);
                    status.current_idempotency_key = None;
                    status
                }
            },
        )
        .add(OplogEntry::recovery_succeeded(), |mut status| {
            status.status = AgentStatus::Idle;
            status.last_error_kind = None;
            status
        })
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn stale_recovery_success_must_not_clear_a_later_interrupt() {
    let retry_from = OplogIndex::from_u64(1);
    let test_case = TestCase::builder(0)
        .add(
            OplogEntry::error(
                None,
                OplogErrorKind::Recovery,
                AgentError::InternalError("replay diverged".to_string()),
                retry_from,
                false,
                Some(RetryPolicyState::Terminal),
            ),
            move |mut status| {
                status.status = AgentStatus::Failed;
                status.last_error_kind = Some(OplogErrorKind::Recovery);
                status
                    .current_retry_state
                    .insert(retry_from, RetryPolicyState::Terminal);
                status
            },
        )
        .add(OplogEntry::interrupted(), |mut status| {
            status.status = AgentStatus::Interrupted;
            status
        })
        .add(OplogEntry::recovery_succeeded(), |mut status| {
            status.status = AgentStatus::Interrupted;
            status.last_error_kind = None;
            status
        })
        .build();

    run_test_case(test_case).await;
}

#[test]
fn recovery_success_in_deleted_region_must_not_clear_failure() {
    let retry_from = OplogIndex::from_u64(1);
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::error(
                None,
                OplogErrorKind::Recovery,
                AgentError::InternalError("replay diverged".to_string()),
                retry_from,
                false,
                Some(RetryPolicyState::Terminal),
            ),
        ),
        (OplogIndex::from_u64(2), OplogEntry::recovery_succeeded()),
    ]);
    let deleted_regions = DeletedRegionsBuilder::from_regions(vec![OplogRegion {
        start: OplogIndex::from_u64(2),
        end: OplogIndex::from_u64(2),
    }])
    .build();

    let (status, last_error_kind, retry_state, _) = calculate_latest_worker_status(
        AgentStatus::Idle,
        None,
        HashMap::new(),
        None,
        &RetryConfig::default(),
        &DeletedRegions::default(),
        &deleted_regions,
        &entries,
    );

    assert_eq!(status, AgentStatus::Failed);
    assert_eq!(last_error_kind, Some(OplogErrorKind::Recovery));
    assert_eq!(
        retry_state.get(&retry_from),
        Some(&RetryPolicyState::Terminal)
    );
}

#[test]
fn recovery_success_preserves_retry_state_from_skipped_atomic_region() {
    let atomic_retry_from = OplogIndex::from_u64(10);
    let recovery_retry_from = OplogIndex::from_u64(20);
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::error(
                None,
                OplogErrorKind::Invocation,
                AgentError::TransientError("atomic attempt failed".to_string()),
                atomic_retry_from,
                true,
                Some(RetryPolicyState::Counter(3)),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::error(
                None,
                OplogErrorKind::Recovery,
                AgentError::InternalError("replay diverged".to_string()),
                recovery_retry_from,
                false,
                Some(RetryPolicyState::Terminal),
            ),
        ),
        (OplogIndex::from_u64(3), OplogEntry::recovery_succeeded()),
    ]);
    let skipped_regions = DeletedRegionsBuilder::from_regions(vec![OplogRegion {
        start: OplogIndex::from_u64(1),
        end: OplogIndex::from_u64(1),
    }])
    .build();

    let (_, _, retry_state, _) = calculate_latest_worker_status(
        AgentStatus::Idle,
        None,
        HashMap::new(),
        None,
        &RetryConfig::default(),
        &skipped_regions,
        &DeletedRegions::default(),
        &entries,
    );

    assert_eq!(
        retry_state.get(&atomic_retry_from),
        Some(&RetryPolicyState::Counter(3)),
        "retry state from skipped atomic history is needed by subsequent attempts"
    );
}

#[test]
fn repeated_infrastructure_recovery_failures_do_not_consume_invocation_retry_state() {
    let invocation_retry_from = OplogIndex::from_u64(10);
    let recovery_retry_from = OplogIndex::from_u64(20);
    let mut retry_state = HashMap::from([(invocation_retry_from, RetryPolicyState::Counter(2))]);

    for attempt in 0..5 {
        let entries = BTreeMap::from([(
            OplogIndex::from_u64(21 + attempt),
            OplogEntry::error(
                None,
                OplogErrorKind::Recovery,
                AgentError::Unknown("component service unavailable".to_string()),
                recovery_retry_from,
                false,
                None,
            ),
        )]);
        let (status, kind, updated_retry_state, _) = calculate_latest_worker_status(
            AgentStatus::Retrying,
            Some(OplogErrorKind::Recovery),
            retry_state,
            None,
            &RetryConfig::default(),
            &DeletedRegions::default(),
            &DeletedRegions::default(),
            &entries,
        );

        assert_eq!(status, AgentStatus::Retrying);
        assert_eq!(kind, Some(OplogErrorKind::Recovery));
        assert_eq!(
            updated_retry_state.get(&invocation_retry_from),
            Some(&RetryPolicyState::Counter(2))
        );
        assert!(!updated_retry_state.contains_key(&recovery_retry_from));
        retry_state = updated_retry_state;
    }
}

#[test]
async fn invocation_failure_is_classified_separately_from_recovery_failure() {
    let retry_from = OplogIndex::from_u64(1);
    let test_case = TestCase::builder(0)
        .add(
            OplogEntry::error(
                None,
                OplogErrorKind::Invocation,
                AgentError::PermanentError("application failed".to_string()),
                retry_from,
                false,
                Some(RetryPolicyState::Terminal),
            ),
            move |mut status| {
                status.status = AgentStatus::Failed;
                status.last_error_kind = Some(OplogErrorKind::Invocation);
                status
                    .current_retry_state
                    .insert(retry_from, RetryPolicyState::Terminal);
                status
            },
        )
        .build();

    run_test_case(test_case).await;
}

#[test]
async fn recovery_permission_denied_is_not_treated_as_an_invocation_rejection() {
    let retry_from = OplogIndex::from_u64(1);
    let test_case = TestCase::builder(0)
        .add(
            OplogEntry::error(
                None,
                OplogErrorKind::Recovery,
                AgentError::PermissionDenied("startup denied".to_string()),
                retry_from,
                false,
                Some(RetryPolicyState::Terminal),
            ),
            move |mut status| {
                status.status = AgentStatus::Failed;
                status.last_error_kind = Some(OplogErrorKind::Recovery);
                status
                    .current_retry_state
                    .insert(retry_from, RetryPolicyState::Terminal);
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
fn recovery_errors_are_not_invocation_results() {
    let key = IdempotencyKey::fresh();
    let retry_from = OplogIndex::from_u64(1);
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::AgentInvocationStarted {
                timestamp: Timestamp::now_utc(),
                idempotency_key: key.clone(),
                payload: OplogPayload::Inline(Box::new(AgentInvocationPayload::AgentMethod {
                    method_name: "agent:method".to_string(),
                    input: SchemaValue::Record { fields: Vec::new() },
                    principal: Principal::anonymous(),
                    scope_card: None,
                })),
                trace_id: TraceId::generate(),
                trace_states: Vec::new(),
                invocation_context: Vec::new(),
                wallet_pin: Box::new(test_invocation_wallet_pin()),
            },
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::error(
                None,
                OplogErrorKind::Recovery,
                AgentError::InternalError("replay diverged".to_string()),
                retry_from,
                false,
                Some(RetryPolicyState::Terminal),
            ),
        ),
    ]);
    let mut current_idempotency_key = None;
    let mut cancelled_idempotency_key = None;
    let mut results = Vec::new();

    fold_invocation_result_entries(
        &mut current_idempotency_key,
        &mut cancelled_idempotency_key,
        &DeletedRegions::default(),
        &entries,
        |key, index| results.push((key.clone(), index)),
    );

    assert!(results.is_empty());
    assert_eq!(current_idempotency_key, Some(key));
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
        filesystem_snapshot: None,
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

    let successful = test_case
        .entries
        .last()
        .unwrap()
        .expected_status
        .successful_updates
        .last()
        .unwrap();
    assert_eq!(
        successful.pending_update.as_ref().unwrap().admission_index,
        OplogIndex::from_u64(6)
    );

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
        filesystem_snapshot: None,
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
        filesystem_snapshot: None,
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

    let failed = test_case
        .entries
        .last()
        .unwrap()
        .expected_status
        .failed_updates
        .last()
        .unwrap();
    assert_eq!(
        failed.pending_update.as_ref().unwrap().admission_index,
        OplogIndex::from_u64(6)
    );

    run_test_case(test_case).await;
}

#[test]
async fn manual_failure_before_pending_update_does_not_remove_automatic_update() {
    let target_revision = ComponentRevision::new(2).unwrap();
    let automatic = UpdateDescription::Automatic { target_revision };
    let manual = UpdateDescription::SnapshotBased {
        target_revision,
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
        filesystem_snapshot: None,
    };

    let test_case = TestCase::builder(1)
        .pending_update(&automatic, |_| {})
        .pending_invocation(AgentInvocation::ManualUpdate { target_revision })
        .failed_update(manual)
        .build();
    let status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(status.pending_updates.len(), 1);
    assert_eq!(status.pending_updates[0].target_revision, target_revision);
    assert_eq!(
        status.failed_updates[0]
            .pending_update
            .as_ref()
            .unwrap()
            .admission_index,
        OplogIndex::from_u64(3)
    );

    run_test_case(test_case).await;
}

#[test]
async fn reverted_manual_failure_does_not_reassign_a_later_failure() {
    let target_revision = ComponentRevision::new(2).unwrap();
    let manual = || UpdateDescription::SnapshotBased {
        target_revision,
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
        filesystem_snapshot: None,
    };

    let test_case = TestCase::builder(1)
        .pending_invocation(AgentInvocation::ManualUpdate { target_revision })
        .failed_update(manual())
        .revert(OplogIndex::from_u64(2))
        .pending_invocation(AgentInvocation::ManualUpdate { target_revision })
        .failed_update(manual())
        .build();
    let status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(status.failed_updates.len(), 1);
    assert_eq!(
        status.failed_updates[0]
            .pending_update
            .as_ref()
            .unwrap()
            .admission_index,
        OplogIndex::from_u64(5)
    );
    assert!(status.pending_invocations.is_empty());

    run_test_case(test_case).await;
}

#[test]
fn manual_failure_identity_is_chunk_independent_across_reverted_admission() {
    let target_revision = ComponentRevision::new(2).unwrap();
    let manual = || UpdateDescription::SnapshotBased {
        target_revision,
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
        filesystem_snapshot: None,
    };
    let test_case = TestCase::builder(1)
        .pending_invocation(AgentInvocation::ManualUpdate { target_revision })
        .revert(OplogIndex::INITIAL)
        .failed_update(manual())
        .build();
    let entries: Vec<_> = test_case
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            (
                OplogIndex::from_u64(index as u64 + 1),
                entry.oplog_entry.clone(),
            )
        })
        .collect();

    let fold = |chunk_size: usize| {
        let mut status = AgentStatusRecord::default();
        let mut history = BTreeMap::new();
        for chunk in entries.chunks(chunk_size) {
            let chunk: BTreeMap<_, _> = chunk.iter().cloned().collect();
            history.extend(chunk.clone());
            status = update_status_with_new_entries(
                AgentMode::Durable,
                status,
                chunk,
                &RetryConfig::default(),
            )
            .unwrap_or_else(|| {
                update_status_with_new_entries(
                    AgentMode::Durable,
                    AgentStatusRecord::default(),
                    history.clone(),
                    &RetryConfig::default(),
                )
                .expect("full fold must reconstruct a reverted baseline")
            });
        }
        status
    };

    let full = fold(entries.len());
    assert_eq!(full.failed_updates.len(), 1);
    assert_eq!(
        full.failed_updates[0]
            .pending_update
            .as_ref()
            .unwrap()
            .admission_index,
        OplogIndex::from_u64(2)
    );
    assert!(full.pending_invocations.is_empty());
    for chunk_size in 1..entries.len() {
        assert_eq!(fold(chunk_size), full, "chunk size {chunk_size}");
    }
}

#[test]
async fn manual_failure_removes_only_one_same_target_admission() {
    let target_revision = ComponentRevision::new(2).unwrap();
    let manual = UpdateDescription::SnapshotBased {
        target_revision,
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
        filesystem_snapshot: None,
    };

    let test_case = TestCase::builder(1)
        .pending_invocation(AgentInvocation::ManualUpdate { target_revision })
        .pending_invocation(AgentInvocation::ManualUpdate { target_revision })
        .failed_update(manual)
        .build();
    let status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(
        status.failed_updates[0]
            .pending_update
            .as_ref()
            .unwrap()
            .admission_index,
        OplogIndex::from_u64(2)
    );
    assert_eq!(status.pending_invocations.len(), 1);
    assert_eq!(
        status.pending_invocations[0].oplog_index,
        OplogIndex::from_u64(3)
    );

    run_test_case(test_case).await;
}

#[test]
fn revert_validation_preserves_jump_while_removing_crossed_snapshot_baseline() {
    let update = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(2).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
        filesystem_snapshot: None,
    };
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(10),
            OplogEntry::jump(
                None,
                OplogRegion {
                    start: OplogIndex::from_u64(4),
                    end: OplogIndex::from_u64(10),
                },
            ),
        ),
        (
            OplogIndex::from_u64(20),
            OplogEntry::pending_update(update.clone(), None),
        ),
        (
            OplogIndex::from_u64(21),
            OplogEntry::successful_update(
                *update.target_revision(),
                100,
                None,
                Default::default(),
                None,
            ),
        ),
    ]);

    let regions = super::revert_validation_regions(
        &entries,
        &OplogRegion {
            start: OplogIndex::from_u64(7),
            end: OplogIndex::from_u64(21),
        },
    );

    assert!(regions.is_in_deleted_region(OplogIndex::from_u64(7)));
    assert!(regions.is_in_deleted_region(OplogIndex::from_u64(10)));
    assert!(!regions.is_in_deleted_region(OplogIndex::from_u64(11)));
    assert!(!regions.is_in_deleted_region(OplogIndex::from_u64(20)));
}

#[test]
fn revert_validation_ignores_unapplied_snapshot_update_in_dropped_region() {
    let update = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(2).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
        filesystem_snapshot: None,
    };
    let entries = BTreeMap::from([(
        OplogIndex::from_u64(3),
        OplogEntry::pending_update(update, None),
    )]);

    let regions = super::revert_validation_regions(
        &entries,
        &OplogRegion {
            start: OplogIndex::from_u64(2),
            end: OplogIndex::from_u64(3),
        },
    );

    assert!(!regions.is_in_deleted_region(OplogIndex::from_u64(2)));
}

#[test]
fn revert_validation_removes_only_crossed_assisted_promotion() {
    let details = SnapshotAssistedUpdateDetails {
        pending_update_index: OplogIndex::from_u64(3),
        source_component_revision: ComponentRevision::new(1).unwrap(),
        source_revision_start_index: OplogIndex::INITIAL,
        snapshot_index: OplogIndex::from_u64(2),
    };
    let successful_update = OplogEntry::successful_update(
        ComponentRevision::new(2).unwrap(),
        100,
        None,
        Default::default(),
        Some(details),
    );
    let dropped_update = OplogRegion::from_range(3..=4);

    let regions = super::revert_validation_regions(
        &BTreeMap::from([(OplogIndex::from_u64(4), successful_update.clone())]),
        &dropped_update,
    );
    assert!(!regions.is_in_deleted_region(OplogIndex::from_u64(2)));

    let regions_with_overlap = super::revert_validation_regions(
        &BTreeMap::from([
            (OplogIndex::from_u64(4), successful_update),
            (
                OplogIndex::from_u64(5),
                OplogEntry::jump(None, OplogRegion::from_range(2..=2)),
            ),
        ]),
        &OplogRegion::from_range(3..=5),
    );
    assert!(regions_with_overlap.is_in_deleted_region(OplogIndex::from_u64(2)));
}

#[test]
fn prospective_revert_keeps_deletions_from_revert_records_it_drops() {
    let update_two = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(2).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
        filesystem_snapshot: None,
    };
    let update_three = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(3).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
        filesystem_snapshot: None,
    };
    let test_case = TestCase::builder(1)
        .pending_update(&update_two, |_| {})
        .successful_update(update_two, 200, &HashSet::new())
        .revert(OplogIndex::INITIAL)
        .pending_update(&update_three, |_| {})
        .successful_update(update_three, 300, &HashSet::new())
        .build();
    let entries = test_case
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            (
                OplogIndex::from_u64(index as u64 + 1),
                entry.oplog_entry.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let physical_prefix = entries
        .range(..=OplogIndex::from_u64(3))
        .map(|(idx, entry)| (*idx, entry.clone()))
        .collect();
    let physical_prefix_status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        physical_prefix,
        &RetryConfig::default(),
    )
    .unwrap();
    assert_eq!(
        physical_prefix_status.component_revision,
        ComponentRevision::new(2).unwrap()
    );

    let mut prospective_entries = entries;
    prospective_entries.insert(
        OplogIndex::from_u64(7),
        OplogEntry::revert(OplogRegion {
            start: OplogIndex::from_u64(4),
            end: OplogIndex::from_u64(6),
        }),
    );
    let prospective_status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord::default(),
        prospective_entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert_eq!(
        prospective_status.component_revision,
        ComponentRevision::new(1).unwrap()
    );
    assert!(
        prospective_status
            .deleted_regions
            .is_in_deleted_region(OplogIndex::from_u64(2))
    );
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
        filesystem_snapshot: None,
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
        filesystem_snapshot: None,
    };
    let update2 = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(2).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
        filesystem_snapshot: None,
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
        filesystem_snapshot: None,
    };
    let update2 = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(3).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
        filesystem_snapshot: None,
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
async fn a_revert_across_an_update_drops_its_name_from_the_status() {
    let k1 = IdempotencyKey::fresh();
    let name = FilesystemSnapshotName::update();
    let update = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(2).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
        filesystem_snapshot: Some(name.clone()),
    };

    let updated = TestCase::builder(1)
        .agent_invocation_started("a", vec![], k1.clone())
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            k1,
            ComponentRevision::INITIAL,
        )
        .pending_update(&update, |_| {})
        .successful_update(update, 2000, &HashSet::new());
    let before_revert = updated.previous_status_record.clone();
    let test_case = updated.revert(OplogIndex::from_u64(3)).build();
    let after_revert = test_case.entries.last().unwrap().expected_status.clone();

    assert_eq!(
        before_revert
            .successful_updates
            .iter()
            .map(|update| update.filesystem_snapshot.clone())
            .collect::<Vec<_>>(),
        vec![Some(name.clone())]
    );
    assert_eq!(
        (
            names_in_use(&before_revert),
            manual_update_baseline(&before_revert.authoritative_snapshot)
        ),
        (Box::from([name]), Some(OplogIndex::from_u64(4)))
    );
    assert_eq!(
        (
            names_in_use(&after_revert),
            manual_update_baseline(&after_revert.authoritative_snapshot)
        ),
        (Box::from([]), None)
    );
    run_test_case(test_case).await;
}

#[test]
fn a_successful_update_without_a_pending_update_has_no_name_and_no_baseline() {
    let entries = BTreeMap::from([(
        OplogIndex::from_u64(2),
        OplogEntry::successful_update(
            ComponentRevision::new(2).unwrap(),
            100,
            None,
            HashSet::new(),
            None,
        ),
    )]);

    let status = update_status_with_new_entries(
        AgentMode::Durable,
        AgentStatusRecord {
            oplog_idx: OplogIndex::from_u64(1),
            ..AgentStatusRecord::default()
        },
        entries,
        &RetryConfig::default(),
    )
    .unwrap();

    assert_eq!(
        status
            .successful_updates
            .iter()
            .map(|update| (update.oplog_index, update.filesystem_snapshot.clone()))
            .collect::<Vec<_>>(),
        vec![(OplogIndex::from_u64(2), None)]
    );
    assert_eq!(status.authoritative_snapshot, None);
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
async fn snapshot_confirmed_with_same_name_sets_flag() {
    let name = FilesystemSnapshotName::periodic();

    let test_case = TestCase::builder(1)
        .snapshot_with_filesystem(Some(name.clone()))
        .grow_memory(10)
        .snapshot_confirmed(name.clone(), true)
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .map(|last| last.index),
        Some(OplogIndex::from_u64(2))
    );
    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .and_then(|last| last.files.name().cloned()),
        Some(name)
    );
    assert!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .is_some_and(|last| matches!(last.files, SnapshotFiles::Confirmed(_)))
    );
    run_test_case(test_case).await;
}

#[test]
async fn snapshot_confirmed_after_successful_update_is_ignored() {
    let name = FilesystemSnapshotName::periodic();
    let update = UpdateDescription::Automatic {
        target_revision: ComponentRevision::new(2).unwrap(),
    };

    let test_case = TestCase::builder(1)
        .snapshot_with_filesystem(Some(name.clone()))
        .snapshot_confirmed(name.clone(), true)
        .pending_update(&update, |_| {})
        .successful_update(update, 2000, &HashSet::new())
        .snapshot_confirmed(name, false)
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .map(|last| last.index),
        None
    );
    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .and_then(|last| last.files.name().cloned()),
        None
    );
    assert!(
        !final_status
            .last_automatic_snapshot
            .as_ref()
            .is_some_and(|last| matches!(last.files, SnapshotFiles::Confirmed(_)))
    );
    run_test_case(test_case).await;
}

#[test]
async fn snapshot_confirmed_after_reverted_snapshot_is_ignored() {
    let name = FilesystemSnapshotName::periodic();

    let test_case = TestCase::builder(1)
        .grow_memory(10)
        .snapshot_with_filesystem(Some(name.clone()))
        .snapshot_confirmed(name.clone(), true)
        .grow_memory(20)
        .revert(OplogIndex::from_u64(2))
        .snapshot_confirmed(name, false)
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .map(|last| last.index),
        None
    );
    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .and_then(|last| last.files.name().cloned()),
        None
    );
    assert!(
        !final_status
            .last_automatic_snapshot
            .as_ref()
            .is_some_and(|last| matches!(last.files, SnapshotFiles::Confirmed(_)))
    );
    run_test_case(test_case).await;
}

#[test]
async fn snapshot_confirmed_matches_only_the_newest_snapshot() {
    let first = FilesystemSnapshotName::periodic();
    let second = FilesystemSnapshotName::periodic();

    let test_case = TestCase::builder(1)
        .snapshot_with_filesystem(Some(first.clone()))
        .snapshot_with_filesystem(Some(second.clone()))
        .snapshot_confirmed(first, false)
        .snapshot_confirmed(second.clone(), true)
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .map(|last| last.index),
        Some(OplogIndex::from_u64(3))
    );
    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .and_then(|last| last.files.name().cloned()),
        Some(second)
    );
    assert!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .is_some_and(|last| matches!(last.files, SnapshotFiles::Confirmed(_)))
    );
    run_test_case(test_case).await;
}

#[test]
async fn snapshot_after_confirmed_snapshot_starts_an_unconfirmed_candidate() {
    let first = FilesystemSnapshotName::periodic();
    let second = FilesystemSnapshotName::periodic();

    let test_case = TestCase::builder(1)
        .snapshot_with_filesystem(Some(first.clone()))
        .snapshot_confirmed(first, true)
        .snapshot_with_filesystem(Some(second.clone()))
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .map(|last| last.index),
        Some(OplogIndex::from_u64(4))
    );
    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .and_then(|last| last.files.name().cloned()),
        Some(second)
    );
    assert!(
        !final_status
            .last_automatic_snapshot
            .as_ref()
            .is_some_and(|last| matches!(last.files, SnapshotFiles::Confirmed(_)))
    );
    run_test_case(test_case).await;
}

#[test]
async fn snapshot_confirmed_for_unknown_name_is_ignored() {
    let name = FilesystemSnapshotName::periodic();

    let test_case = TestCase::builder(1)
        .snapshot_with_filesystem(Some(name))
        .snapshot_confirmed(FilesystemSnapshotName::periodic(), false)
        .snapshot()
        .snapshot_confirmed(FilesystemSnapshotName::periodic(), false)
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .map(|last| last.index),
        Some(OplogIndex::from_u64(4))
    );
    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .and_then(|last| last.files.name().cloned()),
        None
    );
    assert!(
        !final_status
            .last_automatic_snapshot
            .as_ref()
            .is_some_and(|last| matches!(last.files, SnapshotFiles::Confirmed(_)))
    );
    run_test_case(test_case).await;
}

#[test]
async fn a_new_snapshot_keeps_a_confirmed_candidate_as_the_previous_usable_snapshot() {
    let first = FilesystemSnapshotName::periodic();
    let second = FilesystemSnapshotName::periodic();

    let test_case = TestCase::builder(1)
        .snapshot_with_filesystem(Some(first.clone()))
        .snapshot_confirmed(first.clone(), true)
        .snapshot_with_filesystem(Some(second))
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(
        final_status.previous_usable_automatic_snapshot,
        Some(UsableAutomaticSnapshot {
            index: OplogIndex::from_u64(2),
            component_revision: ComponentRevision::new(1).unwrap(),
            filesystem_snapshot: Some(first),
        })
    );
    run_test_case(test_case).await;
}

#[test]
async fn a_new_snapshot_keeps_a_candidate_without_a_name_as_the_previous_usable_snapshot() {
    let test_case = TestCase::builder(1)
        .snapshot()
        .snapshot_with_filesystem(Some(FilesystemSnapshotName::periodic()))
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(
        final_status.previous_usable_automatic_snapshot,
        Some(UsableAutomaticSnapshot {
            index: OplogIndex::from_u64(2),
            component_revision: ComponentRevision::new(1).unwrap(),
            filesystem_snapshot: None,
        })
    );
    run_test_case(test_case).await;
}

#[test]
async fn a_new_snapshot_after_an_unconfirmed_candidate_keeps_the_older_usable_snapshot() {
    let first = FilesystemSnapshotName::periodic();

    let test_case = TestCase::builder(1)
        .snapshot_with_filesystem(Some(first.clone()))
        .snapshot_confirmed(first.clone(), true)
        .snapshot_with_filesystem(Some(FilesystemSnapshotName::periodic()))
        .snapshot_with_filesystem(Some(FilesystemSnapshotName::periodic()))
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(
        final_status
            .previous_usable_automatic_snapshot
            .as_ref()
            .map(|snapshot| (snapshot.index, snapshot.filesystem_snapshot.clone())),
        Some((OplogIndex::from_u64(2), Some(first)))
    );
    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .map(|last| last.index),
        Some(OplogIndex::from_u64(5))
    );
    run_test_case(test_case).await;
}

#[test]
async fn a_record_that_reuses_a_confirmed_name_is_confirmed_and_moves_the_record_before_it_to_the_fallback()
 {
    let older = FilesystemSnapshotName::periodic();
    let name = FilesystemSnapshotName::periodic();

    let test_case = TestCase::builder(1)
        .snapshot_with_filesystem(Some(older.clone()))
        .snapshot_confirmed(older, true)
        .snapshot_with_filesystem(Some(name.clone()))
        .snapshot_confirmed(name.clone(), true)
        .snapshot_with_filesystem(Some(name.clone()))
        .snapshot_confirmed(name.clone(), true)
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .map(|last| last.index),
        Some(OplogIndex::from_u64(6))
    );
    assert!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .is_some_and(|last| matches!(last.files, SnapshotFiles::Confirmed(_)))
    );
    assert_eq!(
        final_status
            .previous_usable_automatic_snapshot
            .as_ref()
            .map(|snapshot| (snapshot.index, snapshot.filesystem_snapshot.clone())),
        Some((OplogIndex::from_u64(4), Some(name)))
    );
    run_test_case(test_case).await;
}

#[test]
async fn a_record_that_reuses_the_name_of_the_candidate_keeps_that_candidate_as_the_fallback() {
    let name = FilesystemSnapshotName::periodic();

    let test_case = TestCase::builder(1)
        .snapshot_with_filesystem(Some(name.clone()))
        .snapshot_confirmed(name.clone(), true)
        .snapshot_with_filesystem(Some(name.clone()))
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(
        final_status.previous_usable_automatic_snapshot,
        Some(UsableAutomaticSnapshot {
            index: OplogIndex::from_u64(2),
            component_revision: ComponentRevision::new(1).unwrap(),
            filesystem_snapshot: Some(name),
        })
    );
    run_test_case(test_case).await;
}

#[test]
async fn a_successful_update_clears_the_previous_usable_snapshot() {
    let update = UpdateDescription::Automatic {
        target_revision: ComponentRevision::new(2).unwrap(),
    };

    let test_case = TestCase::builder(1)
        .snapshot()
        .snapshot()
        .pending_update(&update, |_| {})
        .successful_update(update, 2000, &HashSet::new())
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(final_status.previous_usable_automatic_snapshot, None);
    run_test_case(test_case).await;
}

#[test]
async fn a_failed_update_keeps_the_previous_usable_snapshot() {
    let update = UpdateDescription::Automatic {
        target_revision: ComponentRevision::new(2).unwrap(),
    };

    let test_case = TestCase::builder(1)
        .snapshot()
        .snapshot()
        .pending_update(&update, |_| {})
        .failed_update(update)
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(
        final_status
            .previous_usable_automatic_snapshot
            .as_ref()
            .map(|snapshot| snapshot.index),
        Some(OplogIndex::from_u64(2))
    );
    run_test_case(test_case).await;
}

#[test]
async fn a_revert_of_the_newer_records_restores_the_previous_usable_snapshot() {
    let first = FilesystemSnapshotName::periodic();
    let second = FilesystemSnapshotName::periodic();

    let test_case = TestCase::builder(1)
        .snapshot_with_filesystem(Some(first.clone()))
        .snapshot_confirmed(first.clone(), true)
        .grow_memory(10)
        .snapshot_with_filesystem(Some(second.clone()))
        .snapshot_confirmed(second, true)
        .snapshot_with_filesystem(Some(FilesystemSnapshotName::periodic()))
        .revert(OplogIndex::from_u64(3))
        .build();
    let final_status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .map(|last| last.index),
        Some(OplogIndex::from_u64(2))
    );
    assert!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .is_some_and(|last| matches!(last.files, SnapshotFiles::Confirmed(_)))
    );
    assert_eq!(final_status.previous_usable_automatic_snapshot, None);
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

    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .map(|last| last.index),
        None
    );
    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .map(|last| last.timestamp),
        None
    );
    assert_eq!(
        final_status
            .last_automatic_snapshot
            .as_ref()
            .map(|last| last.component_revision),
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
        final_status
            .last_automatic_snapshot
            .as_ref()
            .map(|last| last.component_revision),
        Some(ComponentRevision::new(1).unwrap())
    );
    run_test_case(test_case).await;
}

fn snapshot_assisted_pending_details(
    pending: &PendingUpdateRef,
) -> (ComponentRevision, OplogIndex, UsableAutomaticSnapshot) {
    match &pending.kind {
        PendingUpdateKind::SnapshotAssistedAutomatic(selection) => (
            selection.snapshot.component_revision,
            selection.source_revision_start_index,
            selection.snapshot.clone(),
        ),
        _ => panic!("expected snapshot-assisted automatic pending update"),
    }
}

fn automatic_update(target_revision: u64) -> UpdateDescription {
    UpdateDescription::Automatic {
        target_revision: ComponentRevision::new(target_revision).unwrap(),
    }
}

fn assisted_update(
    target_revision: u64,
    source_component_revision: u64,
    source_revision_start_index: u64,
    snapshot_index: u64,
) -> UpdateDescription {
    UpdateDescription::SnapshotAssistedAutomatic {
        target_revision: ComponentRevision::new(target_revision).unwrap(),
        source_component_revision: ComponentRevision::new(source_component_revision).unwrap(),
        source_revision_start_index: OplogIndex::from_u64(source_revision_start_index),
        snapshot_index: OplogIndex::from_u64(snapshot_index),
        snapshot_revision: ComponentRevision::new(source_component_revision).unwrap(),
        filesystem_snapshot: None,
    }
}

#[test]
async fn automatic_strategy_refines_admission_in_place_and_freezes_selection() {
    let admission = automatic_update(2);
    let selected = assisted_update(2, 1, 1, 3);
    let test_case = TestCase::builder(1)
        .pending_update(&admission, |_| {})
        // A snapshot committed after admission is eligible when the queue head is activated.
        .snapshot()
        .pending_update(&selected, |_| {})
        // Once persisted, later snapshots cannot change the selected strategy.
        .snapshot()
        .build();
    let status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(status.pending_updates.len(), 1);
    let pending = &status.pending_updates[0];
    assert_eq!(pending.admission_index, OplogIndex::from_u64(2));
    assert_eq!(pending.oplog_index, OplogIndex::from_u64(4));
    assert_eq!(
        snapshot_assisted_pending_details(pending),
        (
            ComponentRevision::new(1).unwrap(),
            OplogIndex::INITIAL,
            UsableAutomaticSnapshot {
                index: OplogIndex::from_u64(3),
                component_revision: ComponentRevision::new(1).unwrap(),
                filesystem_snapshot: None,
            },
        )
    );
    run_test_case(test_case).await;
}

#[test]
async fn automatic_full_replay_strategy_refines_admission_in_place() {
    let update = automatic_update(2);
    let test_case = TestCase::builder(1)
        .pending_update(&update, |_| {})
        .pending_update(&update, |_| {})
        .build();
    let pending = &test_case
        .entries
        .last()
        .unwrap()
        .expected_status
        .pending_updates[0];

    assert_eq!(pending.kind, PendingUpdateKind::Automatic);
    assert_eq!(pending.admission_index, OplogIndex::from_u64(2));
    assert_eq!(pending.oplog_index, OplogIndex::from_u64(3));
    run_test_case(test_case).await;
}

#[test]
async fn different_target_automatic_admissions_remain_distinct() {
    let first = automatic_update(2);
    let second = automatic_update(3);
    let test_case = TestCase::builder(1)
        .pending_update(&first, |_| {})
        .pending_update(&second, |_| {})
        .build();
    let pending = &test_case
        .entries
        .last()
        .unwrap()
        .expected_status
        .pending_updates;

    assert_eq!(pending.len(), 2);
    assert_eq!(
        pending[0].target_revision,
        ComponentRevision::new(2).unwrap()
    );
    assert_eq!(
        pending[1].target_revision,
        ComponentRevision::new(3).unwrap()
    );
    assert_eq!(pending[0].admission_index, OplogIndex::from_u64(2));
    assert_eq!(pending[1].admission_index, OplogIndex::from_u64(3));
    run_test_case(test_case).await;
}

#[test]
async fn snapshot_assisted_selection_survives_outcome_revert_and_checkpoint_fold() {
    let admission = automatic_update(2);
    let selected = assisted_update(2, 1, 1, 2);
    let test_case = TestCase::builder(1)
        .snapshot()
        .pending_update(&admission, |_| {})
        .pending_update(&selected, |_| {})
        .failed_update(selected)
        .revert(OplogIndex::from_u64(4))
        .build();
    let expected = test_case.entries.last().unwrap().expected_status.clone();
    let expected_selection = Some(UsableAutomaticSnapshot {
        index: OplogIndex::from_u64(2),
        component_revision: ComponentRevision::new(1).unwrap(),
        filesystem_snapshot: None,
    });
    assert_eq!(
        Some(snapshot_assisted_pending_details(&expected.pending_updates[0]).2),
        expected_selection,
    );

    let checkpoint = test_case.entries[2].expected_status.clone();
    test_case.read_starts.lock().unwrap().clear();
    let checkpoint_fold = calculate_last_known_status_with_checkpoint_reader(
        &test_case,
        &test_case.owned_agent_id,
        AgentMode::Durable,
        None,
        || async { Some(checkpoint) },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(checkpoint_fold, expected);
    let read_starts = test_case.read_starts.lock().unwrap().clone();
    assert!(read_starts.contains(&OplogIndex::from_u64(4).as_u64()));
    assert!(!read_starts.contains(&OplogIndex::INITIAL.as_u64()));
    run_test_case(test_case).await;
}

#[test]
async fn reverting_strategy_selection_restores_unselected_admission() {
    let admission = automatic_update(2);
    let selected = assisted_update(2, 1, 1, 2);
    let test_case = TestCase::builder(1)
        .snapshot()
        .pending_update(&admission, |_| {})
        .pending_update(&selected, |_| {})
        .revert(OplogIndex::from_u64(3))
        .build();
    let pending = &test_case
        .entries
        .last()
        .unwrap()
        .expected_status
        .pending_updates[0];

    assert_eq!(pending.kind, PendingUpdateKind::Automatic);
    assert_eq!(pending.admission_index, OplogIndex::from_u64(3));
    assert_eq!(pending.oplog_index, pending.admission_index);
    run_test_case(test_case).await;
}

#[test]
async fn snapshot_assisted_selection_survives_successful_outcome_revert() {
    let source_revision = ComponentRevision::new(1).unwrap();
    let admission = automatic_update(2);
    let selected = assisted_update(2, 1, 1, 2);
    let test_case = TestCase::builder(source_revision.get())
        .snapshot()
        .pending_update(&admission, |_| {})
        .pending_update(&selected, |_| {})
        .successful_snapshot_assisted_update(selected, OplogIndex::from_u64(2))
        .revert(OplogIndex::from_u64(4))
        .build();
    let status = &test_case.entries.last().unwrap().expected_status;
    let (selected_source, source_revision_start_index, selection) =
        snapshot_assisted_pending_details(&status.pending_updates[0]);

    assert_eq!(status.component_revision, source_revision);
    assert_eq!(status.component_revision_start_index, OplogIndex::INITIAL);
    assert_eq!(status.authoritative_snapshot, None);
    assert!(
        !status
            .skipped_regions
            .is_in_deleted_region(OplogIndex::from_u64(2))
    );
    assert!(
        !status
            .skipped_regions
            .is_in_deleted_region(OplogIndex::from_u64(3))
    );
    assert!(
        !status
            .skipped_regions
            .is_in_deleted_region(OplogIndex::from_u64(4))
    );
    assert!(
        status
            .skipped_regions
            .is_in_deleted_region(OplogIndex::from_u64(5))
    );
    assert_eq!(selected_source, source_revision);
    assert_eq!(source_revision_start_index, OplogIndex::INITIAL);
    assert_eq!(
        selection,
        UsableAutomaticSnapshot {
            index: OplogIndex::from_u64(2),
            component_revision: source_revision,
            filesystem_snapshot: None,
        }
    );
    run_test_case(test_case).await;
}

#[test]
async fn assisted_strategy_preserves_post_aba_source_start_index() {
    let source_revision = ComponentRevision::new(1).unwrap();
    let away = UpdateDescription::Automatic {
        target_revision: ComponentRevision::new(2).unwrap(),
    };
    let back = UpdateDescription::Automatic {
        target_revision: source_revision,
    };
    let admission = automatic_update(3);
    let test_case = TestCase::builder(source_revision.get())
        .snapshot()
        .pending_update(&away, |_| {})
        .successful_update(away, 2000, &HashSet::new())
        .pending_update(&back, |_| {})
        .successful_update(back, 1000, &HashSet::new())
        .snapshot()
        .pending_update(&admission, |_| {})
        .pending_update(&assisted_update(3, 1, 6, 7), |_| {})
        .build();
    let pending = &test_case
        .entries
        .last()
        .unwrap()
        .expected_status
        .pending_updates[0];
    assert_eq!(
        snapshot_assisted_pending_details(pending).1,
        OplogIndex::from_u64(6)
    );
    run_test_case(test_case).await;
}

#[test]
async fn snapshot_assisted_selection_freezes_source_revision_start_index() {
    let first_update = UpdateDescription::Automatic {
        target_revision: ComponentRevision::new(2).unwrap(),
    };
    let admission = automatic_update(3);
    let test_case = TestCase::builder(1)
        .pending_update(&first_update, |_| {})
        .successful_update(first_update, 2000, &HashSet::new())
        .snapshot()
        .pending_update(&admission, |_| {})
        .pending_update(&assisted_update(3, 2, 3, 4), |_| {})
        .build();
    let pending = test_case
        .entries
        .last()
        .unwrap()
        .expected_status
        .pending_updates[0]
        .clone();
    let (source_component_revision, source_revision_start_index, selection) =
        snapshot_assisted_pending_details(&pending);

    assert_eq!(
        source_component_revision,
        ComponentRevision::new(2).unwrap()
    );
    assert_eq!(source_revision_start_index, OplogIndex::from_u64(3));
    assert_eq!(
        selection,
        UsableAutomaticSnapshot {
            index: OplogIndex::from_u64(4),
            component_revision: ComponentRevision::new(2).unwrap(),
            filesystem_snapshot: None,
        }
    );
    run_test_case(test_case).await;
}

#[test]
async fn successful_snapshot_assisted_update_promotes_only_selected_snapshot_prefix() {
    let source_revision = ComponentRevision::new(1).unwrap();
    let target_revision = ComponentRevision::new(2).unwrap();
    let admission = automatic_update(2);
    let update = assisted_update(2, 1, 1, 3);
    let before_snapshot = AgentResourceId(11);
    let after_snapshot = AgentResourceId(12);
    let snapshot_index = OplogIndex::from_u64(3);
    let mut test_case = TestCase::builder(source_revision.get())
        .create_resource(before_snapshot)
        .snapshot()
        .create_resource(after_snapshot)
        .pending_update(&admission, |_| {})
        .pending_update(&update, |_| {})
        .successful_snapshot_assisted_update(update, snapshot_index)
        .build();
    test_case
        .entries
        .last_mut()
        .unwrap()
        .expected_status
        .owned_resources
        .remove(&before_snapshot);
    let status = &test_case.entries.last().unwrap().expected_status;

    assert_eq!(status.component_revision, target_revision);
    assert_eq!(status.component_revision_for_replay, source_revision);
    assert_eq!(
        status.authoritative_snapshot,
        Some(AuthoritativeSnapshot {
            index: snapshot_index,
            kind: AuthoritativeSnapshotKind::SnapshotAssistedAutomatic {
                filesystem_snapshot: None,
            },
        })
    );
    assert!(status.skipped_regions.is_in_deleted_region(snapshot_index));
    for index in 4..=7 {
        assert!(
            !status
                .skipped_regions
                .is_in_deleted_region(OplogIndex::from_u64(index))
        );
    }
    assert_eq!(status.last_automatic_snapshot, None);
    assert!(!status.owned_resources.contains_key(&before_snapshot));
    assert!(status.owned_resources.contains_key(&after_snapshot));
    run_test_case(test_case).await;
}

#[test]
async fn failed_snapshot_assisted_update_does_not_promote_snapshot() {
    let source_revision = ComponentRevision::new(1).unwrap();
    let admission = automatic_update(2);
    let update = assisted_update(2, 1, 1, 2);
    let test_case = TestCase::builder(source_revision.get())
        .snapshot()
        .pending_update(&admission, |_| {})
        .pending_update(&update, |_| {})
        .failed_snapshot_assisted_update(update)
        .build();
    let status = &test_case.entries.last().unwrap().expected_status;
    let failed = status.failed_updates.last().unwrap();

    assert_eq!(status.component_revision, source_revision);
    assert_eq!(status.component_revision_for_replay, source_revision);
    assert_eq!(status.authoritative_snapshot, None);
    assert!(
        !status
            .skipped_regions
            .is_in_deleted_region(OplogIndex::from_u64(2))
    );
    assert_eq!(
        status
            .last_automatic_snapshot
            .as_ref()
            .map(|snapshot| snapshot.index),
        Some(OplogIndex::from_u64(2))
    );
    assert_eq!(
        failed.pending_update.as_ref().unwrap().oplog_index,
        OplogIndex::from_u64(4)
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
fn jump_repair_fixture(
    agent_mode: AgentMode,
) -> (
    TestCase,
    AgentStatusRecord,
    AgentStatusRecord,
    AgentStatusRecord,
) {
    let k1 = IdempotencyKey::fresh();
    let k2 = IdempotencyKey::fresh();

    let test_case = TestCase::builder(0)
        .agent_mode(agent_mode)
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
#[test_r::timeout("30s")]
async fn bounded_reconstruction_reads_handoff_without_flushing_newer_buffer() {
    use crate::services::oplog::{CommitLevel, OplogArchive, gated_ephemeral_fixture};

    let (test_case, checkpoint, stale_live, expected) = jump_repair_fixture(AgentMode::Ephemeral);
    let fixture = gated_ephemeral_fixture(100).await;
    for entry in &test_case.entries {
        fixture.oplog.add(entry.oplog_entry.clone()).await.unwrap();
    }
    let receipt = fixture.oplog.commit(CommitLevel::Deferred).await.unwrap();
    let horizon = *receipt.last_key_value().unwrap().0;
    fixture.archive.wait_for_appends(1).await;
    let sentinel = fixture
        .oplog
        .add(OplogEntry::grow_memory(9876))
        .await
        .unwrap();

    let reader = StatusOplogReader::new(
        &test_case,
        &test_case.owned_agent_id,
        AgentMode::Ephemeral,
        Some(fixture.oplog.as_ref()),
        horizon,
    );
    // Exercise both checkpoint fallback and full reconstruction with the writer blocked.
    for candidate in [Some(checkpoint), None] {
        let result =
            calculate_status_with_reader(&test_case, &reader, Some(stale_live.clone()), || async {
                candidate
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result, expected);
        assert_eq!(result.oplog_idx, horizon);
    }
    assert!(test_case.read_starts.lock().unwrap().is_empty());
    assert_eq!(fixture.archive.length().await.unwrap(), 0);
    // The actor's fold must not mistake a DurableOnly precommit tip for acknowledged coverage.
    let sampled = fixture.oplog.current_oplog_index().await;
    let receipt = fixture
        .oplog
        .commit(CommitLevel::DurableOnly)
        .await
        .unwrap();
    let (result, gap, projected_through) = fold_committed_status(
        &test_case,
        &test_case.owned_agent_id,
        fixture.oplog.as_ref(),
        AgentMode::Ephemeral,
        CommitLevel::DurableOnly,
        sampled,
        expected.clone(),
        receipt,
    )
    .await;
    assert_eq!(projected_through, horizon);
    assert_eq!(result.unwrap(), Some(expected));
    assert!(!gap);
    fixture.archive.release(2);
    let remaining = fixture.oplog.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(
        remaining.keys().copied().collect::<Vec<_>>(),
        vec![sentinel]
    );
    fixture.oplog.retire();
    fixture.oplog.closed().await.unwrap();
}

#[test]
#[test_r::timeout("30s")]
async fn bounded_fold_covers_receipt_gap_and_unpersisted_tail() {
    use crate::services::oplog::{CommitLevel, OplogArchive, gated_ephemeral_fixture};

    let mut builder = TestCase::builder(0).agent_mode(AgentMode::Ephemeral);
    for delta in 1..80 {
        builder = builder.grow_memory(delta);
    }
    let test_case = builder.build();
    let fixture = gated_ephemeral_fixture(1).await;
    // Forty threshold batches overflow the receipt cache. The last two remain unpersisted.
    fixture.archive.release(38);
    for entry in &test_case.entries {
        fixture.oplog.add(entry.oplog_entry.clone()).await.unwrap();
    }
    let receipt = fixture.oplog.commit(CommitLevel::Deferred).await.unwrap();
    assert!(!receipt.contains_key(&OplogIndex::from_u64(3)));
    let horizon = *receipt.last_key_value().unwrap().0;
    assert_eq!(horizon, OplogIndex::from_u64(80));
    fixture.archive.wait_for_appends(39).await;
    let sentinel = fixture
        .oplog
        .add(OplogEntry::grow_memory(9876))
        .await
        .unwrap();
    // The receipt proves coverage beyond the sampled tip; an empty consumed receipt uses
    // the precommit sample instead. Both paths must stop before the buffered sentinel.
    for (sampled, receipt) in [
        (OplogIndex::from_u64(78), receipt),
        (horizon, BTreeMap::new()),
    ] {
        let (result, gap, projected_through) = fold_committed_status(
            &test_case,
            &test_case.owned_agent_id,
            fixture.oplog.as_ref(),
            AgentMode::Ephemeral,
            CommitLevel::Deferred,
            sampled,
            test_case.entries[0].expected_status.clone(),
            receipt,
        )
        .await;
        assert_eq!(projected_through, horizon);
        let result = result.unwrap().unwrap();
        assert!(gap);
        assert_eq!(result, test_case.entries[79].expected_status);
        assert_eq!(result.total_linear_memory_size, 200 + (79 * 80 / 2));
    }
    assert_eq!(fixture.archive.length().await.unwrap(), 76);
    assert!(test_case.read_starts.lock().unwrap().is_empty());
    fixture.archive.release(3);
    let remaining = fixture.oplog.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(
        remaining.keys().copied().collect::<Vec<_>>(),
        vec![sentinel]
    );
    fixture.oplog.retire();
    fixture.oplog.closed().await.unwrap();
}

#[test]
async fn bounded_fold_excludes_later_jump_and_rejects_ahead_baseline() {
    let (test_case, checkpoint, stale_live, _) = jump_repair_fixture(AgentMode::Durable);
    let reader = StatusOplogReader::new(
        &test_case,
        &test_case.owned_agent_id,
        AgentMode::Durable,
        None,
        OplogIndex::from_u64(6),
    );
    let result = try_fold_status_from_reader(&test_case, &reader, checkpoint)
        .await
        .unwrap();
    assert_eq!(result, Some(stale_live.clone()));
    let reader = StatusOplogReader::new(
        &test_case,
        &test_case.owned_agent_id,
        AgentMode::Durable,
        None,
        OplogIndex::from_u64(5),
    );
    assert!(
        try_fold_status_from_reader(&test_case, &reader, stale_live)
            .await
            .unwrap()
            .is_none()
    );
}

#[test]
async fn invocation_prefix_repairs_jump_without_reading_persisted_checkpoint() {
    let (test_case, invocation_prefix, _, final_expected) = jump_repair_fixture(AgentMode::Durable);
    let reader = StatusOplogReader::new(
        &test_case,
        &test_case.owned_agent_id,
        AgentMode::Durable,
        None,
        final_expected.oplog_idx,
    );
    let repaired =
        calculate_status_with_reader(&test_case, &reader, Some(invocation_prefix), || async {
            panic!("a surviving invocation prefix must avoid the storage checkpoint read")
        })
        .await
        .unwrap();
    assert_eq!(repaired, Some(final_expected));
    assert!(!test_case.read_starts.lock().unwrap().contains(&1));
}

#[test]
async fn checkpoint_repair_folds_from_checkpoint_after_jump() {
    let (test_case, checkpoint, stale_live, final_expected) =
        jump_repair_fixture(AgentMode::Durable);

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
    let (test_case, _checkpoint, stale_live, final_expected) =
        jump_repair_fixture(AgentMode::Durable);

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
    let (test_case, _checkpoint, _stale_live, final_expected) =
        jump_repair_fixture(AgentMode::Durable);

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
    let (test_case, _checkpoint, stale_live, final_expected) =
        jump_repair_fixture(AgentMode::Durable);

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

fn rollback_fixture() -> TestCase {
    TestCase::builder(0)
        .add(
            OplogEntry::BeginAtomicRegion {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
            },
            |status| status,
        ) // 2
        .add(
            OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                function_name: HostFunctionName::Custom("<scope:transaction>".into()),
                invocation_id: None,
                observational_owner: None,
                request: None,
                durable_function_type: DurableFunctionType::WriteRemoteTransaction(None),
                span_started: None,
            },
            |status| status,
        ) // 3
        .grow_memory(17) // 4: work inside the transaction and atomic region
        .add(
            OplogEntry::EndAtomicRegion {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                begin_index: OplogIndex::from_u64(2),
            },
            |status| status,
        ) // 5
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            IdempotencyKey::fresh(),
            ComponentRevision::INITIAL,
        ) // 6: transaction still open
        .add(
            OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index: OplogIndex::from_u64(3),
                response: None,
                forced_commit: true,
                span_finished: None,
                span_attributes: None,
            },
            |status| status,
        ) // 7: may have drained after acceptance of a runtime cut
        .agent_invocation_finished(
            AgentInvocationResult::AgentInitialization,
            IdempotencyKey::fresh(),
            ComponentRevision::INITIAL,
        ) // 8: safe retirement boundary
        .build()
}

fn fold_rollback_fixture(test_case: &TestCase, tip: usize, chunk_size: usize) -> AgentStatusRecord {
    let mut status = AgentStatusRecord::default();
    for (chunk_index, chunk) in test_case.entries[..tip].chunks(chunk_size).enumerate() {
        let entries = chunk
            .iter()
            .enumerate()
            .map(|(offset, entry)| {
                (
                    OplogIndex::from_u64((chunk_index * chunk_size + offset + 1) as u64),
                    entry.oplog_entry.clone(),
                )
            })
            .collect();
        status = super::update_status_with_new_entries(
            AgentMode::Durable,
            status,
            entries,
            &RetryConfig::default(),
        )
        .unwrap()
        .unwrap();
    }
    status
}

#[test]
fn atomic_rollback_retirement_is_entry_ordered_and_chunk_independent() {
    use crate::durable_host::replay_state::suffix_rollback_region;
    let fixture = rollback_fixture();
    for tip in [6, 7, 8] {
        for chunk_size in 1..=tip {
            let status = fold_rollback_fixture(&fixture, tip, chunk_size);
            let state = &status.atomic_rollback;
            assert_eq!(state.last_work, OplogIndex::from_u64(tip as u64));
            assert_eq!(
                state.open_cut_scopes.contains_key(&OplogIndex::from_u64(3)),
                tip == 6
            );
            if tip < 8 {
                assert_eq!(state.retired_through, OplogIndex::NONE);
                assert_eq!(
                    state.regions.get(&OplogIndex::from_u64(2)),
                    Some(&Some(OplogIndex::from_u64(5)))
                );
                assert_eq!(
                    suffix_rollback_region(
                        state,
                        &status.skipped_regions,
                        status.oplog_idx,
                        Some(OplogIndex::from_u64(4))
                    )
                    .unwrap(),
                    Some(OplogRegion::from_range(3..=tip as u64))
                );
            } else {
                assert!(state.regions.is_empty());
                assert_eq!(state.retired_through, OplogIndex::from_u64(8));
                assert!(
                    suffix_rollback_region(
                        state,
                        &status.skipped_regions,
                        status.oplog_idx,
                        Some(OplogIndex::from_u64(4))
                    )
                    .is_err()
                );
            }
        }
    }
}

#[test]
async fn atomic_rollback_serialized_warm_status_never_reads_its_prefix() {
    use crate::durable_host::replay_state::suffix_rollback_region;
    use golem_common::serialization::{deserialize, serialize};
    let mut fixture = rollback_fixture();
    fixture.entries.truncate(7);
    let baseline = fold_rollback_fixture(&fixture, 6, 2);
    let bytes = serialize(&baseline).unwrap();
    let restored: AgentStatusRecord = deserialize(&bytes).unwrap();
    fixture.read_starts.lock().unwrap().clear();
    let status = try_fold_status_from(
        &fixture,
        &fixture.owned_agent_id,
        AgentMode::Durable,
        restored,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        suffix_rollback_region(
            &status.atomic_rollback,
            &status.skipped_regions,
            status.oplog_idx,
            Some(OplogIndex::from_u64(4))
        )
        .unwrap(),
        Some(OplogRegion::from_range(3..=7))
    );
    let reads = fixture.read_starts.lock().unwrap();
    assert!(!reads.is_empty());
    assert!(
        reads.iter().all(|start| *start == 7),
        "read cached prefix: {reads:?}"
    );
}

#[test]
fn atomic_rollback_snapshot_selection_does_not_destroy_fallback_state() {
    use crate::durable_host::replay_state::suffix_rollback_region;
    let fixture = rollback_fixture();
    let status = fold_rollback_fixture(&fixture, 4, 1);
    let mut selected = status.skipped_regions.clone();
    selected.set_override(DeletedRegions::from_regions([OplogRegion::from_range(
        2..=4,
    )]));
    assert_eq!(
        suffix_rollback_region(&status.atomic_rollback, &selected, status.oplog_idx, None).unwrap(),
        None
    );
    assert_eq!(
        suffix_rollback_region(
            &status.atomic_rollback,
            &status.skipped_regions,
            status.oplog_idx,
            None
        )
        .unwrap(),
        Some(OplogRegion::from_range(3..=4))
    );
}

#[test]
fn atomic_rollback_ignores_cancelled_and_transaction_precommit_as_scope_ends() {
    let fixture = rollback_fixture();
    let mut state = fold_rollback_fixture(&fixture, 6, 1).atomic_rollback;
    state.observe(
        OplogIndex::from_u64(7),
        &OplogEntry::Cancelled {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(3),
            partial: None,
            span_finished: None,
        },
    );
    state.observe(
        OplogIndex::from_u64(8),
        &OplogEntry::PreCommitRemoteTransaction {
            timestamp: Timestamp::now_utc(),
            begin_index: OplogIndex::from_u64(4),
        },
    );
    state.observe(OplogIndex::from_u64(9), &fixture.entries[7].oplog_entry);
    assert!(state.open_cut_scopes.contains_key(&OplogIndex::from_u64(3)));
    assert_eq!(state.regions.len(), 1);
    assert_eq!(state.retired_through, OplogIndex::NONE);
}

#[test]
fn atomic_rollback_long_completed_history_does_not_accumulate_regions() {
    let fixture = rollback_fixture();
    let mut state = golem_common::model::AtomicRollbackState::default();
    for invocation in 0..10_000 {
        let begin = OplogIndex::from_u64(invocation * 3 + 1);
        state.observe(begin, &fixture.entries[1].oplog_entry);
        state.observe(
            begin.next(),
            &OplogEntry::EndAtomicRegion {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                begin_index: begin,
            },
        );
        state.observe(begin.next().next(), &fixture.entries[7].oplog_entry);
        assert!(state.regions.is_empty());
        assert!(state.open_cut_scopes.is_empty());
    }
    assert_eq!(state.retired_through, OplogIndex::from_u64(30_000));
}

#[test]
fn baseline_rejects_newly_exposed_snapshot_prefix() {
    let mut baseline = AgentStatusRecord {
        oplog_idx: OplogIndex::from_u64(8),
        ..Default::default()
    };
    baseline
        .skipped_regions
        .set_override(DeletedRegions::from_regions([OplogRegion::from_range(
            2..=5,
        )]));
    assert!(super::baseline_is_invalidated(
        &baseline,
        &DeletedRegions::new(),
        &DeletedRegions::new()
    ));
    let mut later = baseline.skipped_regions.clone();
    later.add(OplogRegion::from_range(9..=10));
    assert!(!super::baseline_is_invalidated(
        &baseline,
        &DeletedRegions::new(),
        &later
    ));
}

#[test]
async fn atomic_rollback_jump_and_revert_repair_from_a_retained_prefix() {
    for revert in [false, true] {
        let mut fixture = rollback_fixture();
        let prefix = fold_rollback_fixture(&fixture, 1, 1);
        let stale = fold_rollback_fixture(&fixture, 8, 2);
        assert_eq!(
            stale.atomic_rollback.retired_through,
            OplogIndex::from_u64(8)
        );
        fixture.entries.push(TestEntry {
            oplog_entry: if revert {
                OplogEntry::revert(OplogRegion::from_range(2..=8))
            } else {
                OplogEntry::jump(None, OplogRegion::from_range(3..=8))
            },
            expected_status: AgentStatusRecord::default(),
        });
        fixture.read_starts.lock().unwrap().clear();
        let reader = StatusOplogReader::new(
            &fixture,
            &fixture.owned_agent_id,
            AgentMode::Durable,
            None,
            OplogIndex::from_u64(9),
        );
        let repaired =
            calculate_status_with_reader(&fixture, &reader, Some(stale), || async { Some(prefix) })
                .await
                .unwrap()
                .unwrap();
        assert_eq!(repaired.atomic_rollback.retired_through, OplogIndex::NONE);
        assert!(repaired.atomic_rollback.open_cut_scopes.is_empty());
        assert_eq!(repaired.atomic_rollback.regions.len(), usize::from(!revert));
        if !revert {
            assert_eq!(
                repaired
                    .atomic_rollback
                    .regions
                    .get(&OplogIndex::from_u64(2)),
                Some(&None)
            );
            assert_eq!(repaired.atomic_rollback.last_work, OplogIndex::from_u64(2));
        }
        assert!(
            fixture
                .read_starts
                .lock()
                .unwrap()
                .iter()
                .all(|start| *start > 1)
        );
    }
}

#[test]
async fn atomic_rollback_failed_snapshot_update_restores_prefix_without_full_scan() {
    let update = UpdateDescription::SnapshotBased {
        target_revision: ComponentRevision::new(2).unwrap(),
        payload: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".into(),
        filesystem_snapshot: None,
    };
    let fixture = TestCase::builder(0)
        .add(
            OplogEntry::BeginAtomicRegion {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
            },
            |status| status,
        )
        .grow_memory(17)
        .pending_update(&update, |_| {})
        .failed_update(update)
        .build();
    let prefix = fold_rollback_fixture(&fixture, 3, 1);
    let pending_reader = StatusOplogReader::new(
        &fixture,
        &fixture.owned_agent_id,
        AgentMode::Durable,
        None,
        OplogIndex::from_u64(4),
    );
    let pending = calculate_status_with_reader(&fixture, &pending_reader, None, || async { None })
        .await
        .unwrap()
        .unwrap();
    assert!(pending.atomic_rollback.regions.is_empty());
    fixture.read_starts.lock().unwrap().clear();
    let reader = StatusOplogReader::new(
        &fixture,
        &fixture.owned_agent_id,
        AgentMode::Durable,
        None,
        OplogIndex::from_u64(5),
    );
    let repaired =
        calculate_status_with_reader(&fixture, &reader, Some(pending), || async { Some(prefix) })
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        repaired
            .atomic_rollback
            .regions
            .get(&OplogIndex::from_u64(2)),
        Some(&None)
    );
    assert_eq!(repaired.total_linear_memory_size, 217);
    assert!(
        fixture
            .read_starts
            .lock()
            .unwrap()
            .iter()
            .all(|start| *start >= 4)
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
        let instance_id = Uuid::new_v4();
        let status = AgentStatusRecord {
            component_revision,
            component_revision_for_replay: component_revision,
            component_size: 100,
            total_linear_memory_size: 200,
            oplog_idx: OplogIndex::INITIAL,
            export_fork_admissions: golem_common::model::ExportForkAdmissions {
                owner_fingerprint: Some(AgentFingerprint(instance_id)),
                ..Default::default()
            },
            ..Default::default()
        };
        TestCaseBuilder {
            entries: vec![TestEntry {
                oplog_entry: OplogEntry::create(Box::new(
                    golem_common::model::oplog::CreateParameters {
                        agent_id: owned_agent_id.agent_id(),
                        owner_kind: OwnerKind::ComponentAgent,
                        agent_mode: AgentMode::Durable,
                        component_revision,
                        env: vec![],
                        environment_id: owned_agent_id.environment_id(),
                        created_by: account_id,
                        parent: None,
                        component_size: 100,
                        initial_total_linear_memory_size: 200,
                        initial_active_plugins: HashSet::new(),
                        local_agent_config: Vec::new(),
                        original_phantom_id: None,
                        instance_id,
                    },
                )),
                expected_status: status.clone(),
            }],
            previous_status_record: status,
            owned_agent_id,
        }
    }

    fn agent_mode(mut self, agent_mode: AgentMode) -> Self {
        assert_eq!(self.entries.len(), 1);
        let OplogEntry::Create { parameters, .. } = &mut self.entries[0].oplog_entry else {
            unreachable!()
        };
        parameters.agent_mode = agent_mode;
        self.entries[0].expected_status.agent_mode = agent_mode;
        self.previous_status_record.agent_mode = agent_mode;
        self
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
                wallet_pin: Box::new(test_invocation_wallet_pin()),
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
                span_started: None,
            },
            |status| status,
        )
        .add(
            OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index,
                response: Some(OplogPayload::Inline(Box::new(o))),
                forced_commit: false,
                span_finished: None,
                span_attributes: None,
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

    pub fn create_resource(self, id: AgentResourceId) -> Self {
        let timestamp = Timestamp::now_utc().rounded();
        let resource_type_id = ResourceTypeId {
            name: format!("resource-{id}"),
            owner: "test:resources".to_string(),
        };
        self.add(
            OplogEntry::CreateResource {
                timestamp,
                entity_parent_start_index: None,
                id,
                resource_type_id: resource_type_id.clone(),
            },
            move |mut status| {
                status.owned_resources.insert(
                    id,
                    AgentResourceDescription {
                        created_at: timestamp,
                        resource_owner: resource_type_id.owner,
                        resource_name: resource_type_id.name,
                    },
                );
                status
            },
        )
    }

    pub fn snapshot(self) -> Self {
        self.snapshot_with_filesystem(None)
    }

    pub fn snapshot_with_filesystem(
        self,
        filesystem_snapshot: Option<FilesystemSnapshotName>,
    ) -> Self {
        let oplog_idx = OplogIndex::from_u64(self.entries.len() as u64 + 1);
        let timestamp = Timestamp::now_utc().rounded();
        self.add(
            OplogEntry::Snapshot {
                timestamp,
                data: OplogPayload::Inline(Box::new(vec![])),
                mime_type: "application/octet-stream".to_string(),
                active_cards: Vec::new(),
                wallet_generation: 0,
                filesystem_snapshot: filesystem_snapshot.clone(),
            },
            move |mut status| {
                if let Some(last) = &status.last_automatic_snapshot
                    && !matches!(last.files, SnapshotFiles::Unconfirmed(_))
                {
                    status.previous_usable_automatic_snapshot = Some(UsableAutomaticSnapshot {
                        index: last.index,
                        component_revision: last.component_revision,
                        filesystem_snapshot: last.files.name().cloned(),
                    });
                }
                status.last_automatic_snapshot = Some(AutomaticSnapshot {
                    index: oplog_idx,
                    timestamp,
                    component_revision: status.component_revision,
                    files: SnapshotFiles::named(filesystem_snapshot.clone()),
                });
                status
            },
        )
    }

    pub fn snapshot_confirmed(
        self,
        filesystem_snapshot: FilesystemSnapshotName,
        expect_confirmed: bool,
    ) -> Self {
        self.add(
            OplogEntry::snapshot_confirmed(filesystem_snapshot.clone()).rounded(),
            move |mut status| {
                if expect_confirmed {
                    status.last_automatic_snapshot =
                        status
                            .last_automatic_snapshot
                            .take()
                            .map(|last| AutomaticSnapshot {
                                files: last.files.confirmed(&filesystem_snapshot),
                                ..last
                            });
                }
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
        // The executor drops the history after `target` up to the entry right before the
        // `Revert` entry.
        let last = OplogIndex::from_u64(self.entries.len() as u64);
        let region = OplogRegion {
            start: target.next(),
            end: last,
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
            status.pending_updates = old_status.pending_updates;
            status.invocation_results = old_status.invocation_results;
            status
                .invocation_results
                .set_revert_generation(revert_generation);
            status.component_revision_for_replay = old_status.component_revision_for_replay;
            status.component_revision_start_index = old_status.component_revision_start_index;
            status.authoritative_snapshot = old_status.authoritative_snapshot;
            status.last_automatic_snapshot = old_status.last_automatic_snapshot;
            status.previous_usable_automatic_snapshot =
                old_status.previous_usable_automatic_snapshot;

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
                OplogErrorKind::Invocation,
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
        let update_attempt_index = self.entries.last().and_then(|entry| {
            let status = &entry.expected_status;
            status
                .pending_invocations
                .iter()
                .find(|invocation| {
                    invocation.manual_update_target_revision
                        == Some(*update_description.target_revision())
                })
                .map(|invocation| invocation.oplog_index)
                .or_else(|| {
                    status.pending_updates.front().and_then(|pending| {
                        (pending.target_revision == *update_description.target_revision()
                            && pending.kind == PendingUpdateKind::Automatic
                            && pending.oplog_index == pending.admission_index)
                            .then_some(pending.admission_index)
                    })
                })
        });
        let entry =
            OplogEntry::pending_update(update_description.clone(), update_attempt_index).rounded();
        let oplog_idx = OplogIndex::from_u64(self.entries.len() as u64 + 1);
        self.add(entry.clone(), move |mut status| {
            let kind = PendingUpdateKind::of(update_description);
            let admission_index = update_attempt_index.unwrap_or(oplog_idx);
            let target_revision = *update_description.target_revision();
            let refines_automatic_admission = update_attempt_index.is_some()
                && !matches!(update_description, UpdateDescription::SnapshotBased { .. })
                && status.pending_updates.front().is_some_and(|pending| {
                    pending.admission_index == admission_index
                        && pending.target_revision == target_revision
                        && pending.kind == PendingUpdateKind::Automatic
                        && pending.oplog_index == pending.admission_index
                });
            if refines_automatic_admission {
                let pending = status.pending_updates.front_mut().unwrap();
                pending.oplog_index = oplog_idx;
                pending.kind = kind;
            } else {
                status.pending_updates.push_back(PendingUpdateRef {
                    timestamp: entry.timestamp(),
                    oplog_index: oplog_idx,
                    admission_index,
                    target_revision,
                    kind,
                });
            }

            if update_attempt_index.is_some()
                && let Some(position) = status
                    .pending_invocations
                    .iter()
                    .position(|invocation| invocation.oplog_index == admission_index)
            {
                status.pending_invocations.remove(position);
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
            None,
        )
        .rounded();
        self.add(entry.clone(), move |mut status| {
            let target_revision = *update_description.target_revision();
            let applied_update = if status
                .pending_updates
                .front()
                .is_some_and(|pending| pending.target_revision == target_revision)
            {
                status.pending_updates.pop_front()
            } else {
                status
                    .pending_invocations
                    .iter()
                    .position(|invocation| {
                        invocation.manual_update_target_revision == Some(target_revision)
                    })
                    .map(|position| status.pending_invocations.remove(position))
                    .map(|invocation| PendingUpdateRef {
                        timestamp: invocation.timestamp,
                        oplog_index: invocation.oplog_index,
                        admission_index: invocation.oplog_index,
                        target_revision,
                        kind: PendingUpdateKind::SnapshotBased {
                            filesystem_snapshot: None,
                        },
                    })
            };
            status.successful_updates.push(SuccessfulUpdateRecord {
                timestamp: entry.timestamp(),
                target_revision,
                oplog_index: status.oplog_idx,
                filesystem_snapshot: match &update_description {
                    UpdateDescription::SnapshotBased {
                        filesystem_snapshot,
                        ..
                    } => filesystem_snapshot.clone(),
                    UpdateDescription::Automatic { .. }
                    | UpdateDescription::SnapshotAssistedAutomatic { .. } => None,
                },
                pending_update: applied_update.clone(),
                snapshot_assisted_details: None,
            });
            status.component_size = new_component_size;
            status.component_revision = *update_description.target_revision();
            status.component_revision_start_index = status.oplog_idx;
            status.active_plugins = new_active_plugins.clone();
            status.last_automatic_snapshot = None;
            status.previous_usable_automatic_snapshot = None;

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
                status.authoritative_snapshot = applied_update.map(|au| AuthoritativeSnapshot {
                    index: au.oplog_index,
                    kind: AuthoritativeSnapshotKind::ManualUpdate,
                });
            };

            status
        })
    }

    pub fn successful_snapshot_assisted_update(
        self,
        update_description: UpdateDescription,
        snapshot_index: OplogIndex,
    ) -> Self {
        let pending = self
            .entries
            .last()
            .unwrap()
            .expected_status
            .pending_updates
            .front()
            .unwrap()
            .clone();
        let (source_component_revision, source_revision_start_index, _) =
            snapshot_assisted_pending_details(&pending);
        let details = SnapshotAssistedUpdateDetails {
            pending_update_index: pending.oplog_index,
            source_component_revision,
            source_revision_start_index,
            snapshot_index,
        };
        let target_revision = *update_description.target_revision();
        let entry = OplogEntry::successful_update(
            target_revision,
            2000,
            None,
            HashSet::new(),
            Some(details.clone()),
        )
        .rounded();
        self.add(entry.clone(), move |mut status| {
            let applied_update = status.pending_updates.pop_front();
            status.successful_updates.push(SuccessfulUpdateRecord {
                timestamp: entry.timestamp(),
                target_revision,
                oplog_index: status.oplog_idx,
                filesystem_snapshot: None,
                pending_update: applied_update,
                snapshot_assisted_details: Some(details.clone()),
            });
            status.component_size = 2000;
            status.component_revision = target_revision;
            status.component_revision_start_index = status.oplog_idx;
            status.component_revision_for_replay = source_component_revision;
            status.authoritative_snapshot = Some(AuthoritativeSnapshot {
                index: snapshot_index,
                kind: AuthoritativeSnapshotKind::SnapshotAssistedAutomatic {
                    filesystem_snapshot: None,
                },
            });
            status.skipped_regions.add(OplogRegion::from_index_range(
                OplogIndex::INITIAL.next()..=snapshot_index,
            ));
            status.last_automatic_snapshot = None;
            status.previous_usable_automatic_snapshot = None;
            status
        })
    }

    pub fn failed_update(self, update_description: UpdateDescription) -> Self {
        let update_attempt_index = self.entries.last().and_then(|entry| {
            let status = &entry.expected_status;
            let manual_admission =
                matches!(update_description, UpdateDescription::SnapshotBased { .. })
                    .then(|| {
                        status
                            .pending_invocations
                            .iter()
                            .find(|invocation| {
                                invocation.manual_update_target_revision
                                    == Some(*update_description.target_revision())
                            })
                            .map(|invocation| invocation.oplog_index)
                    })
                    .flatten();
            manual_admission.or_else(|| {
                status
                    .pending_updates
                    .front()
                    .filter(|pending| {
                        pending.target_revision == *update_description.target_revision()
                    })
                    .map(|pending| pending.admission_index)
                    .or_else(|| {
                        status
                            .pending_invocations
                            .iter()
                            .find(|invocation| {
                                invocation.manual_update_target_revision
                                    == Some(*update_description.target_revision())
                            })
                            .map(|invocation| invocation.oplog_index)
                    })
            })
        });
        let entry = OplogEntry::failed_update(
            *update_description.target_revision(),
            Some("details".to_string()),
            None,
            update_attempt_index,
            None,
        )
        .rounded();
        self.add(entry.clone(), move |mut status| {
            let target_revision = *update_description.target_revision();
            let matches_pending = status.pending_updates.front().is_some_and(|pending| {
                update_attempt_index
                    .is_some_and(|attempt_index| pending.admission_index == attempt_index)
                    || (update_attempt_index.is_none()
                        && pending.target_revision == target_revision)
            });
            let applied_update = if matches_pending {
                status.pending_updates.pop_front()
            } else {
                status
                    .pending_invocations
                    .iter()
                    .position(|invocation| {
                        update_attempt_index
                            .is_some_and(|attempt_index| invocation.oplog_index == attempt_index)
                            || (update_attempt_index.is_none()
                                && invocation.manual_update_target_revision
                                    == Some(target_revision))
                    })
                    .map(|position| status.pending_invocations.remove(position))
                    .map(|invocation| PendingUpdateRef {
                        timestamp: invocation.timestamp,
                        oplog_index: invocation.oplog_index,
                        admission_index: invocation.oplog_index,
                        target_revision,
                        kind: PendingUpdateKind::SnapshotBased {
                            filesystem_snapshot: None,
                        },
                    })
            };
            status.failed_updates.push(FailedUpdateRecord {
                timestamp: entry.timestamp(),
                target_revision,
                details: Some("details".to_string()),
                pending_update: applied_update,
                snapshot_assisted_details: None,
                snapshot_fault: None,
            });

            if status.skipped_regions.is_overridden() {
                status.skipped_regions.drop_override();
            }

            status
        })
    }

    pub fn failed_snapshot_assisted_update(self, update_description: UpdateDescription) -> Self {
        let pending = self
            .entries
            .last()
            .unwrap()
            .expected_status
            .pending_updates
            .front()
            .unwrap()
            .clone();
        let (source_component_revision, source_revision_start_index, selection) =
            snapshot_assisted_pending_details(&pending);
        let snapshot_index = selection.index;
        let details = FailedSnapshotAssistedUpdateDetails {
            pending_update_index: pending.oplog_index,
            source_component_revision,
            source_revision_start_index,
            snapshot_index,
        };
        let target_revision = *update_description.target_revision();
        let entry = OplogEntry::failed_update(
            target_revision,
            Some("details".to_string()),
            Some(details.clone()),
            Some(pending.admission_index),
            None,
        )
        .rounded();
        self.add(entry.clone(), move |mut status| {
            let applied_update = status.pending_updates.pop_front();
            status.failed_updates.push(FailedUpdateRecord {
                timestamp: entry.timestamp(),
                target_revision,
                details: Some("details".to_string()),
                pending_update: applied_update,
                snapshot_assisted_details: Some(details.clone()),
                snapshot_fault: None,
            });
            status
        })
    }

    pub fn build(mut self) -> TestCase {
        // These fixtures assert the other status fields. Rollback state has dedicated assertions
        // below, while each cached baseline here must also carry its derived rollback summary.
        for tip in 0..self.entries.len() {
            let visibility = &self.entries[tip].expected_status;
            let mut state = golem_common::model::AtomicRollbackState::default();
            for (offset, entry) in self.entries[..=tip].iter().enumerate() {
                let index = OplogIndex::from_u64(offset as u64 + 1);
                if !visibility.skipped_regions.is_in_deleted_region(index)
                    && !visibility.deleted_regions.is_in_deleted_region(index)
                {
                    state.observe(index, &entry.oplog_entry);
                }
            }
            self.entries[tip].expected_status.atomic_rollback = state;
        }
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
    async fn staged_exists(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _stage_id: uuid::Uuid,
    ) -> Result<bool, String> {
        unimplemented!()
    }

    async fn lock_lifecycle(&self, _: &AgentId) -> crate::services::oplog::OplogLifecycleGuard {
        unreachable!()
    }

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
        _lifecycle: &mut crate::services::oplog::OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _initial_entry: OplogEntry,
        _initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        _shard_epoch: Option<golem_common::model::ShardEpoch>,
    ) -> Arc<dyn Oplog + 'static> {
        unreachable!()
    }

    async fn create_fresh(
        &self,
        _lifecycle: &mut crate::services::oplog::OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _initial_entry: OplogEntry,
        _initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        _shard_epoch: Option<golem_common::model::ShardEpoch>,
    ) -> Arc<dyn Oplog + 'static> {
        unreachable!()
    }

    async fn open(
        &self,
        _lifecycle: &mut crate::services::oplog::OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _last_oplog_index: Option<OplogIndex>,
        _initial_worker_metadata: AgentMetadata,
        _last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        _execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        _shard_epoch: Option<golem_common::model::ShardEpoch>,
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

    async fn assert_owning_epoch(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _expected_epoch: golem_common::model::ShardEpoch,
    ) -> Result<(), crate::services::oplog::OplogError> {
        Ok(())
    }

    async fn delete(
        &self,
        _lifecycle: &mut crate::services::oplog::OplogLifecycleGuard,
        _owned_agent_id: &OwnedAgentId,
        _agent_mode: AgentMode,
        _expected_epoch: Option<golem_common::model::ShardEpoch>,
    ) -> Result<(), crate::services::oplog::OplogError> {
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
    let session_key = StreamInvocationId {
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
    let record = StreamSessionRecord::Prepared(StreamSessionPreparedRecord {
        format_version: 1,
        public_session_id: session_key.idempotency_key.value.clone(),
        session_key: session_key.idempotency_key.clone(),
        expiry_policy: StreamSessionExpiryPolicy::None,
        expiry_deadline_millis: None,
        attempt: StartAttemptDescriptor {
            format_version: 1,
            session_key: session_key.clone(),
            attachment_id,
            expected_callee_fingerprint: session_key.callee_fingerprint,
            attempt_id: AttemptId::fresh(),
            invocation: PersistedStreamInvocationDescriptor {
                format_version: 1,
                session_key: session_key.clone(),
                target_component_revision: ComponentRevision::INITIAL,
                target: golem_common::base_model::durable_stream::PersistedInvocationTarget::AgentMethod {
                    method_name: "large-cold-status".to_string(),
                },
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
            summary: None,
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

#[test]
async fn incremental_fold_does_not_download_payload_reverted_in_later_chunk() {
    let mut test_case = TestCase::builder(1).build();
    let baseline = test_case.entries[0].expected_status.clone();
    let missing_payload = || OplogEntry::StreamSession {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: None,
        record: OplogPayload::External {
            payload_id: PayloadId::new(),
            md5_hash: vec![0; 16],
            cached: None,
        },
        summary: None,
    };
    test_case.entries.push(TestEntry {
        oplog_entry: missing_payload(),
        expected_status: baseline.clone(),
    });
    test_case.entries.push(TestEntry {
        oplog_entry: missing_payload(),
        expected_status: baseline.clone(),
    });
    test_case.entries.push(TestEntry {
        oplog_entry: OplogEntry::revert(OplogRegion {
            start: OplogIndex::from_u64(2),
            end: OplogIndex::from_u64(3),
        }),
        expected_status: baseline.clone(),
    });

    let status = calculate_last_known_status_for_existing_worker(
        &test_case,
        &test_case.owned_agent_id,
        AgentMode::Durable,
        Some(baseline.clone()),
    )
    .await
    .expect("reverted external payloads must not be downloaded");

    assert!(status.durable_stream_sessions.iter().next().is_none());
    test_case.entries.pop();
    assert!(
        calculate_last_known_status_for_existing_worker(
            &test_case,
            &test_case.owned_agent_id,
            AgentMode::Durable,
            Some(baseline),
        )
        .await
        .is_err(),
        "a missing retained payload must still fail reconstruction"
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
                snapshot_assisted_details: None,
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
                regions_without_updates(DeletedRegions::new()),
                std::collections::VecDeque::new(),
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
    let OplogEntry::Create { parameters, .. } = &mut create else {
        unreachable!()
    };
    parameters.initial_active_plugins.insert(grant_id);

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
            wallet_generation: 1,
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
        OplogEntry::card_event_queued(None, Box::new(QueuedCardEvent::revoke(card_id))),
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
            OplogEntry::card_event_queued(None, Box::new(QueuedCardEvent::revoke(card_id))),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_revoked(None, OplogIndex::from_u64(1), card_id, 1),
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
            local_wallet_generation: 1,
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
            local_wallet_generation: 1,
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
        OplogEntry::card_installed(None, None, Box::new(test_card(card_id).into()), 2),
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
            OplogEntry::card_event_queued(None, Box::new(QueuedCardEvent::revoke(first_card_id))),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_event_queued(None, Box::new(QueuedCardEvent::revoke(second_card_id))),
        ),
        (
            OplogIndex::from_u64(3),
            OplogEntry::card_event_queued(
                None,
                Box::new(QueuedCardEvent::revoke(unrelated_card_id)),
            ),
        ),
        (
            OplogIndex::from_u64(4),
            OplogEntry::CardRevokedCascade {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                revoked_card_ids: vec![first_card_id, second_card_id],
                affected_wallets: Vec::new(),
                local_wallet_generation: 1,
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
                local_wallet_generation: 1,
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
                Box::new(QueuedCardEvent::transfer_started(
                    completed_transfer_id,
                    transferred_card.clone(),
                    target_holder.clone(),
                )),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_event_queued(
                None,
                Box::new(QueuedCardEvent::transfer_started(
                    pending_transfer_id,
                    pending_card.clone(),
                    target_holder.clone(),
                )),
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
                Box::new(QueuedCardEvent::transfer_started_with_source(
                    transfer_id,
                    source_card_id,
                    installed_child.clone(),
                    target_holder.clone(),
                )),
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
                Box::new(QueuedCardEvent::transfer_started_with_source(
                    transfer_id,
                    source_card_id,
                    installed_child.clone(),
                    target_holder.clone(),
                )),
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
            Box::new(QueuedCardEvent::transfer_received(
                transfer_id,
                source_card_id,
                card.clone(),
            )),
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
                && receipt.source_card_id == source_card_id
                && receipt.card_id == card.card_id
                && receipt.card.as_ref() == Some(&stored_card)
    ));
    assert!(matches!(
        status.received_card_transfers.get(&transfer_id),
        Some(ReceivedCardTransferState::Received {
            source_card_id: recorded_source_card_id,
            card: recorded_card,
        }) if *recorded_source_card_id == source_card_id && recorded_card == &stored_card
    ));
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
                Box::new(QueuedCardEvent::transfer_received(
                    skipped_transfer_id,
                    source_card_id,
                    card.clone(),
                )),
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
                Box::new(QueuedCardEvent::transfer_received(
                    deleted_transfer_id,
                    source_card_id,
                    card.clone(),
                )),
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
                source_card_id: recorded_source_card_id,
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
                Box::new(QueuedCardEvent::transfer_received(
                    transfer_id,
                    source_card_id,
                    card.clone(),
                )),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_transferred(
                None,
                transfer_id,
                source_card_id,
                card.card_id,
                target_holder,
                Box::new(card.into()),
                7,
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
                Box::new(QueuedCardEvent::transfer_received(
                    transfer_id,
                    source_card_id,
                    card.clone(),
                )),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_transferred(
                None,
                transfer_id,
                golem_common::model::card::CardId::new(),
                card.card_id,
                target_holder,
                Box::new(card.clone().into()),
                7,
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
                && receipt.source_card_id == source_card_id
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
                Box::new(QueuedCardEvent::transfer_started(
                    transfer_id,
                    pending_card.clone(),
                    target_holder.clone(),
                )),
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
                Box::new(QueuedCardEvent::transfer_started(
                    transfer_id,
                    card.clone(),
                    target_holder.clone(),
                )),
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
                Box::new(QueuedCardEvent::transfer_started(
                    transfer_id,
                    card.clone(),
                    target_holder.clone(),
                )),
            ),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_transferred(
                None,
                transfer_id,
                card.card_id,
                card.card_id,
                target_holder,
                Box::new(card.clone().into()),
                0,
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
                Box::new(QueuedCardEvent::transfer_started(
                    transfer_id,
                    card.clone(),
                    target_holder.clone(),
                )),
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
            OplogEntry::card_event_queued(None, Box::new(QueuedCardEvent::revoke(card_id))),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_event_queued(None, Box::new(QueuedCardEvent::revoke(card_id))),
        ),
        (
            OplogIndex::from_u64(3),
            OplogEntry::card_revoked(None, OplogIndex::from_u64(1), card_id, 1),
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
            Box::new(QueuedCardEvent::install(card.clone())),
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
            OplogEntry::card_event_queued(None, Box::new(QueuedCardEvent::install(card.clone()))),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_installed(
                None,
                Some(OplogIndex::from_u64(1)),
                Box::new(card.into()),
                1,
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
fn card_install_failed_removes_pending_install() {
    let card_id = golem_common::model::card::CardId::new();
    let card = test_card(card_id);
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(1),
            OplogEntry::card_event_queued(None, Box::new(QueuedCardEvent::install(card))),
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
            OplogEntry::card_event_queued(None, Box::new(QueuedCardEvent::revoke(card_id))),
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
            OplogEntry::card_event_queued(None, Box::new(QueuedCardEvent::install(card.clone()))),
        ),
        (
            OplogIndex::from_u64(2),
            OplogEntry::card_installed(
                None,
                Some(OplogIndex::from_u64(1)),
                Box::new(card.into()),
                1,
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
                wallet_generation: 1,
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

fn update_fields_snapshot(filesystem_snapshot: Option<FilesystemSnapshotName>) -> OplogEntry {
    OplogEntry::Snapshot {
        timestamp: Timestamp::from(1_000),
        data: OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
        active_cards: Vec::new(),
        wallet_generation: 0,
        filesystem_snapshot,
    }
}

/// The index of the manual-update `PendingUpdate` entry whose snapshot is the authoritative
/// baseline, if the baseline is a manual update.
fn manual_update_baseline(authoritative: &Option<AuthoritativeSnapshot>) -> Option<OplogIndex> {
    authoritative
        .as_ref()
        .filter(|snapshot| snapshot.kind == AuthoritativeSnapshotKind::ManualUpdate)
        .map(|snapshot| snapshot.index)
}

/// A region fold with `deleted` as its deleted regions, no skipped regions and no update steps.
fn regions_without_updates(deleted: DeletedRegions) -> super::FoldedRegions<'static> {
    static NO_STEPS: BTreeMap<OplogIndex, super::update_queue::UpdateStep> = BTreeMap::new();
    super::FoldedRegions {
        deleted,
        skipped: DeletedRegions::new(),
        steps: &NO_STEPS,
    }
}

fn empty_update_fields() -> super::UpdateFields {
    super::UpdateFields {
        failed_updates: Vec::new(),
        successful_updates: Vec::new(),
        component_revision: ComponentRevision::new(3).unwrap(),
        component_size: 10,
        component_revision_for_replay: ComponentRevision::new(1).unwrap(),
        component_revision_start_index: OplogIndex::INITIAL,
        authoritative_snapshot: None,
        last_automatic_snapshot: None,
        previous_usable_automatic_snapshot: None,
    }
}

#[test]
fn update_fields_after_snapshot_entries_keep_one_candidate_and_its_usable_predecessor() {
    let first = FilesystemSnapshotName::periodic();
    let second = FilesystemSnapshotName::periodic();

    let fields = empty_update_fields()
        .after(
            OplogIndex::from_u64(2),
            &update_fields_snapshot(Some(first.clone())),
            &super::update_queue::UpdateStep::Unchanged,
        )
        .unwrap()
        .after(
            OplogIndex::from_u64(3),
            &OplogEntry::snapshot_confirmed(first.clone()),
            &super::update_queue::UpdateStep::Unchanged,
        )
        .unwrap()
        .after(
            OplogIndex::from_u64(4),
            &update_fields_snapshot(Some(second.clone())),
            &super::update_queue::UpdateStep::Unchanged,
        )
        .unwrap()
        .after(
            OplogIndex::from_u64(5),
            &OplogEntry::snapshot_confirmed(first.clone()),
            &super::update_queue::UpdateStep::Unchanged,
        )
        .unwrap();

    assert_eq!(
        fields.last_automatic_snapshot,
        Some(AutomaticSnapshot {
            index: OplogIndex::from_u64(4),
            timestamp: Timestamp::from(1_000),
            component_revision: ComponentRevision::new(3).unwrap(),
            files: SnapshotFiles::Unconfirmed(second),
        })
    );
    assert_eq!(
        fields.previous_usable_automatic_snapshot,
        Some(UsableAutomaticSnapshot {
            index: OplogIndex::from_u64(2),
            component_revision: ComponentRevision::new(3).unwrap(),
            filesystem_snapshot: Some(first),
        })
    );
}

#[test]
fn update_fields_after_a_successful_snapshot_based_update_clear_the_automatic_snapshots() {
    let target = ComponentRevision::new(4).unwrap();
    let entries = BTreeMap::from([
        (OplogIndex::from_u64(2), update_fields_snapshot(None)),
        (OplogIndex::from_u64(3), update_fields_snapshot(None)),
        (
            OplogIndex::from_u64(4),
            OplogEntry::PendingUpdate {
                timestamp: Timestamp::from(2_000),
                description: UpdateDescription::SnapshotBased {
                    target_revision: target,
                    payload: OplogPayload::Inline(Box::new(vec![])),
                    mime_type: "application/octet-stream".to_string(),
                    filesystem_snapshot: None,
                },
                update_attempt_index: None,
            },
        ),
        (
            OplogIndex::from_u64(5),
            OplogEntry::SuccessfulUpdate {
                timestamp: Timestamp::from(3_000),
                target_revision: target,
                new_component_size: 20,
                new_total_linear_memory_size: None,
                new_active_plugins: HashSet::new(),
                snapshot_assisted_details: None,
            },
        ),
    ]);
    let regions = fold_regions(&AgentStatusRecord::default(), &entries);
    let fields = super::calculate_update_fields(
        empty_update_fields(),
        &regions.deleted,
        &regions.steps,
        &entries,
    )
    .unwrap();

    assert_eq!(fields.last_automatic_snapshot, None);
    assert_eq!(fields.previous_usable_automatic_snapshot, None);
    assert!(regions.queue.into_open().0.is_empty());
    assert_eq!(fields.component_revision, target);
    assert_eq!(fields.component_size, 20);
    assert_eq!(fields.component_revision_for_replay, target);
    assert_eq!(
        manual_update_baseline(&fields.authoritative_snapshot),
        Some(OplogIndex::from_u64(4))
    );
    assert_eq!(fields.successful_updates.len(), 1);
}

#[test]
fn update_fields_skip_the_entries_in_a_deleted_region() {
    let name = FilesystemSnapshotName::periodic();
    let entries = BTreeMap::from([
        (
            OplogIndex::from_u64(2),
            update_fields_snapshot(Some(name.clone())),
        ),
        (
            OplogIndex::from_u64(3),
            OplogEntry::snapshot_confirmed(name.clone()),
        ),
    ]);
    let deleted = DeletedRegionsBuilder::from_regions(vec![OplogRegion::from_index_range(
        OplogIndex::from_u64(3)..=OplogIndex::from_u64(3),
    )])
    .build();

    let fields =
        super::calculate_update_fields(empty_update_fields(), &deleted, &BTreeMap::new(), &entries)
            .unwrap();

    assert_eq!(
        fields.last_automatic_snapshot.map(|last| last.files),
        Some(SnapshotFiles::Unconfirmed(name))
    );
}

/// The filesystem snapshot name of the pending update at `index` in `entries`, when it is a
/// snapshot-based update.
fn pending_update_name(
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    index: Option<OplogIndex>,
) -> Option<FilesystemSnapshotName> {
    match entries.get(&index?)? {
        OplogEntry::PendingUpdate {
            description:
                UpdateDescription::SnapshotBased {
                    filesystem_snapshot,
                    ..
                },
            ..
        } => filesystem_snapshot.clone(),
        _ => None,
    }
}

/// The target revision and the attempt index of each failed update in `entries`.
fn cancelled_updates(entries: &[OplogEntry]) -> Vec<(ComponentRevision, Option<OplogIndex>)> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            OplogEntry::FailedUpdate {
                target_revision,
                update_attempt_index,
                ..
            } => Some((*target_revision, *update_attempt_index)),
            _ => None,
        })
        .collect()
}

mod region_fold {
    use super::fold_regions;
    use super::*;
    use crate::worker::status::update_queue::UpdateStep;
    use golem_common::model::oplog::SnapshotFault;
    use golem_common::model::{AgentInvocationPayload, AssistedSelection};
    use pretty_assertions::assert_eq;
    use test_r::test;

    pub(super) fn idx(value: u64) -> OplogIndex {
        OplogIndex::from_u64(value)
    }

    pub(super) fn revision(value: u64) -> ComponentRevision {
        ComponentRevision::new(value).unwrap()
    }

    pub(super) fn automatic_admission(target: u64) -> OplogEntry {
        OplogEntry::pending_update(
            UpdateDescription::Automatic {
                target_revision: revision(target),
            },
            None,
        )
    }

    pub(super) fn plain_strategy(target: u64, admission: u64) -> OplogEntry {
        OplogEntry::pending_update(
            UpdateDescription::Automatic {
                target_revision: revision(target),
            },
            Some(idx(admission)),
        )
    }

    pub(super) fn assisted_strategy(
        target: u64,
        admission: u64,
        source: u64,
        snapshot: u64,
        filesystem_snapshot: Option<FilesystemSnapshotName>,
    ) -> OplogEntry {
        OplogEntry::pending_update(
            UpdateDescription::SnapshotAssistedAutomatic {
                target_revision: revision(target),
                source_component_revision: revision(source),
                source_revision_start_index: OplogIndex::INITIAL,
                snapshot_index: idx(snapshot),
                snapshot_revision: revision(source),
                filesystem_snapshot,
            },
            Some(idx(admission)),
        )
    }

    pub(super) fn manual_invocation(target: u64) -> OplogEntry {
        OplogEntry::PendingAgentInvocation {
            timestamp: Timestamp::from(1_000),
            idempotency_key: IdempotencyKey::fresh(),
            payload: OplogPayload::Inline(Box::new(AgentInvocationPayload::ManualUpdate {
                target_revision: revision(target),
            })),
            trace_id: TraceId::generate(),
            trace_states: Vec::new(),
            invocation_context: Vec::new(),
        }
    }

    pub(super) fn manual_pending_update(target: u64, admission: Option<u64>) -> OplogEntry {
        OplogEntry::pending_update(
            UpdateDescription::SnapshotBased {
                target_revision: revision(target),
                payload: OplogPayload::Inline(Box::new(vec![])),
                mime_type: "application/octet-stream".to_string(),
                filesystem_snapshot: None,
            },
            admission.map(idx),
        )
    }

    pub(super) fn failed(target: u64, attempt: Option<u64>) -> OplogEntry {
        OplogEntry::failed_update(revision(target), None, None, attempt.map(idx), None)
    }

    pub(super) fn succeeded(target: u64) -> OplogEntry {
        OplogEntry::successful_update(revision(target), 10, None, HashSet::new(), None)
    }

    pub(super) fn revert(start: u64, end: u64) -> OplogEntry {
        OplogEntry::revert(OplogRegion::from_index_range(idx(start)..=idx(end)))
    }

    pub(super) fn entries(
        list: impl IntoIterator<Item = (u64, OplogEntry)>,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        list.into_iter()
            .map(|(index, entry)| (idx(index), entry))
            .collect()
    }

    pub(super) fn fold_status(entries: BTreeMap<OplogIndex, OplogEntry>) -> AgentStatusRecord {
        update_status_with_new_entries(
            AgentMode::Durable,
            AgentStatusRecord::default(),
            entries,
            &RetryConfig::default(),
        )
        .unwrap()
    }

    /// The committed skipped regions and the override of `regions`.
    fn committed_and_override(
        regions: &DeletedRegions,
    ) -> (Vec<OplogRegion>, Option<Vec<OplogRegion>>) {
        let mut committed = regions.clone();
        if committed.is_overridden() {
            committed.drop_override();
        }
        (
            committed.into_regions().collect(),
            regions
                .get_override()
                .map(|regions| regions.into_regions().collect()),
        )
    }

    fn region(start: u64, end: u64) -> OplogRegion {
        OplogRegion::from_index_range(idx(start)..=idx(end))
    }

    #[test]
    fn a_plain_automatic_head_success_neither_commits_nor_drops_the_override_of_a_manual_update_behind_it()
     {
        let status = fold_status(entries([
            (2, automatic_admission(2)),
            (3, manual_pending_update(3, None)),
            (4, plain_strategy(2, 2)),
            (5, succeeded(2)),
        ]));
        assert_eq!(
            committed_and_override(&status.skipped_regions),
            (vec![], Some(vec![region(2, 3)]))
        );

        let after_manual = update_status_with_new_entries(
            AgentMode::Durable,
            status,
            entries([(6, succeeded(3))]),
            &RetryConfig::default(),
        )
        .unwrap();
        assert_eq!(
            committed_and_override(&after_manual.skipped_regions),
            (vec![region(2, 3)], None)
        );
        assert_eq!(
            manual_update_baseline(&after_manual.authoritative_snapshot),
            Some(idx(3))
        );
    }

    #[test]
    fn an_assisted_head_success_commits_only_its_record_and_keeps_the_override_of_a_manual_update_behind_it()
     {
        let status = fold_status(entries([
            (2, update_fields_snapshot(None)),
            (3, automatic_admission(2)),
            (4, assisted_strategy(2, 3, 1, 2, None)),
            (5, manual_pending_update(3, None)),
            (6, succeeded(2)),
        ]));

        assert_eq!(
            committed_and_override(&status.skipped_regions),
            (vec![region(2, 2)], Some(vec![region(2, 5)]))
        );
    }

    #[test]
    fn a_failed_automatic_head_keeps_the_override_of_a_manual_update_behind_it() {
        let status = fold_status(entries([
            (2, automatic_admission(2)),
            (3, manual_pending_update(3, None)),
            (4, failed(2, Some(2))),
        ]));

        assert_eq!(
            committed_and_override(&status.skipped_regions),
            (vec![], Some(vec![region(2, 3)]))
        );
        assert_eq!(status.pending_updates.len(), 1);
    }

    #[test]
    fn a_deleted_failure_of_a_manual_admission_removes_the_invocation_and_adds_no_record() {
        let list = entries([
            (2, manual_invocation(3)),
            (3, failed(3, Some(2))),
            (4, revert(3, 3)),
        ]);
        let regions = fold_regions(&AgentStatusRecord::default(), &list);
        let status = fold_status(list);

        assert_eq!(
            regions.steps.get(&idx(3)),
            Some(&UpdateStep::ConsumedInDeletedRegion(idx(2)))
        );
        assert!(status.pending_invocations.is_empty());
        assert!(status.failed_updates.is_empty());
        assert!(status.pending_updates.is_empty());
    }

    #[test]
    fn a_successful_assisted_update_promotes_its_named_record_from_the_paired_kind() {
        let name = FilesystemSnapshotName::periodic();
        let status = fold_status(entries([
            (2, update_fields_snapshot(Some(name.clone()))),
            (3, OplogEntry::snapshot_confirmed(name.clone())),
            (4, update_fields_snapshot(None)),
            (5, automatic_admission(2)),
            (6, assisted_strategy(2, 5, 1, 2, Some(name.clone()))),
            (7, succeeded(2)),
        ]));

        assert_eq!(
            status.authoritative_snapshot,
            Some(AuthoritativeSnapshot {
                index: idx(2),
                kind: AuthoritativeSnapshotKind::SnapshotAssistedAutomatic {
                    filesystem_snapshot: Some(name.clone()),
                },
            })
        );
        assert_eq!(status.component_revision, revision(2));
        assert_eq!(status.component_revision_for_replay, revision(1));
        assert_eq!(status.component_revision_start_index, idx(7));
        assert_eq!(status.last_automatic_snapshot, None);
        assert_eq!(status.previous_usable_automatic_snapshot, None);
        assert_eq!(
            status.successful_updates[0].filesystem_snapshot,
            Some(name.clone())
        );
        assert!(matches!(
            &status.successful_updates[0].pending_update,
            Some(PendingUpdateRef {
                kind: PendingUpdateKind::SnapshotAssistedAutomatic(selection),
                ..
            }) if selection.snapshot.filesystem_snapshot == Some(name.clone())
        ));
        assert_eq!(
            committed_and_override(&status.skipped_regions),
            (vec![region(2, 2)], None)
        );
        assert_eq!(names_in_use(&status), Box::from([name]));
    }

    #[test]
    fn the_record_of_an_assisted_update_stays_in_use_while_pending_after_success_after_a_revert_of_the_success_and_after_a_later_update()
     {
        let [selected, newer, newest] = [(); 3].map(|()| FilesystemSnapshotName::periodic());
        let later = FilesystemSnapshotName::update();
        let pending = [
            (2, update_fields_snapshot(Some(selected.clone()))),
            (3, OplogEntry::snapshot_confirmed(selected.clone())),
            (4, automatic_admission(2)),
            (5, assisted_strategy(2, 4, 1, 2, Some(selected.clone()))),
            (6, update_fields_snapshot(Some(newer.clone()))),
            (7, OplogEntry::snapshot_confirmed(newer.clone())),
            (8, update_fields_snapshot(Some(newest.clone()))),
            (9, OplogEntry::snapshot_confirmed(newest.clone())),
        ];
        let succeeded_at_10 = pending
            .iter()
            .cloned()
            .chain([(10, succeeded(2))])
            .collect::<Vec<_>>();
        let reverted = succeeded_at_10
            .iter()
            .cloned()
            .chain([(11, revert(10, 10))])
            .collect::<Vec<_>>();
        let later_update = succeeded_at_10
            .iter()
            .cloned()
            .chain([
                (11, manual_invocation(3)),
                (
                    12,
                    OplogEntry::pending_update(
                        UpdateDescription::SnapshotBased {
                            target_revision: revision(3),
                            payload: OplogPayload::Inline(Box::new(vec![])),
                            mime_type: "application/octet-stream".to_string(),
                            filesystem_snapshot: Some(later.clone()),
                        },
                        Some(idx(11)),
                    ),
                ),
                (13, succeeded(3)),
            ])
            .collect::<Vec<_>>();

        let names = [
            fold_status(entries(pending)),
            fold_status(entries(succeeded_at_10)),
            fold_status(entries(reverted)),
            fold_status(entries(later_update)),
        ]
        .map(|status| names_in_use(&status));

        let by_text = |mut names: Vec<FilesystemSnapshotName>| {
            names.sort_unstable_by(|left, right| left.as_str().cmp(right.as_str()));
            names.into_boxed_slice()
        };
        assert_eq!(
            names,
            [
                by_text(vec![newest.clone(), newer.clone(), selected.clone()]),
                by_text(vec![selected.clone()]),
                by_text(vec![newest, newer, selected.clone()]),
                by_text(vec![selected, later]),
            ]
        );
    }

    #[test]
    fn a_success_with_assisted_entry_details_but_a_plain_paired_update_promotes_nothing() {
        let details = golem_common::model::oplog::SnapshotAssistedUpdateDetails {
            pending_update_index: idx(3),
            source_component_revision: revision(1),
            source_revision_start_index: OplogIndex::INITIAL,
            snapshot_index: idx(2),
        };
        let status = fold_status(entries([
            (2, update_fields_snapshot(None)),
            (3, automatic_admission(2)),
            (4, plain_strategy(2, 3)),
            (
                5,
                OplogEntry::successful_update(revision(2), 10, None, HashSet::new(), Some(details)),
            ),
        ]));

        assert_eq!(status.authoritative_snapshot, None);
        assert_eq!(
            status.component_revision_for_replay,
            AgentStatusRecord::default().component_revision_for_replay
        );
        assert_eq!(
            committed_and_override(&status.skipped_regions),
            (vec![], None)
        );
    }

    /// A failed update whose snapshot-assisted attempt lost the filesystem snapshot of its record
    /// excludes that record through the status alone. A status folded from the oplog after a
    /// restart, whole or from any checkpoint, gives the next request of the same update a plain
    /// automatic strategy without any rejection in memory; the same history without the fault
    /// selects the record again.
    #[test]
    fn a_lost_record_stays_excluded_in_a_status_folded_after_a_restart() {
        use crate::worker::snapshot_selection::{SnapshotExclusions, StartDecision, decide_start};
        let name = FilesystemSnapshotName::periodic();
        let history = |fault| {
            entries([
                (2, update_fields_snapshot(Some(name.clone()))),
                (3, OplogEntry::snapshot_confirmed(name.clone())),
                (4, automatic_admission(2)),
                (5, assisted_strategy(2, 4, 1, 2, Some(name.clone()))),
                (
                    6,
                    OplogEntry::failed_update(
                        revision(2),
                        None,
                        Some(
                            golem_common::model::oplog::FailedSnapshotAssistedUpdateDetails {
                                pending_update_index: idx(5),
                                source_component_revision: AgentStatusRecord::default()
                                    .component_revision,
                                source_revision_start_index: OplogIndex::INITIAL,
                                snapshot_index: idx(2),
                            },
                        ),
                        Some(idx(4)),
                        fault,
                    ),
                ),
                (7, automatic_admission(2)),
            ])
        };
        let folds = |list: BTreeMap<OplogIndex, OplogEntry>| {
            std::iter::once(fold_status(list.clone()))
                .chain(list.keys().filter_map(|checkpoint| {
                    let (before, after): (BTreeMap<_, _>, BTreeMap<_, _>) = list
                        .clone()
                        .into_iter()
                        .partition(|(index, _)| index <= checkpoint);
                    update_status_with_new_entries(
                        AgentMode::Durable,
                        fold_status(before),
                        after,
                        &RetryConfig::default(),
                    )
                }))
                .map(|status| decide_start(&status, &SnapshotExclusions::default(), true))
                .collect::<Vec<_>>()
        };
        let lost = folds(history(Some(SnapshotFault::Unavailable)));
        let kept = folds(history(None));

        assert!(lost.len() > 1);
        assert!(lost.iter().all(|decision| matches!(
            decision,
            StartDecision::PersistStrategy {
                description: UpdateDescription::Automatic { .. },
                admission_index,
            } if *admission_index == idx(7)
        )));
        assert!(kept.iter().all(|decision| matches!(
            decision,
            StartDecision::PersistStrategy {
                description: UpdateDescription::SnapshotAssistedAutomatic { snapshot_index, .. },
                ..
            } if *snapshot_index == idx(2)
        )));
    }

    #[test]
    fn a_failed_update_keeps_its_snapshot_fault_in_its_record() {
        let status = fold_status(entries([
            (2, update_fields_snapshot(None)),
            (3, automatic_admission(2)),
            (4, assisted_strategy(2, 3, 1, 2, None)),
            (
                5,
                OplogEntry::failed_update(
                    revision(2),
                    None,
                    None,
                    Some(idx(3)),
                    Some(SnapshotFault::Incompatible),
                ),
            ),
        ]));

        assert_eq!(
            status.failed_updates[0].snapshot_fault,
            Some(SnapshotFault::Incompatible)
        );
        assert!(matches!(
            &status.failed_updates[0].pending_update,
            Some(PendingUpdateRef {
                kind: PendingUpdateKind::SnapshotAssistedAutomatic(selection),
                ..
            }) if **selection == AssistedSelection {
                source_revision_start_index: OplogIndex::INITIAL,
                snapshot: UsableAutomaticSnapshot {
                    index: idx(2),
                    component_revision: revision(1),
                    filesystem_snapshot: None,
                },
            }
        ));
        assert_eq!(status.authoritative_snapshot, None);
    }

    #[test]
    fn a_fold_from_every_checkpoint_gives_the_regions_and_the_steps_of_one_fold() {
        let list = entries([
            (2, manual_invocation(4)),
            (3, update_fields_snapshot(None)),
            (4, automatic_admission(2)),
            (5, assisted_strategy(2, 4, 1, 3, None)),
            (6, manual_pending_update(4, Some(2))),
            (7, succeeded(2)),
            (8, manual_invocation(5)),
            (9, revert(8, 8)),
            (10, succeeded(4)),
            (11, automatic_admission(6)),
        ]);
        let whole = fold_regions(&AgentStatusRecord::default(), &list);
        let status = fold_status(list.clone());

        list.keys().for_each(|checkpoint| {
            let (before, after): (BTreeMap<_, _>, BTreeMap<_, _>) = list
                .clone()
                .into_iter()
                .partition(|(index, _)| index <= checkpoint);
            let checkpoint_status = fold_status(before);
            let rest = fold_regions(&checkpoint_status, &after);
            assert_eq!(
                (rest.deleted, rest.skipped, rest.queue.into_open().0),
                (
                    whole.deleted.clone(),
                    whole.skipped.clone(),
                    whole.queue.clone().into_open().0
                ),
                "checkpoint {checkpoint}"
            );
            assert_eq!(
                rest.steps,
                whole
                    .steps
                    .iter()
                    .filter(|(index, _)| *index > checkpoint)
                    .map(|(index, step)| (*index, step.clone()))
                    .collect::<BTreeMap<_, _>>(),
                "checkpoint {checkpoint}"
            );
        });
        assert_eq!(status.skipped_regions, whole.skipped);
    }

    #[test]
    async fn skipped_regions_at_gives_the_regions_of_the_status_fold() {
        let test_case = [
            update_fields_snapshot(None),
            automatic_admission(2),
            assisted_strategy(2, 3, 1, 2, None),
            manual_pending_update(3, None),
            succeeded(2),
            manual_invocation(4),
            revert(7, 7),
        ]
        .into_iter()
        .fold(TestCase::builder(1), |builder, entry| {
            builder.add(entry, |status| status)
        })
        .build();
        let list = test_case
            .entries
            .iter()
            .enumerate()
            .map(|(position, entry)| (idx(position as u64 + 1), entry.oplog_entry.clone()))
            .collect::<BTreeMap<_, _>>();
        let horizon = *list.keys().next_back().unwrap();

        let regions =
            super::super::skipped_regions_at(&test_case, &test_case.owned_agent_id, horizon)
                .await
                .unwrap();

        assert_eq!(regions, fold_status(list).skipped_regions);
        assert!(regions.is_overridden());
    }

    /// The skipped regions do not depend on the manual update invocations, which the read of
    /// `skipped_regions_at` leaves out: snapshot-based updates that pair them, and one in a
    /// deleted region, give the regions of the status fold.
    #[test]
    async fn skipped_regions_at_gives_the_regions_of_the_status_fold_with_paired_manual_updates() {
        let test_case = [
            update_fields_snapshot(None),
            manual_invocation(3),
            manual_pending_update(3, Some(3)),
            succeeded(3),
            manual_invocation(4),
            manual_pending_update(4, Some(6)),
            manual_invocation(5),
            revert(8, 8),
        ]
        .into_iter()
        .fold(TestCase::builder(1), |builder, entry| {
            builder.add(entry, |status| status)
        })
        .build();
        let list = test_case
            .entries
            .iter()
            .enumerate()
            .map(|(position, entry)| (idx(position as u64 + 1), entry.oplog_entry.clone()))
            .collect::<BTreeMap<_, _>>();
        let horizon = *list.keys().next_back().unwrap();

        let regions =
            super::super::skipped_regions_at(&test_case, &test_case.owned_agent_id, horizon)
                .await
                .unwrap();

        assert_eq!(regions, fold_status(list).skipped_regions);
        assert!(regions.is_overridden());
    }
}

mod invocation_payload_decodes {
    use super::region_fold::{idx, manual_invocation, revision};
    use super::*;
    use crate::worker::status::update_queue::INVOCATION_PAYLOAD_DECODES;
    use pretty_assertions::assert_eq;
    use test_r::test;

    fn serialized(entry: OplogEntry) -> OplogEntry {
        match entry {
            OplogEntry::PendingAgentInvocation {
                timestamp,
                idempotency_key,
                payload: OplogPayload::Inline(payload),
                trace_id,
                trace_states,
                invocation_context,
            } => OplogEntry::PendingAgentInvocation {
                timestamp,
                idempotency_key,
                payload: OplogPayload::SerializedInline {
                    bytes: golem_common::serialization::serialize(payload.as_ref()).unwrap(),
                    cached: None,
                },
                trace_id,
                trace_states,
                invocation_context,
            },
            other => other,
        }
    }

    fn method_invocation() -> OplogEntry {
        serialized(OplogEntry::PendingAgentInvocation {
            timestamp: Timestamp::from(1_000),
            idempotency_key: IdempotencyKey::fresh(),
            payload: OplogPayload::Inline(Box::new(AgentInvocationPayload::AgentMethod {
                method_name: "a".to_string(),
                input: SchemaValue::Record {
                    fields: vec![SchemaValue::String("input".to_string())],
                },
                principal: Principal::anonymous(),
                scope_card: None,
            })),
            trace_id: TraceId::generate(),
            trace_states: Vec::new(),
            invocation_context: Vec::new(),
        })
    }

    /// Three uncached method invocations and one uncached manual update invocation.
    fn history() -> BTreeMap<OplogIndex, OplogEntry> {
        BTreeMap::from([
            (idx(2), method_invocation()),
            (idx(3), method_invocation()),
            (idx(4), serialized(manual_invocation(2))),
            (idx(5), method_invocation()),
        ])
    }

    fn decodes_of(fold: impl FnOnce()) -> usize {
        INVOCATION_PAYLOAD_DECODES.with(|decodes| decodes.set(0));
        fold();
        INVOCATION_PAYLOAD_DECODES.with(|decodes| decodes.get())
    }

    #[test]
    fn a_status_fold_decodes_each_uncached_invocation_payload_once() {
        let mut status = None;
        let decodes = decodes_of(|| {
            status = super::super::update_status_with_new_entries(
                AgentMode::Durable,
                AgentStatusRecord::default(),
                history(),
                &RetryConfig::default(),
            )
            .unwrap();
        });
        let pending = status.unwrap().pending_invocations;

        assert_eq!(decodes, 4);
        assert_eq!(
            pending
                .iter()
                .map(|invocation| (
                    invocation.oplog_index,
                    invocation.manual_update_target_revision,
                    invocation.idempotency_key.is_some()
                ))
                .collect::<Vec<_>>(),
            vec![
                (idx(2), None, true),
                (idx(3), None, true),
                (idx(4), Some(revision(2)), false),
                (idx(5), None, true),
            ]
        );
    }

    #[test]
    fn revert_validation_decodes_no_invocation_payload() {
        let decodes = decodes_of(|| {
            super::super::revert_validation_regions(
                &history(),
                &OplogRegion::from_index_range(idx(4)..=idx(5)),
            );
        });

        assert_eq!(decodes, 0);
    }
}

mod update_entry_sequences {
    use super::fold_regions;
    use super::region_fold::{
        assisted_strategy, automatic_admission, failed, fold_status, idx, manual_invocation,
        plain_strategy, revert, revision, succeeded,
    };
    use super::*;
    use crate::services::worker_fork::ForkUpdates;
    use crate::worker::cut_point::validate_snapshot_update_boundaries;
    use crate::worker::status::update_queue::{UpdateStep, manual_update_target_revision_of};
    use proptest::prelude::*;
    use test_r::test;

    /// One generated entry. A pick selects an earlier entry of the sequence by position.
    #[derive(Clone, Debug)]
    enum Generated {
        ManualInvocation,
        AutomaticAdmission,
        /// A strategy entry of the picked automatic admission, assisted when `true`.
        Strategy(u8, bool),
        /// A manual `PendingUpdate`, paired with the picked manual invocation when `Some`, with a
        /// filesystem snapshot when `true`.
        ManualPending(Option<u8>, bool),
        Success,
        /// A failure by the picked admission or invocation, or by the picked target.
        FailedByAttempt(u8),
        FailedByTarget(u8),
        Snapshot,
        /// A jump or a revert over the history from the picked index to the entry before it, as
        /// the executor writes them.
        Jump(u8),
        Revert(u8),
    }

    fn generated() -> impl Strategy<Value = Generated> {
        prop_oneof![
            Just(Generated::ManualInvocation),
            Just(Generated::AutomaticAdmission),
            (any::<u8>(), any::<bool>())
                .prop_map(|(pick, assisted)| Generated::Strategy(pick, assisted)),
            (proptest::option::of(any::<u8>()), any::<bool>())
                .prop_map(|(pick, named)| Generated::ManualPending(pick, named)),
            Just(Generated::Success),
            any::<u8>().prop_map(Generated::FailedByAttempt),
            any::<u8>().prop_map(Generated::FailedByTarget),
            Just(Generated::Snapshot),
            any::<u8>().prop_map(Generated::Jump),
            any::<u8>().prop_map(Generated::Revert),
        ]
    }

    /// The entries of `generated`, from oplog index 2. Each admission and invocation targets the
    /// revision `index + 1`, so the targets tell the updates apart.
    fn sequence(generated: &[Generated]) -> BTreeMap<OplogIndex, OplogEntry> {
        let pick = |candidates: &[u64], pick: u8| {
            (!candidates.is_empty()).then(|| candidates[pick as usize % candidates.len()])
        };
        let (_, _, _, entries) = generated.iter().enumerate().fold(
            (
                Vec::<u64>::new(),
                Vec::<u64>::new(),
                Vec::<u64>::new(),
                BTreeMap::new(),
            ),
            |(mut automatic, mut manual, mut snapshots, mut entries), (position, generated)| {
                let index = position as u64 + 2;
                let entry = match generated {
                    Generated::ManualInvocation => {
                        manual.push(index);
                        manual_invocation(index + 1)
                    }
                    Generated::AutomaticAdmission => {
                        automatic.push(index);
                        automatic_admission(index + 1)
                    }
                    Generated::Strategy(choice, assisted) => match pick(&automatic, *choice) {
                        Some(admission) => match (assisted, pick(&snapshots, *choice)) {
                            (true, Some(snapshot)) => assisted_strategy(
                                admission + 1,
                                admission,
                                1,
                                snapshot,
                                (choice % 2 == 0).then(FilesystemSnapshotName::periodic),
                            ),
                            _ => plain_strategy(admission + 1, admission),
                        },
                        None => update_fields_snapshot(None),
                    },
                    Generated::ManualPending(choice, named) => {
                        let admission = choice.and_then(|choice| pick(&manual, choice));
                        OplogEntry::pending_update(
                            UpdateDescription::SnapshotBased {
                                target_revision: revision(admission.unwrap_or(index) + 1),
                                payload: OplogPayload::Inline(Box::new(vec![])),
                                mime_type: "application/octet-stream".to_string(),
                                filesystem_snapshot: named.then(FilesystemSnapshotName::update),
                            },
                            admission.map(idx),
                        )
                    }
                    Generated::Success => succeeded(index + 1),
                    Generated::FailedByAttempt(choice) => {
                        let candidates = [automatic.as_slice(), manual.as_slice()].concat();
                        match pick(&candidates, *choice) {
                            Some(attempt) => failed(attempt + 1, Some(attempt)),
                            None => failed(index + 1, None),
                        }
                    }
                    Generated::FailedByTarget(choice) => {
                        let candidates = [automatic.as_slice(), manual.as_slice()].concat();
                        failed(pick(&candidates, *choice).unwrap_or(index) + 1, None)
                    }
                    Generated::Snapshot => {
                        snapshots.push(index);
                        update_fields_snapshot(None)
                    }
                    Generated::Jump(start) | Generated::Revert(start) => {
                        let start = 2 + (*start as u64) % index.saturating_sub(2).max(1);
                        let end = index - 1;
                        if index <= 2 {
                            update_fields_snapshot(None)
                        } else if matches!(generated, Generated::Jump(..)) {
                            OplogEntry::Jump {
                                timestamp: Timestamp::from(1_000),
                                entity_parent_start_index: None,
                                jump: OplogRegion::from_index_range(idx(start)..=idx(end)),
                            }
                        } else {
                            revert(start, end)
                        }
                    }
                };
                entries.insert(idx(index), entry.rounded());
                (automatic, manual, snapshots, entries)
            },
        );
        entries
    }

    /// The update fields of a status that a fold split at a checkpoint must reproduce.
    #[allow(clippy::type_complexity)]
    fn update_view(
        status: &AgentStatusRecord,
    ) -> (
        Vec<PendingUpdateRef>,
        Vec<FailedUpdateRecord>,
        Vec<SuccessfulUpdateRecord>,
        DeletedRegions,
        DeletedRegions,
        Vec<PendingInvocationRef>,
        Option<AuthoritativeSnapshot>,
        (ComponentRevision, ComponentRevision, OplogIndex),
    ) {
        (
            status.pending_updates.iter().cloned().collect(),
            status.failed_updates.clone(),
            status.successful_updates.clone(),
            status.skipped_regions.clone(),
            status.deleted_regions.clone(),
            status.pending_invocations.clone(),
            status.authoritative_snapshot.clone(),
            (
                status.component_revision,
                status.component_revision_for_replay,
                status.component_revision_start_index,
            ),
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn every_caller_of_the_update_queue_agrees_and_a_checkpoint_split_changes_nothing(
            generated in proptest::collection::vec(generated(), 1..28),
        ) {
            let list = sequence(&generated);
            let status = fold_status(list.clone());
            let regions = fold_regions(&AgentStatusRecord::default(), &list);

            // The region fold is the status's.
            prop_assert_eq!(&regions.skipped, &status.skipped_regions);
            prop_assert_eq!(&regions.deleted, &status.deleted_regions);

            // A fold split at every checkpoint gives the same status, or refuses the baseline.
            list.keys().try_for_each(|checkpoint| {
                let (before, after): (BTreeMap<_, _>, BTreeMap<_, _>) = list
                    .clone()
                    .into_iter()
                    .partition(|(index, _)| index <= checkpoint);
                let split = update_status_with_new_entries(
                    AgentMode::Durable,
                    fold_status(before),
                    after,
                    &RetryConfig::default(),
                );
                if let Some(split) = split {
                    prop_assert_eq!(update_view(&split), update_view(&status), "checkpoint {}", checkpoint);
                }
                Ok(())
            })?;

            // The fork cancels the status's pending queue and then its pending manual update
            // invocations, and takes the filesystem snapshot of the authoritative baseline of
            // the status as its baseline. It folds the entries that its copy keeps.
            let (cancelled, baseline) = list
                .iter()
                .filter_map(|(index, entry)| {
                    let manual_update = match entry {
                        OplogEntry::PendingAgentInvocation { payload, .. } => {
                            manual_update_target_revision_of(payload)
                        }
                        _ => None,
                    };
                    ForkUpdates::update_entry(entry.clone(), manual_update).map(|kept| (*index, kept))
                })
                .fold(ForkUpdates::default(), |updates, (index, kept)| {
                    updates.after_kept(index, kept, status.deleted_regions.is_in_deleted_region(index))
                })
                .into_parts();
            let cancelled = cancelled.collect::<Vec<_>>();
            prop_assert_eq!(
                cancelled_updates(&cancelled),
                status
                    .pending_updates
                    .iter()
                    .map(|update| (update.target_revision, Some(update.admission_index)))
                    .chain(status.pending_invocations.iter().filter_map(|invocation| {
                        invocation
                            .manual_update_target_revision
                            .map(|target| (target, Some(invocation.oplog_index)))
                    }))
                    .collect::<Vec<_>>()
            );
            prop_assert_eq!(
                &baseline,
                &status
                    .authoritative_snapshot
                    .as_ref()
                    .and_then(|baseline| match &baseline.kind {
                        AuthoritativeSnapshotKind::ManualUpdate => {
                            pending_update_name(&list, Some(baseline.index))
                        }
                        AuthoritativeSnapshotKind::SnapshotAssistedAutomatic {
                            filesystem_snapshot,
                        } => filesystem_snapshot.clone(),
                    })
            );

            // The copied prefix with the cancellations of the fork leaves no update pending.
            let last = list.keys().next_back().copied().unwrap_or(OplogIndex::INITIAL);
            let target = fold_status(
                list.clone()
                    .into_iter()
                    .chain(cancelled.iter().cloned().enumerate().map(|(offset, entry)| {
                        (OplogIndex::from_u64(u64::from(last) + offset as u64 + 1), entry)
                    }))
                    .collect(),
            );
            prop_assert!(target.pending_updates.is_empty());
            prop_assert!(target
                .pending_invocations
                .iter()
                .all(|invocation| invocation.manual_update_target_revision.is_none()));

            // The cut point refuses exactly the cuts that split a snapshot-based update as the
            // status pairs it.
            let splits = regions
                .steps
                .iter()
                .filter_map(|(index, step)| match step {
                    UpdateStep::Succeeded(Some(paired)) | UpdateStep::FailedQueued(Some(paired))
                        if matches!(paired.kind, PendingUpdateKind::SnapshotBased { .. }) =>
                    {
                        Some((paired.oplog_index, *index))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            list.keys().try_for_each(|cut| {
                let refused = validate_snapshot_update_boundaries(&list, *cut, &status.deleted_regions).is_err();
                let splits_here = splits
                    .iter()
                    .any(|(pending, outcome)| pending <= cut && cut < outcome);
                prop_assert_eq!(refused, splits_here, "cut {}", cut);
                Ok(())
            })?;
        }
    }
}
