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
    assert_eq!(core, status_core(&status));
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
                    decode::<OpenEntityReplayScope>(bytes).unwrap(),
                    OpenEntityReplayScope {
                        last_required: Some(index),
                        parent_entity: None,
                    }
                );
            } else if key.starts_with("ep:") {
                assert_eq!(decode::<OplogIndex>(bytes).unwrap(), root);
            } else if key.starts_with("em:") {
                decode::<()>(bytes).unwrap();
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

#[test]
fn resolved_names_persist_in_their_own_field_written_only_when_they_change() {
    let status = AgentStatusRecord::default();
    let mut core = status.clone();
    let parts = split_status(&mut core);
    let (sets, _) = compute_status_field_writes(None, &[], &core, &parts).unwrap();
    let mut fields: HashMap<String, bytes::Bytes> =
        sets.into_iter().map(|(k, v)| (k, v.into())).collect();

    let mut next = status.clone();
    next.resolved_names
        .links
        .insert("a.test", ["10.0.0.1".parse().unwrap()]);
    next.resolved_names
        .links
        .insert("b.test", ["10.0.0.1".parse().unwrap()]);
    let mut core = next.clone();
    let parts = split_status(&mut core);
    assert_eq!(core, status_core(&next));
    let (sets, deletes) = compute_status_field_writes(Some(&status), &[], &core, &parts).unwrap();
    assert!(deletes.is_empty());
    assert_eq!(
        sets.iter()
            .map(|(name, _)| name.as_str())
            .collect::<HashSet<_>>(),
        HashSet::from([STATUS_CORE_FIELD, STATUS_RESOLVED_NAMES_FIELD])
    );
    fields.extend(sets.into_iter().map(|(k, v)| (k, v.into())));
    let reassembled = reassemble_cached_status(fields.clone()).unwrap();
    assert_eq!(reassembled, next);
    assert_eq!(
        reassembled
            .resolved_names
            .links
            .hostnames("10.0.0.1".parse().unwrap()),
        vec!["b.test", "a.test"]
    );

    // An unchanged index is not written again.
    let (sets, _) = compute_status_field_writes(Some(&next), &[], &core, &parts).unwrap();
    assert!(sets.iter().all(|(name, _)| {
        name != STATUS_RESOLVED_NAMES_FIELD && name != STATUS_FOLLOWED_LOOKUPS_FIELD
    }));

    // A call that links nothing changes only the followed lookups, so the links stay unwritten.
    let mut following = next.clone();
    following
        .resolved_names
        .following
        .entity_scopes
        .insert(OplogIndex::from_u64(7), ());
    let mut following_core = following.clone();
    let following_parts = split_status(&mut following_core);
    let (sets, _) =
        compute_status_field_writes(Some(&next), &[], &following_core, &following_parts).unwrap();
    let written = sets
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<HashSet<_>>();
    assert!(written.contains(STATUS_FOLLOWED_LOOKUPS_FIELD));
    assert!(!written.contains(STATUS_RESOLVED_NAMES_FIELD));
    let mut following_fields = fields.clone();
    following_fields.extend(sets.into_iter().map(|(k, v)| (k, v.into())));
    assert_eq!(
        reassemble_cached_status(following_fields).unwrap(),
        following
    );

    // A cached status without either field is a cache miss, so the status is folded again.
    for field in [STATUS_RESOLVED_NAMES_FIELD, STATUS_FOLLOWED_LOOKUPS_FIELD] {
        let mut missing = fields.clone();
        missing.remove(field);
        assert!(reassemble_cached_status(missing).is_none(), "{field}");
    }
}
