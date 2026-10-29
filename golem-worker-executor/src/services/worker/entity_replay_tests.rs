use super::*;
use golem_common::model::OpenEntityReplayScope;
use test_r::test;

#[test]
fn entity_replay_split_persistence_roundtrip_delta_and_truncation() {
    let mut status = AgentStatusRecord::default();
    let first = OplogIndex::from_u64(2);
    let second = OplogIndex::from_u64(10);
    status.entity_replay_progress.completed.insert(
        first,
        EntityReplayBoundary {
            terminal_index: OplogIndex::from_u64(8),
            last_required: Some(OplogIndex::from_u64(7)),
            parent_entity: None,
        },
    );
    status.entity_replay_progress.open.insert(
        second,
        OpenEntityReplayScope {
            last_required: Some(OplogIndex::from_u64(11)),
            parent_entity: None,
        },
    );
    status
        .entity_replay_progress
        .routes
        .insert(OplogIndex::from_u64(11), second);
    status
        .entity_replay_progress
        .route_members
        .insert((second, OplogIndex::from_u64(11)), ());
    let mut core = status.clone();
    let parts = split_status(&mut core);
    assert_eq!(core.entity_replay_progress, EntityReplayProgress::default());
    assert_eq!(
        serialize(&core).unwrap(),
        serialize(&status_core(&status)).unwrap()
    );
    let (sets, deletes) = compute_status_field_writes(None, &[], &core, &parts).unwrap();
    assert!(deletes.is_empty());
    let mut fields: HashMap<String, bytes::Bytes> =
        sets.into_iter().map(|(k, v)| (k, v.into())).collect();
    assert_eq!(reassemble_cached_status(fields.clone()).unwrap(), status);

    let mut next = status.clone();
    next.entity_replay_progress.completed.insert(
        second,
        EntityReplayBoundary {
            terminal_index: OplogIndex::from_u64(12),
            last_required: Some(OplogIndex::from_u64(11)),
            parent_entity: None,
        },
    );
    next.entity_replay_progress.open.clear();
    next.entity_replay_progress.routes.clear();
    next.entity_replay_progress.route_members.clear();
    let mut core = next.clone();
    let parts = split_status(&mut core);
    let (sets, deletes) = compute_status_field_writes(Some(&status), &[], &core, &parts).unwrap();
    let names: HashSet<_> = sets.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, HashSet::from([STATUS_CORE_FIELD, "er:10"]));
    assert_eq!(
        deletes.iter().map(String::as_str).collect::<HashSet<_>>(),
        HashSet::from(["eo:10", "ep:11", "em:10:11"])
    );
    fields.extend(sets.into_iter().map(|(k, v)| (k, v.into())));
    for key in deletes {
        fields.remove(&key);
    }
    assert_eq!(reassemble_cached_status(fields.clone()).unwrap(), next);

    let keys: Vec<_> = fields.keys().cloned().collect();
    let mut truncated = status.clone();
    let parts = split_status(&mut truncated);
    for previous in [Some(&next), None] {
        let (sets, deletes) =
            compute_status_field_writes(previous, &keys, &truncated, &parts).unwrap();
        assert_eq!(deletes, vec!["er:10".to_string()]);
        let mut restored = fields.clone();
        restored.extend(sets.into_iter().map(|(k, v)| (k, v.into())));
        for key in deletes {
            restored.remove(&key);
        }
        assert_eq!(reassemble_cached_status(restored).unwrap(), status);
    }
    fields.remove(STATUS_ENTITY_REPLAY_CONTINUATION_FIELD);
    assert!(reassemble_cached_status(fields).is_none());
}

#[test]
fn entity_replay_growing_continuation_writes_only_changed_keys() {
    let root = OplogIndex::from_u64(1);
    let mut status = AgentStatusRecord::default();
    status.entity_replay_progress.open.insert(
        root,
        OpenEntityReplayScope {
            last_required: None,
            parent_entity: None,
        },
    );
    let mut core = status.clone();
    let parts = split_status(&mut core);
    let (sets, _) = compute_status_field_writes(None, &[], &core, &parts).unwrap();
    let mut fields: HashMap<String, bytes::Bytes> =
        sets.into_iter().map(|(k, v)| (k, v.into())).collect();
    for i in 2..258 {
        let index = OplogIndex::from_u64(i);
        let mut next = status.clone();
        next.oplog_idx = index;
        next.entity_replay_progress
            .open
            .get_mut(&root)
            .unwrap()
            .last_required = Some(index);
        next.entity_replay_progress.routes.insert(index, root);
        next.entity_replay_progress
            .route_members
            .insert((root, index), ());
        let mut core = next.clone();
        let parts = split_status(&mut core);
        let (sets, deletes) =
            compute_status_field_writes(Some(&status), &[], &core, &parts).unwrap();
        assert!(deletes.is_empty());
        assert_eq!(
            sets.iter().map(|(k, _)| k.clone()).collect::<HashSet<_>>(),
            HashSet::from([
                STATUS_CORE_FIELD.to_string(),
                "eo:1".to_string(),
                format!("ep:{i}"),
                format!("em:1:{i}"),
            ])
        );
        for (key, bytes) in &sets {
            if key == "eo:1" {
                assert_eq!(
                    *bytes,
                    serialize(&next.entity_replay_progress.open[&root]).unwrap()
                );
            } else if key.starts_with("ep:") {
                assert_eq!(*bytes, serialize(&root).unwrap());
            } else if key.starts_with("em:") {
                assert_eq!(*bytes, serialize(&()).unwrap());
            }
        }
        fields.extend(sets.into_iter().map(|(k, v)| (k, v.into())));
        assert_eq!(reassemble_cached_status(fields.clone()).unwrap(), next);
        status = next;
    }
    for i in 300..556 {
        let index = OplogIndex::from_u64(i);
        let mut next = status.clone();
        next.entity_replay_progress.completed.insert(
            index,
            EntityReplayBoundary {
                terminal_index: index.next(),
                last_required: None,
                parent_entity: None,
            },
        );
        let mut core = next.clone();
        let parts = split_status(&mut core);
        let (sets, deletes) =
            compute_status_field_writes(Some(&status), &[], &core, &parts).unwrap();
        assert!(deletes.is_empty());
        assert_eq!(
            sets.iter().map(|(k, _)| k.clone()).collect::<HashSet<_>>(),
            HashSet::from([STATUS_CORE_FIELD.to_string(), format!("er:{i}"),])
        );
        status = next;
    }
    let child = OplogIndex::from_u64(600);
    let mut next = status.clone();
    next.entity_replay_progress.open.insert(
        child,
        OpenEntityReplayScope {
            last_required: None,
            parent_entity: Some(root),
        },
    );
    next.entity_replay_progress
        .entity_children
        .insert((root, child), ());
    let mut core = next.clone();
    let parts = split_status(&mut core);
    let (sets, deletes) = compute_status_field_writes(Some(&status), &[], &core, &parts).unwrap();
    assert!(deletes.is_empty());
    assert_eq!(
        sets.iter().map(|(k, _)| k.as_str()).collect::<HashSet<_>>(),
        HashSet::from([STATUS_CORE_FIELD, "eo:600", "ec:1:600"])
    );
    let (sets, _) = compute_status_field_writes(None, &[], &core, &parts).unwrap();
    assert_eq!(
        reassemble_cached_status(sets.into_iter().map(|(k, v)| (k, v.into()))).unwrap(),
        next
    );
}
