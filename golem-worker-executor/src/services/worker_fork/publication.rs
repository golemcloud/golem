// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use crate::services::agent_filesystem_snapshots::PublishFound;
use crate::services::oplog::{Oplog, OplogOps, OplogService, OplogServiceOps};
use golem_common::model::agent::AgentMode;
use golem_common::model::durable_stream::{StreamId, StreamOffset, StreamSessionRecord};
use golem_common::model::oplog::host_functions::GolemApiFork;
use golem_common::model::oplog::{
    DurableFunctionType, FilesystemSnapshotName, HostPayloadPair, HostRequest, HostRequestNoInput,
    HostResponse, HostResponseGolemApiFork, OplogEntry, OplogIndex,
};
use golem_common::model::{AgentFingerprint, OwnedAgentId, Timestamp};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use uuid::Uuid;

pub(crate) fn request_hash(
    source: &OwnedAgentId,
    fingerprint: AgentFingerprint,
    cut: OplogIndex,
    selected: Option<(StreamId, Option<StreamOffset>)>,
    guest_result: Option<(Option<OplogIndex>, Uuid)>,
) -> Result<[u8; 32], String> {
    let bytes = golem_common::serialization::serialize(&(
        "golem-fork-request-v1".to_string(),
        source.environment_id,
        source.agent_id.clone(),
        fingerprint,
        cut,
        selected,
        guest_result,
    ))
    .map_err(|error| error.to_string())?;
    Ok(*blake3::hash(&bytes).as_bytes())
}

/// What a reconciliation found at the target of a fork.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExistingFork {
    /// No durable target exists.
    Absent,
    /// The target is a fork of this request, with this instance id.
    Matching(AgentFingerprint),
    /// The target has a `Create` with this instance id, and it is not a fork of this request.
    Other(AgentFingerprint),
}

impl ExistingFork {
    /// Whether the target `target` is a fork of this request. Another target is a conflict.
    pub(crate) fn matches(self, target: &OwnedAgentId) -> Result<bool, WorkerExecutorError> {
        match self {
            Self::Absent => Ok(false),
            Self::Matching(_) => Ok(true),
            Self::Other(_) => Err(conflict(target)),
        }
    }
}

/// The answer of a plain fork attempt at the cut `cut` whose copy lacks the snapshot `name` of the
/// baseline of the target, after its reconciliation found `existing`, with the instance id of the
/// live target that the stage of the attempt lost to. A fork of this request answers success,
/// another target answers a conflict, and without a target the fork is refused with an error that
/// names the cut and the snapshot.
pub(crate) fn missing_baseline_answer(
    existing: ExistingFork,
    target: &OwnedAgentId,
    cut: OplogIndex,
    name: &FilesystemSnapshotName,
) -> (Option<AgentFingerprint>, Result<(), WorkerExecutorError>) {
    match existing {
        ExistingFork::Matching(live) => (Some(live), Ok(())),
        ExistingFork::Other(live) => (Some(live), Err(conflict(target))),
        ExistingFork::Absent => (
            None,
            Err(WorkerExecutorError::invalid_request(format!(
                "Cannot fork worker at oplog index {cut}: the filesystem snapshot {name} of the target's baseline is not in the store"
            ))),
        ),
    }
}

/// The error of a fork whose target exists and is not a fork of its request.
pub(crate) fn conflict(target: &OwnedAgentId) -> WorkerExecutorError {
    WorkerExecutorError::worker_already_exists(target.agent_id.clone())
}

/// Reconcile without opening a writer or depending on the fork still being suspended. A fork
/// may already have run, archived, or lost its response; its creation prefix is immutable. Once
/// the reconciliation read the `Create` of the target, it gives the instance id of the target,
/// also when the target is not a fork of this request.
pub(crate) async fn existing_fork(
    service: &dyn OplogService,
    target: &OwnedAgentId,
    cut: OplogIndex,
    hash: [u8; 32],
) -> Result<ExistingFork, WorkerExecutorError> {
    let mode = AgentMode::Durable;
    let conflict = || conflict(target);
    if service.exists(target, AgentMode::Ephemeral).await {
        return Err(conflict());
    }
    if !service.exists(target, mode).await {
        return Ok(ExistingFork::Absent);
    }
    let initial = service
        .read_source(target, mode, OplogIndex::INITIAL, 1)
        .await;
    let Some(OplogEntry::Create { parameters, .. }) = initial.get(&OplogIndex::INITIAL) else {
        return Err(conflict());
    };
    let other = ExistingFork::Other(AgentFingerprint(parameters.instance_id));
    if service.get_last_index(target, mode).await < cut.next() {
        return Ok(other);
    }
    if parameters.agent_id != target.agent_id || parameters.environment_id != target.environment_id
    {
        return Ok(other);
    }
    let entries = service.read_source(target, mode, cut.next(), 1).await;
    let Some(OplogEntry::StreamSession { record, .. }) = entries.get(&cut.next()) else {
        return Ok(other);
    };
    let record = service
        .download_payload(target, mode, record.clone())
        .await
        .map_err(WorkerExecutorError::runtime)?;
    if !record.has_supported_format() {
        return Ok(other);
    }
    match record {
        StreamSessionRecord::ForkCut(record)
            if record.request_hash == hash
                && record.cut_index == cut
                && record.creation_fingerprint.0 == parameters.instance_id =>
        {
            Ok(ExistingFork::Matching(AgentFingerprint(
                parameters.instance_id,
            )))
        }
        _ => Ok(other),
    }
}

/// The instance id of the live durable target, from its `Create`, or `None` when no target with
/// a `Create` is there.
pub(crate) async fn live_instance(
    service: &dyn OplogService,
    target: &OwnedAgentId,
) -> Option<AgentFingerprint> {
    let mode = AgentMode::Durable;
    if !service.exists(target, mode).await {
        return None;
    }
    match service
        .read_source(target, mode, OplogIndex::INITIAL, 1)
        .await
        .get(&OplogIndex::INITIAL)
    {
        Some(OplogEntry::Create { parameters, .. }) => {
            Some(AgentFingerprint(parameters.instance_id))
        }
        _ => None,
    }
}

/// What a plain fork found after a publication that did not give `true`, from the `outcome` of the
/// publication and the `reconciled` target. A read of a `Create` gives the live instance id: a fork
/// of this request succeeds, and another target is a conflict. Without such a read, a refused
/// publication is a loss that the snapshot service checks, and an error leaves the outcome
/// unknown.
pub(crate) fn found_after(
    target: &OwnedAgentId,
    outcome: Result<bool, String>,
    reconciled: Result<ExistingFork, WorkerExecutorError>,
) -> PublishFound<Result<(), WorkerExecutorError>, WorkerExecutorError> {
    match (reconciled, outcome) {
        (Ok(ExistingFork::Matching(live)), _) => PublishFound::Live(live, Ok(())),
        (Ok(ExistingFork::Other(live)), _) => PublishFound::Live(live, Err(conflict(target))),
        (Ok(ExistingFork::Absent), Ok(_)) => PublishFound::Refused(Err(
            WorkerExecutorError::runtime("Fork publication lost its target before reconciliation"),
        )),
        (Err(error), Ok(_)) => PublishFound::Refused(Err(error)),
        (_, Err(error)) => PublishFound::Unknown(WorkerExecutorError::runtime(error)),
    }
}

/// Complete the copied fork call before publishing the target. The synthetic pair must never
/// become visible without its terminal, even if the executor dies during creation.
pub(crate) async fn write_guest_result(
    oplog: &dyn Oplog,
    copied_scope_start: Option<OplogIndex>,
    forked_phantom_id: Uuid,
) -> Result<(), WorkerExecutorError> {
    let request = HostRequest::NoInput(HostRequestNoInput {});
    let response = HostResponse::GolemApiFork(HostResponseGolemApiFork {
        forked_phantom_id,
        result: Ok(golem_common::model::ForkResult::Forked),
    });
    let request_payload = oplog
        .upload_payload(&request)
        .await
        .map_err(WorkerExecutorError::runtime)?;
    let response_payload = oplog
        .upload_payload(&response)
        .await
        .map_err(WorkerExecutorError::runtime)?;
    let now = Timestamp::now_utc();
    oplog
        .add_pair(
            OplogEntry::Start {
                timestamp: now,
                parent_start_index: copied_scope_start,
                function_name: GolemApiFork::HOST_FUNCTION_NAME,
                invocation_id: None,
                observational_owner: None,
                request: Some(request_payload),
                durable_function_type: DurableFunctionType::WriteRemote,
                span_started: None,
            },
            Box::new(move |start_index| OplogEntry::End {
                timestamp: now,
                start_index,
                response: Some(response_payload),
                forced_commit: false,
                span_finished: None,
                span_attributes: None,
            }),
        )
        .await?;
    if let Some(start_index) = copied_scope_start {
        oplog
            .add(OplogEntry::End {
                timestamp: now,
                start_index,
                response: None,
                forced_commit: true,
                span_finished: None,
                span_attributes: None,
            })
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::oplog::{CommitLevel, PrimaryOplogService};
    use crate::storage::indexed::memory::InMemoryIndexedStorage;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::durable_stream::StreamForkCutRecord;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::{
        AgentId, AgentMetadata, AgentStatusRecord, RetryConfig, agent::OwnerKind,
    };
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use std::sync::Arc;
    use test_r::test;

    fn owner(name: &str) -> OwnedAgentId {
        OwnedAgentId::new(
            EnvironmentId(Uuid::from_u128(1)),
            &AgentId {
                component_id: ComponentId(Uuid::from_u128(2)),
                agent_id: name.into(),
            },
        )
    }

    #[test]
    fn a_missing_baseline_answers_by_the_target_that_the_reconciliation_found() {
        let target = owner("target");
        let cut = OplogIndex::from_u64(7);
        let name = FilesystemSnapshotName::update();
        let (matching, other) = (
            AgentFingerprint(Uuid::from_u128(5)),
            AgentFingerprint(Uuid::from_u128(6)),
        );
        let answer = |existing| {
            let (lost_to, answer) = missing_baseline_answer(existing, &target, cut, &name);
            (lost_to, answer.map_err(|error| error.to_string()))
        };

        let (refused_lost_to, refused) = answer(ExistingFork::Absent);

        assert_eq!(
            [
                answer(ExistingFork::Matching(matching)),
                answer(ExistingFork::Other(other)),
            ],
            [
                (Some(matching), Ok(())),
                (Some(other), Err(conflict(&target).to_string())),
            ]
        );
        assert_eq!(refused_lost_to, None);
        let refused = refused.unwrap_err();
        assert!(
            refused.contains("Cannot fork worker at oplog index 7")
                && refused.contains(name.as_str())
                && refused.contains("is not in the store"),
            "{refused}"
        );
    }

    #[test]
    fn request_identity_binds_source_generation_exact_cut_and_result_kind() {
        let source = owner("source");
        let generation = AgentFingerprint(Uuid::from_u128(3));
        let cut = OplogIndex::from_u64(7);
        let hash = request_hash(&source, generation, cut, None, None).unwrap();
        assert_eq!(
            hash,
            request_hash(&source, generation, cut, None, None).unwrap()
        );
        assert_ne!(
            hash,
            request_hash(&owner("other"), generation, cut, None, None).unwrap()
        );
        assert_ne!(
            hash,
            request_hash(
                &source,
                AgentFingerprint(Uuid::from_u128(4)),
                cut,
                None,
                None
            )
            .unwrap()
        );
        assert_ne!(
            hash,
            request_hash(&source, generation, cut.next(), None, None).unwrap()
        );
        assert_ne!(
            hash,
            request_hash(&source, generation, cut, None, Some((None, Uuid::nil()))).unwrap()
        );
        let selected = StreamId(Uuid::from_u128(9));
        let empty = request_hash(&source, generation, cut, Some((selected, None)), None).unwrap();
        assert_ne!(hash, empty);
        assert_ne!(
            empty,
            request_hash(
                &source,
                generation,
                cut,
                Some((selected, Some(StreamOffset::new(cut, 0)))),
                None
            )
            .unwrap()
        );
    }

    #[test]
    async fn published_fork_reconciles_after_advancing_but_never_matches_another_request() {
        let service = PrimaryOplogService::new(
            Arc::new(InMemoryIndexedStorage::new()),
            Arc::new(InMemoryBlobStorage::new()),
            1,
            1,
            1,
            RetryConfig::default(),
        )
        .await;
        let source = owner("source");
        let target = owner("fork");
        let cut = OplogIndex::from_u64(2);
        let source_fingerprint = AgentFingerprint(Uuid::from_u128(3));
        let fingerprint = AgentFingerprint(Uuid::from_u128(4));
        let phantom = Uuid::from_u128(5);
        let hash = request_hash(
            &source,
            source_fingerprint,
            cut,
            None,
            Some((None, phantom)),
        )
        .unwrap();
        assert_eq!(
            existing_fork(&service, &target, cut, hash).await.unwrap(),
            ExistingFork::Absent
        );
        let account = AccountId::new();
        let stage_id = Uuid::new_v4();
        let stage = service
            .create_staged(
                &target,
                AgentMode::Durable,
                stage_id,
                AgentMetadata {
                    agent_id: target.agent_id.clone(),
                    owner_kind: OwnerKind::ComponentAgent,
                    env: vec![],
                    environment_id: target.environment_id,
                    created_by: account,
                    created_by_email: AccountEmail::new("fork@test"),
                    config: vec![],
                    created_at: Timestamp::now_utc(),
                    parent: None,
                    last_known_status: AgentStatusRecord::default(),
                    original_phantom_id: None,
                    fingerprint,
                    agent_mode: AgentMode::Durable,
                },
            )
            .await
            .unwrap();
        stage
            .add(OplogEntry::create(Box::new(
                golem_common::model::oplog::CreateParameters {
                    agent_id: target.agent_id.clone(),
                    owner_kind: OwnerKind::ComponentAgent,
                    agent_mode: AgentMode::Durable,
                    component_revision: ComponentRevision::INITIAL,
                    env: vec![],
                    environment_id: target.environment_id,
                    created_by: account,
                    parent: None,
                    component_size: 100,
                    initial_total_linear_memory_size: 100,
                    initial_active_plugins: Default::default(),
                    local_agent_config: vec![],
                    original_phantom_id: None,
                    instance_id: fingerprint.0,
                },
            )))
            .await
            .unwrap();
        stage.add(OplogEntry::suspend()).await.unwrap();
        let record = StreamSessionRecord::ForkCut(StreamForkCutRecord {
            format_version: 1,
            request_hash: hash.to_vec(),
            creation_fingerprint: fingerprint,
            export: None,
            cut_index: cut,
            revert: None,
            epoch_floor: 1,
            selected_stream_id: None,
            retained_through: None,
        });
        let record = stage.upload_payload(&record).await.unwrap();
        stage
            .add(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record,
                summary: None,
            })
            .await
            .unwrap();
        write_guest_result(stage.as_ref(), None, phantom)
            .await
            .unwrap();
        // Extra entries after creation must not affect retry recognition.
        stage.add(OplogEntry::suspend()).await.unwrap();
        stage.commit(CommitLevel::Always).await.unwrap();
        let last = stage.current_oplog_index().await;
        drop(stage);
        let before = existing_fork(&service, &target, cut, hash).await.unwrap();
        assert!(
            service
                .publish_staged(
                    &target,
                    AgentMode::Durable,
                    crate::services::oplog::StagePublication::for_tests(stage_id),
                    last
                )
                .await
                .unwrap()
        );
        let live = fingerprint;
        // Each read of the `Create` gives the instance id of the target, also for another request.
        assert_eq!(
            [
                before,
                existing_fork(&service, &target, cut, hash).await.unwrap(),
                existing_fork(&service, &target, cut, [99; 32])
                    .await
                    .unwrap(),
                existing_fork(&service, &target, cut.next(), hash)
                    .await
                    .unwrap(),
                existing_fork(&service, &target, last.next(), hash)
                    .await
                    .unwrap(),
            ],
            [
                ExistingFork::Absent,
                ExistingFork::Matching(live),
                ExistingFork::Other(live),
                ExistingFork::Other(live),
                ExistingFork::Other(live),
            ]
        );
        assert_eq!(live_instance(&service, &target).await, Some(live));
        let result = service
            .read_exact(&target, AgentMode::Durable, OplogIndex::from_u64(5), 1)
            .await;
        let OplogEntry::End {
            start_index,
            response: Some(payload),
            ..
        } = &result[&OplogIndex::from_u64(5)]
        else {
            panic!("missing fork result")
        };
        assert_eq!(*start_index, OplogIndex::from_u64(4));
        let result: HostResponse = service
            .download_payload(&target, AgentMode::Durable, payload.clone())
            .await
            .unwrap();
        assert_eq!(
            result,
            HostResponse::GolemApiFork(HostResponseGolemApiFork {
                forked_phantom_id: phantom,
                result: Ok(golem_common::model::ForkResult::Forked),
            })
        );
    }
}
