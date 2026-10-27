// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

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
        request_hash(
            &source,
            generation,
            cut,
            None,
            Some((None, None, Uuid::nil()))
        )
        .unwrap()
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
        32,
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
        Some((None, None, phantom)),
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
    write_guest_result(stage.as_ref(), None, None, phantom)
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
        .await
        .unwrap();
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
