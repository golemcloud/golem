// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::model::ExecutionStatus;
use async_trait::async_trait;
pub use blob::BlobOplogArchiveService;
pub use compressed::{CompressedOplogArchive, CompressedOplogArchiveService, CompressedOplogChunk};
use desert_rust::BinaryCodec;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequest, HostResponse, OplogEntry, OplogIndex, OplogPayload,
    PayloadId, PersistenceLevel, RawOplogPayload, UpdateDescription,
};
use golem_common::model::{
    IdempotencyKey, OwnedWorkerId, ScanCursor, Timestamp, WorkerId, WorkerMetadata,
    WorkerStatusRecord,
};
use golem_common::read_only_lock;
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_wasm::{Value, ValueAndType};
pub use multilayer::{MultiLayerOplog, MultiLayerOplogService, OplogArchiveService};
pub use primary::PrimaryOplogService;
use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

mod blob;
mod compressed;
mod ephemeral;
mod multilayer;
pub mod plugin;
mod primary;

#[cfg(test)]
pub mod tests;

/// A top-level service for managing worker oplogs
///
/// For write access an oplog has to be opened with the `open` function (or if it doesn't exist,
/// created with the `create` function), which returns an implementation of the `Oplog` trait
/// providing synchronized access to the worker's oplog.
///
/// The following implementations are provided:
/// - `PrimaryOplogService` - based on the configured indexed storage, directly stores oplog entries.
///    This should always be the top-level implementation even in case of multi-layering.
/// - `CompressedOplogService` - uses the configured indexed storage, but stores oplog entries in
///    compressed chunks. Reads a whole chunk in memory when accessed. Should not be used on top level.
/// - `MultiLayerOplogService` - a service that can be used to stack multiple oplog services on each
///    other. Old entries are moved down the stack based on configurable conditions.
///
#[async_trait]
pub trait OplogService: Debug + Send + Sync {
    async fn create(
        &self,
        owned_worker_id: &OwnedWorkerId,
        initial_entry: OplogEntry,
        initial_worker_metadata: WorkerMetadata,
        last_known_status: read_only_lock::tokio::ReadOnlyLock<WorkerStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog>;

    async fn open(
        &self,
        owned_worker_id: &OwnedWorkerId,
        last_oplog_index: OplogIndex,
        initial_worker_metadata: WorkerMetadata,
        last_known_status: read_only_lock::tokio::ReadOnlyLock<WorkerStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Arc<dyn Oplog>;

    async fn get_last_index(&self, owned_worker_id: &OwnedWorkerId) -> OplogIndex;

    async fn delete(&self, owned_worker_id: &OwnedWorkerId);

    async fn read(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry>;

    /// Reads an inclusive range of entries from the oplog
    async fn read_range(
        &self,
        owned_worker_id: &OwnedWorkerId,
        start_idx: OplogIndex,
        last_idx: OplogIndex,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        assert!(
            start_idx <= last_idx,
            "Invalid range passed to OplogService::read_range: start_idx = {start_idx}, last_idx = {last_idx}"
        );

        self.read(
            owned_worker_id,
            start_idx,
            Into::<u64>::into(last_idx) - Into::<u64>::into(start_idx) + 1,
        )
        .await
    }

    async fn read_prefix(
        &self,
        owned_worker_id: &OwnedWorkerId,
        last_idx: OplogIndex,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.read_range(owned_worker_id, OplogIndex::INITIAL, last_idx)
            .await
    }

    /// Checks whether the oplog exists in the oplog, without opening it
    async fn exists(&self, owned_worker_id: &OwnedWorkerId) -> bool;

    /// Scans the oplog for all workers belonging to the given component, in a paginated way.
    ///
    /// Pages can be empty. This operation is slow and is not locking the oplog.
    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedWorkerId>), WorkerExecutorError>;

    /// Uploads a big oplog payload and returns a reference to it
    async fn upload_raw_payload(
        &self,
        owned_worker_id: &OwnedWorkerId,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String>;

    /// Downloads a big oplog payload by its reference
    async fn download_raw_payload(
        &self,
        owned_worker_id: &OwnedWorkerId,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String>;
}

/// Level of commit guarantees
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CommitLevel {
    /// Always commit immediately and do not return until it is done
    Always,
    /// Only commit immediately if the worker is durable
    DurableOnly,
}

/// An open oplog providing write access
#[async_trait]
pub trait Oplog: Any + Debug + Send + Sync {
    /// Adds a single entry to the oplog (possibly buffered), and returns its index
    async fn add(&self, entry: OplogEntry) -> OplogIndex;

    /// A variant of add that can inject failures in tests. TO BE REMOVED
    async fn fallible_add(&self, entry: OplogEntry) -> Result<(), String> {
        self.add(entry).await;
        Ok(())
    }

    /// Drop a chunk of entries from the beginning of the oplog
    ///
    /// This should only be called _after_ `append` succeeded in the layer below this one
    ///
    /// Returns the number of dropped entries.
    async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64;

    /// Commits the buffered entries to the oplog
    async fn commit(&self, level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry>;

    /// Returns the current oplog index
    async fn current_oplog_index(&self) -> OplogIndex;

    /// Returns the index of the last non-hint entry which was added in this session with `add`. If
    /// there is no such entry, returns `None`.
    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex>;

    /// Waits until indexed store writes all changes into at least `replicas` replicas (or the maximum
    /// available).
    /// Returns true if the maximum possible number of replicas is reached within the timeout,
    /// otherwise false.
    async fn wait_for_replicas(&self, replicas: u8, timeout: Duration) -> bool;

    /// Reads the entry at the given oplog index
    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry;

    /// Reads the entry at the given oplog index
    async fn read_many(&self, oplog_index: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry>;

    /// Gets the total number of entries in the oplog
    async fn length(&self) -> u64;

    /// Adds an entry to the oplog and immediately commits it
    async fn add_and_commit(&self, entry: OplogEntry) -> OplogIndex {
        let index = self.add(entry).await;
        self.commit(CommitLevel::Always).await;
        index
    }

    /// Uploads a big oplog payload and returns a reference to it
    async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String>;

    /// Downloads a big oplog payload by its reference
    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String>;

    /// Switched to a different persistence level. This can be used as an optimization hint in the implementations.
    async fn switch_persistence_level(&self, mode: PersistenceLevel);
}

pub(crate) fn downcast_oplog<T: Oplog>(oplog: &Arc<dyn Oplog>) -> Option<Arc<T>> {
    if oplog.deref().type_id() == TypeId::of::<T>() {
        let raw: *const dyn Oplog = Arc::into_raw(oplog.clone());
        let raw: *const T = raw.cast();
        Some(unsafe { Arc::from_raw(raw) })
    } else {
        None
    }
}

#[async_trait]
pub trait OplogOps: Oplog {
    /// Uploads a big oplog payload and returns a reference to it
    async fn upload_payload<T: BinaryCodec + Debug + Clone + PartialEq + Sync>(
        &self,
        data: &T,
    ) -> Result<OplogPayload<T>, String> {
        let bytes = serialize(&data)?;
        let raw_payload = self.upload_raw_payload(bytes).await?;
        let payload = raw_payload.into_payload()?;
        Ok(payload)
    }

    /// Downloads a big oplog payload by its reference
    async fn download_payload<T: BinaryCodec + Debug + Clone + PartialEq + Send>(
        &self,
        payload: OplogPayload<T>,
    ) -> Result<T, String> {
        match payload {
            OplogPayload::Inline(value) => Ok(*value),
            OplogPayload::SerializedInline(data) => deserialize(&data),
            OplogPayload::External {
                payload_id,
                md5_hash,
            } => {
                let bytes = self.download_raw_payload(payload_id, md5_hash).await?;
                deserialize(&bytes)
            }
        }
    }

    async fn add_imported_function_invoked(
        &self,
        function_name: HostFunctionName,
        request: &HostRequest,
        response: &HostResponse,
        function_type: DurableFunctionType,
    ) -> Result<OplogEntry, String> {
        let request_payload: OplogPayload<HostRequest> = self.upload_payload(request).await?;
        let response_payload: OplogPayload<HostResponse> = self.upload_payload(response).await?;
        let entry = OplogEntry::ImportedFunctionInvoked {
            timestamp: Timestamp::now_utc(),
            function_name,
            request: request_payload,
            response: response_payload,
            durable_function_type: function_type,
        };
        self.add(entry.clone()).await;
        Ok(entry)
    }

    async fn add_exported_function_invoked(
        &self,
        function_name: String,
        request: &Vec<Value>,
        idempotency_key: IdempotencyKey,
        invocation_context: InvocationContextStack,
    ) -> Result<OplogEntry, String> {
        let payload = self.upload_payload(request).await?;
        let entry = OplogEntry::ExportedFunctionInvoked {
            timestamp: Timestamp::now_utc(),
            function_name,
            request: payload,
            idempotency_key,
            invocation_context: invocation_context.to_oplog_data(),
            trace_id: invocation_context.trace_id,
            trace_states: invocation_context.trace_states,
        };

        self.add(entry.clone()).await;
        Ok(entry)
    }

    async fn add_exported_function_completed(
        &self,
        response: &Option<ValueAndType>,
        consumed_fuel: u64,
    ) -> Result<OplogEntry, String> {
        // TODO: align types
        let consumed_fuel = if consumed_fuel > i64::MAX as u64 {
            i64::MAX
        } else {
            consumed_fuel as i64
        };

        let payload = self.upload_payload(response).await?;
        let entry = OplogEntry::ExportedFunctionCompleted {
            timestamp: Timestamp::now_utc(),
            response: payload,
            consumed_fuel,
        };
        self.add(entry.clone()).await;
        Ok(entry)
    }

    async fn create_snapshot_based_update_description(
        &self,
        target_revision: ComponentRevision,
        payload: Vec<u8>,
        mime_type: String,
    ) -> Result<UpdateDescription, String> {
        let payload = self.upload_payload(&payload).await?;
        Ok(UpdateDescription::SnapshotBased {
            target_revision,
            payload,
            mime_type,
        })
    }

    async fn get_upload_description_payload(
        &self,
        description: UpdateDescription,
    ) -> Result<Option<(Vec<u8>, String)>, String> {
        match description {
            UpdateDescription::SnapshotBased {
                payload, mime_type, ..
            } => {
                let bytes = self.download_payload(payload).await?;
                Ok(Some((bytes, mime_type)))
            }
            UpdateDescription::Automatic { .. } => Ok(None),
        }
    }
}

#[async_trait]
impl<O: Oplog + ?Sized> OplogOps for O {}

#[async_trait]
pub trait OplogServiceOps: OplogService {
    /// Uploads a big oplog payload and returns a reference to it
    async fn upload_payload<T: BinaryCodec + Debug + Clone + PartialEq + Sync>(
        &self,
        owned_worker_id: &OwnedWorkerId,
        data: &T,
    ) -> Result<OplogPayload<T>, String> {
        let bytes = serialize(&data)?;
        let raw_payload = self.upload_raw_payload(owned_worker_id, bytes).await?;
        let payload = raw_payload.into_payload()?;
        Ok(payload)
    }

    /// Downloads a big oplog payload by its reference
    async fn download_payload<T: BinaryCodec + Debug + Clone + PartialEq + Send>(
        &self,
        owned_worker_id: &OwnedWorkerId,
        payload: OplogPayload<T>,
    ) -> Result<T, String> {
        match payload {
            OplogPayload::Inline(value) => Ok(*value),
            OplogPayload::SerializedInline(data) => deserialize(&data),
            OplogPayload::External {
                payload_id,
                md5_hash,
            } => {
                let bytes = self
                    .download_raw_payload(owned_worker_id, payload_id, md5_hash)
                    .await?;
                deserialize(&bytes)
            }
        }
    }
}

#[async_trait]
impl<O: OplogService + ?Sized> OplogServiceOps for O {}

#[derive(Clone)]
struct OpenOplogEntry {
    pub oplog: Weak<dyn Oplog>,
    pub initial: Arc<AtomicBool>,
}

impl OpenOplogEntry {
    pub fn new(oplog: Arc<dyn Oplog>) -> Self {
        Self {
            oplog: Arc::downgrade(&oplog),
            initial: Arc::new(AtomicBool::new(true)),
        }
    }
}

#[derive(Clone)]
pub struct OpenOplogs {
    oplogs: Cache<WorkerId, (), OpenOplogEntry, ()>,
}

impl OpenOplogs {
    pub fn new(name: &'static str) -> Self {
        Self {
            oplogs: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                name,
            ),
        }
    }

    pub async fn get_or_open(
        &self,
        worker_id: &WorkerId,
        constructor: impl OplogConstructor + 'static,
    ) -> Arc<dyn Oplog> {
        loop {
            let constructor_clone = constructor.clone();
            let close = Box::new(self.oplogs.create_weak_remover(worker_id.clone()));

            let entry = self
                .oplogs
                .get_or_insert(
                    worker_id,
                    || (),
                    async |_| {
                        let result = constructor_clone.create_oplog(close).await;

                        // Temporarily increasing ref count because we want to store a weak pointer
                        // but not drop it before we re-gain a strong reference when got out of the cache
                        let result = unsafe {
                            let ptr = Arc::into_raw(result);
                            Arc::increment_strong_count(ptr);
                            Arc::from_raw(ptr)
                        };
                        Ok(OpenOplogEntry::new(result))
                    },
                )
                .await
                .unwrap();
            if let Some(oplog) = entry.oplog.upgrade() {
                let oplog = if entry.initial.load(Ordering::Acquire) {
                    let oplog = unsafe {
                        let ptr = Arc::into_raw(oplog);
                        Arc::decrement_strong_count(ptr);
                        Arc::from_raw(ptr)
                    };
                    entry.initial.store(false, Ordering::Release);
                    oplog
                } else {
                    oplog
                };

                break oplog;
            } else {
                self.oplogs.remove(worker_id).await;
                continue;
            }
        }
    }
}

impl Debug for OpenOplogs {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenOplogs").finish()
    }
}

#[async_trait]
pub trait OplogConstructor: Clone + Send {
    async fn create_oplog(self, close: Box<dyn FnOnce() + Send + Sync>) -> Arc<dyn Oplog>;
}
