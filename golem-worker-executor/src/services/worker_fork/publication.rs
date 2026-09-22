// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use crate::services::oplog::{Oplog, OplogOps, OplogService, OplogServiceOps};
use golem_common::model::agent::AgentMode;
use golem_common::model::durable_stream::{StreamId, StreamOffset, StreamSessionRecord};
use golem_common::model::oplog::host_functions::GolemApiFork;
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostRequestNoInput, HostResponse,
    HostResponseGolemApiFork, OplogEntry, OplogIndex,
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

/// Reconcile without opening a writer or depending on the fork still being suspended. A fork
/// may already have run, archived, or lost its response; its creation prefix is immutable.
pub(crate) async fn existing_fork(
    service: &dyn OplogService,
    target: &OwnedAgentId,
    cut: OplogIndex,
    hash: [u8; 32],
) -> Result<bool, WorkerExecutorError> {
    let mode = AgentMode::Durable;
    let conflict = || WorkerExecutorError::worker_already_exists(target.agent_id.clone());
    if service.exists(target, AgentMode::Ephemeral).await {
        return Err(conflict());
    }
    if !service.exists(target, mode).await {
        return Ok(false);
    }
    if service.get_last_index(target, mode).await < cut.next() {
        return Err(conflict());
    }
    let initial = service
        .read_source(target, mode, OplogIndex::INITIAL, 1)
        .await;
    let Some(OplogEntry::Create {
        instance_id,
        agent_id,
        environment_id,
        ..
    }) = initial.get(&OplogIndex::INITIAL)
    else {
        return Err(conflict());
    };
    if *agent_id != target.agent_id || *environment_id != target.environment_id {
        return Err(conflict());
    }
    let entries = service.read_source(target, mode, cut.next(), 1).await;
    let Some(OplogEntry::StreamSession { record, .. }) = entries.get(&cut.next()) else {
        return Err(conflict());
    };
    let record = service
        .download_payload(target, mode, record.clone())
        .await
        .map_err(WorkerExecutorError::runtime)?;
    if !record.has_supported_format() {
        return Err(conflict());
    }
    match record {
        StreamSessionRecord::ForkCut(record)
            if record.request_hash == hash
                && record.cut_index == cut
                && record.creation_fingerprint.0 == *instance_id =>
        {
            Ok(true)
        }
        _ => Err(conflict()),
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
            },
            Box::new(move |start_index| OplogEntry::End {
                timestamp: now,
                start_index,
                response: Some(response_payload),
                forced_commit: false,
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
        assert!(!existing_fork(&service, &target, cut, hash).await.unwrap());
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
            .add(OplogEntry::create(
                target.agent_id.clone(),
                OwnerKind::ComponentAgent,
                AgentMode::Durable,
                ComponentRevision::INITIAL,
                vec![],
                target.environment_id,
                account,
                None,
                100,
                100,
                Default::default(),
                vec![],
                None,
                fingerprint.0,
            ))
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
        assert!(!existing_fork(&service, &target, cut, hash).await.unwrap());
        assert!(
            service
                .publish_staged(&target, AgentMode::Durable, stage_id, last)
                .await
                .unwrap()
        );
        assert!(existing_fork(&service, &target, cut, hash).await.unwrap());
        assert!(
            existing_fork(&service, &target, cut, [99; 32])
                .await
                .is_err()
        );
        assert!(
            existing_fork(&service, &target, cut.next(), hash)
                .await
                .is_err()
        );
        assert!(
            existing_fork(&service, &target, last.next(), hash)
                .await
                .is_err()
        );
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
