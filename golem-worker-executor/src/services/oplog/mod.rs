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
use bincode::{Decode, Encode};
pub use blob::BlobOplogArchiveService;
use bytes::Bytes;
pub use compressed::{CompressedOplogArchive, CompressedOplogArchiveService, CompressedOplogChunk};
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{
    DurableFunctionType, OplogEntry, OplogIndex, OplogPayload, UpdateDescription,
};
use golem_common::model::{
    ComponentId, ComponentVersion, IdempotencyKey, OwnedWorkerId, ProjectId, ScanCursor, Timestamp,
    WorkerId, WorkerMetadata, WorkerStatusRecord,
};
use golem_common::read_only_lock;
use golem_common::serialization::{serialize, try_deserialize};
use golem_service_base::error::worker_executor::WorkerExecutorError;
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
        project_id: &ProjectId,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedWorkerId>), WorkerExecutorError>;

    /// Uploads a big oplog payload and returns a reference to it
    async fn upload_payload(
        &self,
        owned_worker_id: &OwnedWorkerId,
        data: &[u8],
    ) -> Result<OplogPayload, String>;

    /// Downloads a big oplog payload by its reference
    async fn download_payload(
        &self,
        owned_worker_id: &OwnedWorkerId,
        payload: &OplogPayload,
    ) -> Result<Bytes, String>;
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

    async fn add_safe(&self, entry: OplogEntry) -> Result<(), String> {
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

    /// Gets the total number of entries in the oplog
    async fn length(&self) -> u64;

    /// Adds an entry to the oplog and immediately commits it
    async fn add_and_commit(&self, entry: OplogEntry) -> OplogIndex {
        self.add(entry).await;
        self.commit(CommitLevel::Always).await;
        self.current_oplog_index().await
    }

    /// Uploads a big oplog payload and returns a reference to it
    async fn upload_payload(&self, data: &[u8]) -> Result<OplogPayload, String>;

    /// Downloads a big oplog payload by its reference
    async fn download_payload(&self, payload: &OplogPayload) -> Result<Bytes, String>;
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
    async fn add_imported_function_invoked<I: Encode + Sync, O: Encode + Sync>(
        &self,
        function_name: String,
        request: &I,
        response: &O,
        function_type: DurableFunctionType,
    ) -> Result<OplogEntry, String> {
        let serialized_request = serialize(request)?.to_vec();
        let serialized_response = serialize(response)?.to_vec();

        self.add_raw_imported_function_invoked(
            function_name,
            &serialized_request,
            &serialized_response,
            function_type,
        )
        .await
    }

    async fn add_raw_imported_function_invoked(
        &self,
        function_name: String,
        serialized_request: &[u8],
        serialized_response: &[u8],
        function_type: DurableFunctionType,
    ) -> Result<OplogEntry, String> {
        let request_payload: OplogPayload = self.upload_payload(serialized_request).await?;
        let response_payload = self.upload_payload(serialized_response).await?;
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

    async fn add_exported_function_invoked<R: Encode + Sync>(
        &self,
        function_name: String,
        request: &R,
        idempotency_key: IdempotencyKey,
        invocation_context: InvocationContextStack,
    ) -> Result<OplogEntry, String> {
        let serialized_request = serialize(request)?.to_vec();

        let payload = self.upload_payload(&serialized_request).await?;
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

    async fn add_exported_function_completed<R: Encode + Sync>(
        &self,
        response: &R,
        consumed_fuel: i64,
    ) -> Result<OplogEntry, String> {
        let serialized_response = serialize(response)?.to_vec();

        let payload = self.upload_payload(&serialized_response).await?;
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
        target_version: ComponentVersion,
        payload: &[u8],
    ) -> Result<UpdateDescription, String> {
        let payload = self.upload_payload(payload).await?;
        Ok(UpdateDescription::SnapshotBased {
            target_version,
            payload,
        })
    }

    async fn get_raw_payload_of_entry(&self, entry: &OplogEntry) -> Result<Option<Bytes>, String> {
        match entry {
            OplogEntry::ImportedFunctionInvoked { response, .. } => {
                Ok(Some(self.download_payload(response).await?))
            }
            OplogEntry::ExportedFunctionInvoked { request, .. } => {
                Ok(Some(self.download_payload(request).await?))
            }
            OplogEntry::ExportedFunctionCompleted { response, .. } => {
                Ok(Some(self.download_payload(response).await?))
            }
            _ => Ok(None),
        }
    }

    async fn get_payload_of_entry<T: Decode<()>>(
        &self,
        entry: &OplogEntry,
    ) -> Result<Option<T>, String> {
        match self.get_raw_payload_of_entry(entry).await? {
            Some(response_bytes) => try_deserialize(&response_bytes),
            None => Ok(None),
        }
    }

    async fn get_upload_description_payload(
        &self,
        description: &UpdateDescription,
    ) -> Result<Option<Bytes>, String> {
        match description {
            UpdateDescription::SnapshotBased { payload, .. } => {
                let bytes: Bytes = self.download_payload(payload).await?;
                Ok(Some(bytes))
            }
            UpdateDescription::Automatic { .. } => Ok(None),
        }
    }
}

#[async_trait]
impl<O: Oplog + ?Sized> OplogOps for O {}

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
