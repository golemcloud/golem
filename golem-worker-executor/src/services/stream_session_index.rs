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

use crate::durable_host::durable_session::SessionControlMetadata;
use crate::durable_host::durable_stream::metadata::{
    ProducerMetadataKey, ProducerMetadataRow, project_producer_metadata,
};
use crate::services::oplog::{OplogService, OplogServiceOps};
use crate::services::worker::DurableStreamRecoveryMetadata;
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use golem_common::base_model::durable_stream::{
    StreamId, StreamSessionKeyV1, StreamSessionRecordV1,
};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload};
use golem_common::model::{
    AgentFingerprint, DurableStreamSessionStatus, IdempotencyKey, OwnedAgentId,
};
use golem_common::serialization::{deserialize, serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, Weak};
use tokio::sync::Mutex as AsyncMutex;

pub(super) const METADATA_FIELD: &str = "coverage";
const SESSION_FIELD_PREFIX: &str = "session:";
pub(crate) const CONSUMER_JOURNAL_INDEX_PAGE_SIZE: u64 = 256;
const RECOVERY_CATALOGUE_PAGE_SIZE: u64 = 256;

fn recovery_catalogue_field(page: u64) -> String {
    format!("recovery:{page}")
}

fn stream_resume_index_field(
    key: &StreamSessionKeyV1,
    attempt: golem_common::model::durable_stream::AttemptId,
) -> Result<String, String> {
    Ok(format!(
        "resume:{}",
        hex::encode(serialize(&(key, attempt))?)
    ))
}

fn stream_control_index_field(key: &StreamSessionKeyV1) -> Result<String, String> {
    Ok(format!("control:{}", hex::encode(serialize(key)?)))
}

fn consumer_journal_index_field(
    key: &StreamSessionKeyV1,
    stream: StreamId,
    page: u64,
) -> Result<String, String> {
    Ok(format!(
        "journal:{}:{}:{page}",
        hex::encode(serialize(key)?),
        stream.0
    ))
}

#[derive(Clone, Debug, Default, desert_rust::BinaryCodec)]
pub(super) struct Metadata {
    pub(super) covered_through: OplogIndex,
    pub(super) recovery_session_count: u64,
    pub(super) producer_fingerprint: Option<AgentFingerprint>,
}

#[derive(Clone, Debug)]
pub struct StreamSessionIndexService {
    kv: Arc<dyn KeyValueStorage + Send + Sync>,
    oplog: Weak<dyn OplogService>,
    locks: Arc<StdMutex<HashMap<OwnedAgentId, Weak<AsyncMutex<()>>>>>,
}

struct IndexLock {
    registry: Arc<StdMutex<HashMap<OwnedAgentId, Weak<AsyncMutex<()>>>>>,
    id: OwnedAgentId,
    inner: Arc<AsyncMutex<()>>,
}

#[derive(Clone, Copy)]
enum SessionLookup {
    Exact(OplogIndex),
    Offsets(OplogIndex),
    Latest,
}

impl Drop for IndexLock {
    fn drop(&mut self) {
        let mut locks = self.registry.lock().unwrap();
        if Arc::strong_count(&self.inner) == 1
            && locks
                .get(&self.id)
                .is_some_and(|entry| entry.ptr_eq(&Arc::downgrade(&self.inner)))
        {
            locks.remove(&self.id);
        }
    }
}

impl StreamSessionIndexService {
    pub async fn lookup_producer_metadata(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        keys: Vec<ProducerMetadataKey>,
    ) -> Result<(OplogIndex, Vec<Option<ProducerMetadataRow>>), String> {
        let this = self.clone();
        let id = id.clone();
        tokio::spawn(async move {
            let oplog = this.oplog.upgrade().ok_or("oplog service is unavailable")?;
            let horizon = oplog.get_last_index(&id, mode).await;
            this.catch_up_inner(&id, mode, horizon).await?;
            let mut fields = vec![METADATA_FIELD.to_string()];
            for key in keys {
                fields.push(key.field()?);
            }
            let values = this
                .kv
                .with_entity("stream_session_index", "lookup_producer", "metadata")
                .get_many_raw(Self::namespace(&id), fields)
                .await?;
            let covered = values
                .first()
                .and_then(Option::as_ref)
                .map(|bytes| deserialize::<Metadata>(bytes))
                .transpose()?
                .map_or(OplogIndex::NONE, |metadata| metadata.covered_through);
            if covered < horizon {
                return Err("producer metadata coverage is unavailable".into());
            }
            let rows = values
                .into_iter()
                .skip(1)
                .map(|value| value.map(|bytes| deserialize(&bytes)).transpose())
                .collect::<Result<Vec<_>, String>>()?;
            Ok((covered, rows))
        })
        .await
        .map_err(|error| format!("producer metadata lookup task failed: {error}"))?
    }

    pub async fn lookup_resume_offset(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        key: &StreamSessionKeyV1,
        attempt: golem_common::model::durable_stream::AttemptId,
    ) -> Result<Option<OplogIndex>, String> {
        let this = self.clone();
        let id = id.clone();
        let key = key.clone();
        tokio::spawn(async move {
            let oplog = this.oplog.upgrade().ok_or("oplog service is unavailable")?;
            let horizon = oplog.get_last_index(&id, mode).await;
            this.catch_up_inner(&id, mode, horizon).await?;
            let offset: Option<OplogIndex> = this
                .kv
                .with_entity("stream_session_index", "lookup_resume", "attempt")
                .get(
                    Self::namespace(&id),
                    &stream_resume_index_field(&key, attempt)?,
                )
                .await?;
            Ok(offset.filter(|index| *index <= horizon))
        })
        .await
        .map_err(|error| format!("durable resume lookup task failed: {error}"))?
    }

    pub async fn lookup_recovery_metadata(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
    ) -> Result<DurableStreamRecoveryMetadata, String> {
        let this = self.clone();
        let id = id.clone();
        tokio::spawn(async move {
            let oplog = this.oplog.upgrade().ok_or("oplog service is unavailable")?;
            let horizon = oplog.get_last_index(&id, mode).await;
            this.catch_up_inner(&id, mode, horizon).await?;
            let namespace = Self::namespace(&id);
            let mut requested_pages = 0;
            let (covered_through, keys, consumer_deleting) = loop {
                let mut names = vec![METADATA_FIELD.into(), "consumer-deleting".into()];
                names.extend((0..requested_pages).map(recovery_catalogue_field));
                let fields = this
                    .kv
                    .with_entity("stream_session_index", "read_recovery", "page")
                    .get_many_raw(namespace.clone(), names)
                    .await?;
                let metadata: Metadata = fields[0]
                    .as_ref()
                    .map(|bytes| deserialize(bytes))
                    .transpose()?
                    .unwrap_or_default();
                if metadata.covered_through < horizon {
                    return Err("recovery catalogue coverage is unavailable".into());
                }
                let needed_pages = metadata
                    .recovery_session_count
                    .div_ceil(RECOVERY_CATALOGUE_PAGE_SIZE);
                if needed_pages > requested_pages {
                    requested_pages = needed_pages;
                    continue;
                }
                let mut keys = Vec::new();
                for page in 0..needed_pages {
                    let entries: Vec<StreamSessionKeyV1> = deserialize(
                        fields[page as usize + 2]
                            .as_ref()
                            .ok_or("recovery catalogue page is missing")?,
                    )?;
                    let needed = (metadata.recovery_session_count
                        - page * RECOVERY_CATALOGUE_PAGE_SIZE)
                        .min(RECOVERY_CATALOGUE_PAGE_SIZE);
                    if entries.len() as u64 != needed {
                        return Err("recovery catalogue page does not match its coverage".into());
                    }
                    keys.extend(entries);
                }
                let deleting = fields[1]
                    .as_ref()
                    .map(|bytes| deserialize(bytes))
                    .transpose()?;
                break (metadata.covered_through, keys, deleting);
            };
            let mut sessions = Vec::with_capacity(keys.len());
            for keys in keys.chunks(RECOVERY_CATALOGUE_PAGE_SIZE as usize) {
                let mut fields = vec![METADATA_FIELD.into()];
                fields.extend(
                    keys.iter()
                        .map(stream_control_index_field)
                        .collect::<Result<Vec<_>, _>>()?,
                );
                let values = this
                    .kv
                    .with_entity("stream_session_index", "read_recovery", "session")
                    .get_many_raw(namespace.clone(), fields)
                    .await?;
                let coverage: Metadata = deserialize(
                    values[0]
                        .as_ref()
                        .ok_or("recovery catalogue coverage is missing")?,
                )?;
                for (key, value) in keys.iter().zip(values.iter().skip(1)) {
                    let mut control: SessionControlMetadata = deserialize(
                        value
                            .as_ref()
                            .ok_or("recovery catalogue session is missing")?,
                    )?;
                    control.covered_through = coverage.covered_through;
                    sessions.push((key.clone(), control));
                }
            }
            Ok(DurableStreamRecoveryMetadata {
                covered_through,
                sessions,
                consumer_deleting,
            })
        })
        .await
        .map_err(|error| format!("durable recovery metadata task failed: {error}"))?
    }

    async fn recovery_page<'a>(
        &self,
        namespace: &KeyValueStorageNamespace,
        pages: &'a mut HashMap<u64, Vec<StreamSessionKeyV1>>,
        page: u64,
    ) -> Result<&'a mut Vec<StreamSessionKeyV1>, String> {
        if let std::collections::hash_map::Entry::Vacant(e) = pages.entry(page) {
            let keys = self
                .kv
                .with_entity("stream_session_index", "read_recovery", "page")
                .get(namespace.clone(), &recovery_catalogue_field(page))
                .await?
                .unwrap_or_default();
            e.insert(keys);
        }
        Ok(pages.get_mut(&page).unwrap())
    }

    async fn update_recovery_catalogue(
        &self,
        id: &OwnedAgentId,
        namespace: &KeyValueStorageNamespace,
        controls: &mut HashMap<StreamSessionKeyV1, SessionControlMetadata>,
        metadata: &mut Metadata,
    ) -> Result<HashMap<u64, Vec<StreamSessionKeyV1>>, String> {
        let mut pages = HashMap::new();
        let keys: Vec<_> = controls.keys().cloned().collect();
        for key in keys {
            let control = controls.get(&key).unwrap();
            match (
                control.recovery_slot,
                control.needs_topology_recovery(id, &key),
            ) {
                (None, true) => {
                    let slot = metadata.recovery_session_count;
                    let page = self
                        .recovery_page(namespace, &mut pages, slot / RECOVERY_CATALOGUE_PAGE_SIZE)
                        .await?;
                    if page.len() as u64 != slot % RECOVERY_CATALOGUE_PAGE_SIZE {
                        return Err("recovery catalogue page does not match its coverage".into());
                    }
                    page.push(key.clone());
                    controls.get_mut(&key).unwrap().recovery_slot = Some(slot);
                    metadata.recovery_session_count =
                        slot.checked_add(1).ok_or("recovery catalogue overflow")?;
                }
                (Some(slot), false) => {
                    let last = metadata
                        .recovery_session_count
                        .checked_sub(1)
                        .ok_or("empty recovery catalogue has a session slot")?;
                    let moved = self
                        .recovery_page(namespace, &mut pages, last / RECOVERY_CATALOGUE_PAGE_SIZE)
                        .await?
                        .pop()
                        .ok_or("recovery catalogue tail is missing")?;
                    if slot != last {
                        let page = self
                            .recovery_page(
                                namespace,
                                &mut pages,
                                slot / RECOVERY_CATALOGUE_PAGE_SIZE,
                            )
                            .await?;
                        let entry = page
                            .get_mut((slot % RECOVERY_CATALOGUE_PAGE_SIZE) as usize)
                            .ok_or("recovery catalogue session slot is missing")?;
                        if entry != &key {
                            return Err(
                                "recovery catalogue session slot identifies another session".into(),
                            );
                        }
                        *entry = moved.clone();
                        if !controls.contains_key(&moved) {
                            let control = self
                                .kv
                                .with_entity("stream_session_index", "read_recovery", "session")
                                .get(namespace.clone(), &stream_control_index_field(&moved)?)
                                .await?
                                .ok_or("recovery catalogue refers to a missing session")?;
                            controls.insert(moved.clone(), control);
                        }
                        controls.get_mut(&moved).unwrap().recovery_slot = Some(slot);
                    } else if moved != key {
                        return Err("recovery catalogue tail identifies another session".into());
                    }
                    controls.get_mut(&key).unwrap().recovery_slot = None;
                    metadata.recovery_session_count = last;
                }
                _ => {}
            }
        }
        Ok(pages)
    }

    pub async fn read_consumer_page(
        &self,
        id: &OwnedAgentId,
        key: &StreamSessionKeyV1,
        stream: StreamId,
        page: u64,
    ) -> Result<Vec<OplogIndex>, String> {
        self.kv
            .with_entity("stream_session_index", "read_consumer_journal", "page")
            .get(
                Self::namespace(id),
                &consumer_journal_index_field(key, stream, page)?,
            )
            .await?
            .ok_or_else(|| "consumer journal index page is missing".into())
    }

    pub async fn lookup_control_metadata(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        key: &StreamSessionKeyV1,
    ) -> Result<SessionControlMetadata, String> {
        let this = self.clone();
        let id = id.clone();
        let key = key.clone();
        tokio::spawn(async move {
            let oplog = this
                .oplog
                .upgrade()
                .ok_or_else(|| "oplog service is unavailable".to_string())?;
            let horizon = oplog.get_last_index(&id, mode).await;
            this.catch_up_inner(&id, mode, horizon).await?;
            let lock = this.index_lock(&id);
            let _guard = lock.inner.lock().await;
            let fields = this
                .kv
                .with_entity("stream_session_index", "lookup_control", "session")
                .get_many_raw(
                    Self::namespace(&id),
                    vec![
                        METADATA_FIELD.into(),
                        stream_control_index_field(&key)?,
                        "consumer-deleting".into(),
                    ],
                )
                .await?;
            let coverage = fields
                .first()
                .and_then(Option::as_ref)
                .map(|bytes| deserialize::<Metadata>(bytes))
                .transpose()?
                .map_or(OplogIndex::NONE, |value| value.covered_through);
            if coverage < horizon {
                return Err("durable stream control metadata coverage is unavailable".into());
            }
            let mut snapshot = fields
                .get(1)
                .and_then(Option::as_ref)
                .map(|bytes| deserialize::<SessionControlMetadata>(bytes))
                .transpose()?
                .unwrap_or_default();
            snapshot.covered_through = coverage;
            snapshot.consumer_deleting = fields
                .get(2)
                .and_then(Option::as_ref)
                .map(|bytes| deserialize(bytes))
                .transpose()?;
            Ok(snapshot)
        })
        .await
        .map_err(|error| format!("durable stream metadata lookup task failed: {error}"))?
    }

    pub fn new(kv: Arc<dyn KeyValueStorage + Send + Sync>, oplog: Weak<dyn OplogService>) -> Self {
        Self {
            kv,
            oplog,
            locks: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    pub async fn catch_up(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        horizon: OplogIndex,
    ) -> Result<(), String> {
        let this = self.clone();
        let id = id.clone();
        tokio::spawn(async move { this.catch_up_inner(&id, mode, horizon).await })
            .await
            .map_err(|err| format!("stream session index task failed: {err}"))?
    }

    pub async fn lookup_persisted(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        horizon: OplogIndex,
        key: &IdempotencyKey,
    ) -> Result<Option<DurableStreamSessionStatus>, String> {
        let this = self.clone();
        let id = id.clone();
        let key = key.clone();
        tokio::spawn(async move {
            this.lookup_inner(&id, mode, &key, SessionLookup::Exact(horizon))
                .await
        })
        .await
        .map_err(|err| format!("stream session index task failed: {err}"))?
    }

    /// Looks up only the four lifecycle oplog offsets. If storage has advanced beyond `horizon`, all
    /// attachment identity fields are discarded because they cannot be rewound from the index.
    pub async fn lookup_persisted_offsets(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        horizon: OplogIndex,
        key: &IdempotencyKey,
    ) -> Result<Option<DurableStreamSessionStatus>, String> {
        let this = self.clone();
        let id = id.clone();
        let key = key.clone();
        tokio::spawn(async move {
            this.lookup_inner(&id, mode, &key, SessionLookup::Offsets(horizon))
                .await
        })
        .await
        .map_err(|err| format!("stream session index task failed: {err}"))?
    }

    pub async fn lookup_latest(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        key: &IdempotencyKey,
    ) -> Result<Option<DurableStreamSessionStatus>, String> {
        let this = self.clone();
        let id = id.clone();
        let key = key.clone();
        tokio::spawn(async move {
            this.lookup_inner(&id, mode, &key, SessionLookup::Latest)
                .await
        })
        .await
        .map_err(|err| format!("stream session index task failed: {err}"))?
    }

    pub async fn clear(&self, id: &OwnedAgentId) -> Result<(), String> {
        let this = self.clone();
        let id = id.clone();
        tokio::spawn(async move {
            let lock = this.index_lock(&id);
            let _guard = lock.inner.lock().await;
            this.clear_inner(&id).await
        })
        .await
        .map_err(|err| format!("stream session index task failed: {err}"))?
    }

    pub(super) fn namespace(id: &OwnedAgentId) -> KeyValueStorageNamespace {
        KeyValueStorageNamespace::AgentDurableStreamSessionIndex {
            agent_id: id.agent_id.clone(),
        }
    }

    pub(super) fn field(key: &IdempotencyKey) -> String {
        format!("{SESSION_FIELD_PREFIX}{}", key.value)
    }

    fn index_lock(&self, id: &OwnedAgentId) -> IndexLock {
        let mut locks = self.locks.lock().unwrap();
        let inner = locks.get(id).and_then(Weak::upgrade).unwrap_or_else(|| {
            let lock = Arc::new(AsyncMutex::new(()));
            locks.insert(id.clone(), Arc::downgrade(&lock));
            lock
        });
        IndexLock {
            registry: self.locks.clone(),
            id: id.clone(),
            inner,
        }
    }

    async fn clear_inner(&self, id: &OwnedAgentId) -> Result<(), String> {
        let namespace = Self::namespace(id);
        let keys = self
            .kv
            .with("stream_session_index", "clear")
            .keys(namespace.clone())
            .await?;
        if !keys.is_empty() {
            self.kv
                .with("stream_session_index", "clear")
                .del_many(namespace, keys)
                .await?;
        }
        Ok(())
    }

    async fn catch_up_inner(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        horizon: OplogIndex,
    ) -> Result<(), String> {
        let lock = self.index_lock(id);
        let _guard = lock.inner.lock().await;
        let namespace = Self::namespace(id);
        let oplog = self
            .oplog
            .upgrade()
            .ok_or_else(|| "oplog service is unavailable".to_string())?;
        loop {
            let expected = self
                .kv
                .with_entity("stream_session_index", "read", "metadata")
                .get_raw(namespace.clone(), METADATA_FIELD)
                .await?;
            let mut metadata = expected
                .as_ref()
                .map(|bytes| deserialize::<Metadata>(bytes))
                .transpose()?
                .unwrap_or(Metadata {
                    covered_through: OplogIndex::NONE,
                    ..Default::default()
                });
            if metadata.covered_through >= horizon {
                return Ok(());
            }
            let count = (horizon.as_u64() - metadata.covered_through.as_u64()).min(1024);
            let mut entries = oplog
                .read_exact(id, mode, metadata.covered_through.next(), count)
                .await;
            if entries.is_empty() {
                return Err(format!(
                    "empty oplog range while indexing stream sessions for {id}"
                ));
            }
            while let Some((last, OplogEntry::StreamRegistered { record, .. })) =
                entries.last_key_value()
            {
                let record = oplog.download_payload(id, mode, record.clone()).await?;
                if !matches!(
                    record.coordinate,
                    golem_common::model::durable_stream::StreamRegistrationCoordinateV1::Nested { .. }
                ) || *last >= horizon
                {
                    break;
                }
                let next = oplog.read_exact(id, mode, last.next(), 1).await;
                if next.is_empty() {
                    return Err(
                        "missing enclosing item while extending producer metadata chunk".into(),
                    );
                }
                entries.extend(next);
            }
            if metadata.producer_fingerprint.is_none() {
                metadata.producer_fingerprint = entries.values().find_map(|entry| match entry {
                    OplogEntry::Create { instance_id, .. } => Some(AgentFingerprint(*instance_id)),
                    _ => None,
                });
            }
            let fields = async {
                let producer_fields = if let Some(fingerprint) = metadata.producer_fingerprint {
                    project_producer_metadata(
                        self.kv.as_ref(),
                        namespace.clone(),
                        oplog.as_ref(),
                        id,
                        mode,
                        fingerprint,
                        &entries,
                    )
                    .await?
                } else {
                    Vec::new()
                };
                let mut updates = HashMap::<IdempotencyKey, DurableStreamSessionStatus>::new();
                let mut controls = HashMap::<StreamSessionKeyV1, SessionControlMetadata>::new();
                let mut journal_pages = HashMap::<String, Vec<OplogIndex>>::new();
                let mut resume_offsets = HashMap::<String, OplogIndex>::new();
                let mut consumer_deleting = None;
                for (idx, entry) in &entries {
                    let OplogEntry::StreamSession { record, .. } = entry else {
                        continue;
                    };
                    let decoded;
                    let record = match record {
                        OplogPayload::Inline(value) => value.as_ref(),
                        OplogPayload::SerializedInline {
                            cached: Some(value),
                            ..
                        }
                        | OplogPayload::External {
                            cached: Some(value),
                            ..
                        } => value.as_ref(),
                        OplogPayload::SerializedInline {
                            bytes,
                            cached: None,
                        } => {
                            decoded = deserialize(bytes)?;
                            &decoded
                        }
                        OplogPayload::External { cached: None, .. } => {
                            unreachable!("stream session records are inline")
                        }
                    };
                    if let StreamSessionRecordV1::ConsumerDeleting(record) = record {
                        consumer_deleting = Some(record.clone());
                    }
                    if let StreamSessionRecordV1::ResumeAttempt(record) = record {
                        let field = stream_resume_index_field(
                            &record.attempt.session_key,
                            record.attempt.attempt_id,
                        )?;
                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            resume_offsets.entry(field)
                        {
                            let old: Option<OplogIndex> = self
                                .kv
                                .with_entity("stream_session_index", "read_resume", "attempt")
                                .get(namespace.clone(), entry.key())
                                .await?;
                            entry.insert(old.unwrap_or(*idx));
                        }
                    }
                    if let Some(key) = crate::worker::stream_session_record_key(record) {
                        if !controls.contains_key(key) {
                            let old: Option<SessionControlMetadata> = self
                                .kv
                                .with_entity("stream_session_index", "read_control", "session")
                                .get(namespace.clone(), &stream_control_index_field(key)?)
                                .await?;
                            controls.insert(key.clone(), old.unwrap_or_default());
                        }
                        let control = controls.get_mut(key).unwrap();
                        let stream = match record {
                            StreamSessionRecordV1::ConsumerItemValue(record) => {
                                Some(record.stream_id)
                            }
                            StreamSessionRecordV1::ConsumerTerminal(record) => {
                                Some(record.stream_id)
                            }
                            StreamSessionRecordV1::SourceUnavailable(record) => {
                                Some(record.key.stream_id)
                            }
                            _ => None,
                        };
                        if let Some(stream) = stream {
                            let count = control
                                .consumer_record_counts
                                .get(&stream)
                                .copied()
                                .unwrap_or_default();
                            let field = consumer_journal_index_field(
                                key,
                                stream,
                                count / CONSUMER_JOURNAL_INDEX_PAGE_SIZE,
                            )?;
                            if !journal_pages.contains_key(&field) {
                                let old: Option<Vec<OplogIndex>> = self
                                    .kv
                                    .with_entity(
                                        "stream_session_index",
                                        "read_consumer_journal",
                                        "page",
                                    )
                                    .get(namespace.clone(), &field)
                                    .await?;
                                journal_pages.insert(field.clone(), old.unwrap_or_default());
                            }
                            let page = journal_pages.get_mut(&field).unwrap();
                            if page.len() as u64 != count % CONSUMER_JOURNAL_INDEX_PAGE_SIZE {
                                return Err(
                                    "consumer journal index page does not match its coverage"
                                        .into(),
                                );
                            }
                            page.push(*idx);
                        }
                        control.apply(*idx, key, record);
                    }
                    let Some(key) = record_key(record).cloned() else {
                        continue;
                    };
                    if !updates.contains_key(&key) {
                        let old: Option<Result<DurableStreamSessionStatus, String>> = self
                            .kv
                            .with_entity("stream_session_index", "read", "session")
                            .get_attempt_deserialize(namespace.clone(), &Self::field(&key))
                            .await?;
                        updates.insert(key.clone(), old.transpose()?.unwrap_or_default());
                    }
                    let status = updates.get_mut(&key).unwrap();
                    if status.first_prepared.is_some()
                        || matches!(record, StreamSessionRecordV1::Prepared(_))
                    {
                        status.apply_record(*idx, record);
                    }
                }
                let recovery_pages = self
                    .update_recovery_catalogue(id, &namespace, &mut controls, &mut metadata)
                    .await?;
                metadata.covered_through = *entries.keys().max().unwrap();
                let mut fields: Vec<(String, Vec<u8>)> = updates
                    .into_iter()
                    .filter(|(_, value)| value.first_prepared.is_some())
                    .map(|(key, value)| Ok((Self::field(&key), serialize(&value)?)))
                    .collect::<Result<_, String>>()?;
                fields.extend(producer_fields);
                for (key, value) in controls {
                    fields.push((stream_control_index_field(&key)?, serialize(&value)?));
                }
                for (field, page) in journal_pages {
                    fields.push((field, serialize(&page)?));
                }
                for (field, offset) in resume_offsets {
                    fields.push((field, serialize(&offset)?));
                }
                for (page, keys) in recovery_pages {
                    fields.push((recovery_catalogue_field(page), serialize(&keys)?));
                }
                if let Some(record) = consumer_deleting {
                    fields.push(("consumer-deleting".into(), serialize(&record)?));
                }
                fields.push((METADATA_FIELD.into(), serialize(&metadata)?));
                Ok::<_, String>(fields)
            }
            .await;
            let fields = match fields {
                Ok(fields) => fields,
                Err(error) => {
                    // Separate row reads may straddle another executor's atomic update.
                    let current = self
                        .kv
                        .with_entity("stream_session_index", "read", "metadata")
                        .get_raw(namespace.clone(), METADATA_FIELD)
                        .await?;
                    if current == expected {
                        return Err(error);
                    }
                    continue;
                }
            };
            let refs: Vec<_> = fields
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_slice()))
                .collect();
            self.kv
                .with_entity("stream_session_index", "advance", "session")
                .compare_and_set_many_raw(
                    namespace.clone(),
                    METADATA_FIELD,
                    expected.as_deref(),
                    &refs,
                )
                .await?;
            // Reload after either winning the CAS or observing another executor's progress.
        }
    }

    async fn lookup_inner(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        key: &IdempotencyKey,
        lookup: SessionLookup,
    ) -> Result<Option<DurableStreamSessionStatus>, String> {
        let horizon = match lookup {
            SessionLookup::Exact(horizon) | SessionLookup::Offsets(horizon) => horizon,
            SessionLookup::Latest => {
                self.oplog
                    .upgrade()
                    .ok_or_else(|| "oplog service is unavailable".to_string())?
                    .get_last_index(id, mode)
                    .await
            }
        };
        self.catch_up_inner(id, mode, horizon).await?;
        let lock = self.index_lock(id);
        let _guard = lock.inner.lock().await;
        let values = self
            .kv
            .with_entity("stream_session_index", "lookup", "session")
            .get_many_raw(
                Self::namespace(id),
                vec![METADATA_FIELD.into(), Self::field(key)],
            )
            .await?;
        let Some(metadata) = values.first().and_then(Option::as_ref) else {
            return Err("stream session index coverage is unavailable after catch-up".into());
        };
        let metadata: Metadata = deserialize(metadata)?;
        if metadata.covered_through < horizon {
            return Err("stream session index coverage regressed after catch-up".into());
        }
        if metadata.covered_through > horizon && matches!(lookup, SessionLookup::Exact(_)) {
            return Err("stream session index is newer than the requested horizon".into());
        }
        let Some(value) = values.get(1).and_then(Option::as_ref) else {
            return Ok(None);
        };
        let mut value: DurableStreamSessionStatus = deserialize(value)?;
        if matches!(lookup, SessionLookup::Offsets(_)) {
            for field in [
                &mut value.first_prepared,
                &mut value.prepared,
                &mut value.invocation_result,
                &mut value.finished,
            ] {
                if field.is_some_and(|idx| idx > horizon) {
                    *field = None;
                }
            }
            if metadata.covered_through > horizon {
                value.session_key = None;
                value.prepared_attempt_id = None;
                value.initial_attachment_epoch = None;
                value.initial_attachment_attempt_id = None;
                value.initial_pending_invocation_oplog_index = None;
                value.attachment_epoch = None;
                value.attachment_attempt_id = None;
                value.attachment_attached = None;
                value.lifecycle_error = None;
            }
        }
        Ok(value.first_prepared.map(|_| value))
    }
}

fn record_key(record: &StreamSessionRecordV1) -> Option<&IdempotencyKey> {
    match record {
        StreamSessionRecordV1::Prepared(value) => Some(&value.attempt.session_key.idempotency_key),
        StreamSessionRecordV1::Attached(value) => Some(&value.session_key.idempotency_key),
        StreamSessionRecordV1::ResumeAttempt(value) => {
            Some(&value.attempt.session_key.idempotency_key)
        }
        StreamSessionRecordV1::Detached(value) => Some(&value.session_key.idempotency_key),
        StreamSessionRecordV1::InvocationResult(value) => Some(&value.session_key.idempotency_key),
        StreamSessionRecordV1::Finished(value) => Some(&value.session_key.idempotency_key),
        _ => None,
    }
}
