use super::*;
use test_r::test;

#[test]
async fn gc_retry_after_recreation_preserves_new_incarnation() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for failure in 0..3 {
            let mut f = Fixture::new(indexed.clone()).await;
            let pending = seal_fixture(&f).await;
            let a = f.metadata.fingerprint;
            mature_collections(&mut f);
            let oplog = f.oplog_service().await;
            oplog
                .delete(
                    &mut oplog.lock_lifecycle(&f.id().agent_id).await,
                    &f.id(),
                    AgentMode::Durable,
                    a,
                    Some(ShardEpoch(6)),
                )
                .await
                .unwrap();
            put_chunk_sentinel(&f, a).await;
            let tombstones = list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode)
                .await
                .unwrap();
            assert_eq!(tombstones.len(), 1);
            assert_eq!(tombstones[0].key.kind, SealRecordKind::Deleted);
            match failure {
                0 => f.blobs.fail_next_delete_dir(),
                1 => f.blobs.fail_next_delete(),
                2 => f.blobs.fail_delete_after_commit(0),
                _ => unreachable!(),
            }
            assert!(matches!(
                oplog
                    .collect_tombstone(&f.id(), f.metadata.agent_mode)
                    .await,
                Err(OplogError::Maintenance(_))
            ));
            assert_eq!(
                record_bytes(&f, &tombstones[0].key.path()).await.is_some(),
                failure != 2,
                "the injected failure must hit the intended GC boundary"
            );

            let b = AgentFingerprint::new();
            f.register(7, true);
            write_primary_entry(&f, &pending.state, &create_entry(&f, b), ShardEpoch(7)).await;
            put_chunk_sentinel(&f, b).await;
            assert_eq!(
                f.service().indexed_identity(&f.id()).await.unwrap(),
                Some(b)
            );

            let restarted = f.oplog_service().await;
            restarted
                .collect_tombstone(&f.id(), f.metadata.agent_mode)
                .await
                .unwrap();
            restarted
                .collect_tombstone(&f.id(), f.metadata.agent_mode)
                .await
                .unwrap();
            assert!(
                list_records(f.blobs.as_ref(), &f.id(), f.metadata.agent_mode)
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert_chunk_sentinel(&f, a, false).await;
            assert_chunk_sentinel(&f, b, true).await;
            assert_eq!(
                f.service().indexed_identity(&f.id()).await.unwrap(),
                Some(b)
            );
            assert!(matches!(
                f.service().state(&f.id()).await.unwrap(),
                AgentSealState::Present
            ));
            assert_fence(&f, pending.state.fence_namespace(), Some(ShardEpoch(7))).await;
            assert_eq!(
                restarted
                    .read_source(&f.id(), AgentMode::Durable, OplogIndex::INITIAL, 1)
                    .await
                    .unwrap(),
                BTreeMap::from([(OplogIndex::INITIAL, create_entry(&f, b))])
            );
        }
    }
}

#[test]
async fn gc_trimmed_marker_pin_preserves_moved_data_in_both_modes() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
            let mut f = Fixture::with_mode(indexed.clone(), mode).await;
            f.put_promise(3, Some(vec![17])).await;
            f.archive(1)
                .open(&f.id(), mode, None, f.metadata.fingerprint)
                .await
                .unwrap()
                .drop_prefix(OplogIndex::from_u64(3))
                .await
                .unwrap();
            let seal = f.service();
            let pending = seal
                .prepare(f.metadata.clone(), ShardEpoch(6))
                .await
                .unwrap()
                .unwrap();
            seal.remove_moved_copies(&pending).await.unwrap();
            f.register(7, true);
            match mode {
                AgentMode::Durable => seal.claim(&f.id(), ShardEpoch(7)).await.unwrap(),
                AgentMode::Ephemeral => seal.claim_ephemeral(&f.id(), ShardEpoch(7)).await.unwrap(),
            };
            assert_eq!(f.service().indexed_identity(&f.id()).await.unwrap(), None);
            let before = f
                .indexed
                .observe_agent("test", "before_gc", &f.id())
                .await
                .unwrap();
            let marker = match mode {
                AgentMode::Durable => before.durable_marker,
                AgentMode::Ephemeral => before.ephemeral_marker,
            }
            .unwrap();
            assert_eq!(marker.fingerprint, f.metadata.fingerprint);
            let tombstone = put_tombstone(&f, f.metadata.fingerprint, 7, 0).await;
            let foreign = SealRecordKey {
                fingerprint: AgentFingerprint::new(),
                epoch: ShardEpoch(8),
                seq: 0,
                kind: SealRecordKind::Sealed { valid_until_ms: 0 },
                ..pending.key
            };
            put_record(&f, foreign, b"inadmissible foreign seal").await;
            put_chunk_sentinel(&f, f.metadata.fingerprint).await;
            let original = record_bytes(&f, &pending.key.path()).await.unwrap();
            mature_collections(&mut f);
            let restarted = f.oplog_service().await;
            restarted
                .collect_tombstone(&f.id(), f.metadata.agent_mode)
                .await
                .unwrap();
            restarted
                .collect_tombstone(&f.id(), f.metadata.agent_mode)
                .await
                .unwrap();
            assert!(record_bytes(&f, &tombstone.path()).await.is_none());
            assert_eq!(record_bytes(&f, &pending.key.path()).await, Some(original));
            assert_eq!(
                record_bytes(&f, &foreign.path()).await.unwrap(),
                b"inadmissible foreign seal"
            );
            assert_chunk_sentinel(&f, f.metadata.fingerprint, true).await;
            assert_eq!(
                f.indexed
                    .observe_agent("test", "after_gc", &f.id())
                    .await
                    .unwrap(),
                before
            );
            assert_fence(&f, pending.state.fence_namespace(), Some(ShardEpoch(7))).await;
            assert_eq!(
                f.service()
                    .completed_promise(f.id().environment_id(), &f.promise(3))
                    .await
                    .unwrap(),
                Some(vec![17])
            );
        }
    }
}

#[test]
async fn same_epoch_marker_replacement_rejects_obsolete_proof() {
    let root = tempfile::TempDir::new().unwrap();
    for indexed in storages(&root).await {
        for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
            let f = Fixture::with_mode(indexed.clone(), mode).await;
            let seal = f.service();
            let pending = seal
                .prepare(f.metadata.clone(), ShardEpoch(6))
                .await
                .unwrap()
                .unwrap();
            let before = indexed
                .observe_agent("test", "original", &f.id())
                .await
                .unwrap();
            let original = match mode {
                AgentMode::Durable => before.durable_marker,
                AgentMode::Ephemeral => before.ephemeral_marker,
            }
            .unwrap();
            let winner = AgentFingerprint::new();
            indexed
                .mark_sealed(
                    "test",
                    "winner",
                    &pending.state,
                    ShardEpoch(6),
                    winner,
                    Some(original),
                )
                .await
                .unwrap();
            let committed = indexed
                .observe_agent("test", "winner", &f.id())
                .await
                .unwrap();
            indexed
                .mark_sealed(
                    "test",
                    "winner_retry",
                    &pending.state,
                    ShardEpoch(6),
                    winner,
                    Some(original),
                )
                .await
                .unwrap();
            for stale_target in [f.metadata.fingerprint, AgentFingerprint::new()] {
                assert!(
                    indexed
                        .mark_sealed(
                            "test",
                            "obsolete_proof",
                            &pending.state,
                            ShardEpoch(6),
                            stale_target,
                            Some(original)
                        )
                        .await
                        .is_err()
                );
                assert_eq!(
                    indexed
                        .observe_agent("test", "unchanged", &f.id())
                        .await
                        .unwrap(),
                    committed
                );
            }
        }
    }
}
