// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::*;
use golem_common::model::durable_stream::{StreamForkCutRecordV1, StreamId};

impl SessionControlMetadata {
    /// Projects retained consumer observations, not live producer attachment authority.
    pub(crate) fn for_fork(
        &self,
        session: &StreamSessionKeyV1,
        cut: &StreamForkCutRecordV1,
        retained_streams: &HashSet<StreamId>,
    ) -> Result<Self, String> {
        let streams: HashMap<_, _> = cut
            .streams
            .iter()
            .map(|mapping| (mapping.source.stream_id, &mapping.continuation))
            .collect();
        let local = cut
            .sessions
            .iter()
            .find(|mapping| &mapping.source == session);
        let epoch = cut.epoch_floor;
        let session_key = |key: &StreamSessionKeyV1| {
            cut.sessions
                .iter()
                .find(|mapping| &mapping.source == key)
                .map_or_else(|| key.clone(), |mapping| mapping.continuation.clone())
        };
        let stream_id = |id: StreamId| match streams.get(&id) {
            Some(handle) => retained_streams
                .contains(&handle.stream_id)
                .then_some(handle.stream_id),
            None => Some(id),
        };
        let handle = |value: &DurableStreamHandleV1| {
            stream_id(value.stream_id).map(|_| {
                streams
                    .get(&value.stream_id)
                    .map_or_else(|| value.clone(), |value| (*value).clone())
            })
        };
        let mapping = |value: &StreamSessionMappingRecordV1| {
            handle(&value.handle).map(|handle| StreamSessionMappingRecordV1 {
                transport_stream_id: value.transport_stream_id,
                handle,
                role: value.role,
            })
        };
        let mut result = self.clone();
        result.covered_through = cut
            .revert
            .as_ref()
            .map_or(cut.cut_index.next(), |region| region.end.next().next());
        if let Some(local) = local {
            result.recovery_slot = None;
            result.caller_attempt = self.caller_attempt.map(|_| local.continuation_attempt_id);
            result.topology_epoch = self.topology_epoch.map(|_| epoch);
        } else {
            result.topology_epoch = None;
        }
        for set in [
            &mut result.explicit_mappings,
            &mut result.persisted_mappings,
            &mut result.visible_mappings,
        ] {
            *set = set
                .iter()
                .filter_map(|(id, old, role)| handle(old).map(|new| (*id, new, *role)))
                .collect();
        }
        result.acceptance_mappings = self
            .acceptance_mappings
            .iter()
            .filter_map(mapping)
            .collect();
        result.recoverable_mappings = self
            .recoverable_mappings
            .iter()
            .filter_map(|(index, old)| mapping(old).map(|new| (*index, new)))
            .collect();
        result.root_outputs.retain(|id| {
            result.acceptance_mappings.iter().any(|mapping| {
                mapping.transport_stream_id == *id && mapping.role == SessionStreamRoleV1::Output
            })
        });
        result.consumer_record_counts = self
            .consumer_record_counts
            .iter()
            .filter_map(|(id, count)| stream_id(*id).map(|id| (id, *count)))
            .collect();
        result.closed_consumer_streams = self
            .closed_consumer_streams
            .iter()
            .filter_map(|id| stream_id(*id))
            .collect();
        let intent = |old: &StreamConsumerCancelIntentRecordV1| {
            stream_id(old.stream_id).map(|stream_id| {
                let mut intent = old.clone();
                intent.session_key = session_key(&old.session_key);
                intent.stream_id = stream_id;
                intent.epoch = epoch;
                intent
            })
        };
        result.cancel_intents = self
            .cancel_intents
            .values()
            .filter_map(intent)
            .map(|intent| (intent.stream_id, intent))
            .collect();
        result.applied_cancel_intents = self
            .applied_cancel_intents
            .iter()
            .filter_map(intent)
            .collect();
        result.topologies.clear();
        result.finalized_attachments.clear();
        for old in self.topologies.values() {
            // Obsolete local epochs must not become current when epochs are normalized.
            if local.is_some()
                && self
                    .topology_epoch
                    .is_some_and(|epoch| epoch != old.attachment.epoch)
            {
                continue;
            }
            let Some(mapping) = mapping(&old.mapping) else {
                continue;
            };
            if stream_id(old.attachment.stream_id).is_none() {
                continue;
            }
            let finalized = self
                .finalized_attachments
                .get(&(old.attachment.attachment_id, old.attachment.stream_id))
                == Some(&old.attachment);
            let mut topology = old.clone();
            topology.mapping = mapping;
            let key = &mut topology.attachment;
            crate::services::worker_fork::lineage::project_attachment_key(cut, key)?;
            if finalized {
                result
                    .finalized_attachments
                    .insert((key.attachment_id, key.stream_id), key.clone());
            }
            result.topologies.insert(
                (
                    key.attachment_id,
                    key.stream_id,
                    topology.mapping.transport_stream_id,
                    topology.mapping.role,
                ),
                topology,
            );
        }
        result.consumer_deleting = None;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_host::durable_stream::tests::identity;
    use golem_common::model::durable_stream::{
        StreamForkSessionMappingV1, StreamForkStreamMappingV1, StreamOffsetV1,
        StreamSourceUnavailableRecordV1,
    };
    use golem_common::model::{AgentFingerprint, OwnedAgentId};
    use test_r::test;

    fn cut() -> StreamForkCutRecordV1 {
        let source = identity().invocation;
        let mut target = source.clone();
        target.callee.agent_id = "fork".into();
        target.callee_fingerprint = AgentFingerprint(uuid::Uuid::from_u128(99));
        StreamForkCutRecordV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            request_hash: vec![0; 32],
            export: None,
            source_environment_id: source.callee_environment_id,
            source: source.callee.clone(),
            source_fingerprint: source.callee_fingerprint,
            target_environment_id: target.callee_environment_id,
            target: target.callee.clone(),
            target_fingerprint: target.callee_fingerprint,
            cut_index: OplogIndex::from_u64(20),
            revert: None,
            epoch_floor: 1,
            selected_stream_id: None,
            retained_through: None,
            streams: vec![],
            sessions: vec![StreamForkSessionMappingV1 {
                source,
                continuation: target,
                continuation_attempt_id: AttemptId::fresh(),
            }],
        }
    }

    fn mapping(session: &StreamSessionKeyV1, id: u128) -> StreamSessionMappingRecordV1 {
        StreamSessionMappingRecordV1 {
            transport_stream_id: id as u64,
            role: SessionStreamRoleV1::Output,
            handle: DurableStreamHandleV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                stream_id: StreamId(uuid::Uuid::from_u128(id)),
                producer_environment_id: session.callee_environment_id,
                producer: session.callee.clone(),
                expected_producer_fingerprint: session.callee_fingerprint,
                source_invocation: session.clone(),
                component_revision: golem_common::model::component::ComponentRevision::INITIAL,
                element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
            },
        }
    }

    #[test]
    fn outbound_fork_preserves_cancellation_evidence_without_source_attachment_identity() {
        let cut = cut();
        let source = &cut.sessions[0].source;
        let mut remote = source.clone();
        remote.callee.agent_id = "remote".into();
        let mapping = mapping(&remote, 50);
        let key = StreamAttachmentKeyV1 {
            attachment_id: AttachmentId::primary(
                remote.callee_environment_id,
                &remote.callee,
                &remote.idempotency_key,
            )
            .unwrap(),
            stream_id: mapping.handle.stream_id,
            epoch: 3,
            session_key: remote.clone(),
            producer_environment_id: remote.callee_environment_id,
            producer: remote.callee.clone(),
            expected_producer_fingerprint: remote.callee_fingerprint,
            consumer_environment_id: source.callee_environment_id,
            consumer: source.callee.clone(),
            expected_consumer_fingerprint: source.callee_fingerprint,
            consumer_invocation: source.clone(),
        };
        let intent = StreamConsumerCancelIntentRecordV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: remote.clone(),
            stream_id: key.stream_id,
            epoch: 3,
            role: StreamCancelRoleV1::OutputConsumer,
            reason: StreamCancelReasonV1::Cancelled,
            details: Some("stop".into()),
        };
        let attempt = AttemptId::fresh();
        let mut old = SessionControlMetadata {
            caller_attempt: Some(attempt),
            ..Default::default()
        };
        old.apply(
            OplogIndex::from_u64(2),
            &remote,
            &StreamSessionRecordV1::TopologyPrepared(StreamTopologyPreparedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: remote.clone(),
                attachment: key.clone(),
                mapping: mapping.clone(),
            }),
        );
        old.apply(
            OplogIndex::from_u64(3),
            &remote,
            &StreamSessionRecordV1::TopologyActivated(StreamTopologyActivatedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: remote.clone(),
                attachment: key.clone(),
                mapping: mapping.clone(),
            }),
        );
        let mut stale = key.clone();
        stale.epoch = 1;
        old.finalized_attachments
            .insert((key.attachment_id, key.stream_id), stale);
        let projected = old.for_fork(&remote, &cut, &HashSet::new()).unwrap();
        let owner = OwnedAgentId::new(cut.target_environment_id, &cut.target);
        let topology = projected.recovery_topologies(&owner, &remote).unwrap();
        assert_eq!(
            topology.len(),
            1,
            "obsolete finalization must not close the new slot"
        );
        let fresh = &topology[0].0;
        assert_eq!(fresh.consumer, cut.target);
        assert_eq!(fresh.consumer_invocation, cut.sessions[0].continuation);
        assert_eq!(fresh.attachment_id, key.attachment_id);
        assert_eq!(fresh.epoch, 1);
        let lineage = crate::services::worker_fork::lineage::StreamForkLineage::validate(
            vec![(cut.cut_index.next(), cut.clone())],
            &[],
            &owner,
            cut.target_fingerprint,
        )
        .unwrap();
        let mut unavailable =
            StreamSessionRecordV1::SourceUnavailable(StreamSourceUnavailableRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                key: key.clone(),
                source_offset: StreamOffsetV1::new(OplogIndex::from_u64(8), 2),
                consumer_read_ordinal: 3,
            });
        lineage
            .project_session_payload(OplogIndex::from_u64(9), &mut unavailable)
            .unwrap();
        assert!(unavailable.has_supported_format());
        let StreamSessionRecordV1::SourceUnavailable(unavailable) = unavailable else {
            unreachable!()
        };
        assert_eq!(&unavailable.key, fresh);
        assert_eq!(projected.caller_attempt, Some(attempt));
        assert_eq!(topology[0].1, mapping);
        assert_eq!(
            projected.topology_status(fresh, Some(&mapping)).unwrap(),
            ConsumerAttachmentStatus::Active
        );
        let mut closed = old.clone();
        closed
            .finalized_attachments
            .insert((key.attachment_id, key.stream_id), key.clone());
        assert!(
            closed
                .for_fork(&remote, &cut, &HashSet::new())
                .unwrap()
                .recovery_topologies(&owner, &remote)
                .unwrap()
                .is_empty()
        );
        old.apply(
            OplogIndex::from_u64(4),
            &remote,
            &StreamSessionRecordV1::ConsumerCancelIntent(intent.clone()),
        );
        let pending = old.for_fork(&remote, &cut, &HashSet::new()).unwrap();
        let new_intent = &pending.cancel_intents[&key.stream_id];
        assert!(pending.has_cancellation_intents());
        assert!(
            pending
                .has_committed_cancellation(fresh, &mapping, new_intent)
                .unwrap()
        );
        let mut source_at_fork_epoch = key.clone();
        source_at_fork_epoch.epoch = fresh.epoch;
        assert!(
            !pending
                .has_committed_cancellation(&source_at_fork_epoch, &mapping, new_intent)
                .unwrap()
        );
        assert_eq!(new_intent.details, intent.details);
        old.apply(
            OplogIndex::from_u64(5),
            &remote,
            &StreamSessionRecordV1::ConsumerCancelApplied(StreamConsumerCancelAppliedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                intent,
            }),
        );
        let applied = old.for_fork(&remote, &cut, &HashSet::new()).unwrap();
        assert!(!applied.has_cancellation_intents());
        assert_eq!(applied.applied_cancel_intents.len(), 1);
        let new_intent = &applied.cancel_intents[&key.stream_id];
        assert!(
            applied
                .has_committed_cancellation(fresh, &mapping, new_intent)
                .unwrap()
        );
        assert!(
            !applied
                .has_committed_cancellation(&source_at_fork_epoch, &mapping, new_intent)
                .unwrap()
        );
    }

    #[test]
    fn local_fork_keeps_observations_and_discards_unretained_mapping_state() {
        let mut cut = cut();
        let source = cut.sessions[0].source.clone();
        let first = mapping(&source, 50);
        let discarded = mapping(&source, 51);
        let next = mapping(&cut.sessions[0].continuation, 150);
        let discarded_next = mapping(&cut.sessions[0].continuation, 151);
        cut.streams = vec![
            StreamForkStreamMappingV1 {
                source: first.handle.clone(),
                continuation: next.handle.clone(),
            },
            StreamForkStreamMappingV1 {
                source: discarded.handle.clone(),
                continuation: discarded_next.handle,
            },
        ];
        let mut old = SessionControlMetadata {
            prepared: Some(OplogIndex::from_u64(2)),
            initial_attached: Some(OplogIndex::from_u64(3)),
            finished: Some(OplogIndex::from_u64(19)),
            caller_attempt: Some(AttemptId::fresh()),
            caller_attempt_conflict: true,
            topology_epoch: Some(3),
            recovery_slot: Some(7),
            cancellation_requested: true,
            ..Default::default()
        };
        old.acceptance_mappings = vec![first.clone(), discarded.clone()];
        old.consumer_record_counts =
            HashMap::from([(first.handle.stream_id, 7), (discarded.handle.stream_id, 9)]);
        old.closed_consumer_streams.insert(first.handle.stream_id);
        old.tombstoned_slots
            .insert("$result".into(), SessionStreamRoleV1::Output);
        let new = old
            .for_fork(&source, &cut, &HashSet::from([next.handle.stream_id]))
            .unwrap();
        assert_eq!(new.prepared, old.prepared);
        assert_eq!(new.initial_attached, old.initial_attached);
        assert_eq!(new.finished, old.finished);
        assert!(new.recovery_slot.is_none());
        assert!(new.caller_attempt_conflict);
        assert_eq!(
            new.caller_attempt,
            Some(cut.sessions[0].continuation_attempt_id)
        );
        assert_eq!(new.topology_epoch, Some(1));
        assert_eq!(
            new.consumer_record_counts,
            HashMap::from([(next.handle.stream_id, 7)])
        );
        assert_eq!(
            new.closed_consumer_streams,
            HashSet::from([next.handle.stream_id])
        );
        assert_eq!(new.acceptance_mappings.len(), 1);
        assert_eq!(new.acceptance_mappings[0].handle, next.handle);
        assert_eq!(
            new.acceptance_mappings[0].transport_stream_id,
            first.transport_stream_id
        );
        assert_eq!(new.tombstoned_slots, old.tombstoned_slots);
        assert!(new.cancellation_requested);
        assert_eq!(old.consumer_record_counts.len(), 2);
    }
}
