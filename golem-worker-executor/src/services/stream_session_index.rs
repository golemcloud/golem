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

use crate::durable_host::durable_stream::metadata::{
    ProducerMetadataKey, ProducerMetadataProjection, ProducerMetadataRow, ProjectionError,
    project_producer_metadata, read_projection,
};
use crate::durable_host::durable_stream::{SessionControlMetadata, SessionError};
use crate::metrics::workers::record_derived_cache_publication_failed;
use crate::services::activity::spawn_with_activity;
use crate::services::oplog::{
    OplogPayloadDownloadError, OplogReadError, OplogReadSource, OplogService, OplogServiceOps,
    UnreadableOplog,
};
use crate::services::shutdown::join_or_park_at_runtime_shutdown;
use crate::services::worker::DERIVED_CACHE_EXPIRY;
use crate::services::worker::DurableStreamRecoveryMetadata;
use crate::services::worker_fork::lineage::StreamForkLineage;
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use bytes::Bytes;
use futures::StreamExt;
use golem_common::base_model::durable_stream::{
    LocalStreamReaderId, StreamSessionKey, StreamSessionRecord,
};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload};
use golem_common::model::{
    AgentFingerprint, DurableStreamPublicBinding, DurableStreamSessionStatus, InvocationId,
    OwnedAgentId,
};
use golem_common::serialization::{DecodeError, decode, serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

pub(super) const METADATA_FIELD: &str = "coverage";
const SESSION_FIELD_PREFIX: &str = "session:";
const PUBLIC_BINDING_FIELD_PREFIX: &str = "public-binding:";
pub(crate) const CONSUMER_JOURNAL_INDEX_PAGE_SIZE: u64 = 256;
const RECOVERY_CATALOGUE_PAGE_SIZE: u64 = 256;

fn recovery_catalogue_field(page: u64) -> String {
    format!("recovery:{page}")
}

fn stream_resume_index_field(
    key: &StreamSessionKey,
    attempt: golem_common::model::durable_stream::AttemptId,
) -> Result<String, SessionError> {
    Ok(format!(
        "resume:{}",
        hex::encode(serialize(&(key, attempt))?)
    ))
}

fn stream_control_index_field(key: &StreamSessionKey) -> Result<String, SessionError> {
    Ok(format!("control:{}", hex::encode(serialize(key)?)))
}

fn consumer_journal_index_field(
    key: &StreamSessionKey,
    reader: LocalStreamReaderId,
    page: u64,
) -> Result<String, SessionError> {
    Ok(format!(
        "journal:{}:{}:{page}",
        hex::encode(serialize(key)?),
        hex::encode(serialize(&reader)?)
    ))
}

#[derive(Clone, Debug, Default, desert_rust::BinaryCodec)]
pub(super) struct Metadata {
    pub(super) generation: Uuid,
    pub(super) covered_through: OplogIndex,
    pub(super) recovery_session_count: u64,
    pub(super) producer_fingerprint: Option<AgentFingerprint>,
    pub(super) lineage_discovered_through: OplogIndex,
    pub(super) stream_fork_lineage: StreamForkLineage,
    pub(super) export_fork_initialized_sessions: HashSet<InvocationId>,
}

/// Producer identity read from the same projection as stream metadata. `covered_through` is at
/// least the oplog horizon observed by the lookup, so the fingerprint belongs to that identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProducerIdentityLookup {
    pub covered_through: OplogIndex,
    pub producer_fingerprint: AgentFingerprint,
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
    async fn cached_snapshot<T>(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        horizon: OplogIndex,
        fields: Arc<[String]>,
        decode_fields: impl Fn(&[Option<Bytes>]) -> Result<T, DecodeError> + Sync,
    ) -> Result<T, SessionError> {
        let namespace = Self::namespace(id, fingerprint);
        let attempts = futures::stream::repeat(()).filter_map(|()| async {
            let attempt = async {
                let values = self
                    .kv
                    .with_entity("stream_session_index", "read_cache", "snapshot")
                    .get_many_raw(namespace.clone(), fields.clone())
                    .await?;
                match decode_fields(&values) {
                    Ok(value) => Ok(Some(value)),
                    Err(_) => {
                        let expected = values.first().and_then(Option::as_deref);
                        let rebuild_through = expected
                            .and_then(|bytes| decode::<Metadata>(bytes).ok())
                            .map_or(horizon, |metadata| metadata.covered_through.max(horizon));
                        self.refold_and_publish(
                            id,
                            mode,
                            fingerprint,
                            rebuild_through,
                            &namespace,
                            expected,
                        )
                        .await?;
                        Ok(None)
                    }
                }
            }
            .await;
            attempt.transpose()
        });
        futures::pin_mut!(attempts);
        attempts.next().await.expect("cache attempts do not end")
    }

    pub async fn lookup_producer_identity(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
    ) -> Result<ProducerIdentityLookup, SessionError> {
        let oplog = self.oplog.upgrade().ok_or("oplog service is unavailable")?;
        let horizon = oplog.get_last_index(id, mode).await?;
        self.catch_up(id, mode, fingerprint, horizon).await?;
        let metadata: Metadata = self
            .cached_snapshot(
                id,
                mode,
                fingerprint,
                horizon,
                vec![METADATA_FIELD.into()].into(),
                |values| values[0].as_deref().map(decode).transpose(),
            )
            .await?
            .ok_or("producer identity metadata is unavailable")?;
        if metadata.covered_through < horizon {
            return Err("producer identity coverage is unavailable".into());
        }
        Ok(ProducerIdentityLookup {
            covered_through: metadata.covered_through,
            producer_fingerprint: metadata
                .producer_fingerprint
                .ok_or("producer identity is missing from covered metadata")?,
        })
    }

    #[tracing::instrument(
        name = "stream_session_index.lookup_producer",
        level = "debug",
        skip_all
    )]
    pub async fn lookup_producer_metadata(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        keys: Vec<ProducerMetadataKey>,
    ) -> Result<(OplogIndex, Vec<Option<ProducerMetadataRow>>), SessionError> {
        let this = self.clone();
        let id = id.clone();
        join_or_park_at_runtime_shutdown(spawn_with_activity(async move {
            let oplog = this.oplog.upgrade().ok_or("oplog service is unavailable")?;
            let horizon = oplog.get_last_index(&id, mode).await?;
            this.catch_up_inner(&id, mode, fingerprint, horizon).await?;
            loop {
                let mut fields = vec![METADATA_FIELD.to_string()];
                for key in &keys {
                    fields.push(key.field()?);
                }
                let (expected, metadata, values) = this
                    .cached_snapshot(&id, mode, fingerprint, horizon, fields.into(), |values| {
                        let metadata = values[0].as_deref().map(decode::<Metadata>).transpose()?;
                        let rows = keys
                            .iter()
                            .zip(values.iter().skip(1))
                            .map(|(key, value)| {
                                if matches!(key, ProducerMetadataKey::Lineage) {
                                    Ok(None)
                                } else {
                                    value
                                        .as_deref()
                                        .map(decode::<ProducerMetadataRow>)
                                        .transpose()
                                }
                            })
                            .collect::<Result<Vec<_>, DecodeError>>()?;
                        Ok((values[0].clone(), metadata, rows))
                    })
                    .await?;
                let covered = metadata
                    .as_ref()
                    .map_or(OplogIndex::NONE, |metadata| metadata.covered_through);
                if covered < horizon {
                    return Err("producer metadata coverage is unavailable".into());
                }
                let result = async {
                    let mut rows = Vec::with_capacity(keys.len());
                    for (key, value) in keys.iter().zip(values) {
                        if matches!(key, ProducerMetadataKey::Lineage) {
                            rows.push(Some(ProducerMetadataRow::Lineage(
                                metadata
                                    .as_ref()
                                    .map(|metadata| metadata.stream_fork_lineage.through(covered))
                                    .transpose()?
                                    .unwrap_or_default(),
                            )));
                            continue;
                        }
                        rows.push(value);
                    }
                    Ok::<_, SessionError>((covered, rows))
                }
                .await;
                let current = this
                    .kv
                    .with_entity("stream_session_index", "lookup_producer", "metadata")
                    .get_raw(Self::namespace(&id, fingerprint), METADATA_FIELD)
                    .await?;
                if current == expected {
                    return result;
                }
            }
        }))
        .await
        .map_err(|error| format!("producer metadata lookup task failed: {error}"))?
    }

    pub async fn lookup_resume_offset(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        key: &StreamSessionKey,
        attempt: golem_common::model::durable_stream::AttemptId,
    ) -> Result<Option<OplogIndex>, SessionError> {
        let this = self.clone();
        let id = id.clone();
        let key = key.clone();
        join_or_park_at_runtime_shutdown(spawn_with_activity(async move {
            let oplog = this.oplog.upgrade().ok_or("oplog service is unavailable")?;
            let horizon = oplog.get_last_index(&id, mode).await?;
            let resume_field = stream_resume_index_field(&key, attempt)?;
            loop {
                this.catch_up_inner(&id, mode, fingerprint, horizon).await?;
                let (coverage, offset) = this
                    .cached_snapshot(
                        &id,
                        mode,
                        fingerprint,
                        horizon,
                        vec![METADATA_FIELD.into(), resume_field.clone()].into(),
                        |values| {
                            Ok((
                                values[0]
                                    .as_deref()
                                    .map(decode::<Metadata>)
                                    .transpose()?
                                    .map_or(OplogIndex::NONE, |metadata| metadata.covered_through),
                                values[1].as_deref().map(decode::<OplogIndex>).transpose()?,
                            ))
                        },
                    )
                    .await?;
                if coverage < horizon {
                    continue;
                }
                return Ok(offset.filter(|index| *index <= horizon));
            }
        }))
        .await
        .map_err(|error| format!("durable resume lookup task failed: {error}"))?
    }

    pub async fn lookup_recovery_metadata(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
    ) -> Result<DurableStreamRecoveryMetadata, SessionError> {
        let this = self.clone();
        let id = id.clone();
        join_or_park_at_runtime_shutdown(spawn_with_activity(async move {
            let oplog = this.oplog.upgrade().ok_or("oplog service is unavailable")?;
            let horizon = oplog.get_last_index(&id, mode).await?;
            this.catch_up_inner(&id, mode, fingerprint, horizon).await?;
            'snapshot: loop {
                let mut requested_pages = 0;
                let (covered_through, keys, consumer_deleting) = loop {
                    let mut names = vec![METADATA_FIELD.into(), "consumer-deleting".into()];
                    names.extend((0..requested_pages).map(recovery_catalogue_field));
                    let (metadata, deleting, pages) = this
                        .cached_snapshot(&id, mode, fingerprint, horizon, names.into(), |fields| {
                            Ok((
                                fields[0]
                                    .as_deref()
                                    .map(decode::<Metadata>)
                                    .transpose()?
                                    .unwrap_or_default(),
                                fields[1].as_deref().map(decode).transpose()?.flatten(),
                                fields
                                    .iter()
                                    .skip(2)
                                    .map(|value| {
                                        value
                                            .as_deref()
                                            .map(decode::<Vec<StreamSessionKey>>)
                                            .transpose()
                                    })
                                    .collect::<Result<Vec<_>, DecodeError>>()?,
                            ))
                        })
                        .await?;
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
                    for (page, entries) in pages.into_iter().take(needed_pages as usize).enumerate()
                    {
                        let entries = entries.ok_or("recovery catalogue page is missing")?;
                        let needed = (metadata.recovery_session_count
                            - page as u64 * RECOVERY_CATALOGUE_PAGE_SIZE)
                            .min(RECOVERY_CATALOGUE_PAGE_SIZE);
                        if entries.len() as u64 != needed {
                            return Err(
                                "recovery catalogue page does not match its coverage".into()
                            );
                        }
                        keys.extend(entries);
                    }
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
                    let (coverage, values) = this
                        .cached_snapshot(&id, mode, fingerprint, horizon, fields.into(), |values| {
                            Ok((
                                values[0].as_deref().map(decode::<Metadata>).transpose()?,
                                values
                                    .iter()
                                    .skip(1)
                                    .map(|value| {
                                        value
                                            .as_deref()
                                            .map(decode::<SessionControlMetadata>)
                                            .transpose()
                                    })
                                    .collect::<Result<Vec<_>, DecodeError>>()?,
                            ))
                        })
                        .await?;
                    let coverage = coverage.ok_or("recovery catalogue coverage is missing")?;
                    if coverage.covered_through != covered_through {
                        continue 'snapshot;
                    }
                    for (key, value) in keys.iter().zip(values) {
                        let mut control = value.ok_or("recovery catalogue session is missing")?;
                        control.advance_coverage(coverage.covered_through);
                        sessions.push((key.clone(), control));
                    }
                }
                return Ok(DurableStreamRecoveryMetadata {
                    covered_through,
                    sessions,
                    consumer_deleting,
                });
            }
        }))
        .await
        .map_err(|error| format!("durable recovery metadata task failed: {error}"))?
    }

    async fn recovery_page<'a>(
        &self,
        namespace: &KeyValueStorageNamespace,
        pages: &'a mut HashMap<u64, Vec<StreamSessionKey>>,
        page: u64,
    ) -> Result<&'a mut Vec<StreamSessionKey>, ProjectionError> {
        if let std::collections::hash_map::Entry::Vacant(e) = pages.entry(page) {
            let keys = read_projection(
                self.kv.as_ref(),
                namespace.clone(),
                &recovery_catalogue_field(page),
            )
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
        controls: &mut HashMap<StreamSessionKey, SessionControlMetadata>,
        metadata: &mut Metadata,
    ) -> Result<HashMap<u64, Vec<StreamSessionKey>>, ProjectionError> {
        let mut pages = HashMap::new();
        let keys: Vec<_> = controls.keys().cloned().collect();
        for key in keys {
            let control = controls.get(&key).unwrap();
            match (control.recovery_slot(), control.needs_recovery(id, &key)) {
                (None, true) => {
                    let slot = metadata.recovery_session_count;
                    let page = self
                        .recovery_page(namespace, &mut pages, slot / RECOVERY_CATALOGUE_PAGE_SIZE)
                        .await?;
                    if page.len() as u64 != slot % RECOVERY_CATALOGUE_PAGE_SIZE {
                        return Err("recovery catalogue page does not match its coverage".into());
                    }
                    page.push(key.clone());
                    controls
                        .get_mut(&key)
                        .unwrap()
                        .assign_recovery_slot(Some(slot));
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
                            let control = read_projection(
                                self.kv.as_ref(),
                                namespace.clone(),
                                &stream_control_index_field(&moved)?,
                            )
                            .await?
                            .ok_or("recovery catalogue refers to a missing session")?;
                            controls.insert(moved.clone(), control);
                        }
                        controls
                            .get_mut(&moved)
                            .unwrap()
                            .assign_recovery_slot(Some(slot));
                    } else if moved != key {
                        return Err("recovery catalogue tail identifies another session".into());
                    }
                    controls.get_mut(&key).unwrap().assign_recovery_slot(None);
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
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        key: &StreamSessionKey,
        reader: LocalStreamReaderId,
        page: u64,
    ) -> Result<Vec<OplogIndex>, SessionError> {
        let namespace = Self::namespace(id, fingerprint);
        loop {
            let expected = self
                .kv
                .with_entity("stream_session_index", "read_consumer_journal", "metadata")
                .get_raw(namespace.clone(), METADATA_FIELD)
                .await?;
            let result = match self
                .read_consumer_page_inner(&namespace, key, reader, page)
                .await
            {
                Ok(row) => Ok(row),
                Err(ProjectionError::CacheDecode) => {
                    let horizon = self
                        .oplog
                        .upgrade()
                        .ok_or("oplog service is unavailable")?
                        .get_last_index(id, mode)
                        .await?;
                    self.refold_and_publish(
                        id,
                        mode,
                        fingerprint,
                        horizon,
                        &namespace,
                        expected.as_deref(),
                    )
                    .await?;
                    continue;
                }
                Err(ProjectionError::Other(error)) => Err(*error),
            };
            let current = self
                .kv
                .with_entity("stream_session_index", "read_consumer_journal", "metadata")
                .get_raw(namespace.clone(), METADATA_FIELD)
                .await?;
            if current == expected {
                return result?.ok_or_else(|| "consumer journal index page is missing".into());
            }
        }
    }

    async fn read_consumer_page_inner(
        &self,
        namespace: &KeyValueStorageNamespace,
        key: &StreamSessionKey,
        reader: LocalStreamReaderId,
        page: u64,
    ) -> Result<Option<Vec<OplogIndex>>, ProjectionError> {
        page.checked_mul(CONSUMER_JOURNAL_INDEX_PAGE_SIZE)
            .ok_or("consumer journal page overflow")?;
        let row: Option<Vec<OplogIndex>> = read_projection(
            self.kv.as_ref(),
            namespace.clone(),
            &consumer_journal_index_field(key, reader, page)?,
        )
        .await?;
        if let Some(ref row) = row
            && row.len() as u64 > CONSUMER_JOURNAL_INDEX_PAGE_SIZE
        {
            return Err("consumer journal index page exceeds its bound".into());
        }
        Ok(row)
    }

    pub async fn lookup_control_metadata(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        key: &StreamSessionKey,
    ) -> Result<SessionControlMetadata, SessionError> {
        let this = self.clone();
        let id = id.clone();
        let key = key.clone();
        join_or_park_at_runtime_shutdown(spawn_with_activity(async move {
            let oplog = this
                .oplog
                .upgrade()
                .ok_or_else(|| "oplog service is unavailable".to_string())?;
            let horizon = oplog.get_last_index(&id, mode).await?;
            this.catch_up_inner(&id, mode, fingerprint, horizon).await?;
            let lock = this.index_lock(&id);
            let _guard = lock.inner.lock().await;
            let (coverage, mut snapshot, consumer_deleting) = this
                .cached_snapshot(
                    &id,
                    mode,
                    fingerprint,
                    horizon,
                    vec![
                        METADATA_FIELD.into(),
                        stream_control_index_field(&key)?,
                        "consumer-deleting".into(),
                    ]
                    .into(),
                    |fields| {
                        Ok((
                            fields[0]
                                .as_deref()
                                .map(decode::<Metadata>)
                                .transpose()?
                                .map_or(OplogIndex::NONE, |value| value.covered_through),
                            fields[1]
                                .as_deref()
                                .map(decode::<SessionControlMetadata>)
                                .transpose()?
                                .unwrap_or_default(),
                            fields[2].as_deref().map(decode).transpose()?.flatten(),
                        ))
                    },
                )
                .await?;
            if coverage < horizon {
                return Err("durable stream control metadata coverage is unavailable".into());
            }
            snapshot.restore_index_state(coverage, consumer_deleting);
            Ok(snapshot)
        }))
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
        fingerprint: AgentFingerprint,
        horizon: OplogIndex,
    ) -> Result<(), SessionError> {
        let this = self.clone();
        let id = id.clone();
        join_or_park_at_runtime_shutdown(spawn_with_activity(async move {
            this.catch_up_inner(&id, mode, fingerprint, horizon).await
        }))
        .await
        .map_err(|err| format!("stream session index task failed: {err}"))?
    }

    pub async fn lookup_persisted(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        horizon: OplogIndex,
        key: &InvocationId,
    ) -> Result<Option<DurableStreamSessionStatus>, SessionError> {
        let this = self.clone();
        let id = id.clone();
        let key = *key;
        join_or_park_at_runtime_shutdown(spawn_with_activity(async move {
            this.lookup_inner(&id, mode, fingerprint, &key, SessionLookup::Exact(horizon))
                .await
        }))
        .await
        .map_err(|err| format!("stream session index task failed: {err}"))?
    }

    /// Looks up only the four lifecycle oplog offsets. If storage has advanced beyond `horizon`, all
    /// attachment identity fields are discarded because they cannot be rewound from the index.
    pub async fn lookup_persisted_offsets(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        horizon: OplogIndex,
        key: &InvocationId,
    ) -> Result<Option<DurableStreamSessionStatus>, SessionError> {
        let this = self.clone();
        let id = id.clone();
        let key = *key;
        join_or_park_at_runtime_shutdown(spawn_with_activity(async move {
            this.lookup_inner(
                &id,
                mode,
                fingerprint,
                &key,
                SessionLookup::Offsets(horizon),
            )
            .await
        }))
        .await
        .map_err(|err| format!("stream session index task failed: {err}"))?
    }

    pub async fn lookup_latest(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        key: &InvocationId,
    ) -> Result<Option<DurableStreamSessionStatus>, SessionError> {
        let this = self.clone();
        let id = id.clone();
        let key = *key;
        join_or_park_at_runtime_shutdown(spawn_with_activity(async move {
            this.lookup_inner(&id, mode, fingerprint, &key, SessionLookup::Latest)
                .await
        }))
        .await
        .map_err(|err| format!("stream session index task failed: {err}"))?
    }

    pub async fn lookup_public_binding(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        route_scope: &str,
        public_session_id: &str,
    ) -> Result<Option<DurableStreamPublicBinding>, SessionError> {
        let oplog = self.oplog.upgrade().ok_or("oplog service is unavailable")?;
        let horizon = oplog.get_last_index(id, mode).await?;
        self.catch_up(id, mode, fingerprint, horizon).await?;
        let lock = self.index_lock(id);
        let _guard = lock.inner.lock().await;
        let (metadata, binding) = self
            .cached_snapshot(
                id,
                mode,
                fingerprint,
                horizon,
                vec![
                    METADATA_FIELD.into(),
                    Self::public_binding_field(route_scope, public_session_id),
                ]
                .into(),
                |values| {
                    Ok((
                        values[0].as_deref().map(decode::<Metadata>).transpose()?,
                        values[1]
                            .as_deref()
                            .map(decode::<DurableStreamPublicBinding>)
                            .transpose()?,
                    ))
                },
            )
            .await?;
        let metadata =
            metadata.ok_or("stream session index coverage is unavailable after catch-up")?;
        if metadata.covered_through < horizon {
            return Err("stream session index coverage regressed after catch-up".into());
        }
        Ok(binding)
    }

    pub async fn clear(
        &self,
        id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(), SessionError> {
        let this = self.clone();
        let id = id.clone();
        join_or_park_at_runtime_shutdown(spawn_with_activity(async move {
            let lock = this.index_lock(&id);
            let _guard = lock.inner.lock().await;
            this.clear_inner(&id, fingerprint).await
        }))
        .await
        .map_err(|err| format!("stream session index task failed: {err}"))?
    }

    pub(super) fn namespace(
        id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> KeyValueStorageNamespace {
        KeyValueStorageNamespace::AgentDurableStreamSessionIndex {
            owned_agent_id: id.clone(),
            fingerprint,
        }
    }

    pub(super) fn field(key: &InvocationId) -> String {
        format!("{SESSION_FIELD_PREFIX}{key}")
    }

    pub(super) fn public_binding_field(route_scope: &str, public_session_id: &str) -> String {
        let identity = serde_json::to_string(&(route_scope, public_session_id))
            .expect("string tuple is serializable");
        format!("{PUBLIC_BINDING_FIELD_PREFIX}{identity}")
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

    async fn clear_inner(
        &self,
        id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(), SessionError> {
        let namespace = Self::namespace(id, fingerprint);
        loop {
            let expected = self
                .kv
                .with_entity("stream_session_index", "snapshot_clear", "metadata")
                .get_raw(namespace.clone(), METADATA_FIELD)
                .await?;
            let mut keys = self
                .kv
                .with("stream_session_index", "snapshot_clear")
                .keys(namespace.clone())
                .await?;
            if expected.is_none() && keys.is_empty() {
                return Ok(());
            }
            if !keys.iter().any(|key| key == METADATA_FIELD) {
                keys.push(METADATA_FIELD.into());
            }
            let deletions: Vec<_> = keys.iter().map(String::as_str).collect();
            if self
                .kv
                .with_entity("stream_session_index", "clear", "metadata")
                .compare_and_mutate_many_raw(
                    namespace.clone(),
                    METADATA_FIELD,
                    expected.as_deref(),
                    &[],
                    &deletions,
                    DERIVED_CACHE_EXPIRY,
                )
                .await?
            {
                return Ok(());
            }
        }
    }

    #[tracing::instrument(name = "stream_session_index.catch_up", level = "debug", skip_all)]
    async fn catch_up_inner(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        horizon: OplogIndex,
    ) -> Result<(), SessionError> {
        let lock = self.index_lock(id);
        let _guard = lock.inner.lock().await;
        let namespace = Self::namespace(id, fingerprint);
        let oplog = self
            .oplog
            .upgrade()
            .ok_or_else(|| "oplog service is unavailable".to_string())?;
        let payload_error = |index, error| {
            UnreadableOplog::new(
                &id.agent_id,
                OplogReadError::of_payload(
                    OplogReadSource::Other("stream index payload"),
                    index,
                    error,
                ),
            )
        };
        loop {
            let mut expected = self
                .kv
                .with_entity("stream_session_index", "read", "metadata")
                .get_raw(namespace.clone(), METADATA_FIELD)
                .await?;
            if expected.is_none() {
                let initial = Metadata {
                    generation: Uuid::new_v4(),
                    covered_through: OplogIndex::NONE,
                    ..Default::default()
                };
                let initial = serialize(&initial)?;
                if !self
                    .kv
                    .with_entity("stream_session_index", "initialize", "metadata")
                    .compare_and_mutate_many_raw(
                        namespace.clone(),
                        METADATA_FIELD,
                        None,
                        &[(METADATA_FIELD, initial.as_slice())],
                        &[],
                        DERIVED_CACHE_EXPIRY,
                    )
                    .await
                    .inspect_err(|_err| {
                        record_derived_cache_publication_failed("durable_stream_index");
                    })?
                {
                    continue;
                }
                expected = Some(initial.into());
            }
            let mut metadata = match decode::<Metadata>(expected.as_ref().unwrap()) {
                Ok(metadata) => metadata,
                Err(_) => {
                    self.refold_and_publish(
                        id,
                        mode,
                        fingerprint,
                        horizon,
                        &namespace,
                        expected.as_deref(),
                    )
                    .await?;
                    continue;
                }
            };
            if metadata.covered_through >= horizon {
                return Ok(());
            }
            let count = (horizon.as_u64() - metadata.covered_through.as_u64()).min(1024);
            let mut entries = oplog
                .read_exact(id, mode, metadata.covered_through.next(), count)
                .await?;
            if entries.is_empty() {
                return Err(
                    format!("empty oplog range while indexing stream sessions for {id}").into(),
                );
            }
            let previously_discovered = metadata.lineage_discovered_through;
            if metadata.producer_fingerprint.is_none() {
                metadata.producer_fingerprint = entries.values().find_map(|entry| match entry {
                    OplogEntry::Create { parameters, .. } => {
                        Some(AgentFingerprint(parameters.instance_id))
                    }
                    _ => None,
                });
            }
            if let Some(fingerprint) = metadata.producer_fingerprint
                && metadata.lineage_discovered_through < horizon
            {
                let rebuild = if metadata.lineage_discovered_through == OplogIndex::NONE {
                    true
                } else if entries.keys().max().is_some_and(|index| *index >= horizon) {
                    let mut found = false;
                    for (_, entry) in
                        entries.range(metadata.lineage_discovered_through.next()..=horizon)
                    {
                        if matches!(entry, OplogEntry::Revert { .. }) {
                            found = true;
                            break;
                        }
                        // The full loader resolves later deletions before it reads payloads.
                        if let OplogEntry::StreamSession { record, .. } = entry
                            && matches!(
                                oplog.download_payload(id, mode, record.clone()).await,
                                Ok(StreamSessionRecord::ForkCut(_)) | Err(_)
                            )
                        {
                            found = true;
                            break;
                        }
                    }
                    found
                } else {
                    StreamForkLineage::suffix_contains_fork_cut(
                        oplog.as_ref(),
                        id,
                        mode,
                        metadata.lineage_discovered_through.next(),
                        horizon,
                    )
                    .await?
                };
                if rebuild {
                    metadata.stream_fork_lineage = StreamForkLineage::load_from_service(
                        oplog.as_ref(),
                        id,
                        mode,
                        fingerprint,
                        horizon,
                    )
                    .await?;
                }
                metadata.lineage_discovered_through = horizon;
            }
            if metadata
                .stream_fork_lineage
                .cuts()
                .iter()
                .any(|(index, _)| {
                    *index > previously_discovered && metadata.covered_through != OplogIndex::NONE
                })
            {
                self.refold_and_publish(
                    id,
                    mode,
                    fingerprint,
                    horizon,
                    &namespace,
                    expected.as_deref(),
                )
                .await?;
                continue;
            }
            let mut physical_chunk_end = *entries.keys().max().unwrap();
            entries.retain(|index, _| {
                !metadata
                    .stream_fork_lineage
                    .deleted_regions()
                    .is_in_deleted_region(*index)
            });
            while let Some((index, OplogEntry::StreamRegistered { record, .. })) =
                entries.last_key_value()
            {
                let record = oplog
                    .download_payload(id, mode, record.clone())
                    .await
                    .map_err(|error| payload_error(*index, error))?;
                if !matches!(
                    record.coordinate,
                    golem_common::model::durable_stream::StreamRegistrationRecordCoordinate::Nested { .. }
                ) || physical_chunk_end >= horizon
                {
                    break;
                }
                let next_index = physical_chunk_end.next();
                let next = oplog.read_exact(id, mode, next_index, 1).await?;
                if next.is_empty() {
                    return Err(
                        "missing enclosing item while extending producer metadata chunk".into(),
                    );
                }
                physical_chunk_end = next_index;
                entries.extend(next.into_iter().filter(|(index, _)| {
                    !metadata
                        .stream_fork_lineage
                        .deleted_regions()
                        .is_in_deleted_region(*index)
                }));
            }
            if let Some((marker, _)) = metadata
                .stream_fork_lineage
                .cuts()
                .iter()
                .find(|(index, _)| entries.contains_key(index))
            {
                let marker_is_first = entries
                    .first_key_value()
                    .is_some_and(|(index, _)| index == marker);
                entries.retain(|index, _| {
                    if marker_is_first {
                        index == marker
                    } else {
                        index < marker
                    }
                });
                physical_chunk_end = *entries.keys().max().unwrap();
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
                        &metadata.stream_fork_lineage,
                    )
                    .await?
                } else {
                    ProducerMetadataProjection::default()
                };
                let mut updates = HashMap::<InvocationId, DurableStreamSessionStatus>::new();
                let mut public_bindings =
                    HashMap::<String, Option<DurableStreamPublicBinding>>::new();
                let mut controls = HashMap::<StreamSessionKey, SessionControlMetadata>::new();
                let mut journal_pages = HashMap::<String, Vec<OplogIndex>>::new();
                let mut resume_offsets = HashMap::<String, OplogIndex>::new();
                let mut consumer_deleting = None;
                for (idx, entry) in &entries {
                    if let OplogEntry::PendingAgentInvocation {
                        invocation_id: idempotency_key, ..
                    } = entry
                    {
                        if !updates.contains_key(idempotency_key) {
                            let old: Option<DurableStreamSessionStatus> = read_projection(
                                self.kv.as_ref(), namespace.clone(), &Self::field(idempotency_key),
                            ).await?;
                            if let Some(old) = old {
                                updates.insert(*idempotency_key, old);
                            }
                        }
                        if let Some(status) = updates.get_mut(idempotency_key) {
                            status.apply_pending_invocation(*idx, idempotency_key);
                        }
                        continue;
                    }
                    let OplogEntry::StreamSession { record, .. } = entry else {
                        continue;
                    };
                    let producer_fingerprint = metadata.producer_fingerprint.ok_or_else(|| {
                        "stream session records require producer fingerprint metadata".to_string()
                    })?;
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
                            decoded = decode(bytes).map_err(|error| payload_error(*idx, OplogPayloadDownloadError::Corrupt(anyhow::Error::new(error))))?;
                            &decoded
                        }
                        OplogPayload::External { cached: None, .. } => {
                            decoded = oplog.download_payload(id, mode, record.clone()).await.map_err(|error| payload_error(*idx, error))?;
                            &decoded
                        }
                    };
                    if let StreamSessionRecord::ExportForkInitialized(record) = record {
                        metadata
                            .export_fork_initialized_sessions
                            .insert(record.session_key);
                    }
                    if let StreamSessionRecord::ForkCut(cut) = record {
                        for key in metadata.export_fork_initialized_sessions.clone() {
                            if let std::collections::hash_map::Entry::Vacant(entry) = updates.entry(key) {
                                let status: Option<DurableStreamSessionStatus> = read_projection(
                                    self.kv.as_ref(), namespace.clone(), &Self::field(&key),
                                ).await?;
                                if let Some(status) = status {
                                    entry.insert(status);
                                }
                            }
                            if let Some(status) = updates.get_mut(&key) {
                                status.apply_record(*idx, record);
                            }
                        }
                        if let Some(fork) = producer_fields.fork.as_ref() {
                            for key in &fork.sessions {
                                let old = if let Some(control) = controls.remove(key) {
                                    control
                                } else {
                                    read_projection(
                                        self.kv.as_ref(), namespace.clone(), &stream_control_index_field(key)?,
                                    )
                                        .await?
                                        .unwrap_or_default()
                                };
                                let mut projected = old.for_fork(cut)?;
                                projected.assign_recovery_slot(old.recovery_slot());
                                controls.insert(key.clone(), projected);
                                if let std::collections::hash_map::Entry::Vacant(entry) = updates.entry(key.idempotency_key) {
                                    let status: Option<DurableStreamSessionStatus> = read_projection(
                                        self.kv.as_ref(), namespace.clone(), &Self::field(&key.idempotency_key),
                                    ).await?;
                                    if let Some(status) = status {
                                        entry.insert(status);
                                    }
                                }
                                if let Some(status) = updates.get_mut(&key.idempotency_key) {
                                    status.apply_record(*idx, record);
                                }
                            }
                        }
                        consumer_deleting = Some(None);
                        continue;
                    }
                    if metadata.stream_fork_lineage.resets_control(*idx, record) {
                        continue;
                    }
                    let inherited = metadata
                        .stream_fork_lineage
                        .cuts()
                        .iter()
                        .rev()
                        .find(|(_, cut)| cut.revert.is_none())
                        .is_some_and(|(marker, _)| idx < marker);
                    let public_session_id = if inherited {
                        None
                    } else {
                        match record {
                            StreamSessionRecord::Prepared(record) => {
                                record.route_scope.as_ref().map(|scope| Self::public_binding_field(scope, &record.public_session_id))
                            }
                            StreamSessionRecord::ExportForkInitialized(record) => {
                                Some(Self::public_binding_field(&record.route_scope, &record.public_session_id))
                            }
                            StreamSessionRecord::ExpiryRefreshed(record) => {
                                Some(Self::public_binding_field(&record.route_scope, &record.public_session_id))
                            }
                            StreamSessionRecord::Expired(record) => {
                                Some(Self::public_binding_field(&record.route_scope, &record.public_session_id))
                            }
                            _ => None,
                        }
                    };
                    if let Some(public_session_id) = public_session_id {
                        if !public_bindings.contains_key(&public_session_id) {
                            let old = read_projection(
                                self.kv.as_ref(), namespace.clone(), &public_session_id,
                            ).await?;
                            public_bindings.insert(public_session_id.clone(), old);
                        }
                        let current = public_bindings
                            .get(&public_session_id)
                            .and_then(Option::as_ref);
                        let binding = DurableStreamPublicBinding::fold(current, record);
                        public_bindings.insert(public_session_id.clone(), binding);
                    }
                    if let StreamSessionRecord::ConsumerDeleting(record) = record {
                        consumer_deleting = Some(Some(record.clone()));
                    }
                    if let StreamSessionRecord::ResumeAttempt(record) = record {
                        let field = stream_resume_index_field(
                            &StreamSessionKey {
                                callee_environment_id: id.environment_id,
                                callee: id.agent_id,
                                callee_fingerprint: producer_fingerprint,
                                idempotency_key: record.session_key,
                            },
                            record.attempt.attempt_id,
                        )?;
                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            resume_offsets.entry(field)
                        {
                            let old: Option<OplogIndex> = read_projection(
                                self.kv.as_ref(), namespace.clone(), entry.key(),
                            ).await?;
                            entry.insert(old.unwrap_or(*idx));
                        }
                    }
                    for key in crate::worker::stream_session_record_keys(
                        record,
                        id.environment_id,
                        &id.agent_id,
                        producer_fingerprint,
                    ) {
                        if !controls.contains_key(&key) {
                            let old: Option<SessionControlMetadata> = read_projection(
                                self.kv.as_ref(), namespace.clone(), &stream_control_index_field(&key)?,
                            ).await?;
                            controls.insert(key.clone(), old.unwrap_or_default());
                        }
                        let control = controls.get_mut(&key).unwrap();
                        let reader = match record {
                            StreamSessionRecord::ConsumerItemValue(record) => {
                                Some(record.reader_id)
                            }
                            StreamSessionRecord::ConsumerTerminal(record)
                            | StreamSessionRecord::ConsumerOutcome(record) => {
                                Some(record.reader_id)
                            }
                            StreamSessionRecord::SourceUnavailable(record) => {
                                Some(record.reader_id)
                            }
                            _ => None,
                        };
                        if let Some(reader) = reader {
                            let count = control.consumer_record_count(reader);
                            let field = consumer_journal_index_field(
                                &key,
                                reader,
                                count / CONSUMER_JOURNAL_INDEX_PAGE_SIZE,
                            )?;
                            if !journal_pages.contains_key(&field) {
                                let old = self.read_consumer_page_inner(&namespace, &key, reader, count / CONSUMER_JOURNAL_INDEX_PAGE_SIZE).await?;
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
                        control.apply(
                            *idx,
                            &key,
                            record,
                            id.environment_id,
                            &id.agent_id,
                            producer_fingerprint,
                        );
                    }
                    let Some(key) = record.local_session_key().cloned() else {
                        continue;
                    };
                    if let std::collections::hash_map::Entry::Vacant(entry) = updates.entry(key) {
                        let old: Option<DurableStreamSessionStatus> = read_projection(
                            self.kv.as_ref(), namespace.clone(), &Self::field(&key),
                        ).await?;
                        entry.insert(old.unwrap_or_default());
                    }
                    let status = updates.get_mut(&key).unwrap();
                    if let StreamSessionRecord::Attached(attached) = record
                        && status.lifecycle_error.is_none()
                        && status.validated_initial_pending_invocation.is_none()
                    {
                        if !status.validate_initial_attachment_reference(*idx, attached) {
                            continue;
                        }
                        let persisted_referent;
                        let referent = if let Some(entry) =
                            entries.get(&attached.pending_invocation_oplog_index)
                        {
                            Some(entry)
                        } else {
                            persisted_referent = oplog
                                .read_exact(
                                    id,
                                    mode,
                                    attached.pending_invocation_oplog_index,
                                    1,
                                )
                                .await?;
                            persisted_referent.get(&attached.pending_invocation_oplog_index)
                        };
                        match referent {
                            Some(OplogEntry::PendingAgentInvocation {
                                invocation_id: idempotency_key, ..
                            }) => status.apply_pending_invocation(
                                attached.pending_invocation_oplog_index,
                                idempotency_key,
                            ),
                            _ => {
                                status.lifecycle_error = Some(
                                    "durable Attached record references a missing or invalid pending invocation"
                                        .into(),
                                )
                            }
                        }
                    }
                    if status.first_prepared.is_some()
                        || status.export_fork_initialized
                        || matches!(
                            record,
                            StreamSessionRecord::Prepared(_)
                                | StreamSessionRecord::ExportForkInitialized(_)
                        )
                    {
                        status.apply_record(*idx, record);
                    }
                }
                let recovery_pages = self
                    .update_recovery_catalogue(id, &namespace, &mut controls, &mut metadata)
                    .await?;
                metadata.covered_through = physical_chunk_end;
                let mut fields: Vec<(String, Vec<u8>)> = updates
                    .into_iter()
                    .filter(|(_, value)| {
                        value.first_prepared.is_some() || value.export_fork_initialized
                    })
                    .map(|(key, value)| Ok((Self::field(&key), serialize(&value)?)))
                    .collect::<Result<_, SessionError>>()?;
                for (public_session_id, binding) in public_bindings {
                    if let Some(binding) = binding {
                        fields.push((
                            public_session_id,
                            serialize(&binding)?,
                        ));
                    }
                }
                fields.extend(producer_fields.rows);
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
                Ok::<_, ProjectionError>(fields)
            }
            .await;
            let fields = match fields {
                Ok(fields) => fields,
                Err(ProjectionError::CacheDecode) => {
                    self.refold_and_publish(
                        id,
                        mode,
                        fingerprint,
                        horizon,
                        &namespace,
                        expected.as_deref(),
                    )
                    .await?;
                    continue;
                }
                Err(ProjectionError::Other(error)) => {
                    // Separate row reads may straddle another executor's atomic update.
                    let current = self
                        .kv
                        .with_entity("stream_session_index", "read", "metadata")
                        .get_raw(namespace.clone(), METADATA_FIELD)
                        .await?;
                    if current == expected {
                        return Err(*error);
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
                .compare_and_mutate_many_raw(
                    namespace.clone(),
                    METADATA_FIELD,
                    expected.as_deref(),
                    &refs,
                    &[],
                    DERIVED_CACHE_EXPIRY,
                )
                .await
                .inspect_err(|_err| {
                    record_derived_cache_publication_failed("durable_stream_index");
                })?;
            // Reload after either winning the CAS or observing another executor's progress.
        }
    }

    async fn refold_and_publish(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        horizon: OplogIndex,
        namespace: &KeyValueStorageNamespace,
        expected: Option<&[u8]>,
    ) -> Result<(), SessionError> {
        let scratch: Arc<dyn KeyValueStorage + Send + Sync> =
            Arc::new(InMemoryKeyValueStorage::new());
        let scratch_service = Self::new(scratch.clone(), self.oplog.clone());
        Box::pin(scratch_service.catch_up_inner(id, mode, fingerprint, horizon)).await?;

        let scratch_keys = scratch
            .with("stream_session_index", "refold_read")
            .keys(namespace.clone())
            .await?;
        let scratch_values = scratch
            .with_entity("stream_session_index", "refold_read", "session")
            .get_many_raw(namespace.clone(), scratch_keys.clone().into())
            .await?;
        let retained = scratch_keys.iter().cloned().collect::<HashSet<_>>();
        let old_keys = self
            .kv
            .with("stream_session_index", "refold_publish")
            .keys(namespace.clone())
            .await?;
        let deletes = old_keys
            .iter()
            .filter(|key| !retained.contains(*key))
            .map(String::as_str)
            .collect::<Vec<_>>();
        let pairs = scratch_keys
            .iter()
            .zip(scratch_values.iter())
            .map(|(key, value)| {
                Ok((
                    key.as_str(),
                    value
                        .as_deref()
                        .ok_or("scratch stream index row is missing")?,
                ))
            })
            .collect::<Result<Vec<_>, SessionError>>()?;
        self.kv
            .with_entity("stream_session_index", "refold_publish", "session")
            .compare_and_mutate_many_raw(
                namespace.clone(),
                METADATA_FIELD,
                expected,
                &pairs,
                &deletes,
                DERIVED_CACHE_EXPIRY,
            )
            .await?;
        Ok(())
    }

    #[tracing::instrument(name = "stream_session_index.lookup", level = "debug", skip_all)]
    async fn lookup_inner(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        fingerprint: AgentFingerprint,
        key: &InvocationId,
        lookup: SessionLookup,
    ) -> Result<Option<DurableStreamSessionStatus>, SessionError> {
        let horizon = match lookup {
            SessionLookup::Exact(horizon) | SessionLookup::Offsets(horizon) => horizon,
            SessionLookup::Latest => {
                self.oplog
                    .upgrade()
                    .ok_or_else(|| "oplog service is unavailable".to_string())?
                    .get_last_index(id, mode)
                    .await?
            }
        };
        self.catch_up_inner(id, mode, fingerprint, horizon).await?;
        let lock = self.index_lock(id);
        let _guard = lock.inner.lock().await;
        let (metadata, value) = self
            .cached_snapshot(
                id,
                mode,
                fingerprint,
                horizon,
                vec![METADATA_FIELD.into(), Self::field(key)].into(),
                |values| {
                    Ok((
                        values[0].as_deref().map(decode::<Metadata>).transpose()?,
                        values[1]
                            .as_deref()
                            .map(decode::<DurableStreamSessionStatus>)
                            .transpose()?,
                    ))
                },
            )
            .await?;
        let Some(metadata) = metadata else {
            return Err("stream session index coverage is unavailable after catch-up".into());
        };
        if metadata.covered_through < horizon {
            return Err("stream session index coverage regressed after catch-up".into());
        }
        if metadata.covered_through > horizon && matches!(lookup, SessionLookup::Exact(_)) {
            return Err("stream session index is newer than the requested horizon".into());
        }
        let Some(mut value) = value else {
            return Ok(None);
        };
        if matches!(lookup, SessionLookup::Offsets(_)) {
            for field in [
                &mut value.first_prepared,
                &mut value.prepared,
                &mut value.invocation_result,
                &mut value.finished,
                &mut value.validated_initial_pending_invocation,
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
                value.validated_initial_pending_invocation = None;
                value.attachment_epoch = None;
                value.attachment_attempt_id = None;
                value.attachment_attached = None;
                value.lifecycle_error = None;
                value.public_session_id = None;
                value.route_scope = None;
                value.expiry_policy =
                    golem_common::model::durable_stream::StreamSessionExpiryPolicy::None;
                value.expiry_deadline_millis = None;
                value.expired = false;
                value.export_fork_initialized = false;
                value.export_source_invocation = None;
            }
        }
        Ok((value.first_prepared.is_some() || value.export_fork_initialized).then_some(value))
    }
}
