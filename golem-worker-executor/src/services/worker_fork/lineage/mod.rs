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

use crate::services::oplog::{Oplog, OplogOps, OplogService, OplogServiceOps, UnreadableOplog};
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
    ) -> Result<std::collections::BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
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
            } => service
                .download_payload(owner, *mode, payload)
                .await
                .map_err(String::from),
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
            } => service
                .download_payload(owner, *mode, payload)
                .await
                .map_err(String::from),
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
            let entries = source
                .read_exact(covered.next(), count)
                .await
                .map_err(|error| error.to_string())?;
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
            let entries = source
                .read_exact(covered.next(), count)
                .await
                .map_err(|error| error.to_string())?;
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
            let entries = source
                .read_exact(covered.next(), count)
                .await
                .map_err(|error| error.to_string())?;
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
pub(crate) mod tests;
