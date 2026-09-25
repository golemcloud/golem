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

use crate::services::oplog::{Oplog, OplogOps, OplogService, OplogServiceOps};
use golem_common::model::agent::AgentMode;
use golem_common::model::durable_stream::{
    DURABLE_STREAM_FORMAT_VERSION, LocalStreamId, StreamForkCutRecord, StreamItemsPayload,
    StreamItemsRecord, StreamRecordReference, StreamRegisteredRecord, StreamRegistrationInvocation,
    StreamSessionRecord,
};
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::{AgentFingerprint, OwnedAgentId};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, Eq, PartialEq, desert_rust::BinaryCodec)]
pub struct StreamForkLineage {
    cuts: Vec<(OplogIndex, StreamForkCutRecord)>,
    deleted_regions: DeletedRegions,
    revert_regions: Vec<(OplogIndex, OplogRegion)>,
}

enum LineageSource<'a> {
    Open(&'a dyn Oplog),
    Service {
        service: &'a dyn OplogService,
        owner: &'a OwnedAgentId,
        mode: AgentMode,
    },
}

impl LineageSource<'_> {
    async fn read_exact(
        &self,
        index: OplogIndex,
        count: u64,
    ) -> std::collections::BTreeMap<OplogIndex, OplogEntry> {
        match self {
            Self::Open(oplog) => oplog.read_exact(index, count).await,
            Self::Service {
                service,
                owner,
                mode,
            } => service.read_exact(owner, *mode, index, count).await,
        }
    }

    async fn download_session(
        &self,
        payload: golem_common::model::oplog::OplogPayload<StreamSessionRecord>,
    ) -> Result<StreamSessionRecord, String> {
        match self {
            Self::Open(oplog) => oplog.download_payload(payload).await,
            Self::Service {
                service,
                owner,
                mode,
            } => service.download_payload(owner, *mode, payload).await,
        }
    }

    async fn download_registration(
        &self,
        payload: golem_common::model::oplog::OplogPayload<StreamRegisteredRecord>,
    ) -> Result<StreamRegisteredRecord, String> {
        match self {
            Self::Open(oplog) => oplog.download_payload(payload).await,
            Self::Service {
                service,
                owner,
                mode,
            } => service.download_payload(owner, *mode, payload).await,
        }
    }
}

impl StreamForkLineage {
    pub async fn suffix_contains_fork_cut(
        service: &dyn OplogService,
        owner: &OwnedAgentId,
        mode: AgentMode,
        from: OplogIndex,
        horizon: OplogIndex,
    ) -> Result<bool, String> {
        if from > horizon {
            return Ok(false);
        }
        let source = LineageSource::Service {
            service,
            owner,
            mode,
        };
        let mut covered = from.previous();
        while covered < horizon {
            let count = (horizon.as_u64() - covered.as_u64()).min(1024);
            let entries = source.read_exact(covered.next(), count).await;
            if entries.len() as u64 != count {
                return Err("missing oplog entries while discovering stream fork lineage".into());
            }
            for (index, entry) in entries {
                if index != covered.next() {
                    return Err("noncontiguous oplog while discovering stream fork lineage".into());
                }
                covered = index;
                if matches!(entry, OplogEntry::Revert { .. }) {
                    return Ok(true);
                }
                if let OplogEntry::StreamSession { record, .. } = entry
                    && matches!(
                        source.download_session(record).await?,
                        StreamSessionRecord::ForkCut(_)
                    )
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub async fn load(
        oplog: &dyn Oplog,
        owner: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<Self, String> {
        let horizon = oplog.current_oplog_index().await;
        Self::load_at_horizon(oplog, owner, fingerprint, horizon).await
    }

    pub(crate) async fn load_at_horizon(
        oplog: &dyn Oplog,
        _owner: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
        horizon: OplogIndex,
    ) -> Result<Self, String> {
        Self::load_from_source(LineageSource::Open(oplog), horizon, None).await
    }

    pub(crate) async fn validate_fork_cut(
        oplog: &dyn Oplog,
        cut: StreamForkCutRecord,
    ) -> Result<Self, String> {
        Self::load_from_source(LineageSource::Open(oplog), cut.cut_index, Some(cut)).await
    }

    pub async fn load_from_service(
        service: &dyn OplogService,
        owner: &OwnedAgentId,
        mode: AgentMode,
        _fingerprint: AgentFingerprint,
        horizon: OplogIndex,
    ) -> Result<Self, String> {
        Self::load_from_source(
            LineageSource::Service {
                service,
                owner,
                mode,
            },
            horizon,
            None,
        )
        .await
    }

    async fn load_from_source(
        source: LineageSource<'_>,
        horizon: OplogIndex,
        pending_cut: Option<StreamForkCutRecord>,
    ) -> Result<Self, String> {
        // The first bounded pass reads no external payloads. It builds the complete deletion mask
        // used by the second pass, so payloads in deleted regions are never downloaded.
        let mut reverts = Vec::new();
        let mut first_stream_entry = None;
        let mut covered = OplogIndex::NONE;
        while covered < horizon {
            let count = (horizon.as_u64() - covered.as_u64()).min(1024);
            let entries = source.read_exact(covered.next(), count).await;
            if entries.len() as u64 != count {
                return Err(
                    "missing oplog entries while discovering reverted stream history".into(),
                );
            }
            for (index, entry) in entries {
                if index != covered.next() {
                    return Err(
                        "noncontiguous oplog while discovering reverted stream history".into(),
                    );
                }
                covered = index;
                if matches!(
                    entry,
                    OplogEntry::StreamRegistered { .. }
                        | OplogEntry::StreamItems { .. }
                        | OplogEntry::StreamEnd { .. }
                        | OplogEntry::StreamCancel { .. }
                        | OplogEntry::StreamSession { .. }
                ) {
                    first_stream_entry.get_or_insert(index);
                }
                if let OplogEntry::Revert { dropped_region, .. } = entry {
                    reverts.push((index, dropped_region));
                }
            }
        }
        let deleted_regions =
            DeletedRegionsBuilder::from_regions(reverts.iter().map(|(_, region)| region.clone()))
                .build();

        let mut cuts = Vec::new();
        let mut registrations = Vec::new();
        let mut finished = Vec::new();
        let mut covered = OplogIndex::NONE;
        while covered < horizon {
            let count = (horizon.as_u64() - covered.as_u64()).min(1024);
            let entries = source.read_exact(covered.next(), count).await;
            if entries.len() as u64 != count {
                return Err("missing oplog entries while loading stream fork lineage".into());
            }
            for (index, entry) in entries {
                if index != covered.next() {
                    return Err("noncontiguous oplog while loading stream fork lineage".into());
                }
                covered = index;
                if deleted_regions.is_in_deleted_region(index) {
                    continue;
                }
                match entry {
                    OplogEntry::StreamRegistered { record, .. } => {
                        registrations.push((index, source.download_registration(record).await?));
                    }
                    OplogEntry::StreamSession { record, .. } => {
                        match source.download_session(record).await? {
                            StreamSessionRecord::ForkCut(cut) => {
                                validate_revert_adjacency(index, &cut, &reverts)?;
                                cuts.push((index, cut));
                            }
                            StreamSessionRecord::Finished(record) => {
                                finished.push((index, record.session_key));
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(cut) = pending_cut {
            let marker = marker_index(&cut)?;
            cuts.push((marker, cut));
        }
        for (index, region) in &reverts {
            if !deleted_regions.is_in_deleted_region(*index)
                && first_stream_entry.is_some_and(|first| first < *index)
                && !cuts.iter().any(|(marker, cut)| {
                    index.next() == *marker && cut.revert.as_ref() == Some(region)
                })
            {
                return Err("reverted stream history has no adjacent fork marker".into());
            }
        }
        let mut lineage = Self::validate_history(cuts, &registrations, &finished)?;
        lineage.deleted_regions = deleted_regions;
        lineage.revert_regions = reverts;
        Ok(lineage)
    }

    pub fn validate(
        cuts: Vec<(OplogIndex, StreamForkCutRecord)>,
        registrations: &[(OplogIndex, StreamRegisteredRecord)],
        _owner: &OwnedAgentId,
        _fingerprint: AgentFingerprint,
    ) -> Result<Self, String> {
        Self::validate_history(cuts, registrations, &[])
    }

    fn validate_history(
        cuts: Vec<(OplogIndex, StreamForkCutRecord)>,
        registrations: &[(OplogIndex, StreamRegisteredRecord)],
        finished: &[(OplogIndex, StreamRegistrationInvocation)],
    ) -> Result<Self, String> {
        let mut local = HashMap::new();
        for (index, record) in registrations {
            if record.format_version != DURABLE_STREAM_FORMAT_VERSION
                || local.insert(LocalStreamId(*index), record).is_some()
            {
                return Err("invalid historical stream registration in fork lineage".into());
            }
        }
        for (id, record) in &local {
            if let golem_common::model::durable_stream::StreamRegistrationRecordCoordinate::Nested {
                parent_stream: StreamRecordReference::Local(parent),
                ..
            } = &record.coordinate
                && (parent.0 >= id.0 || !local.contains_key(parent))
            {
                return Err("nested stream references an unknown local registration".into());
            }
        }

        let mut previous_marker = None;
        let mut previous_revert_floor = None;
        for (marker, cut) in &cuts {
            if !StreamSessionRecord::ForkCut(cut.clone()).has_supported_format()
                || marker_index(cut)? != *marker
                || previous_marker.is_some_and(|previous| previous >= *marker)
                || (cut.revert.is_none() && cut.epoch_floor != 1)
                || (cut.revert.is_some() && cut.epoch_floor <= 1)
            {
                return Err("invalid stream fork lineage chain".into());
            }
            if cut.revert.is_some() {
                if previous_revert_floor.is_some_and(|floor| cut.epoch_floor <= floor) {
                    return Err("stream revert does not advance the attachment epoch floor".into());
                }
                previous_revert_floor = Some(cut.epoch_floor);
            }
            if let Some(selected) = cut.selected_stream_id {
                let registration = local
                    .get(&selected)
                    .filter(|_| selected.0 <= cut.cut_index)
                    .ok_or("fork cut selects an unregistered local stream")?;
                if cut.retained_through.is_some_and(|offset| {
                    offset.producer_oplog_index() <= selected.0
                        || offset.producer_oplog_index() > cut.cut_index
                }) {
                    return Err(
                        "fork lineage retained item is outside the selected stream prefix".into(),
                    );
                }
                if finished.iter().any(|(index, session)| {
                    *index <= cut.cut_index && session == &registration.source_invocation
                }) {
                    return Err("selected stream cut retains its owning session completion".into());
                }
                if let golem_common::model::durable_stream::StreamRegistrationRecordCoordinate::Nested {
                    parent_stream: StreamRecordReference::Local(parent),
                    ..
                } = &registration.coordinate
                    && parent.0 > cut.cut_index
                {
                    return Err("selected nested stream has no retained parent registration".into());
                }
            } else if cut.retained_through.is_some() {
                return Err("retained stream offset requires a selected local stream".into());
            }
            previous_marker = Some(*marker);
        }
        Ok(Self {
            cuts,
            deleted_regions: DeletedRegions::new(),
            revert_regions: Vec::new(),
        })
    }

    pub(crate) fn project_item_batch(
        &self,
        index: OplogIndex,
        record: &mut StreamItemsRecord,
    ) -> Result<(), String> {
        for (_, cut) in &self.cuts {
            if index > cut.cut_index || cut.selected_stream_id != Some(record.stream_id) {
                continue;
            }
            let retained = cut
                .retained_through
                .ok_or("stream batch is outside the retained prefix")?;
            if index > retained.producer_oplog_index() {
                return Err("stream batch is outside the retained prefix".into());
            }
            if index == retained.producer_oplog_index() {
                let count = retained.sub_index() as usize + 1;
                if record.offsets.len() < count || record.payload.logical_item_count() < count {
                    return Err("retained stream offset exceeds its batch".into());
                }
                record.offsets.truncate(count);
                match &mut record.payload {
                    StreamItemsPayload::Values(values) => values.truncate(count),
                    StreamItemsPayload::PackedU8(bytes) => bytes.truncate(count),
                }
            }
        }
        Ok(())
    }

    pub(crate) fn through(&self, covered: OplogIndex) -> Result<Self, String> {
        if self
            .revert_regions
            .iter()
            .any(|(index, region)| *index > covered && region.start <= covered)
        {
            return Err("earlier lineage horizon intersects subsequently reverted history".into());
        }
        let revert_regions = self
            .revert_regions
            .iter()
            .take_while(|(index, _)| *index <= covered)
            .cloned()
            .collect::<Vec<_>>();
        let deleted_regions = DeletedRegionsBuilder::from_regions(
            revert_regions.iter().map(|(_, region)| region.clone()),
        )
        .build();
        Ok(Self {
            cuts: self
                .cuts
                .iter()
                .take_while(|(index, _)| *index <= covered)
                .cloned()
                .collect(),
            deleted_regions,
            revert_regions,
        })
    }

    pub fn deleted_regions(&self) -> &DeletedRegions {
        &self.deleted_regions
    }

    /// Wire attachment receipts describe past connections, not authority inherited by a fork.
    pub(crate) fn resets_control(&self, index: OplogIndex, record: &StreamSessionRecord) -> bool {
        self.cuts.last().is_some_and(|(marker, _)| index < *marker)
            && matches!(
                record,
                StreamSessionRecord::AttachmentPrepared(_)
                    | StreamSessionRecord::AttachmentActivated(_)
                    | StreamSessionRecord::AttachmentFinalized(_)
                    | StreamSessionRecord::TopologyPrepared(_)
                    | StreamSessionRecord::TopologyActivated(_)
                    | StreamSessionRecord::ProducerDeleting(_)
                    | StreamSessionRecord::ConsumerDeleting(_)
                    | StreamSessionRecord::CascadeOutbox(_)
                    | StreamSessionRecord::InputHighWater(_)
                    | StreamSessionRecord::ExternalProducerState(_)
            )
    }

    pub fn cuts(&self) -> &[(OplogIndex, StreamForkCutRecord)] {
        &self.cuts
    }
}

fn marker_index(cut: &StreamForkCutRecord) -> Result<OplogIndex, String> {
    cut.revert
        .as_ref()
        .map_or(cut.cut_index, |region| region.end)
        .as_u64()
        .checked_add(if cut.revert.is_some() { 2 } else { 1 })
        .map(OplogIndex::from_u64)
        .ok_or_else(|| "stream fork marker index overflow".into())
}

fn validate_revert_adjacency(
    marker: OplogIndex,
    cut: &StreamForkCutRecord,
    reverts: &[(OplogIndex, OplogRegion)],
) -> Result<(), String> {
    if let Some(region) = &cut.revert
        && !reverts
            .iter()
            .any(|(index, dropped)| index.next() == marker && dropped == region)
    {
        return Err("stream revert marker has no adjacent matching Revert entry".into());
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::ExecutionStatus;
    use crate::services::oplog::tests::{ReadCountingBlobStorage, ReadCountingIndexedStorage};
    use crate::services::oplog::{CommitLevel, PrimaryOplogService};
    use arc_swap::ArcSwap;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::component::ComponentRevision;
    use golem_common::model::durable_stream::{
        AttachmentId, AttemptId, PersistedInvocationTarget, PersistedStreamInvocationDescriptor,
        StartAttemptDescriptor, StreamForkCutRecord, StreamItemsPayload, StreamOffset,
        StreamRegistrationRecordCoordinate, StreamRootKind, StreamSessionExpiryPolicy,
        StreamSessionFinishedRecord, StreamSessionKey, StreamSessionPreparedRecord,
        StreamSourceKind,
    };
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::{
        AgentId, AgentMetadata, AgentStatusRecord, IdempotencyKey, RetryConfig, Timestamp,
        agent::OwnerKind,
    };
    use golem_common::read_only_lock;
    use golem_schema::schema::SchemaFingerprintV1;
    use std::collections::HashSet;
    use std::sync::{Arc, RwLock};
    use test_r::test;

    struct ServiceFixture {
        service: Arc<dyn OplogService>,
        oplog: Arc<dyn Oplog>,
        blobs: Arc<ReadCountingBlobStorage>,
    }

    impl ServiceFixture {
        async fn new(owner: &OwnedAgentId, fingerprint: AgentFingerprint) -> Self {
            let indexed = Arc::new(ReadCountingIndexedStorage::new());
            let blobs = Arc::new(ReadCountingBlobStorage::new());
            let service: Arc<dyn OplogService> = Arc::new(
                PrimaryOplogService::new(indexed, blobs.clone(), 1, 1, 1, RetryConfig::default())
                    .await,
            );
            let account = AccountId::new();
            let metadata = AgentMetadata {
                agent_id: owner.agent_id.clone(),
                owner_kind: OwnerKind::ComponentAgent,
                env: vec![],
                environment_id: owner.environment_id,
                created_by: account,
                created_by_email: AccountEmail::new("lineage@test"),
                config: vec![],
                created_at: Timestamp::now_utc(),
                parent: None,
                last_known_status: AgentStatusRecord::default(),
                original_phantom_id: None,
                fingerprint,
                agent_mode: AgentMode::Durable,
            };
            let create =
                OplogEntry::create(Box::new(golem_common::model::oplog::CreateParameters {
                    agent_id: owner.agent_id.clone(),
                    owner_kind: OwnerKind::ComponentAgent,
                    agent_mode: AgentMode::Durable,
                    component_revision: ComponentRevision::INITIAL,
                    env: vec![],
                    environment_id: owner.environment_id,
                    created_by: account,
                    parent: None,
                    component_size: 100,
                    initial_total_linear_memory_size: 100,
                    initial_active_plugins: HashSet::new(),
                    local_agent_config: vec![],
                    original_phantom_id: None,
                    instance_id: fingerprint.0,
                }));
            let oplog = service
                .create_fresh(
                    &mut service.lock_lifecycle(&owner.agent_id).await,
                    owner,
                    AgentMode::Durable,
                    create,
                    metadata,
                    read_only_lock::arc_swap::ReadOnlyView::new(Arc::new(ArcSwap::from_pointee(
                        AgentStatusRecord::default(),
                    ))),
                    read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(
                        ExecutionStatus::Suspended {
                            agent_mode: AgentMode::Durable,
                            timestamp: Timestamp::now_utc(),
                        },
                    ))),
                )
                .await;
            Self {
                service,
                oplog,
                blobs,
            }
        }
    }

    fn owner(name: &str) -> OwnedAgentId {
        OwnedAgentId::new(
            EnvironmentId(uuid::Uuid::from_u128(11)),
            &AgentId {
                component_id: golem_common::model::component::ComponentId(uuid::Uuid::from_u128(
                    23,
                )),
                agent_id: name.to_string(),
            },
        )
    }

    fn registration() -> StreamRegisteredRecord {
        let invocation =
            StreamRegistrationInvocation::Local(IdempotencyKey::new("session-1".into()));
        StreamRegisteredRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            coordinate: StreamRegistrationRecordCoordinate::Root {
                invocation: invocation.clone(),
                root_kind: StreamRootKind::MethodInput,
                recursive_value_path: vec![],
            },
            source_invocation: invocation,
            component_revision: ComponentRevision::new(3).unwrap(),
            element_schema_fingerprint: SchemaFingerprintV1([47; 32]),
            source_kind: StreamSourceKind::ExternalInlineInput,
            session_role: None,
        }
    }

    pub(crate) fn prepared(key: &StreamSessionKey) -> StreamSessionPreparedRecord {
        StreamSessionPreparedRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: key.idempotency_key.clone(),
            public_session_id: key.idempotency_key.value.clone(),
            expiry_policy: StreamSessionExpiryPolicy::None,
            expiry_deadline_millis: None,
            attempt: StartAttemptDescriptor {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: key.clone(),
                attachment_id: AttachmentId::primary(
                    key.callee_environment_id,
                    &key.callee,
                    &key.idempotency_key,
                )
                .unwrap(),
                expected_callee_fingerprint: key.callee_fingerprint,
                attempt_id: AttemptId::fresh(),
                invocation: PersistedStreamInvocationDescriptor {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: key.clone(),
                    target_component_revision: ComponentRevision::new(3).unwrap(),
                    target: PersistedInvocationTarget::AgentMethod {
                        method_name: "output".into(),
                    },
                    invocation_value: vec![],
                    stream_handles: vec![],
                    execution_config: vec![],
                    effective_identity: vec![],
                },
                effective_identity: vec![],
                live_join_buffer_events: 32,
            },
            stream_mappings: vec![],
        }
    }

    pub(crate) fn fork(
        selected_stream_id: Option<LocalStreamId>,
        cut_index: OplogIndex,
    ) -> StreamForkCutRecord {
        StreamForkCutRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            request_hash: vec![0; 32],
            creation_fingerprint: AgentFingerprint(uuid::Uuid::new_v4()),
            export: None,
            cut_index,
            revert: None,
            epoch_floor: 1,
            selected_stream_id,
            retained_through: None,
        }
    }

    fn self_revert(end: u64) -> StreamForkCutRecord {
        let mut cut = fork(None, OplogIndex::INITIAL);
        cut.revert = Some(OplogRegion::from_range(2..=end));
        cut.epoch_floor = 2;
        cut
    }

    fn registration_key(
        registration: &StreamRegisteredRecord,
        owner: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> StreamSessionKey {
        registration
            .source_invocation
            .qualify(owner.environment_id, &owner.agent_id, fingerprint)
    }

    #[test]
    async fn revert_marker_requires_adjacent_matching_revert() {
        let identity = owner("source");
        let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(37));
        for matching in [None, Some(false), Some(true)] {
            let fixture = ServiceFixture::new(&identity, fingerprint).await;
            for _ in 2..=3 {
                fixture
                    .oplog
                    .add(OplogEntry::NoOp {
                        timestamp: Timestamp::now_utc(),
                        entity_parent_start_index: None,
                    })
                    .await;
            }
            if let Some(matching) = matching {
                fixture
                    .oplog
                    .add(OplogEntry::revert(OplogRegion::from_range(
                        2..=if matching { 3 } else { 2 },
                    )))
                    .await;
            } else {
                fixture
                    .oplog
                    .add(OplogEntry::NoOp {
                        timestamp: Timestamp::now_utc(),
                        entity_parent_start_index: None,
                    })
                    .await;
            }
            let marker = StreamSessionRecord::ForkCut(self_revert(3));
            fixture
                .oplog
                .add(OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: fixture.oplog.upload_payload(&marker).await.unwrap(),
                })
                .await;
            let result = StreamForkLineage::load(&*fixture.oplog, &identity, fingerprint).await;
            assert_eq!(result.is_ok(), matching == Some(true));
        }
    }

    #[test]
    async fn bare_revert_and_guest_jump_remain_valid_lineage_history() {
        let identity = owner("source");
        let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(37));
        let fixture = ServiceFixture::new(&identity, fingerprint).await;
        fixture
            .oplog
            .add(OplogEntry::jump(None, OplogRegion::from_range(2..=2)))
            .await;
        fixture
            .oplog
            .add(OplogEntry::revert(OplogRegion::from_range(2..=2)))
            .await;
        let lineage = StreamForkLineage::load(&*fixture.oplog, &identity, fingerprint)
            .await
            .unwrap();
        assert!(
            lineage
                .deleted_regions()
                .is_in_deleted_region(OplogIndex::from_u64(2))
        );
        assert!(lineage.cuts().is_empty());
    }

    #[test]
    fn through_does_not_expose_future_deletions() {
        let lineage = StreamForkLineage {
            revert_regions: vec![(OplogIndex::from_u64(8), OplogRegion::from_range(2..=5))],
            deleted_regions: DeletedRegions::from_regions([OplogRegion::from_range(2..=5)]),
            ..Default::default()
        };
        assert!(
            !lineage
                .through(OplogIndex::INITIAL)
                .unwrap()
                .deleted_regions()
                .is_in_deleted_region(OplogIndex::from_u64(3))
        );
        assert!(lineage.through(OplogIndex::from_u64(7)).is_err());
        assert!(
            lineage
                .through(OplogIndex::from_u64(8))
                .unwrap()
                .deleted_regions()
                .is_in_deleted_region(OplogIndex::from_u64(3))
        );
    }

    #[test]
    fn through_merges_overlapping_revert_regions() {
        let lineage = StreamForkLineage {
            revert_regions: vec![
                (OplogIndex::from_u64(21), OplogRegion::from_range(3..=20)),
                (OplogIndex::from_u64(22), OplogRegion::from_range(4..=9)),
            ],
            ..Default::default()
        };
        assert_eq!(
            lineage
                .through(OplogIndex::from_u64(22))
                .unwrap()
                .deleted_regions()
                .regions()
                .cloned()
                .collect::<Vec<_>>(),
            vec![OplogRegion::from_range(3..=20)]
        );
    }

    #[test]
    async fn lineage_skips_deleted_stream_payloads_without_downloading_them() {
        let identity = owner("source");
        let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(37));
        let fixture = ServiceFixture::new(&identity, fingerprint).await;
        let registration = registration();
        let key = registration_key(&registration, &identity, fingerprint);
        for index in 2..=9 {
            let entry = match index {
                5 => OplogEntry::StreamRegistered {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: fixture.oplog.upload_payload(&registration).await.unwrap(),
                },
                6 => OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: fixture
                        .oplog
                        .upload_payload(&StreamSessionRecord::Prepared(prepared(&key)))
                        .await
                        .unwrap(),
                },
                7 => {
                    let mut discarded = registration.clone();
                    discarded.format_version = 0;
                    OplogEntry::StreamRegistered {
                        timestamp: Timestamp::now_utc(),
                        entity_parent_start_index: None,
                        record: fixture.oplog.upload_payload(&discarded).await.unwrap(),
                    }
                }
                8 => {
                    let mut discarded = prepared(&key);
                    discarded.format_version = 0;
                    OplogEntry::StreamSession {
                        timestamp: Timestamp::now_utc(),
                        entity_parent_start_index: None,
                        record: fixture
                            .oplog
                            .upload_payload(&StreamSessionRecord::Prepared(discarded))
                            .await
                            .unwrap(),
                    }
                }
                9 => {
                    let mut discarded = self_revert(7);
                    discarded.format_version = 0;
                    OplogEntry::StreamSession {
                        timestamp: Timestamp::now_utc(),
                        entity_parent_start_index: None,
                        record: fixture
                            .oplog
                            .upload_payload(&StreamSessionRecord::ForkCut(discarded))
                            .await
                            .unwrap(),
                    }
                }
                _ => OplogEntry::NoOp {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                },
            };
            fixture.oplog.add(entry).await;
        }
        let region = OplogRegion::from_range(7..=9);
        fixture.oplog.add(OplogEntry::revert(region.clone())).await;
        let mut marker = fork(None, OplogIndex::from_u64(6));
        marker.revert = Some(region);
        marker.epoch_floor = 2;
        fixture
            .oplog
            .add(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: fixture
                    .oplog
                    .upload_payload(&StreamSessionRecord::ForkCut(marker))
                    .await
                    .unwrap(),
            })
            .await;
        fixture.oplog.commit(CommitLevel::Always).await;
        fixture.blobs.reset();

        let lineage = StreamForkLineage::load_from_service(
            &*fixture.service,
            &identity,
            AgentMode::Durable,
            fingerprint,
            OplogIndex::from_u64(11),
        )
        .await
        .unwrap();
        assert_eq!(lineage.cuts().len(), 1);
        assert_eq!(fixture.blobs.reads(), 3);
        assert!(
            StreamForkLineage::load_at_horizon(
                &*fixture.oplog,
                &identity,
                fingerprint,
                OplogIndex::from_u64(10),
            )
            .await
            .is_err()
        );
    }

    #[test]
    async fn lineage_loader_finds_marker_after_chunk_boundary_without_a_cache() {
        use crate::durable_host::durable_stream::tests::TestOplog;
        let registration = registration();
        let target = owner("target");
        let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(67));
        let key = registration_key(&registration, &target, fingerprint);
        let cut = fork(
            Some(LocalStreamId(OplogIndex::from_u64(5))),
            OplogIndex::from_u64(1024),
        );
        let oplog = TestOplog::default();
        for index in 1..=1025 {
            let entry = match index {
                5 => OplogEntry::StreamRegistered {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: oplog.upload_payload(&registration).await.unwrap(),
                },
                6 => OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: oplog
                        .upload_payload(&StreamSessionRecord::Prepared(prepared(&key)))
                        .await
                        .unwrap(),
                },
                1025 => OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: oplog
                        .upload_payload(&StreamSessionRecord::ForkCut(cut.clone()))
                        .await
                        .unwrap(),
                },
                _ => OplogEntry::NoOp {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                },
            };
            oplog.add(entry).await;
        }
        for _ in 0..2 {
            assert_eq!(
                StreamForkLineage::load(&oplog, &target, fingerprint)
                    .await
                    .unwrap()
                    .cuts(),
                &[(OplogIndex::from_u64(1025), cut.clone())]
            );
            assert_eq!(
                oplog.take_read_ranges(),
                vec![
                    (OplogIndex::INITIAL, 1024),
                    (OplogIndex::from_u64(1025), 1),
                    (OplogIndex::INITIAL, 1024),
                    (OplogIndex::from_u64(1025), 1),
                ]
            );
        }
    }

    #[test]
    async fn lineage_completion_validation_observes_the_marker_boundary() {
        use crate::durable_host::durable_stream::tests::TestOplog;
        for (selected, finished_before) in [(true, true), (false, true), (true, false)] {
            let registration = registration();
            let target = owner("target");
            let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(67));
            let key = registration_key(&registration, &target, fingerprint);
            let mut cut = fork(
                Some(LocalStreamId(OplogIndex::from_u64(5))),
                OplogIndex::from_u64(7),
            );
            if !selected {
                cut.selected_stream_id = None;
            }
            let oplog = TestOplog::default();
            for index in 1..=9 {
                let session = match index {
                    6 => Some(StreamSessionRecord::Prepared(prepared(&key))),
                    7 if finished_before => {
                        Some(StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                            format_version: 1,
                            session_key: registration.source_invocation.clone(),
                            result: Ok(()),
                        }))
                    }
                    8 => Some(StreamSessionRecord::ForkCut(cut.clone())),
                    9 if !finished_before => {
                        Some(StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                            format_version: 1,
                            session_key: registration.source_invocation.clone(),
                            result: Ok(()),
                        }))
                    }
                    _ => None,
                };
                let entry = if index == 5 {
                    OplogEntry::StreamRegistered {
                        timestamp: Timestamp::now_utc(),
                        entity_parent_start_index: None,
                        record: oplog.upload_payload(&registration).await.unwrap(),
                    }
                } else if let Some(session) = session {
                    OplogEntry::StreamSession {
                        timestamp: Timestamp::now_utc(),
                        entity_parent_start_index: None,
                        record: oplog.upload_payload(&session).await.unwrap(),
                    }
                } else {
                    OplogEntry::NoOp {
                        timestamp: Timestamp::now_utc(),
                        entity_parent_start_index: None,
                    }
                };
                oplog.add(entry).await;
            }
            assert_eq!(
                StreamForkLineage::load(&oplog, &target, fingerprint)
                    .await
                    .is_ok(),
                !selected || !finished_before
            );
        }
    }

    #[test]
    async fn service_lineage_loader_matches_open_oplog_across_chunk_boundary() {
        let registration = registration();
        let target = owner("target");
        let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(67));
        let key = registration_key(&registration, &target, fingerprint);
        let cut = fork(
            Some(LocalStreamId(OplogIndex::from_u64(5))),
            OplogIndex::from_u64(1024),
        );
        let fixture = ServiceFixture::new(&target, fingerprint).await;
        for index in 2..=1025 {
            let entry = match index {
                5 => OplogEntry::StreamRegistered {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: fixture.oplog.upload_payload(&registration).await.unwrap(),
                },
                6 => OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: fixture
                        .oplog
                        .upload_payload(&StreamSessionRecord::Prepared(prepared(&key)))
                        .await
                        .unwrap(),
                },
                1025 => OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: fixture
                        .oplog
                        .upload_payload(&StreamSessionRecord::ForkCut(cut.clone()))
                        .await
                        .unwrap(),
                },
                _ => OplogEntry::NoOp {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                },
            };
            assert_eq!(fixture.oplog.add(entry).await, OplogIndex::from_u64(index));
        }
        fixture.oplog.commit(CommitLevel::Always).await;
        let from_oplog = StreamForkLineage::load(&*fixture.oplog, &target, fingerprint)
            .await
            .unwrap();
        fixture.blobs.reset();
        let from_service = StreamForkLineage::load_from_service(
            &*fixture.service,
            &target,
            AgentMode::Durable,
            fingerprint,
            OplogIndex::from_u64(1025),
        )
        .await
        .unwrap();
        assert_eq!(from_service, from_oplog);
        assert!(fixture.blobs.reads() >= 3);
        assert!(
            StreamForkLineage::load_from_service(
                &*fixture.service,
                &target,
                AgentMode::Durable,
                fingerprint,
                OplogIndex::from_u64(1024),
            )
            .await
            .unwrap()
            .cuts()
            .is_empty()
        );
    }

    #[test]
    async fn suffix_discovery_is_fixed_to_its_horizon_and_finds_a_later_marker() {
        let target = owner("target");
        let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(67));
        let fixture = ServiceFixture::new(&target, fingerprint).await;
        for index in 2..=1024 {
            assert_eq!(
                fixture
                    .oplog
                    .add(OplogEntry::NoOp {
                        timestamp: Timestamp::now_utc(),
                        entity_parent_start_index: None,
                    })
                    .await,
                OplogIndex::from_u64(index)
            );
        }
        fixture.oplog.commit(CommitLevel::Always).await;
        assert!(
            !StreamForkLineage::suffix_contains_fork_cut(
                &*fixture.service,
                &target,
                AgentMode::Durable,
                OplogIndex::from_u64(2),
                OplogIndex::from_u64(1024),
            )
            .await
            .unwrap()
        );
        let marker = StreamSessionRecord::ForkCut(fork(None, OplogIndex::from_u64(1024)));
        assert_eq!(
            fixture
                .oplog
                .add(OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: fixture.oplog.upload_payload(&marker).await.unwrap(),
                })
                .await,
            OplogIndex::from_u64(1025)
        );
        fixture.oplog.commit(CommitLevel::Always).await;
        assert!(
            StreamForkLineage::suffix_contains_fork_cut(
                &*fixture.service,
                &target,
                AgentMode::Durable,
                OplogIndex::from_u64(1025),
                OplogIndex::from_u64(1025),
            )
            .await
            .unwrap()
        );
        assert!(
            !StreamForkLineage::suffix_contains_fork_cut(
                &*fixture.service,
                &target,
                AgentMode::Durable,
                OplogIndex::from_u64(2),
                OplogIndex::from_u64(1024),
            )
            .await
            .unwrap()
        );
    }

    #[test]
    #[should_panic(expected = "missing oplog entries in range [2..=2]")]
    async fn service_lineage_loader_rejects_a_missing_requested_range() {
        let source = owner("source");
        let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(37));
        let fixture = ServiceFixture::new(&source, fingerprint).await;
        StreamForkLineage::load_from_service(
            &*fixture.service,
            &source,
            AgentMode::Durable,
            fingerprint,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();
    }

    #[test]
    fn project_item_batch_clips_only_the_selected_local_stream() {
        let local = LocalStreamId(OplogIndex::from_u64(5));
        let mut cut = fork(Some(local), OplogIndex::from_u64(9));
        cut.retained_through = Some(StreamOffset::new(OplogIndex::from_u64(7), 1));
        let lineage = StreamForkLineage {
            cuts: vec![(OplogIndex::from_u64(10), cut)],
            ..Default::default()
        };
        let mut batch = StreamItemsRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            stream_id: local,
            first_sequence: 3,
            nested_stream_ids: vec![StreamRecordReference::Local(local)],
            newly_registered_stream_ids: vec![],
            payload: StreamItemsPayload::PackedU8(vec![1, 2, 3]),
            offsets: (0..3)
                .map(|sub| StreamOffset::new(OplogIndex::from_u64(7), sub))
                .collect(),
        };
        lineage
            .project_item_batch(OplogIndex::from_u64(7), &mut batch)
            .unwrap();
        assert_eq!(batch.payload, StreamItemsPayload::PackedU8(vec![1, 2]));
        assert_eq!(
            batch.nested_stream_ids,
            vec![StreamRecordReference::Local(local)]
        );
    }

    #[test]
    fn through_preserves_marker_and_deletion_boundaries() {
        let deleted = OplogRegion::from_range(3..=5);
        let lineage = StreamForkLineage {
            cuts: vec![(
                OplogIndex::from_u64(10),
                fork(None, OplogIndex::from_u64(9)),
            )],
            revert_regions: vec![(OplogIndex::from_u64(6), deleted.clone())],
            deleted_regions: DeletedRegions::from_regions([deleted]),
        };
        assert!(lineage.through(OplogIndex::from_u64(4)).is_err());
        assert!(
            lineage
                .through(OplogIndex::from_u64(9))
                .unwrap()
                .cuts()
                .is_empty()
        );
        assert_eq!(
            lineage
                .through(OplogIndex::from_u64(10))
                .unwrap()
                .cuts()
                .len(),
            1
        );
    }
}
