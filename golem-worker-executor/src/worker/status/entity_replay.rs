use super::*;
use golem_common::model::durable_stream::StreamConsumerObservation;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::{EntityReplayBoundary, EntityReplayProgress, OpenEntityReplayScope};

fn parent(progress: &EntityReplayProgress, entity: OplogIndex) -> Option<OplogIndex> {
    progress
        .open
        .get(&entity)
        .map(|scope| scope.parent_entity)
        .or_else(|| {
            progress
                .completed
                .get(&entity)
                .map(|scope| scope.parent_entity)
        })
        .flatten()
}

fn advance(progress: &mut EntityReplayProgress, mut entity: Option<OplogIndex>, index: OplogIndex) {
    while let Some(current) = entity {
        entity = parent(progress, current);
        if let Some(scope) = progress.open.get_mut(&current) {
            scope.last_required = Some(index);
        }
    }
}

fn enclosing(progress: &EntityReplayProgress, start: OplogIndex) -> Option<OplogIndex> {
    progress.routes.get(&start).copied().or_else(|| {
        (progress.open.contains_key(&start) || progress.completed.contains_key(&start))
            .then_some(start)
    })
}

fn has_open_ancestor(progress: &EntityReplayProgress, mut entity: Option<OplogIndex>) -> bool {
    while let Some(current) = entity {
        if progress.open.contains_key(&current) {
            return true;
        }
        entity = parent(progress, current);
    }
    false
}

fn insert_route(progress: &mut EntityReplayProgress, start: OplogIndex, entity: OplogIndex) {
    progress.routes.insert(start, entity);
    progress.route_members.insert((entity, start), ());
}

/// Visits only the retiring subtree. An open child retains its routes and takes ownership of
/// retiring its subtree when its own terminal arrives, even though its parent is already closed.
fn retire_routes(progress: &mut EntityReplayProgress, root: OplogIndex) {
    let mut pending = vec![root];
    while let Some(entity) = pending.pop() {
        if progress.open.contains_key(&entity) {
            continue;
        }
        let range = (entity, OplogIndex::NONE)..=(entity, OplogIndex::from_u64(u64::MAX));
        let members: Vec<_> = progress
            .route_members
            .range(range.clone())
            .map(|(key, ())| *key)
            .collect();
        for key in members {
            progress.route_members.remove(&key);
            progress.routes.remove(&key.1);
        }
        let children: Vec<_> = progress
            .entity_children
            .range(range)
            .map(|(key, ())| *key)
            .collect();
        for key in children {
            progress.entity_children.remove(&key);
            pending.push(key.1);
        }
    }
}

pub(crate) fn calculate_entity_replay_progress(
    mut progress: EntityReplayProgress,
    skipped: &DeletedRegions,
    deleted: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Result<EntityReplayProgress, String> {
    for (&index, entry) in entries {
        if skipped.is_in_deleted_region(index) || deleted.is_in_deleted_region(index) {
            continue;
        }
        if let OplogEntry::Start {
            function_name,
            parent_start_index,
            ..
        } = entry
        {
            let ancestor = parent_start_index.and_then(|start| enclosing(&progress, start));
            advance(&mut progress, ancestor, index);
            if matches!(
                function_name,
                HostFunctionName::GolemEntityInvoke | HostFunctionName::GolemToolInvocationRejected
            ) {
                if let Some(parent) = ancestor
                    && has_open_ancestor(&progress, ancestor)
                {
                    progress.entity_children.insert((parent, index), ());
                }
                progress.open.insert(
                    index,
                    OpenEntityReplayScope {
                        last_required: None,
                        parent_entity: ancestor,
                    },
                );
                insert_route(&mut progress, index, index);
            } else if let Some(entity) = ancestor
                && has_open_ancestor(&progress, ancestor)
            {
                insert_route(&mut progress, index, entity);
            }
            continue;
        }

        let start = match entry {
            OplogEntry::End { start_index, .. }
            | OplogEntry::Cancelled { start_index, .. }
            | OplogEntry::CompletionDelivered { start_index, .. }
            | OplogEntry::CompletionDiscarded { start_index, .. } => Some(*start_index),
            OplogEntry::HostStreamFrame {
                parent_start_index, ..
            }
            | OplogEntry::Log {
                parent_start_index: Some(parent_start_index),
                ..
            } => Some(*parent_start_index),
            _ => entry.entity_parent_start_index(),
        };
        let mut entity = start.and_then(|start| enclosing(&progress, start));
        let terminal = matches!(entry, OplogEntry::End { .. } | OplogEntry::Cancelled { .. });
        let mut frozen = false;
        if terminal
            && let Some(start) = start
            && let Some(scope) = progress.open.remove(&start)
        {
            frozen = true;
            entity = scope.parent_entity;
            progress.completed.insert(
                start,
                EntityReplayBoundary {
                    terminal_index: index,
                    last_required: scope.last_required,
                    parent_entity: scope.parent_entity,
                },
            );
        }
        if !entry.is_hint() || matches!(entry, OplogEntry::CompletionDelivered { .. }) {
            advance(&mut progress, entity, index);
        }

        // A native read's consuming scope can differ from the session's attribution.
        let observation = match entry {
            OplogEntry::ToolAttachmentRead { observation, .. } => {
                Some(observation.observation.clone())
            }
            OplogEntry::StreamSession { record, .. } => {
                let decoded;
                let record = match record {
                    OplogPayload::Inline(record) => record.as_ref(),
                    OplogPayload::SerializedInline {
                        cached: Some(record),
                        ..
                    }
                    | OplogPayload::External {
                        cached: Some(record),
                        ..
                    } => record.as_ref(),
                    OplogPayload::SerializedInline {
                        bytes,
                        cached: None,
                    } => {
                        decoded = try_deserialize::<StreamSessionRecord>(bytes)
                            .map_err(|error| {
                                format!("failed to decode consumer observation: {error}")
                            })?
                            .ok_or_else(|| {
                                "unsupported consumer observation serialization".to_string()
                            })?;
                        &decoded
                    }
                    OplogPayload::External { cached: None, .. } => {
                        return Err(
                            "durable stream session record payload has not been loaded".into()
                        );
                    }
                };
                match record {
                    StreamSessionRecord::ConsumerItemValue(record) => {
                        Some(record.observation.clone())
                    }
                    StreamSessionRecord::ConsumerTerminal(record) => {
                        Some(record.observation.clone())
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(StreamConsumerObservation::Guest { parent_start_index }) = observation {
            let consumer = parent_start_index.and_then(|start| enclosing(&progress, start));
            advance(&mut progress, consumer, index);
            advance(&mut progress, entity, index);
        } else if matches!(entry, OplogEntry::ToolAttachmentRead { .. }) {
            advance(&mut progress, entity, index);
        }

        if frozen && !has_open_ancestor(&progress, entity) {
            // An End does not retire a protocol parent: later children and deliveries may still
            // name it. Retire routes only after every enclosing entity boundary is frozen.
            retire_routes(&mut progress, start.expect("entity terminal has a Start"));
        }
    }
    Ok(progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::Timestamp;
    use golem_common::model::durable_stream::{
        LocalStreamReaderId, STREAM_CONSUMER_RECORD_FORMAT_VERSION, StreamConsumerItemValueRecord,
        StreamOffset, StreamRegistrationInvocation,
    };
    use golem_common::model::oplog::DurableFunctionType;
    use golem_common::model::regions::OplogRegion;
    use golem_common::serialization::{deserialize, serialize};
    use test_r::test;

    fn idx(index: u64) -> OplogIndex {
        OplogIndex::from_u64(index)
    }

    fn start(parent: Option<u64>, entity: bool) -> OplogEntry {
        OplogEntry::Start {
            timestamp: Timestamp::from(1000u64),
            parent_start_index: parent.map(idx),
            function_name: if entity {
                HostFunctionName::GolemEntityInvoke
            } else {
                HostFunctionName::Custom("protocol".into())
            },
            invocation_id: None,
            observational_owner: None,
            request: None,
            durable_function_type: DurableFunctionType::WriteRemote,
            span_started: None,
            denial: None,
        }
    }

    fn end(start: u64) -> OplogEntry {
        OplogEntry::End {
            timestamp: Timestamp::from(1000u64),
            start_index: idx(start),
            response: None,
            forced_commit: false,
            span_finished: None,
            span_attributes: None,
        }
    }

    fn delivered(start: u64) -> OplogEntry {
        OplogEntry::CompletionDelivered {
            timestamp: Timestamp::from(1000u64),
            start_index: idx(start),
        }
    }

    fn entries(entries: Vec<OplogEntry>) -> BTreeMap<OplogIndex, OplogEntry> {
        entries
            .into_iter()
            .enumerate()
            .map(|(index, entry)| (idx(index as u64 + 1), entry))
            .collect()
    }

    fn fold(entries: &BTreeMap<OplogIndex, OplogEntry>) -> EntityReplayProgress {
        calculate_entity_replay_progress(
            Default::default(),
            &DeletedRegions::new(),
            &DeletedRegions::new(),
            entries,
        )
        .unwrap()
    }

    #[test]
    fn entity_replay_checkpoint_every_entry_preserves_nested_and_completed_parent_routes() {
        let entries = entries(vec![
            start(None, true),     // 1 outer
            start(Some(1), false), // 2 protocol
            end(2),                // 3 protocol may have later children
            start(Some(2), true),  // 4 nested entity
            start(Some(4), false), // 5 child call
            end(5),                // 6
            delivered(5),          // 7 nested boundary
            end(4),                // 8 freezes nested before advancing outer
            start(Some(5), false), // 9 later descendant through completed parents
            end(9),                // 10
            start(None, true),     // 11 unrelated
            end(11),               // 12
            OplogEntry::CompletionDiscarded {
                timestamp: Timestamp::from(1000u64),
                start_index: idx(9),
            }, // 13 hint
            end(1),                // 14 outer cutoff
            delivered(9),          // 15 cannot change frozen boundary
        ]);
        let expected = fold(&entries);
        assert_eq!(expected.completed[&idx(4)].last_required, Some(idx(7)));
        assert_eq!(expected.completed[&idx(4)].parent_entity, Some(idx(1)));
        assert_eq!(expected.completed[&idx(1)].last_required, Some(idx(10)));
        assert_eq!(expected.completed[&idx(11)].last_required, None);
        assert!(expected.routes.is_empty());
        assert!(expected.open.is_empty());
        assert!(expected.route_members.is_empty());
        assert!(expected.entity_children.is_empty());
        let mut partial = EntityReplayProgress::default();
        for (&index, entry) in &entries {
            partial = calculate_entity_replay_progress(
                partial,
                &DeletedRegions::new(),
                &DeletedRegions::new(),
                &BTreeMap::from([(index, entry.clone())]),
            )
            .unwrap();
            partial = deserialize(&serialize(&partial).unwrap()).unwrap();
            assert_eq!(
                partial.route_members,
                partial
                    .routes
                    .iter()
                    .map(|(&start, &entity)| ((entity, start), ()))
                    .collect::<im::OrdMap<_, _>>()
            );
            assert_eq!(
                partial,
                fold(
                    &entries
                        .range(..=index)
                        .map(|(&i, e)| (i, e.clone()))
                        .collect()
                )
            );
        }
        assert_eq!(partial, expected);
        let mut status = AgentStatusRecord::default();
        for (&index, entry) in &entries {
            status = super::super::update_status_with_new_entries(
                AgentMode::Durable,
                status,
                BTreeMap::from([(index, entry.clone())]),
                &RetryConfig::default(),
            )
            .unwrap()
            .unwrap();
            status = deserialize(&serialize(&status).unwrap()).unwrap();
        }
        assert_eq!(status.entity_replay_progress, expected);
    }

    #[test]
    fn entity_replay_guest_observation_uses_consuming_scope_and_decodes_receipts() {
        let record = StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
            format_version: STREAM_CONSUMER_RECORD_FORMAT_VERSION,
            observation: StreamConsumerObservation::Guest {
                parent_start_index: Some(idx(2)),
            },
            reobserves: None,
            session_key: StreamRegistrationInvocation::Local(IdempotencyKey::new("input".into())),
            reader_id: LocalStreamReaderId {
                introducing_oplog_index: idx(1),
                binding_slot: 0,
            },
            source_offset: StreamOffset::new(idx(1), 0),
            consumer_read_ordinal: 0,
            value: vec![1],
            packed_u8: true,
            recursive_mappings: vec![],
        });
        for payload in [
            OplogPayload::Inline(Box::new(record.clone())),
            OplogPayload::SerializedInline {
                bytes: serialize(&record).unwrap(),
                cached: None,
            },
            OplogPayload::External {
                payload_id: golem_common::model::oplog::PayloadId::new(),
                md5_hash: vec![],
                cached: Some(Arc::new(record.clone())),
            },
        ] {
            let entries = entries(vec![
                start(None, true),
                start(Some(1), false),
                end(2),
                OplogEntry::StreamSession {
                    timestamp: Timestamp::from(1000u64),
                    entity_parent_start_index: None,
                    record: payload,
                    summary: None,
                },
                end(1),
            ]);
            assert_eq!(
                fold(&entries).completed[&idx(1)].last_required,
                Some(idx(4))
            );
            let mut host_entries = entries;
            if let OplogEntry::StreamSession {
                record: payload, ..
            } = host_entries.get_mut(&idx(4)).unwrap()
            {
                let mut host = record.clone();
                if let StreamSessionRecord::ConsumerItemValue(record) = &mut host {
                    record.observation = StreamConsumerObservation::Host;
                }
                *payload = OplogPayload::Inline(Box::new(host));
            }
            assert_eq!(
                fold(&host_entries).completed[&idx(1)].last_required,
                Some(idx(3))
            );
        }
    }

    #[test]
    fn entity_replay_hidden_prefix_requires_refold_and_excludes_hidden_work() {
        let entries = entries(vec![
            start(None, true),
            start(Some(1), false),
            end(2),
            delivered(2),
            end(1),
        ]);
        let hidden = DeletedRegions::from_regions([OplogRegion::from_index_range(idx(2)..=idx(4))]);
        for (skipped, deleted) in [
            (hidden.clone(), DeletedRegions::new()),
            (DeletedRegions::new(), hidden),
        ] {
            let result =
                calculate_entity_replay_progress(Default::default(), &skipped, &deleted, &entries)
                    .unwrap();
            assert_eq!(result.completed[&idx(1)].last_required, None);
            let baseline = AgentStatusRecord {
                oplog_idx: idx(4),
                entity_replay_progress: fold(
                    &entries
                        .range(..=idx(4))
                        .map(|(&i, e)| (i, e.clone()))
                        .collect(),
                ),
                ..Default::default()
            };
            assert!(super::super::baseline_is_invalidated(
                &baseline, &deleted, &skipped
            ));
        }
        let prefix = fold(
            &entries
                .range(..idx(5))
                .map(|(&i, e)| (i, e.clone()))
                .collect(),
        );
        assert!(prefix.completed.is_empty());
        assert_eq!(prefix.open[&idx(1)].last_required, Some(idx(4)));
    }

    #[test]
    fn entity_replay_parent_terminal_does_not_retire_open_child_routes() {
        let mut observational = start(None, false);
        if let OplogEntry::Start {
            observational_owner,
            ..
        } = &mut observational
        {
            *observational_owner = Some(idx(2));
        }
        let entries = entries(vec![
            start(None, true),
            start(Some(1), true),
            start(Some(2), false),
            end(1),
            end(3),
            delivered(3),
            observational,
            OplogEntry::Cancelled {
                timestamp: Timestamp::from(1000u64),
                start_index: idx(2),
                partial: None,
                span_finished: None,
            },
        ]);
        let progress = fold(&entries);
        assert_eq!(progress.completed[&idx(1)].last_required, Some(idx(3)));
        assert_eq!(progress.completed[&idx(2)].last_required, Some(idx(6)));
        assert_eq!(progress.completed[&idx(2)].terminal_index, idx(8));
        assert!(progress.routes.is_empty());
        assert!(progress.open.is_empty());
        assert!(progress.route_members.is_empty());
        assert!(progress.entity_children.is_empty());
    }

    #[test]
    fn entity_replay_unrelated_terminals_retire_only_their_indexed_members() {
        let mut history = vec![start(None, true)];
        for _ in 0..256 {
            history.push(start(Some(1), false));
        }
        let mut progress = fold(&entries(history));
        let retained_routes = progress.routes.clone();
        let retained_members = progress.route_members.clone();
        for i in 300..556 {
            let root = i * 2;
            progress = calculate_entity_replay_progress(
                progress,
                &DeletedRegions::new(),
                &DeletedRegions::new(),
                &BTreeMap::from([(idx(root), start(None, true)), (idx(root + 1), end(root))]),
            )
            .unwrap();
            assert_eq!(progress.routes, retained_routes);
            assert_eq!(progress.route_members, retained_members);
            assert!(progress.entity_children.is_empty());
        }
        // A child opened after an earlier protocol parent's terminal still belongs to the root.
        progress = calculate_entity_replay_progress(
            progress,
            &DeletedRegions::new(),
            &DeletedRegions::new(),
            &BTreeMap::from([
                (idx(1200), end(2)),
                (idx(1201), start(Some(2), true)),
                (idx(1202), start(Some(1201), false)),
                (idx(1203), end(1)),
            ]),
        )
        .unwrap();
        assert_eq!(progress.routes.len(), 2);
        assert_eq!(progress.routes[&idx(1202)], idx(1201));
        assert!(progress.entity_children.is_empty());
        progress = deserialize(&serialize(&progress).unwrap()).unwrap();
        progress = calculate_entity_replay_progress(
            progress,
            &DeletedRegions::new(),
            &DeletedRegions::new(),
            &BTreeMap::from([
                (idx(1204), end(1202)),
                (idx(1205), delivered(1202)),
                (idx(1206), end(1201)),
            ]),
        )
        .unwrap();
        assert_eq!(
            progress.completed[&idx(1201)].last_required,
            Some(idx(1205))
        );
        assert!(progress.routes.is_empty());
        assert!(progress.route_members.is_empty());
    }
}
