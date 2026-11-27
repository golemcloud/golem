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

use super::*;
use crate::services::oplog::OplogReadError;
use crate::services::oplog::retention::test_oplog::{RetainedOplog, expired, idx, jump};
use test_r::test;

#[test]
async fn the_retained_start_survives_a_reopen_and_never_moves_back() {
    let oplog = RetainedOplog::new().await;
    oplog.with_jumps(10).await;
    assert_eq!(oplog.open().await.retention(), OplogRetention::FULL);

    assert_eq!(oplog.advance(5).await.start(), idx(5));
    assert_eq!(oplog.advance(3).await.start(), idx(5));

    assert_eq!(oplog.open().await.retention().start(), idx(5));
    assert_eq!(oplog.start().await, idx(5));
}

/// Each agent mode has its own start. An ephemeral start survives a reopen of the ephemeral
/// oplog next to the durable start of the same agent.
#[test]
async fn each_agent_mode_keeps_its_own_start_through_a_reopen() {
    let oplog = RetainedOplog::new().await;
    assert_eq!(
        oplog.advance_in(AgentMode::Ephemeral, 6).await.start(),
        idx(6)
    );
    assert_eq!(
        oplog.advance_in(AgentMode::Durable, 4).await.start(),
        idx(4)
    );

    assert_eq!(
        oplog
            .open_in(AgentMode::Ephemeral)
            .await
            .retention()
            .start(),
        idx(6)
    );
    assert_eq!(
        oplog.open_in(AgentMode::Durable).await.retention().start(),
        idx(4)
    );
    let stored = |agent_mode| oplog.service.retention(&oplog.owned_agent_id, agent_mode);
    assert_eq!(stored(AgentMode::Ephemeral).await.unwrap().start(), idx(6));
    assert_eq!(stored(AgentMode::Durable).await.unwrap().start(), idx(4));
}

#[test]
async fn the_length_counts_the_retained_entries_and_the_last_index_stays() {
    let oplog = RetainedOplog::new().await;
    oplog.with_jumps(10).await;
    let full = oplog.open().await;
    assert_eq!(full.add(jump(11)).await.unwrap(), idx(11));
    assert_eq!(full.current_oplog_index().await, idx(11));
    assert_eq!(full.length().await.unwrap(), 10);
    assert_eq!(full.try_length().await.unwrap(), 10);

    // The entries below the start stay in storage, so the count of the stored entries is
    // 10, not the 7 retained entries.
    oplog.advance(4).await;

    let handle = oplog.open().await;
    assert_eq!(handle.length().await.unwrap(), 7);
    assert_eq!(handle.try_length().await.unwrap(), 7);
    assert_eq!(handle.current_oplog_index().await, idx(11));
    assert_eq!(
        oplog
            .service
            .get_last_index(&oplog.owned_agent_id, AgentMode::Durable)
            .await
            .unwrap(),
        idx(10)
    );
    handle.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(handle.length().await.unwrap(), 8);
    assert_eq!(handle.try_length().await.unwrap(), 8);
}

#[test]
async fn a_failed_retained_tail_read_returns_an_error_from_length() {
    use crate::services::oplog::tests::ReadCountingIndexedStorage;
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;

    let indexed = Arc::new(ReadCountingIndexedStorage::new());
    let oplog =
        RetainedOplog::with_storage(indexed.clone(), Arc::new(InMemoryKeyValueStorage::new()))
            .await;
    oplog.with_jumps(10).await;
    oplog.advance(4).await;
    let handle = oplog.open().await;

    indexed.fail_next_last_id();
    let error = handle.length().await.unwrap_err();
    assert_eq!(error.agent_id, *oplog.agent_id());
    assert!(matches!(
        error.error,
        OplogReadError::SourceFailure { detail, .. } if detail.contains("injected tail lookup failure")
    ));
    assert_eq!(handle.length().await.unwrap(), 7);
}

#[test]
async fn a_read_below_the_start_is_expired_and_the_create_entry_stays_readable() {
    let oplog = RetainedOplog::new().await;
    oplog.with_jumps(10).await;
    oplog.advance(4).await;
    let handle = oplog.open().await;

    assert_eq!(
        handle.read_exact(idx(3), 2).await.unwrap_err().error,
        expired(4)
    );
    assert_eq!(
        handle.read_source(idx(2), 5).await.unwrap_err().error,
        expired(4)
    );
    let refused = handle.try_read_source(idx(3), 2).await.unwrap_err();
    assert!(refused.contains(&expired(4).to_string()), "{refused}");
    assert_eq!(oplog.read(1, 4).await.unwrap_err(), expired(4));
    assert_eq!(oplog.read(3, 1).await.unwrap_err(), expired(4));
    assert_eq!(
        handle
            .read_exact(idx(4), 7)
            .await
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        (4..=10)
            .map(|index| (idx(index), jump(index)))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        handle
            .try_read_source(idx(4), 1)
            .await
            .unwrap()
            .get(&idx(4)),
        Some(&jump(4))
    );
    assert_eq!(
        handle.read(idx(1)).await.unwrap(),
        jump(1),
        "the entry at index 1 is kept below the start"
    );
    assert_eq!(oplog.read(1, 1).await.unwrap().get(&idx(1)), Some(&jump(1)));
}

/// The identity read of the worker service reads the `Create` entry alone through
/// `read_source` of the service. At a start of 4 it gives the `Create` entry, and every other
/// read below the start through the same call is expired.
#[test]
async fn the_identity_read_through_the_service_gives_the_create_entry_at_a_start_above_1() {
    let oplog = RetainedOplog::new().await;
    let handle = oplog.create(oplog.create_entry(), ShardEpoch(1)).await;
    oplog.add((2..=10).map(jump)).await;
    drop(handle);
    oplog.advance(4).await;
    let read_source = |start: u64, n: u64| {
        oplog
            .service
            .read_source(&oplog.owned_agent_id, AgentMode::Durable, idx(start), n)
    };

    let identity = read_source(1, 1).await.unwrap();
    assert!(
        matches!(
            identity.get(&idx(1)),
            Some(OplogEntry::Create { parameters, .. })
                if parameters.agent_id == *oplog.agent_id()
        ) && identity.len() == 1,
        "{identity:?}"
    );
    assert_eq!(read_source(1, 2).await.unwrap_err().error, expired(4));
    assert_eq!(read_source(3, 1).await.unwrap_err().error, expired(4));
    assert_eq!(
        read_source(4, 2).await.unwrap(),
        BTreeMap::from([(idx(4), jump(4)), (idx(5), jump(5))])
    );
}

#[test]
async fn only_a_gap_that_ends_below_the_start_is_expired() {
    let oplog = RetainedOplog::new().await;
    oplog.with_jumps(10).await;
    oplog.open().await.drop_prefix(idx(5)).await;
    let gap = oplog.read(1, 1).await.unwrap_err();
    assert!(
        matches!(gap, OplogReadError::Gap { start, .. } if start == idx(1)),
        "with the start at index 1 a missing `Create` entry stays a gap: {gap:?}"
    );
    oplog.advance(4).await;

    let gap = oplog.read(4, 3).await.unwrap_err();
    assert!(
        matches!(gap, OplogReadError::Gap { start, end } if start == idx(4) && end == idx(5)),
        "{gap:?}"
    );
    let gap = oplog
        .open()
        .await
        .read_exact(idx(4), 3)
        .await
        .unwrap_err()
        .error;
    assert!(matches!(gap, OplogReadError::Gap { .. }), "{gap:?}");

    oplog.advance(6).await;
    assert_eq!(oplog.read(1, 1).await.unwrap_err(), expired(6));
    assert_eq!(
        oplog.open().await.read(idx(1)).await.unwrap_err().error,
        expired(6)
    );
}

#[test]
async fn a_refused_delete_keeps_the_retained_start_and_a_delete_removes_it() {
    let oplog = RetainedOplog::new().await;
    let handle = oplog.create(jump(1), ShardEpoch(5)).await;
    handle.add_and_commit(jump(2)).await.unwrap();
    drop(handle);
    oplog.advance(2).await;

    let refused = oplog.delete(Some(ShardEpoch(4))).await;
    assert!(matches!(refused, Err(OplogError::Fenced(_))), "{refused:?}");
    assert_eq!(
        oplog.start().await,
        idx(2),
        "a refused delete keeps the start"
    );
    assert!(
        oplog
            .service
            .exists(&oplog.owned_agent_id, AgentMode::Durable)
            .await
            .unwrap()
    );

    oplog.delete(Some(ShardEpoch(5))).await.unwrap();
    assert_eq!(oplog.start().await, OplogIndex::INITIAL);
    assert!(
        !oplog
            .service
            .exists(&oplog.owned_agent_id, AgentMode::Durable)
            .await
            .unwrap()
    );
}

/// A failed read of the `Create` entry alone needs the record to classify its error. When the
/// read of the record fails too, the error of the read stays, because it is the cause.
#[test]
async fn a_failed_record_read_keeps_the_error_of_the_read() {
    use crate::storage::keyvalue::KeyValueStorageError;
    use crate::storage::keyvalue::fault_injecting::{
        FaultInjectingKeyValueStorage, KeyValueStorageFaults,
    };
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;

    let faults = KeyValueStorageFaults::default();
    let oplog =
        RetainedOplog::with_key_value_storage(Arc::new(FaultInjectingKeyValueStorage::new(
            Arc::new(InMemoryKeyValueStorage::new()),
            faults.clone(),
        )))
        .await;
    oplog.with_jumps(10).await;
    oplog.open().await.drop_prefix(idx(5)).await;
    let calls = faults.calls("read_retention");
    faults.fail(
        "read_retention",
        1,
        KeyValueStorageError::Other("unavailable".to_string()),
    );

    let gap = oplog.read(1, 1).await.unwrap_err();
    assert!(
        matches!(gap, OplogReadError::Gap { start, .. } if start == idx(1)),
        "{gap:?}"
    );
    assert_eq!(faults.calls("read_retention"), calls + 1);
}

/// A delete whose removal of the retention record fails reports the failure, so the caller
/// does not take the delete as complete while the record keeps the old start.
#[test]
async fn a_failed_removal_of_the_retention_record_fails_the_delete() {
    use crate::storage::keyvalue::KeyValueStorageError;
    use crate::storage::keyvalue::fault_injecting::{
        FaultInjectingKeyValueStorage, KeyValueStorageFaults,
    };
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;

    let faults = KeyValueStorageFaults::default();
    let oplog =
        RetainedOplog::with_key_value_storage(Arc::new(FaultInjectingKeyValueStorage::new(
            Arc::new(InMemoryKeyValueStorage::new()),
            faults.clone(),
        )))
        .await;
    let handle = oplog.create(jump(1), ShardEpoch(5)).await;
    handle.add_and_commit(jump(2)).await.unwrap();
    drop(handle);
    oplog.advance(2).await;
    faults.fail(
        "remove_retention",
        1,
        KeyValueStorageError::Other("unavailable".to_string()),
    );

    let failed = oplog.delete(Some(ShardEpoch(5))).await;
    assert!(
        matches!(failed, Err(OplogError::Maintenance(_))),
        "{failed:?}"
    );
    assert_eq!(faults.calls("remove_retention"), 1);
    assert_eq!(oplog.start().await, idx(2), "the record keeps the start");
}

/// Every call of an open oplog that the layer does not check reaches the inner oplog with the
/// same arguments, and its answer comes back unchanged.
#[test]
async fn the_retained_oplog_forwards_every_other_call_to_the_inner_oplog() {
    use crate::services::oplog::retention::test_oplog::{RecordingOplog, at, retained_handle};

    let inner = Arc::new(RecordingOplog::default());
    let handle = retained_handle(inner.clone(), AgentId::from_parts(7, 1), at(4));
    let payload_id = PayloadId(uuid::Uuid::from_u128(9));

    assert!(!handle.is_retired());
    handle.retire();
    assert!(inner.is_retired());
    assert!(handle.is_retired());
    assert!(std::ptr::eq(handle.task_owner().unwrap(), &inner.tasks));
    assert_eq!(
        handle
            .add_durable_stream_batch(Box::new(|_| Vec::new()))
            .await
            .unwrap(),
        vec![(idx(21), jump(21))]
    );
    assert_eq!(handle.try_drop_prefix(idx(6)).await, Ok(6));
    assert_eq!(
        handle.commit(CommitLevel::Always).await.unwrap(),
        BTreeMap::from([(idx(8), jump(8))])
    );
    assert_eq!(handle.try_current_oplog_index().await, Ok(idx(12)));
    assert_eq!(handle.last_added_non_hint_entry().await, Some(idx(11)));
    assert!(
        handle
            .wait_for_replicas(2, Duration::from_secs(3))
            .await
            .unwrap()
    );
    assert!(
        !handle
            .wait_for_replicas(1, Duration::from_secs(3))
            .await
            .unwrap()
    );
    assert_eq!(
        handle
            .download_raw_payload(payload_id.clone(), vec![7, 8])
            .await,
        Ok(vec![7, 8])
    );
    assert_eq!(
        handle
            .download_raw_payload_classified(payload_id.clone(), vec![8, 7])
            .await
            .unwrap(),
        vec![8, 7]
    );
    assert!(Arc::ptr_eq(
        &handle.inner().unwrap(),
        &(inner.clone() as Arc<dyn Oplog>)
    ));
    assert_eq!(
        handle.closed().await,
        Err("the recording oplog is closed".to_string())
    );
    assert_eq!(handle.enqueue_add(jump(9)).await.unwrap(), idx(14));
    assert_eq!(handle.drop_prefix(idx(5)).await, 10);
    assert_eq!(handle.current_oplog_index().await, idx(13));
    let session_key = StreamSessionKey {
        callee_environment_id: EnvironmentId::new(),
        callee: AgentId::from_parts(7, 2),
        callee_fingerprint: golem_common::model::AgentFingerprint::new(),
        idempotency_key: golem_common::model::InvocationId::from(uuid::Uuid::from_u128(23)),
    };
    assert_eq!(
        handle.raw_durable_stream_session_status(&session_key).await,
        RawDurableStreamSessionStatus {
            watermark: idx(15),
            status: Ok(None),
        }
    );
    assert!(matches!(
        handle.upload_raw_payload(vec![1, 2, 3]).await,
        Ok(RawOplogPayload::SerializedInline(data)) if data == vec![1, 2, 3]
    ));
    // The builder gives a `Start` whose index is the length of the inline payload, so the
    // entry shows that the payload of the call reached the builder of the call.
    let inline_length = |payload: RawOplogPayload| match payload {
        RawOplogPayload::SerializedInline(bytes) => Ok(jump(bytes.len() as u64)),
        RawOplogPayload::External { .. } => Err("an external payload".to_string()),
    };
    let start = handle
        .add_start_with_reserved_raw_payload(vec![4; 5], Box::new(inline_length))
        .await
        .unwrap();
    assert_eq!((start.index, start.entry), (idx(16), jump(5)));
    let start = handle
        .add_start_with_indexed_reserved_raw_payload(Box::new(move |index| {
            Ok((
                vec![6; index.as_u64() as usize],
                Box::new(inline_length) as ReservedRawStartBuilder,
            ))
        }))
        .await
        .unwrap();
    assert_eq!((start.index, start.entry), (idx(17), jump(17)));
    assert_eq!(
        handle
            .enqueue_add_pair(jump(2), Box::new(|index| jump(index.as_u64())))
            .await
            .unwrap(),
        (idx(18), idx(19))
    );

    assert_eq!(
        inner.take_calls(),
        vec![
            "retire".to_string(),
            "add_durable_stream_batch(0)".to_string(),
            "try_drop_prefix(6)".to_string(),
            "commit(Always)".to_string(),
            "try_current_oplog_index".to_string(),
            "last_added_non_hint_entry".to_string(),
            "wait_for_replicas(2, 3s)".to_string(),
            "wait_for_replicas(1, 3s)".to_string(),
            format!("download_raw_payload({payload_id:?}, [7, 8])"),
            format!("download_raw_payload_classified({payload_id:?}, [8, 7])"),
            "closed".to_string(),
            format!("enqueue_add({:?})", jump(9)),
            "drop_prefix(5)".to_string(),
            "current_oplog_index".to_string(),
            format!(
                "raw_durable_stream_session_status({})",
                session_key.idempotency_key
            ),
            "upload_raw_payload([1, 2, 3])".to_string(),
            "add_start_with_reserved_raw_payload([4, 4, 4, 4, 4])".to_string(),
            format!(
                "add_start_with_indexed_reserved_raw_payload({:?})",
                vec![6u8; 17]
            ),
            format!("enqueue_add_pair({:?}, {:?})", jump(2), jump(19)),
        ]
    );
}

/// Every call of the service that the layer does not check reaches the inner service with
/// the same arguments, and its answer comes back unchanged. A read through the layer reads
/// the inner service after the check of the retained start, and a read below the start does
/// not reach the inner service.
#[test]
async fn the_retention_service_forwards_every_other_call_to_the_inner_service() {
    use crate::services::oplog::StagePublication;
    use crate::services::oplog::retention::advance_oplog_retention;
    use crate::services::oplog::retention::test_oplog::{
        RecordingOplogService, RetainedOplog, at, execution_status, last_known_status,
    };
    use crate::services::stream_session_index::StreamSessionIndexService;
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;

    let owner = OwnedAgentId::new(EnvironmentId::new(), &AgentId::from_parts(7, 1));
    let other = OwnedAgentId::new(EnvironmentId::new(), &AgentId::from_parts(7, 2));
    let inner = Arc::new(RecordingOplogService::new(owner.clone()));
    let key_value_storage: Arc<dyn KeyValueStorage + Send + Sync> =
        Arc::new(InMemoryKeyValueStorage::new());
    let service = RetentionOplogService::new(inner.clone(), key_value_storage.clone());
    advance_oplog_retention(
        key_value_storage.as_ref(),
        &owner,
        AgentMode::Durable,
        idx(4),
    )
    .await
    .unwrap();
    let odd_stage = uuid::Uuid::from_u128(3);
    let even_stage = uuid::Uuid::from_u128(4);
    let payload_id = PayloadId(uuid::Uuid::from_u128(9));
    let type_id = TypeId::try_from(3u32).unwrap();
    let durable = AgentMode::Durable;
    let ephemeral = AgentMode::Ephemeral;

    let index = Arc::new(StreamSessionIndexService::new(
        key_value_storage.clone(),
        Arc::downgrade(&(inner.clone() as Arc<dyn OplogService>)),
    ));
    service.set_stream_session_index(index.clone());
    assert!(Arc::ptr_eq(&inner.stream_session_index().unwrap(), &index));
    assert!(Arc::ptr_eq(
        &service.stream_session_index().unwrap(),
        &index
    ));
    assert_eq!(
        service.staged_exists(&owner, durable, odd_stage).await,
        Ok(true)
    );
    assert_eq!(
        service.staged_exists(&owner, durable, even_stage).await,
        Ok(false)
    );
    assert_eq!(
        service
            .publish_staged(
                &owner,
                durable,
                StagePublication::for_tests(odd_stage),
                idx(6)
            )
            .await,
        Ok(true)
    );
    assert_eq!(
        service
            .publish_staged(
                &owner,
                ephemeral,
                StagePublication::for_tests(even_stage),
                idx(3)
            )
            .await,
        Ok(false)
    );
    assert_eq!(
        service.discard_staged(&owner, durable, odd_stage).await,
        Err(format!("no stage {odd_stage}"))
    );
    assert_eq!(
        service.try_get_last_index(&owner, durable).await,
        Ok(idx(13))
    );
    service
        .assert_owning_epoch(&owner, durable, ShardEpoch(5))
        .await
        .unwrap();
    let fenced = service
        .assert_owning_epoch(&owner, durable, ShardEpoch(4))
        .await;
    assert!(
        matches!(&fenced, Err(OplogError::Fenced(fence)) if fence.expected_epoch == ShardEpoch(4)),
        "{fenced:?}"
    );
    assert_eq!(
        service
            .read_source(&owner, durable, idx(5), 2)
            .await
            .unwrap(),
        BTreeMap::from([(idx(5), jump(5)), (idx(6), jump(6))])
    );
    assert_eq!(
        service
            .read_source(&owner, durable, idx(2), 3)
            .await
            .unwrap_err()
            .error,
        expired(4)
    );
    assert_eq!(service.try_exists(&owner, durable).await, Ok(true));
    assert_eq!(service.try_exists(&owner, ephemeral).await, Ok(false));
    assert_eq!(
        service
            .scan_for_type(
                &other.environment_id,
                type_id,
                Some(durable),
                ScanCursor::new("from".to_string()),
                17
            )
            .await
            .unwrap(),
        ScanPage {
            cursor: ScanCursor::new("after 3".to_string()),
            agent_ids: vec![owner.clone()],
            examined_keys: 17,
        }
    );
    assert_eq!(
        service
            .download_raw_payload(&owner, durable, payload_id.clone(), vec![7, 8])
            .await,
        Ok(vec![7, 8])
    );
    assert_eq!(
        service
            .download_raw_payload_classified(&other, ephemeral, payload_id.clone(), vec![8, 7])
            .await
            .unwrap(),
        vec![8, 7]
    );
    let recorded = inner.oplog.clone() as Arc<dyn Oplog>;
    let metadata = RetainedOplog::new().await.metadata();
    let created = service
        .create(
            &mut service.lock_lifecycle(&owner.agent_id).await,
            &owner,
            durable,
            jump(1),
            metadata.clone(),
            last_known_status(),
            execution_status(),
            Some(ShardEpoch(5)),
        )
        .await
        .unwrap();
    assert_eq!(created.retention(), OplogRetention::FULL);
    assert!(Arc::ptr_eq(&created.inner().unwrap(), &recorded));
    let fresh = service
        .create_fresh(
            &mut service.lock_lifecycle(&other.agent_id).await,
            &other,
            ephemeral,
            jump(2),
            metadata.clone(),
            last_known_status(),
            execution_status(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(fresh.retention(), OplogRetention::FULL);
    assert!(Arc::ptr_eq(&fresh.inner().unwrap(), &recorded));
    let opened = service
        .open(
            &mut service.lock_lifecycle(&owner.agent_id).await,
            &owner,
            durable,
            Some(idx(9)),
            metadata.clone(),
            last_known_status(),
            execution_status(),
            Some(ShardEpoch(6)),
        )
        .await
        .unwrap();
    assert_eq!(opened.retention(), at(4));
    assert!(Arc::ptr_eq(&opened.inner().unwrap(), &recorded));
    assert!(Arc::ptr_eq(
        &service
            .create_staged(&owner, ephemeral, odd_stage, metadata.clone())
            .await
            .unwrap(),
        &recorded
    ));
    assert_eq!(
        service.get_last_index(&other, durable).await.unwrap(),
        idx(14)
    );
    assert!(service.exists(&owner, ephemeral).await.unwrap());
    assert!(!service.exists(&owner, durable).await.unwrap());
    assert!(matches!(
        service.upload_raw_payload(&other, ephemeral, vec![1, 2]).await,
        Ok(RawOplogPayload::SerializedInline(data)) if data == vec![1, 2]
    ));
    service
        .delete(
            &mut service.lock_lifecycle(&owner.agent_id).await,
            &owner,
            durable,
            Some(ShardEpoch(5)),
        )
        .await
        .unwrap();
    assert_eq!(
        service.retention(&owner, durable).await.unwrap(),
        OplogRetention::FULL,
        "the delete removed the record after the inner delete"
    );

    assert_eq!(
        inner.take_calls(),
        vec![
            "set_stream_session_index".to_string(),
            format!("staged_exists({owner}, Durable, {odd_stage})"),
            format!("staged_exists({owner}, Durable, {even_stage})"),
            format!("publish_staged({owner}, Durable, {odd_stage}, 6)"),
            format!("publish_staged({owner}, Ephemeral, {even_stage}, 3)"),
            format!("discard_staged({owner}, Durable, {odd_stage})"),
            format!("try_get_last_index({owner}, Durable)"),
            format!("assert_owning_epoch({owner}, Durable, {:?})", ShardEpoch(5)),
            format!("assert_owning_epoch({owner}, Durable, {:?})", ShardEpoch(4)),
            format!("read_source({owner}, Durable, 5, 2)"),
            format!("try_exists({owner}, Durable)"),
            format!("try_exists({owner}, Ephemeral)"),
            format!(
                "scan_for_type({:?}, 3, Some(Durable), {:?}, 17)",
                other.environment_id,
                ScanCursor::new("from".to_string())
            ),
            format!("download_raw_payload({owner}, Durable, {payload_id:?}, [7, 8])"),
            format!("download_raw_payload_classified({other}, Ephemeral, {payload_id:?}, [8, 7])"),
            format!("lock_lifecycle({})", owner.agent_id),
            format!(
                "create({owner}, Durable, {:?}, {}, {:?})",
                jump(1),
                metadata.agent_id,
                Some(ShardEpoch(5))
            ),
            format!("lock_lifecycle({})", other.agent_id),
            format!(
                "create_fresh({other}, Ephemeral, {:?}, {}, None)",
                jump(2),
                metadata.agent_id
            ),
            format!("lock_lifecycle({})", owner.agent_id),
            format!(
                "open({owner}, Durable, {:?}, {}, {:?})",
                Some(idx(9)),
                metadata.agent_id,
                Some(ShardEpoch(6))
            ),
            format!(
                "create_staged({owner}, Ephemeral, {odd_stage}, {})",
                metadata.agent_id
            ),
            format!("get_last_index({other}, Durable)"),
            format!("exists({owner}, Ephemeral)"),
            format!("exists({owner}, Durable)"),
            format!("upload_raw_payload({other}, Ephemeral, [1, 2])"),
            format!("lock_lifecycle({})", owner.agent_id),
            format!("delete({owner}, Durable, {:?})", Some(ShardEpoch(5))),
        ]
    );
}
