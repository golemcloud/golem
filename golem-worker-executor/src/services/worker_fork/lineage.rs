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
    AttachmentId, DURABLE_STREAM_FORMAT_VERSION, DurableStreamHandle, StreamForkCutRecord,
    StreamId, StreamRegisteredRecord, StreamSessionKey, StreamSessionMappingRecord,
    StreamSessionPreparedRecord, StreamSessionRecord,
};
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::regions::{DeletedRegions, DeletedRegionsBuilder, OplogRegion};
use golem_common::model::{AgentFingerprint, OwnedAgentId};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// A view of durable fork markers, not a substitute for the records they describe. Loading this
/// before stream folds lets them validate each retained registration against its original author.
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
    /// Checks only newly uncovered history for a fork marker. Callers can reuse an already
    /// validated lineage when this returns false; a marker requires rebuilding lineage from the
    /// full fixed horizon because its mappings validate retained registrations and sessions.
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
        owner: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        horizon: OplogIndex,
    ) -> Result<Self, String> {
        Self::load_from_source(
            LineageSource::Open(oplog),
            owner,
            fingerprint,
            horizon,
            None,
        )
        .await
    }

    pub(crate) async fn validate_fork_cut(
        oplog: &dyn Oplog,
        cut: StreamForkCutRecord,
    ) -> Result<Self, String> {
        let target = OwnedAgentId::new(cut.target_environment_id, &cut.target);
        Self::load_from_source(
            LineageSource::Open(oplog),
            &target,
            cut.target_fingerprint,
            cut.cut_index,
            Some(cut),
        )
        .await
    }

    /// Loads lineage directly from oplog storage without opening a writable oplog actor.
    /// `horizon` is fixed by the caller so every validation pass observes the same history.
    pub async fn load_from_service(
        service: &dyn OplogService,
        owner: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        horizon: OplogIndex,
    ) -> Result<Self, String> {
        Self::load_from_source(
            LineageSource::Service {
                service,
                owner,
                mode,
            },
            owner,
            fingerprint,
            horizon,
            None,
        )
        .await
    }

    async fn load_from_source(
        source: LineageSource<'_>,
        owner: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        horizon: OplogIndex,
        pending_cut: Option<StreamForkCutRecord>,
    ) -> Result<Self, String> {
        // Discover deletion metadata without hydrating any external stream payload. A second
        // bounded pass then downloads only records that remain visible at this horizon.
        let mut discovered_reverts = Vec::new();
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
                    discovered_reverts.push((index, dropped_region));
                }
            }
        }
        let mut deleted_builder = DeletedRegionsBuilder::new();
        for (_, region) in &discovered_reverts {
            deleted_builder.add(region.clone());
        }
        let deleted_regions = deleted_builder.build();

        let mut covered = OplogIndex::NONE;
        let mut cuts = Vec::new();
        let mut registrations = Vec::new();
        let mut prepared = Vec::new();
        let mut finished = HashSet::new();
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
                    OplogEntry::StreamSession { record, .. } => {
                        match source.download_session(record).await? {
                            StreamSessionRecord::ForkCut(record) => {
                                if let Some(region) = &record.revert
                                    && discovered_reverts
                                        .iter()
                                        .find(|(revert_index, _)| *revert_index == index.previous())
                                        .is_none_or(|(_, dropped)| dropped != region)
                                {
                                    return Err(
                                        "stream revert marker has no adjacent matching Revert entry"
                                            .into(),
                                    );
                                }
                                if record.streams.iter().any(|mapping| {
                                    Some(mapping.source.stream_id) == record.selected_stream_id
                                        && finished.contains(&mapping.source.source_invocation)
                                }) {
                                    return Err(
                                        "selected stream cut retains its owning session completion"
                                            .into(),
                                    );
                                }
                                for mapping in &record.sessions {
                                    if finished.remove(&mapping.source) {
                                        finished.insert(mapping.continuation.clone());
                                    }
                                }
                                cuts.push((index, record));
                            }
                            StreamSessionRecord::Prepared(record) => prepared.push((index, record)),
                            StreamSessionRecord::Finished(record) => {
                                finished.insert(record.session_key);
                            }
                            _ => {}
                        }
                    }
                    OplogEntry::StreamRegistered { record, .. } => {
                        registrations.push((index, source.download_registration(record).await?));
                    }
                    _ => {}
                }
            }
        }
        if let Some(cut) = pending_cut {
            if cut.streams.iter().any(|mapping| {
                Some(mapping.source.stream_id) == cut.selected_stream_id
                    && finished.contains(&mapping.source.source_invocation)
            }) {
                return Err("selected stream cut retains its owning session completion".into());
            }
            let marker = cut
                .revert
                .as_ref()
                .map_or_else(
                    || horizon.as_u64().checked_add(1),
                    |region| region.end.as_u64().checked_add(2),
                )
                .ok_or("stream fork marker index overflow")?;
            cuts.push((OplogIndex::from_u64(marker), cut));
        }
        for (index, region) in &discovered_reverts {
            if !deleted_regions.is_in_deleted_region(*index)
                && first_stream_entry.is_some_and(|first| first < *index)
                && !cuts.iter().any(|(marker, cut)| {
                    index.as_u64().checked_add(1) == Some(marker.as_u64())
                        && cut.revert.as_ref() == Some(region)
                })
            {
                return Err("reverted stream history has no adjacent fork marker".into());
            }
        }
        let mut lineage = Self::validate(cuts, &registrations, owner, fingerprint)?;
        lineage.deleted_regions = deleted_regions;
        lineage.revert_regions = discovered_reverts;
        lineage.validate_session_history(&prepared, owner, fingerprint)?;
        Ok(lineage)
    }

    fn validate_session_history(
        &self,
        prepared: &[(OplogIndex, StreamSessionPreparedRecord)],
        owner: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(), String> {
        let mut sessions = HashMap::new();
        for (index, record) in prepared {
            let key = &record.attempt.session_key;
            let (author, fingerprint) = self.author_at(*index, owner, fingerprint);
            if !StreamSessionRecord::Prepared(record.clone()).has_supported_format()
                || key.callee_environment_id != author.environment_id
                || key.callee != author.agent_id
                || key.callee_fingerprint != fingerprint
                || sessions.insert(key.clone(), *index).is_some()
            {
                return Err("invalid historical session preparation in fork lineage".into());
            }
        }
        for (_, cut) in &self.cuts {
            for mapping in &cut.sessions {
                if sessions
                    .get(&mapping.source)
                    .is_none_or(|index| *index > cut.cut_index)
                {
                    return Err("fork session mapping has no retained preparation".into());
                }
            }
            if sessions.iter().any(|(key, index)| {
                *index <= cut.cut_index
                    && key.callee_environment_id == cut.source_environment_id
                    && key.callee == cut.source
                    && key.callee_fingerprint == cut.source_fingerprint
                    && !cut.sessions.iter().any(|mapping| &mapping.source == key)
            }) {
                return Err("fork lineage omits a retained local session".into());
            }
            let mapped = cut
                .sessions
                .iter()
                .map(|mapping| (mapping.continuation.clone(), sessions[&mapping.source]))
                .collect::<Vec<_>>();
            for mapping in &cut.sessions {
                sessions.remove(&mapping.source);
            }
            for (key, index) in mapped {
                if sessions.insert(key, index).is_some() {
                    return Err("fork session continuation conflicts with session history".into());
                }
            }
        }
        Ok(())
    }

    /// Validates registration provenance and mapping identities. Session existence and retained
    /// item extent require their respective journals; registrations alone cannot establish them.
    pub fn validate(
        cuts: Vec<(OplogIndex, StreamForkCutRecord)>,
        registrations: &[(OplogIndex, StreamRegisteredRecord)],
        owner: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<Self, String> {
        let mut expected_owner = owner.clone();
        let mut expected_fingerprint = fingerprint;
        let mut next_marker = None;
        for pair in cuts.windows(2) {
            if pair[1].1.cut_index < pair[0].0 {
                return Err("stream fork lineage includes a discarded marker".into());
            }
            if pair[1].1.revert.is_some() && pair[1].1.epoch_floor <= pair[0].1.epoch_floor {
                return Err("stream revert does not advance the attachment epoch floor".into());
            }
        }
        for (marker, cut) in cuts.iter().rev() {
            let expected_marker = cut
                .revert
                .as_ref()
                .map_or(cut.cut_index, |region| region.end)
                .as_u64()
                .checked_add(if cut.revert.is_some() { 2 } else { 1 });
            if !StreamSessionRecord::ForkCut(cut.clone()).has_supported_format()
                || (cut.revert.is_none() && cut.epoch_floor != 1)
                || (cut.revert.is_some() && cut.epoch_floor <= 1)
                || expected_marker != Some(marker.as_u64())
                || next_marker.is_some_and(|next| *marker >= next)
                || cut.target_environment_id != expected_owner.environment_id
                || cut.target != expected_owner.agent_id
                || cut.target_fingerprint != expected_fingerprint
            {
                return Err("invalid stream fork lineage chain".into());
            }
            expected_owner = OwnedAgentId::new(cut.source_environment_id, &cut.source);
            expected_fingerprint = cut.source_fingerprint;
            next_marker = Some(*marker);
        }

        let lineage = Self {
            cuts,
            deleted_regions: DeletedRegions::new(),
            revert_regions: Vec::new(),
        };
        let mut registered = HashMap::new();
        for (index, registration) in registrations {
            let (author, author_fingerprint) = lineage.author_at(*index, owner, fingerprint);
            let handle = &registration.handle;
            if registration.format_version != DURABLE_STREAM_FORMAT_VERSION
                || handle.format_version != DURABLE_STREAM_FORMAT_VERSION
                || registration.registration_oplog_index != *index
                || handle.producer_environment_id != author.environment_id
                || handle.producer != author.agent_id
                || handle.expected_producer_fingerprint != author_fingerprint
                || StreamId::derive(
                    author.environment_id,
                    &author.agent_id,
                    author_fingerprint,
                    *index,
                )
                .map_err(|error| error.to_string())?
                    != handle.stream_id
                || registered
                    .insert(handle.stream_id, (*index, handle.clone()))
                    .is_some()
            {
                return Err("invalid historical stream registration in fork lineage".into());
            }
        }
        for (_, cut) in &lineage.cuts {
            let mut session_sources = HashSet::new();
            let mut session_targets = HashSet::new();
            for mapping in &cut.sessions {
                let source = &mapping.source;
                let target = &mapping.continuation;
                if source.callee_environment_id != cut.source_environment_id
                    || source.callee != cut.source
                    || source.callee_fingerprint != cut.source_fingerprint
                    || target.callee_environment_id != cut.target_environment_id
                    || target.callee != cut.target
                    || target.callee_fingerprint != cut.target_fingerprint
                    || target.idempotency_key != source.idempotency_key
                    || !session_sources.insert(source.clone())
                    || !session_targets.insert(target.clone())
                {
                    return Err("invalid local session mapping in fork lineage".into());
                }
            }
            let mut source_ids = HashSet::new();
            let mut target_ids = HashSet::new();
            let mut mapped = Vec::new();
            for mapping in &cut.streams {
                let Some((index, original)) = registered.get(&mapping.source.stream_id) else {
                    return Err("fork lineage references an unknown source stream".into());
                };
                if *index > cut.cut_index
                    || original != &mapping.source
                    || original.producer_environment_id != cut.source_environment_id
                    || original.producer != cut.source
                    || original.expected_producer_fingerprint != cut.source_fingerprint
                {
                    return Err("fork lineage source differs from retained registration".into());
                }
                if cut.selected_stream_id == Some(original.stream_id)
                    && cut
                        .retained_through
                        .is_some_and(|offset| offset.producer_oplog_index() <= *index)
                {
                    return Err("fork lineage retained item precedes its stream's data".into());
                }
                let mut expected = original.clone();
                expected.stream_id = continuation_stream_id(cut, original.stream_id)?;
                expected.producer_environment_id = cut.target_environment_id;
                expected.producer = cut.target.clone();
                expected.expected_producer_fingerprint = cut.target_fingerprint;
                if let Some(session) = cut
                    .sessions
                    .iter()
                    .find(|s| s.source == expected.source_invocation)
                {
                    expected.source_invocation = session.continuation.clone();
                } else if expected.source_invocation.callee_environment_id
                    == cut.source_environment_id
                    && expected.source_invocation.callee == cut.source
                    && expected.source_invocation.callee_fingerprint == cut.source_fingerprint
                {
                    return Err("fork lineage omits a local stream session".into());
                }
                if expected != mapping.continuation
                    || !source_ids.insert(original.stream_id)
                    || !target_ids.insert(expected.stream_id)
                    || registered.contains_key(&expected.stream_id)
                {
                    return Err("invalid stream continuation mapping in fork lineage".into());
                }
                mapped.push((expected.stream_id, (*index, expected)));
            }
            if registered.values().any(|(index, handle)| {
                *index <= cut.cut_index
                    && handle.producer_environment_id == cut.source_environment_id
                    && handle.producer == cut.source
                    && handle.expected_producer_fingerprint == cut.source_fingerprint
                    && !source_ids.contains(&handle.stream_id)
            }) {
                return Err("fork lineage omits a retained local stream".into());
            }
            for source in source_ids {
                registered.remove(&source);
            }
            registered.extend(mapped);
        }
        Ok(lineage)
    }

    /// Resolves authorship before the first revert that removed this position. Ordinary forks
    /// may copy a physical prefix that is no longer visible in the source's current history.
    pub(crate) async fn historical_author_at(
        &self,
        oplog: &dyn Oplog,
        index: OplogIndex,
        horizon: OplogIndex,
        owner: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(OwnedAgentId, AgentFingerprint), String> {
        let Some((revert_index, region)) = self
            .revert_regions
            .iter()
            .find(|(_, region)| region.start <= index && index <= region.end)
        else {
            return Ok(self.author_at(index, owner, fingerprint));
        };
        if *revert_index == horizon {
            return Ok(self.author_at(index, owner, fingerprint));
        }
        let OplogEntry::StreamSession { record, .. } = oplog.read(revert_index.next()).await else {
            return Ok(self.author_at(index, owner, fingerprint));
        };
        let record = oplog.download_payload(record).await?;
        let StreamSessionRecord::ForkCut(cut) = &record else {
            return Ok(self.author_at(index, owner, fingerprint));
        };
        if cut.revert.is_none() {
            return Ok(self.author_at(index, owner, fingerprint));
        }
        if cut.revert.as_ref() != Some(region) || !record.has_supported_format() {
            return Err("invalid historical revert marker".into());
        }
        let historical_owner = OwnedAgentId::new(cut.target_environment_id, &cut.target);
        let historical = Self::load_at_horizon(
            oplog,
            &historical_owner,
            cut.target_fingerprint,
            revert_index.previous(),
        )
        .await?;
        Ok(historical.author_at(index, &historical_owner, cut.target_fingerprint))
    }

    pub fn author_at(
        &self,
        index: OplogIndex,
        owner: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> (OwnedAgentId, AgentFingerprint) {
        let mut author = owner.clone();
        let mut fingerprint = fingerprint;
        for (_, cut) in self.cuts.iter().rev() {
            if index <= cut.cut_index {
                author = OwnedAgentId::new(cut.source_environment_id, &cut.source);
                fingerprint = cut.source_fingerprint;
            }
        }
        (author, fingerprint)
    }

    /// Internal replay resolution only. Public handle validation must still require the current
    /// producer identity, so possession of a historical handle never grants target authority.
    pub fn continuation_handle(&self, handle: &DurableStreamHandle) -> DurableStreamHandle {
        let mut handle = handle.clone();
        for (_, cut) in &self.cuts {
            if let Some(mapping) = cut.streams.iter().find(|mapping| mapping.source == handle) {
                handle = mapping.continuation.clone();
            }
        }
        handle
    }

    pub fn continuation_session(&self, key: &StreamSessionKey) -> StreamSessionKey {
        let mut key = key.clone();
        for (_, cut) in &self.cuts {
            if let Some(mapping) = cut.sessions.iter().find(|mapping| mapping.source == key) {
                key = mapping.continuation.clone();
            }
        }
        key
    }

    pub(crate) fn session_at(&self, key: &StreamSessionKey, index: OplogIndex) -> StreamSessionKey {
        let mut key = key.clone();
        for (marker, cut) in self.cuts.iter().rev() {
            if *marker > index
                && let Some(mapping) = cut
                    .sessions
                    .iter()
                    .find(|mapping| mapping.continuation == key)
            {
                key = mapping.source.clone();
            }
        }
        key
    }

    pub fn continuation_stream_id(&self, stream_id: StreamId) -> StreamId {
        let mut stream_id = stream_id;
        for (_, cut) in &self.cuts {
            if let Some(mapping) = cut
                .streams
                .iter()
                .find(|mapping| mapping.source.stream_id == stream_id)
            {
                stream_id = mapping.continuation.stream_id;
            }
        }
        stream_id
    }

    /// Materializes retained observations under continuation identities, not live attachment
    /// authority. Prepared/Attached attempts and epochs remain historical replay evidence.
    pub(crate) fn project_session_payload(
        &self,
        index: OplogIndex,
        record: &mut StreamSessionRecord,
    ) -> Result<(), String> {
        for (_, cut) in &self.cuts {
            if index > cut.cut_index {
                continue;
            }
            let session = |key: &mut StreamSessionKey| {
                if let Some(mapping) = cut.sessions.iter().find(|mapping| mapping.source == *key) {
                    *key = mapping.continuation.clone();
                }
            };
            let stream = |id: &mut StreamId| {
                if let Some(mapping) = cut
                    .streams
                    .iter()
                    .find(|mapping| mapping.source.stream_id == *id)
                {
                    *id = mapping.continuation.stream_id;
                }
            };
            let handle = |handle: &mut DurableStreamHandle| -> Result<(), String> {
                if let Some(mapping) = cut
                    .streams
                    .iter()
                    .find(|mapping| mapping.source.stream_id == handle.stream_id)
                {
                    if mapping.source != *handle {
                        return Err(
                            "historical session handle differs from its registration".into()
                        );
                    }
                    *handle = mapping.continuation.clone();
                }
                Ok(())
            };
            let mappings = |mappings: &mut Vec<StreamSessionMappingRecord>| -> Result<(), String> {
                for mapping in mappings {
                    handle(&mut mapping.handle)?;
                }
                Ok(())
            };
            match record {
                StreamSessionRecord::Prepared(record) => {
                    session(&mut record.attempt.session_key);
                    session(&mut record.attempt.invocation.session_key);
                    record.attempt.expected_callee_fingerprint =
                        record.attempt.session_key.callee_fingerprint;
                    record.attempt.attachment_id = AttachmentId::primary(
                        record.attempt.session_key.callee_environment_id,
                        &record.attempt.session_key.callee,
                        &record.attempt.session_key.idempotency_key,
                    )
                    .map_err(|error| error.to_string())?;
                    for value in &mut record.attempt.invocation.stream_handles {
                        handle(value)?;
                    }
                    mappings(&mut record.stream_mappings)?;
                }
                StreamSessionRecord::Attached(record) => {
                    session(&mut record.session_key);
                    record.attachment_id = AttachmentId::primary(
                        record.session_key.callee_environment_id,
                        &record.session_key.callee,
                        &record.session_key.idempotency_key,
                    )
                    .map_err(|error| error.to_string())?;
                }
                StreamSessionRecord::InvocationResult(record) => {
                    session(&mut record.session_key);
                    for value in &mut record.output_streams {
                        handle(value)?;
                    }
                    mappings(&mut record.stream_mappings)?;
                }
                StreamSessionRecord::ConsumerItemValue(record) => {
                    session(&mut record.session_key);
                    stream(&mut record.stream_id);
                    for value in &mut record.recursive_handles {
                        handle(value)?;
                    }
                    mappings(&mut record.recursive_mappings)?;
                }
                StreamSessionRecord::ConsumerTerminal(record) => {
                    session(&mut record.session_key);
                    stream(&mut record.stream_id);
                }
                StreamSessionRecord::SourceUnavailable(record) => {
                    project_attachment_key(cut, &mut record.key)?;
                }
                StreamSessionRecord::Finished(record) => session(&mut record.session_key),
                StreamSessionRecord::CallerAttempt(_)
                | StreamSessionRecord::ResumeAttempt(_)
                | StreamSessionRecord::Detached(_)
                | StreamSessionRecord::Mapping(_)
                | StreamSessionRecord::AttachmentPrepared(_)
                | StreamSessionRecord::AttachmentActivated(_)
                | StreamSessionRecord::AttachmentRenewed(_)
                | StreamSessionRecord::AttachmentFinalized(_)
                | StreamSessionRecord::ProducerDeleting(_)
                | StreamSessionRecord::CascadeOutbox(_)
                | StreamSessionRecord::ConsumerDeleting(_)
                | StreamSessionRecord::TopologyPrepared(_)
                | StreamSessionRecord::TopologyActivated(_)
                | StreamSessionRecord::InputHighWater(_)
                | StreamSessionRecord::ExternalProducerState(_)
                | StreamSessionRecord::ConsumerCancelIntent(_)
                | StreamSessionRecord::ConsumerCancelApplied(_)
                | StreamSessionRecord::Tombstoned(_)
                | StreamSessionRecord::CancelRequested(_)
                | StreamSessionRecord::ForkCut(_) => {}
            }
        }
        Ok(())
    }

    pub(crate) fn project_item_batch(
        &self,
        index: OplogIndex,
        record: &mut golem_common::model::durable_stream::StreamItemsRecord,
    ) -> Result<(), String> {
        use golem_common::model::durable_stream::StreamItemsPayload;
        for (_, cut) in &self.cuts {
            if index > cut.cut_index {
                continue;
            }
            let Some(mapping) = cut
                .streams
                .iter()
                .find(|mapping| mapping.source.stream_id == record.stream_id)
            else {
                continue;
            };
            if record.producer_fingerprint != mapping.source.expected_producer_fingerprint {
                return Err(
                    "historical stream batch fingerprint differs from its registration".into(),
                );
            }
            if cut.selected_stream_id == Some(record.stream_id) {
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
            record.stream_id = mapping.continuation.stream_id;
            record.producer_fingerprint = mapping.continuation.expected_producer_fingerprint;
            for id in record
                .nested_stream_ids
                .iter_mut()
                .chain(&mut record.newly_registered_stream_ids)
            {
                if let Some(mapping) = cut
                    .streams
                    .iter()
                    .find(|mapping| mapping.source.stream_id == *id)
                {
                    *id = mapping.continuation.stream_id;
                }
            }
        }
        Ok(())
    }

    /// Projects only prefixes whose provenance was not discarded by a later revert. Loading
    /// that earlier horizon directly is required when its markers are no longer in this view.
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

    pub fn cuts(&self) -> &[(OplogIndex, StreamForkCutRecord)] {
        &self.cuts
    }
}

/// Projects retained topology or source-unavailable evidence, without granting live authority.
pub(crate) fn project_attachment_key(
    cut: &StreamForkCutRecord,
    key: &mut golem_common::model::durable_stream::StreamAttachmentKey,
) -> Result<(), String> {
    if let Some(mapping) = cut
        .sessions
        .iter()
        .find(|mapping| mapping.source == key.session_key)
    {
        key.session_key = mapping.continuation.clone();
        key.epoch = cut.epoch_floor;
        key.attachment_id = AttachmentId::primary(
            key.session_key.callee_environment_id,
            &key.session_key.callee,
            &key.session_key.idempotency_key,
        )
        .map_err(|error| error.to_string())?;
    } else if (key.consumer_environment_id == cut.source_environment_id
        && key.consumer == cut.source
        && key.expected_consumer_fingerprint == cut.source_fingerprint)
        || cut
            .streams
            .iter()
            .any(|mapping| mapping.source.stream_id == key.stream_id)
    {
        key.epoch = cut.epoch_floor;
    }
    if let Some(mapping) = cut
        .streams
        .iter()
        .find(|mapping| mapping.source.stream_id == key.stream_id)
    {
        if key.producer_environment_id != mapping.source.producer_environment_id
            || key.producer != mapping.source.producer
            || key.expected_producer_fingerprint != mapping.source.expected_producer_fingerprint
        {
            return Err("historical attachment producer differs from its registration".into());
        }
        let handle = &mapping.continuation;
        key.stream_id = handle.stream_id;
        key.producer_environment_id = handle.producer_environment_id;
        key.producer = handle.producer.clone();
        key.expected_producer_fingerprint = handle.expected_producer_fingerprint;
    }
    if key.consumer_environment_id == cut.source_environment_id
        && key.consumer == cut.source
        && key.expected_consumer_fingerprint == cut.source_fingerprint
    {
        key.consumer_environment_id = cut.target_environment_id;
        key.consumer = cut.target.clone();
        key.expected_consumer_fingerprint = cut.target_fingerprint;
        key.consumer_invocation.callee_environment_id = cut.target_environment_id;
        key.consumer_invocation.callee = cut.target.clone();
        key.consumer_invocation.callee_fingerprint = cut.target_fingerprint;
    }
    Ok(())
}

/// Domain separation by the original stream gives every continuation a distinct identity,
/// including a revert onto the same agent and fingerprint.
pub fn continuation_stream_id(
    cut: &StreamForkCutRecord,
    source: StreamId,
) -> Result<StreamId, String> {
    let target = StreamId::derive(
        cut.target_environment_id,
        &cut.target,
        cut.target_fingerprint,
        cut.revert
            .as_ref()
            .map_or(cut.cut_index, |region| region.end),
    )
    .map_err(|error| error.to_string())?;
    Ok(StreamId(Uuid::new_v5(&source.0, target.0.as_bytes())))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::ExecutionStatus;
    use crate::services::oplog::tests::{ReadCountingBlobStorage, ReadCountingIndexedStorage};
    use crate::services::oplog::{CommitLevel, PrimaryOplogService};
    use arc_swap::ArcSwap;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::durable_stream::{
        AttachmentId, AttemptId, PersistedStreamInvocationDescriptor, StartAttemptDescriptor,
        StreamForkSessionMapping, StreamForkStreamMapping, StreamRegistrationCoordinate,
        StreamRootKind, StreamSourceKind,
    };
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::{
        AgentId, AgentMetadata, AgentStatusRecord, IdempotencyKey, RetryConfig, Timestamp,
    };
    use golem_common::read_only_lock;
    use golem_schema::schema::SchemaFingerprintV1;
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
            let create = OplogEntry::create(
                owner.agent_id.clone(),
                AgentMode::Durable,
                ComponentRevision::INITIAL,
                vec![],
                owner.environment_id,
                account,
                None,
                100,
                100,
                HashSet::new(),
                vec![],
                None,
                fingerprint.0,
            );
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

    pub(crate) fn prepared(key: &StreamSessionKey) -> StreamSessionPreparedRecord {
        StreamSessionPreparedRecord {
            format_version: 1,
            attempt: StartAttemptDescriptor {
                format_version: 1,
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
                    format_version: 1,
                    session_key: key.clone(),
                    target_component_revision: ComponentRevision::new(3).unwrap(),
                    method_name: "output".into(),
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

    fn owner(name: &str) -> OwnedAgentId {
        OwnedAgentId::new(
            EnvironmentId(Uuid::from_u128(11)),
            &AgentId {
                component_id: ComponentId(Uuid::from_u128(23)),
                agent_id: name.to_string(),
            },
        )
    }

    fn registration() -> StreamRegisteredRecord {
        let source = owner("source");
        let fingerprint = AgentFingerprint(Uuid::from_u128(37));
        let key = StreamSessionKey {
            callee_environment_id: source.environment_id,
            callee: source.agent_id.clone(),
            callee_fingerprint: fingerprint,
            idempotency_key: IdempotencyKey::new("session-1".into()),
        };
        let index = OplogIndex::from_u64(5);
        StreamRegisteredRecord {
            format_version: 1,
            coordinate: StreamRegistrationCoordinate::Root {
                invocation_id: key.clone(),
                root_kind: StreamRootKind::MethodInput,
                recursive_value_path: vec![],
            },
            registration_oplog_index: index,
            handle: DurableStreamHandle {
                format_version: 1,
                stream_id: StreamId::derive(
                    source.environment_id,
                    &source.agent_id,
                    fingerprint,
                    index,
                )
                .unwrap(),
                producer_environment_id: source.environment_id,
                producer: source.agent_id,
                expected_producer_fingerprint: fingerprint,
                source_invocation: key,
                component_revision: ComponentRevision::new(3).unwrap(),
                element_schema_fingerprint: SchemaFingerprintV1([47; 32]),
            },
            source_kind: StreamSourceKind::ExternalInlineInput,
            session_mapping: None,
        }
    }

    pub(crate) fn fork(
        source: &DurableStreamHandle,
        target: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        index: u64,
    ) -> StreamForkCutRecord {
        let mut session = source.source_invocation.clone();
        session.callee_environment_id = target.environment_id;
        session.callee = target.agent_id.clone();
        session.callee_fingerprint = fingerprint;
        let mut cut = StreamForkCutRecord {
            format_version: 1,
            request_hash: vec![0; 32],
            export: None,
            source_environment_id: source.producer_environment_id,
            source: source.producer.clone(),
            source_fingerprint: source.expected_producer_fingerprint,
            target_environment_id: target.environment_id,
            target: target.agent_id.clone(),
            target_fingerprint: fingerprint,
            cut_index: OplogIndex::from_u64(index),
            revert: None,
            epoch_floor: 1,
            selected_stream_id: Some(source.stream_id),
            retained_through: None,
            streams: vec![],
            sessions: vec![StreamForkSessionMapping {
                source: source.source_invocation.clone(),
                continuation: session.clone(),
                continuation_attempt_id: AttemptId::fresh(),
            }],
        };
        let mut continuation = source.clone();
        continuation.stream_id = continuation_stream_id(&cut, source.stream_id).unwrap();
        continuation.producer_environment_id = target.environment_id;
        continuation.producer = target.agent_id.clone();
        continuation.expected_producer_fingerprint = fingerprint;
        continuation.source_invocation = session;
        cut.streams.push(StreamForkStreamMapping {
            source: source.clone(),
            continuation,
        });
        cut
    }

    fn self_revert(
        identity: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        end: u64,
    ) -> StreamForkCutRecord {
        StreamForkCutRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            request_hash: vec![0; 32],
            export: None,
            source_environment_id: identity.environment_id,
            source: identity.agent_id.clone(),
            source_fingerprint: fingerprint,
            target_environment_id: identity.environment_id,
            target: identity.agent_id.clone(),
            target_fingerprint: fingerprint,
            cut_index: OplogIndex::from_u64(1),
            revert: Some(OplogRegion::from_range(2..=end)),
            epoch_floor: 2,
            selected_stream_id: None,
            retained_through: None,
            streams: vec![],
            sessions: vec![],
        }
    }

    #[test]
    fn repeated_same_cut_reverts_have_distinct_continuations_and_must_grow() {
        let identity = owner("source");
        let fingerprint = AgentFingerprint(Uuid::from_u128(37));
        let source = StreamId(Uuid::from_u128(41));
        let first = self_revert(&identity, fingerprint, 3);
        let second = self_revert(&identity, fingerprint, 7);
        assert_ne!(
            continuation_stream_id(&first, source).unwrap(),
            continuation_stream_id(&second, source).unwrap()
        );
        StreamForkLineage::validate(
            vec![(OplogIndex::from_u64(9), second.clone())],
            &[],
            &identity,
            fingerprint,
        )
        .unwrap();
        assert!(
            StreamForkLineage::validate(
                vec![
                    (OplogIndex::from_u64(5), first),
                    (OplogIndex::from_u64(9), second),
                ],
                &[],
                &identity,
                fingerprint,
            )
            .is_err()
        );
    }

    #[test]
    async fn revert_marker_requires_adjacent_matching_revert() {
        let identity = owner("source");
        let fingerprint = AgentFingerprint(Uuid::from_u128(37));
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
            let marker = StreamSessionRecord::ForkCut(self_revert(&identity, fingerprint, 3));
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
        let fingerprint = AgentFingerprint(Uuid::from_u128(37));
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
    async fn lineage_skips_deleted_stream_payloads_without_downloading_them() {
        let identity = owner("source");
        let fingerprint = AgentFingerprint(Uuid::from_u128(37));
        let fixture = ServiceFixture::new(&identity, fingerprint).await;
        let registration = registration();
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
                        .upload_payload(&StreamSessionRecord::Prepared(prepared(
                            &registration.handle.source_invocation,
                        )))
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
                8 => OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: fixture
                        .oplog
                        .upload_payload(&StreamSessionRecord::Prepared({
                            let mut discarded = prepared(&registration.handle.source_invocation);
                            discarded.format_version = 0;
                            discarded
                        }))
                        .await
                        .unwrap(),
                },
                9 => OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: fixture
                        .oplog
                        .upload_payload(&StreamSessionRecord::ForkCut({
                            let mut discarded = self_revert(&identity, fingerprint, 7);
                            discarded.format_version = 0;
                            discarded
                        }))
                        .await
                        .unwrap(),
                },
                _ => OplogEntry::NoOp {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                },
            };
            fixture.oplog.add(entry).await;
        }
        let region = OplogRegion::from_range(7..=9);
        fixture.oplog.add(OplogEntry::revert(region.clone())).await;
        let mut marker = fork(&registration.handle, &identity, fingerprint, 6);
        marker.revert = Some(region);
        marker.epoch_floor = 2;
        marker.selected_stream_id = None;
        marker.streams[0].continuation.stream_id =
            continuation_stream_id(&marker, registration.handle.stream_id).unwrap();
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
            .is_err(),
            "a horizon between Revert and its marker must not expose partially reverted state"
        );
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
    fn lineage_composes_forks_without_rewriting_historical_identity() {
        let registration = registration();
        let middle = owner("middle");
        let target = owner("target");
        let middle_fingerprint = AgentFingerprint(Uuid::from_u128(53));
        let target_fingerprint = AgentFingerprint(Uuid::from_u128(67));
        let first = fork(&registration.handle, &middle, middle_fingerprint, 9);
        let second = fork(
            &first.streams[0].continuation,
            &target,
            target_fingerprint,
            15,
        );
        let expected = second.streams[0].continuation.clone();
        let cuts = vec![
            (OplogIndex::from_u64(10), first),
            (OplogIndex::from_u64(16), second),
        ];
        let lineage = StreamForkLineage::validate(
            cuts,
            &[(OplogIndex::from_u64(5), registration.clone())],
            &target,
            target_fingerprint,
        )
        .unwrap();
        assert_eq!(
            lineage.author_at(OplogIndex::from_u64(5), &target, target_fingerprint),
            (
                owner("source"),
                registration.handle.expected_producer_fingerprint
            )
        );
        assert_eq!(
            lineage.author_at(OplogIndex::from_u64(11), &target, target_fingerprint),
            (middle, middle_fingerprint)
        );
        assert_eq!(
            lineage.author_at(OplogIndex::from_u64(17), &target, target_fingerprint),
            (target, target_fingerprint)
        );
        assert_eq!(lineage.continuation_handle(&registration.handle), expected);
        assert_eq!(
            lineage.continuation_session(&registration.handle.source_invocation),
            expected.source_invocation
        );
        let mut forged = registration.handle.clone();
        forged.element_schema_fingerprint = SchemaFingerprintV1([99; 32]);
        assert_eq!(lineage.continuation_handle(&forged), forged);
    }

    #[test]
    fn call_authorship_survives_forks_before_stream_registration() {
        let registration = registration();
        let middle = owner("middle");
        let target = owner("target");
        let middle_fingerprint = AgentFingerprint(Uuid::from_u128(53));
        let target_fingerprint = AgentFingerprint(Uuid::from_u128(67));
        let mut first = fork(&registration.handle, &middle, middle_fingerprint, 2);
        let mut second = fork(
            &first.streams[0].continuation,
            &target,
            target_fingerprint,
            7,
        );
        first.streams.clear();
        first.sessions.clear();
        first.selected_stream_id = None;
        second.streams.clear();
        second.sessions.clear();
        second.selected_stream_id = None;
        let lineage = StreamForkLineage::validate(
            vec![
                (OplogIndex::from_u64(3), first),
                (OplogIndex::from_u64(8), second),
            ],
            &[],
            &target,
            target_fingerprint,
        )
        .unwrap();
        for (index, expected) in [
            (
                2,
                (
                    owner("source"),
                    registration.handle.expected_producer_fingerprint,
                ),
            ),
            (3, (middle.clone(), middle_fingerprint)),
            (7, (middle, middle_fingerprint)),
            (8, (target.clone(), target_fingerprint)),
        ] {
            assert_eq!(
                lineage.author_at(OplogIndex::from_u64(index), &target, target_fingerprint),
                expected
            );
        }
    }

    #[test]
    fn payload_projection_composes_cuts_and_preserves_value_bytes() {
        use golem_common::model::durable_stream::{
            StreamItemsPayload, StreamItemsRecord, StreamOffset,
        };
        let registration = registration();
        let middle = owner("middle");
        let target = owner("target");
        let middle_fingerprint = AgentFingerprint(Uuid::from_u128(53));
        let target_fingerprint = AgentFingerprint(Uuid::from_u128(67));
        let index = OplogIndex::from_u64(7);
        let mut first = fork(&registration.handle, &middle, middle_fingerprint, 9);
        first.retained_through = Some(StreamOffset::new(index, 3));
        let mut second = fork(
            &first.streams[0].continuation,
            &target,
            target_fingerprint,
            15,
        );
        second.retained_through = Some(StreamOffset::new(index, 1));
        let target_id = second.streams[0].continuation.stream_id;
        let middle_session = first.sessions[0].continuation.clone();
        let target_session = second.sessions[0].continuation.clone();
        let lineage = StreamForkLineage::validate(
            vec![
                (OplogIndex::from_u64(10), first),
                (OplogIndex::from_u64(16), second),
            ],
            &[(registration.registration_oplog_index, registration.clone())],
            &target,
            target_fingerprint,
        )
        .unwrap();
        for (position, expected) in [
            (0, &registration.handle.source_invocation),
            (9, &registration.handle.source_invocation),
            (10, &middle_session),
            (15, &middle_session),
            (16, &target_session),
            (17, &target_session),
        ] {
            assert_eq!(
                lineage.session_at(&target_session, OplogIndex::from_u64(position)),
                *expected
            );
        }
        let batch = StreamItemsRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            stream_id: registration.handle.stream_id,
            producer_fingerprint: registration.handle.expected_producer_fingerprint,
            first_sequence: 5,
            offsets: (0..5).map(|sub| StreamOffset::new(index, sub)).collect(),
            nested_stream_ids: vec![],
            newly_registered_stream_ids: vec![],
            payload: StreamItemsPayload::PackedU8(vec![10, 11, 12, 13, 14]),
        };
        let mut projected = batch.clone();
        lineage.project_item_batch(index, &mut projected).unwrap();
        assert_eq!(projected.stream_id, target_id);
        assert_eq!(projected.producer_fingerprint, target_fingerprint);
        assert_eq!(projected.first_sequence, 5);
        assert_eq!(
            projected.offsets,
            vec![StreamOffset::new(index, 0), StreamOffset::new(index, 1)]
        );
        assert_eq!(
            projected.payload,
            StreamItemsPayload::PackedU8(vec![10, 11])
        );

        let mut value = batch.clone();
        value.offsets = vec![StreamOffset::new(OplogIndex::from_u64(6), 0)];
        value.payload = StreamItemsPayload::Values(vec![vec![8, 1, 16, 0]]);
        value.nested_stream_ids = vec![registration.handle.stream_id];
        value.newly_registered_stream_ids = vec![registration.handle.stream_id];
        lineage
            .project_item_batch(OplogIndex::from_u64(6), &mut value)
            .unwrap();
        assert_eq!(
            value.payload,
            StreamItemsPayload::Values(vec![vec![8, 1, 16, 0]])
        );
        assert_eq!(value.nested_stream_ids, vec![target_id]);
        assert_eq!(value.newly_registered_stream_ids, vec![target_id]);

        let mut forged = batch.clone();
        forged.producer_fingerprint = target_fingerprint;
        assert!(lineage.project_item_batch(index, &mut forged).is_err());
        let mut discarded = batch.clone();
        assert!(
            lineage
                .project_item_batch(OplogIndex::from_u64(8), &mut discarded)
                .is_err()
        );
        let mut later = batch.clone();
        lineage
            .project_item_batch(OplogIndex::from_u64(17), &mut later)
            .unwrap();
        assert_eq!(later, batch);
        assert!(
            lineage
                .through(OplogIndex::from_u64(9))
                .unwrap()
                .cuts()
                .is_empty()
        );
        let covered = lineage.through(OplogIndex::from_u64(10)).unwrap();
        assert_eq!(covered.cuts().len(), 1);
        let mut prefix = batch;
        covered.project_item_batch(index, &mut prefix).unwrap();
        assert_eq!(prefix.producer_fingerprint, middle_fingerprint);
        assert_eq!(
            prefix.payload,
            StreamItemsPayload::PackedU8(vec![10, 11, 12, 13])
        );
    }

    #[test]
    fn session_payload_projection_keeps_chained_records_well_formed() {
        use golem_common::model::durable_stream::{
            SessionStreamRole, StreamAttachmentKey, StreamConsumerTerminal,
            StreamConsumerTerminalRecord, StreamEndResult, StreamOffset,
            StreamSessionAttachedRecord, StreamSessionFinishedRecord,
            StreamSessionInvocationResultRecord, StreamSourceUnavailableRecord,
        };
        let original = registration();
        let target = owner("target");
        let fingerprint = AgentFingerprint(Uuid::from_u128(67));
        let mut first = fork(
            &original.handle,
            &owner("middle"),
            AgentFingerprint(Uuid::from_u128(53)),
            9,
        );
        first.selected_stream_id = None;
        let mut second = fork(&first.streams[0].continuation, &target, fingerprint, 15);
        second.selected_stream_id = None;
        let expected = second.streams[0].continuation.clone();
        let lineage = StreamForkLineage::validate(
            vec![
                (OplogIndex::from_u64(10), first),
                (OplogIndex::from_u64(16), second),
            ],
            &[(original.registration_oplog_index, original.clone())],
            &target,
            fingerprint,
        )
        .unwrap();
        let source = &original.handle.source_invocation;
        let preparation = prepared(source);
        let attempt = preparation.attempt.attempt_id;
        let source_key = StreamAttachmentKey {
            attachment_id: preparation.attempt.attachment_id,
            stream_id: original.handle.stream_id,
            epoch: 7,
            session_key: source.clone(),
            producer_environment_id: original.handle.producer_environment_id,
            producer: original.handle.producer.clone(),
            expected_producer_fingerprint: original.handle.expected_producer_fingerprint,
            consumer_environment_id: source.callee_environment_id,
            consumer: source.callee.clone(),
            expected_consumer_fingerprint: source.callee_fingerprint,
            consumer_invocation: source.clone(),
        };
        let offset = StreamOffset::new(OplogIndex::from_u64(7), 2);
        let mut records = vec![
            StreamSessionRecord::Prepared(preparation),
            StreamSessionRecord::Attached(StreamSessionAttachedRecord {
                format_version: 1,
                session_key: source.clone(),
                attachment_id: source_key.attachment_id,
                attempt_id: attempt,
                epoch: 7,
                pending_invocation_oplog_index: OplogIndex::from_u64(6),
            }),
            StreamSessionRecord::SourceUnavailable(StreamSourceUnavailableRecord {
                format_version: 1,
                key: source_key,
                source_offset: offset,
                consumer_read_ordinal: 3,
            }),
            StreamSessionRecord::ConsumerTerminal(StreamConsumerTerminalRecord {
                format_version: 1,
                session_key: source.clone(),
                stream_id: original.handle.stream_id,
                source_offset: offset,
                consumer_read_ordinal: 3,
                terminal: StreamConsumerTerminal::End(StreamEndResult::Ok),
            }),
            StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                format_version: 1,
                session_key: source.clone(),
                result: Err(vec![11, 23]),
            }),
            StreamSessionRecord::InvocationResult(StreamSessionInvocationResultRecord {
                format_version: 1,
                session_key: source.clone(),
                result: vec![11, 23],
                output_streams: vec![original.handle.clone()],
                stream_mappings: vec![StreamSessionMappingRecord {
                    transport_stream_id: 23,
                    handle: original.handle.clone(),
                    role: SessionStreamRole::Output,
                }],
            }),
        ];
        let mut forged = records.last().unwrap().clone();
        if let StreamSessionRecord::InvocationResult(record) = &mut forged {
            record.output_streams[0].element_schema_fingerprint = SchemaFingerprintV1([99; 32]);
        }
        assert!(
            lineage
                .project_session_payload(OplogIndex::from_u64(8), &mut forged)
                .is_err()
        );
        for record in &mut records {
            assert!(record.has_supported_format());
            let mut later = record.clone();
            lineage
                .project_session_payload(OplogIndex::from_u64(17), &mut later)
                .unwrap();
            assert_eq!(&later, record);
            lineage
                .project_session_payload(OplogIndex::from_u64(8), record)
                .unwrap();
            assert!(record.has_supported_format(), "{record:?}");
            assert_eq!(
                crate::worker::stream_session_record_key(record),
                Some(&expected.source_invocation)
            );
        }
        let StreamSessionRecord::Attached(attached) = &records[1] else {
            unreachable!()
        };
        assert_eq!((attached.epoch, attached.attempt_id), (7, attempt));
        let StreamSessionRecord::SourceUnavailable(unavailable) = &records[2] else {
            unreachable!()
        };
        assert_eq!(unavailable.key.epoch, 1);
        assert_eq!(unavailable.key.stream_id, expected.stream_id);
        assert_eq!(unavailable.key.producer, target.agent_id);
        assert_eq!(unavailable.key.consumer, target.agent_id);
        assert_eq!(unavailable.key.expected_producer_fingerprint, fingerprint);
        assert_eq!(unavailable.key.expected_consumer_fingerprint, fingerprint);
        assert_eq!(unavailable.source_offset, offset);
        let StreamSessionRecord::InvocationResult(result) = records.last().unwrap() else {
            unreachable!()
        };
        assert_eq!(result.output_streams, vec![expected.clone()]);
        assert_eq!(result.stream_mappings[0].handle, expected);
        assert_eq!(result.result, vec![11, 23]);
    }

    #[test]
    fn lineage_includes_streams_registered_between_forks() {
        let original = registration();
        let middle = owner("middle");
        let target = owner("target");
        let middle_fingerprint = AgentFingerprint(Uuid::from_u128(53));
        let target_fingerprint = AgentFingerprint(Uuid::from_u128(67));
        let first = fork(&original.handle, &middle, middle_fingerprint, 9);
        let mut added = original.clone();
        added.handle = first.streams[0].continuation.clone();
        added.registration_oplog_index = OplogIndex::from_u64(12);
        added.handle.stream_id = StreamId::derive(
            middle.environment_id,
            &middle.agent_id,
            middle_fingerprint,
            added.registration_oplog_index,
        )
        .unwrap();
        added.coordinate = StreamRegistrationCoordinate::Root {
            invocation_id: added.handle.source_invocation.clone(),
            root_kind: StreamRootKind::MethodResult,
            recursive_value_path: vec![],
        };
        let mut second = fork(
            &first.streams[0].continuation,
            &target,
            target_fingerprint,
            15,
        );
        let added_mapping = fork(&added.handle, &target, target_fingerprint, 15)
            .streams
            .remove(0);
        assert_ne!(
            second.streams[0].continuation.stream_id,
            added_mapping.continuation.stream_id
        );
        assert_eq!(
            continuation_stream_id(&second, added.handle.stream_id).unwrap(),
            added_mapping.continuation.stream_id
        );
        second.streams.push(added_mapping.clone());
        let registrations = vec![
            (OplogIndex::from_u64(5), original.clone()),
            (OplogIndex::from_u64(12), added.clone()),
        ];
        let lineage = StreamForkLineage::validate(
            vec![
                (OplogIndex::from_u64(10), first.clone()),
                (OplogIndex::from_u64(16), second.clone()),
            ],
            &registrations,
            &target,
            target_fingerprint,
        )
        .unwrap();
        assert_eq!(
            lineage.continuation_handle(&added.handle),
            added_mapping.continuation
        );
        assert_eq!(
            lineage.continuation_handle(&original.handle),
            second.streams[0].continuation
        );
        second.streams.pop();
        assert!(
            StreamForkLineage::validate(
                vec![
                    (OplogIndex::from_u64(10), first),
                    (OplogIndex::from_u64(16), second)
                ],
                &registrations,
                &target,
                target_fingerprint
            )
            .is_err()
        );
    }

    #[test]
    fn lineage_rejects_corrupt_history_and_incomplete_or_forged_mappings() {
        let registration = registration();
        let target = owner("target");
        let fingerprint = AgentFingerprint(Uuid::from_u128(67));
        let cut = fork(&registration.handle, &target, fingerprint, 9);
        for mutation in 0..7 {
            let mut cut = cut.clone();
            let mut registration = registration.clone();
            match mutation {
                0 => registration.handle.stream_id = StreamId(Uuid::from_u128(99)),
                1 => {
                    cut.streams[0].continuation.element_schema_fingerprint =
                        SchemaFingerprintV1([99; 32])
                }
                2 => cut.streams.push(cut.streams[0].clone()),
                3 => {
                    cut.streams.clear();
                    cut.selected_stream_id = None;
                }
                4 => cut.sessions.clear(),
                5 => cut.target_fingerprint = AgentFingerprint(Uuid::from_u128(99)),
                6 => cut.cut_index = OplogIndex::from_u64(4),
                _ => unreachable!(),
            }
            assert!(
                StreamForkLineage::validate(
                    vec![(OplogIndex::from_u64(10), cut)],
                    &[(OplogIndex::from_u64(5), registration)],
                    &target,
                    fingerprint
                )
                .is_err(),
                "mutation {mutation} was accepted"
            );
        }
    }

    #[test]
    fn continuation_identity_is_fresh_even_for_same_agent_and_fingerprint() {
        let registration = registration();
        let owner = owner("source");
        let fingerprint = registration.handle.expected_producer_fingerprint;
        let cut = fork(&registration.handle, &owner, fingerprint, 9);
        assert_ne!(
            cut.streams[0].continuation.stream_id,
            registration.handle.stream_id
        );
        let next_cut = fork(&cut.streams[0].continuation, &owner, fingerprint, 15);
        assert_ne!(
            next_cut.streams[0].continuation.stream_id,
            cut.streams[0].continuation.stream_id
        );
        let lineage = StreamForkLineage::validate(
            vec![
                (OplogIndex::from_u64(10), cut),
                (OplogIndex::from_u64(16), next_cut.clone()),
            ],
            &[(OplogIndex::from_u64(5), registration.clone())],
            &owner,
            fingerprint,
        )
        .unwrap();
        assert_eq!(
            lineage.continuation_handle(&registration.handle),
            next_cut.streams[0].continuation
        );
    }

    #[test]
    fn fork_record_roundtrips_without_changing_handle_or_cut_offsets() {
        let registration = registration();
        let mut cut = fork(
            &registration.handle,
            &owner("target"),
            AgentFingerprint(Uuid::from_u128(67)),
            9,
        );
        cut.retained_through = Some(golem_common::model::durable_stream::StreamOffset::new(
            OplogIndex::from_u64(8),
            3,
        ));
        let record = StreamSessionRecord::ForkCut(cut);
        let bytes = golem_common::serialization::serialize(&record).unwrap();
        let decoded: StreamSessionRecord =
            golem_common::serialization::deserialize(&bytes).unwrap();
        assert_eq!(record, decoded);
        assert!(decoded.has_supported_format());
    }

    #[test]
    fn lineage_rejects_retained_offset_before_selected_stream_registration() {
        let registration = registration();
        let target = owner("target");
        let fingerprint = AgentFingerprint(Uuid::from_u128(67));
        for index in [4, 5, 6] {
            let mut cut = fork(&registration.handle, &target, fingerprint, 9);
            cut.retained_through = Some(golem_common::model::durable_stream::StreamOffset::new(
                OplogIndex::from_u64(index),
                0,
            ));
            assert_eq!(
                StreamForkLineage::validate(
                    vec![(OplogIndex::from_u64(10), cut)],
                    &[(OplogIndex::from_u64(5), registration.clone())],
                    &target,
                    fingerprint,
                )
                .is_ok(),
                index > 5,
                "retained data must follow registration"
            );
        }
    }

    #[test]
    async fn lineage_loader_finds_marker_after_chunk_boundary_without_a_cache() {
        use crate::durable_host::durable_stream::tests::TestOplog;
        use golem_common::model::Timestamp;
        let registration = registration();
        let target = owner("target");
        let fingerprint = AgentFingerprint(Uuid::from_u128(67));
        let cut = fork(&registration.handle, &target, fingerprint, 1024);
        let oplog = TestOplog::default();
        for index in 1..=1025 {
            let timestamp = Timestamp::now_utc();
            let entry = match index {
                5 => OplogEntry::StreamRegistered {
                    timestamp,
                    entity_parent_start_index: None,
                    record: oplog.upload_payload(&registration).await.unwrap(),
                },
                6 => OplogEntry::StreamSession {
                    timestamp,
                    entity_parent_start_index: None,
                    record: oplog
                        .upload_payload(&StreamSessionRecord::Prepared(prepared(
                            &registration.handle.source_invocation,
                        )))
                        .await
                        .unwrap(),
                },
                1025 => OplogEntry::StreamSession {
                    timestamp,
                    entity_parent_start_index: None,
                    record: oplog
                        .upload_payload(&StreamSessionRecord::ForkCut(cut.clone()))
                        .await
                        .unwrap(),
                },
                _ => OplogEntry::NoOp {
                    timestamp,
                    entity_parent_start_index: None,
                },
            };
            oplog.add(entry).await;
        }
        for _ in 0..2 {
            let lineage = StreamForkLineage::load(&oplog, &target, fingerprint)
                .await
                .unwrap();
            assert_eq!(
                lineage.continuation_handle(&registration.handle),
                cut.streams[0].continuation
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
        use golem_common::model::Timestamp;
        use golem_common::model::durable_stream::StreamSessionFinishedRecord;
        for (selected, finished_before) in [(true, true), (false, true), (true, false)] {
            let registration = registration();
            let target = owner("target");
            let fingerprint = AgentFingerprint(Uuid::from_u128(67));
            let mut cut = fork(&registration.handle, &target, fingerprint, 7);
            if !selected {
                cut.selected_stream_id = None;
            }
            let oplog = TestOplog::default();
            for index in 1..=9 {
                let timestamp = Timestamp::now_utc();
                let record = match index {
                    6 => Some(StreamSessionRecord::Prepared(prepared(
                        &registration.handle.source_invocation,
                    ))),
                    7 if finished_before => {
                        Some(StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                            format_version: 1,
                            session_key: registration.handle.source_invocation.clone(),
                            result: Ok(()),
                        }))
                    }
                    8 => Some(StreamSessionRecord::ForkCut(cut.clone())),
                    9 if !finished_before => {
                        Some(StreamSessionRecord::Finished(StreamSessionFinishedRecord {
                            format_version: 1,
                            session_key: cut.sessions[0].continuation.clone(),
                            result: Ok(()),
                        }))
                    }
                    _ => None,
                };
                let entry = if index == 5 {
                    OplogEntry::StreamRegistered {
                        timestamp,
                        entity_parent_start_index: None,
                        record: oplog.upload_payload(&registration).await.unwrap(),
                    }
                } else if let Some(record) = record {
                    OplogEntry::StreamSession {
                        timestamp,
                        entity_parent_start_index: None,
                        record: oplog.upload_payload(&record).await.unwrap(),
                    }
                } else {
                    OplogEntry::NoOp {
                        timestamp,
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
        let source = owner("source");
        let source_fingerprint = registration.handle.expected_producer_fingerprint;
        let target = owner("target");
        let target_fingerprint = AgentFingerprint(Uuid::from_u128(67));
        let cut = fork(&registration.handle, &target, target_fingerprint, 1024);
        let fixture = ServiceFixture::new(&target, target_fingerprint).await;

        for index in 2..=1025 {
            let timestamp = Timestamp::now_utc();
            let entry = match index {
                5 => OplogEntry::StreamRegistered {
                    timestamp,
                    entity_parent_start_index: None,
                    record: fixture.oplog.upload_payload(&registration).await.unwrap(),
                },
                6 => OplogEntry::StreamSession {
                    timestamp,
                    entity_parent_start_index: None,
                    record: fixture
                        .oplog
                        .upload_payload(&StreamSessionRecord::Prepared(prepared(
                            &registration.handle.source_invocation,
                        )))
                        .await
                        .unwrap(),
                },
                1025 => OplogEntry::StreamSession {
                    timestamp,
                    entity_parent_start_index: None,
                    record: fixture
                        .oplog
                        .upload_payload(&StreamSessionRecord::ForkCut(cut.clone()))
                        .await
                        .unwrap(),
                },
                _ => OplogEntry::NoOp {
                    timestamp,
                    entity_parent_start_index: None,
                },
            };
            assert_eq!(fixture.oplog.add(entry).await, OplogIndex::from_u64(index));
        }
        fixture.oplog.commit(CommitLevel::Always).await;

        let from_oplog = StreamForkLineage::load(&*fixture.oplog, &target, target_fingerprint)
            .await
            .unwrap();
        fixture.blobs.reset();
        let from_service = StreamForkLineage::load_from_service(
            &*fixture.service,
            &target,
            AgentMode::Durable,
            target_fingerprint,
            OplogIndex::from_u64(1025),
        )
        .await
        .unwrap();
        assert_eq!(from_service.cuts(), from_oplog.cuts());
        assert_eq!(
            from_service.continuation_handle(&registration.handle),
            cut.streams[0].continuation
        );
        assert!(
            fixture.blobs.reads() >= 3,
            "external payloads were not downloaded"
        );

        let source_fixture = ServiceFixture::new(&source, source_fingerprint).await;
        for index in 2..=1024 {
            let timestamp = Timestamp::now_utc();
            let entry = match index {
                5 => OplogEntry::StreamRegistered {
                    timestamp,
                    entity_parent_start_index: None,
                    record: source_fixture
                        .oplog
                        .upload_payload(&registration)
                        .await
                        .unwrap(),
                },
                6 => OplogEntry::StreamSession {
                    timestamp,
                    entity_parent_start_index: None,
                    record: source_fixture
                        .oplog
                        .upload_payload(&StreamSessionRecord::Prepared(prepared(
                            &registration.handle.source_invocation,
                        )))
                        .await
                        .unwrap(),
                },
                _ => OplogEntry::NoOp {
                    timestamp,
                    entity_parent_start_index: None,
                },
            };
            source_fixture.oplog.add(entry).await;
        }
        source_fixture
            .oplog
            .add(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: source_fixture
                    .oplog
                    .upload_payload(&StreamSessionRecord::ForkCut(cut.clone()))
                    .await
                    .unwrap(),
            })
            .await;
        source_fixture.oplog.commit(CommitLevel::Always).await;
        let shorter = StreamForkLineage::load_from_service(
            &*source_fixture.service,
            &source,
            AgentMode::Durable,
            source_fingerprint,
            OplogIndex::from_u64(1024),
        )
        .await
        .unwrap();
        assert!(shorter.cuts().is_empty());
    }

    #[test]
    async fn suffix_discovery_is_fixed_to_its_horizon_and_finds_a_later_marker() {
        let target = owner("target");
        let fingerprint = AgentFingerprint(Uuid::from_u128(67));
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

        let marker = StreamSessionRecord::ForkCut(StreamForkCutRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            request_hash: vec![0; 32],
            export: None,
            source_environment_id: target.environment_id,
            source: target.agent_id.clone(),
            source_fingerprint: fingerprint,
            target_environment_id: target.environment_id,
            target: target.agent_id.clone(),
            target_fingerprint: fingerprint,
            cut_index: OplogIndex::from_u64(1024),
            revert: None,
            epoch_floor: 1,
            selected_stream_id: None,
            retained_through: None,
            streams: vec![],
            sessions: vec![],
        });
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
            .unwrap(),
            "a fixed discovery horizon must not observe a later marker"
        );
    }

    #[test]
    #[should_panic(expected = "missing oplog entries in range [2..=2]")]
    async fn service_lineage_loader_rejects_a_missing_requested_range() {
        let source = owner("source");
        let fingerprint = AgentFingerprint(Uuid::from_u128(37));
        let fixture = ServiceFixture::new(&source, fingerprint).await;
        let _ = StreamForkLineage::load_from_service(
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
    fn session_lineage_requires_retained_preparation_not_registered_streams() {
        let registration = registration();
        let target = owner("target");
        let fingerprint = AgentFingerprint(Uuid::from_u128(67));
        let mut cut = fork(&registration.handle, &target, fingerprint, 9);
        cut.selected_stream_id = None;
        cut.streams.clear();
        let lineage = StreamForkLineage::validate(
            vec![(OplogIndex::from_u64(10), cut.clone())],
            &[],
            &target,
            fingerprint,
        )
        .unwrap();
        let record = prepared(&cut.sessions[0].source);
        assert!(
            lineage
                .validate_session_history(
                    &[(OplogIndex::from_u64(6), record.clone())],
                    &target,
                    fingerprint
                )
                .is_ok()
        );
        assert!(
            lineage
                .validate_session_history(&[], &target, fingerprint)
                .is_err()
        );
        assert!(
            lineage
                .validate_session_history(
                    &[(OplogIndex::from_u64(11), record)],
                    &target,
                    fingerprint
                )
                .is_err()
        );
        let mut unrelated = cut.sessions[0].source.clone();
        unrelated.idempotency_key = IdempotencyKey::new("another-session".into());
        assert!(
            lineage
                .validate_session_history(
                    &[(OplogIndex::from_u64(6), prepared(&unrelated))],
                    &target,
                    fingerprint
                )
                .is_err()
        );
    }

    #[test]
    fn prepared_session_provenance_survives_multiple_forks() {
        let registration = registration();
        let middle = owner("middle");
        let target = owner("target");
        let fingerprint = AgentFingerprint(Uuid::from_u128(67));
        let first = fork(
            &registration.handle,
            &middle,
            AgentFingerprint(Uuid::from_u128(53)),
            9,
        );
        let second = fork(&first.streams[0].continuation, &target, fingerprint, 15);
        let lineage = StreamForkLineage::validate(
            vec![
                (OplogIndex::from_u64(10), first),
                (OplogIndex::from_u64(16), second),
            ],
            &[(OplogIndex::from_u64(5), registration.clone())],
            &target,
            fingerprint,
        )
        .unwrap();
        assert!(
            lineage
                .validate_session_history(
                    &[(
                        OplogIndex::from_u64(6),
                        prepared(&registration.handle.source_invocation)
                    )],
                    &target,
                    fingerprint
                )
                .is_ok()
        );
    }
}
