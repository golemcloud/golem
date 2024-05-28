// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use bincode::{Decode, Encode};
use bytes::Bytes;
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;

pub use compressed::CompressedOplogArchive;
use golem_common::model::oplog::{
    OplogEntry, OplogIndex, OplogPayload, UpdateDescription, WrappedFunctionType,
};
use golem_common::model::{
    AccountId, CallingConvention, ComponentId, ComponentVersion, IdempotencyKey, ScanCursor,
    Timestamp, WorkerId,
};
use golem_common::serialization::{serialize, try_deserialize};

pub use compressed::CompressedOplogArchiveService;
pub use multilayer::{MultiLayerOplogService, OplogArchiveService};
pub use primary::PrimaryOplogService;

use crate::error::GolemError;

mod compressed;
mod multilayer;
mod primary;

#[cfg(any(feature = "mocks", test))]
pub mod mock;
#[cfg(test)]
mod tests;

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
pub trait OplogService: Debug {
    async fn create(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        initial_entry: OplogEntry,
    ) -> Arc<dyn Oplog + Send + Sync + 'static>;
    async fn open(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
    ) -> Arc<dyn Oplog + Send + Sync + 'static>;

    async fn get_first_index(&self, worker_id: &WorkerId) -> OplogIndex;
    async fn get_last_index(&self, worker_id: &WorkerId) -> OplogIndex;

    async fn delete(&self, worker_id: &WorkerId);

    async fn read(
        &self,
        worker_id: &WorkerId,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry>;

    /// Reads an inclusive range of entries from the oplog
    async fn read_range(
        &self,
        worker_id: &WorkerId,
        start_idx: OplogIndex,
        last_idx: OplogIndex,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.read(
            worker_id,
            start_idx,
            Into::<u64>::into(last_idx) - Into::<u64>::into(start_idx) + 1,
        )
        .await
    }

    async fn read_prefix(
        &self,
        worker_id: &WorkerId,
        last_idx: OplogIndex,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.read_range(worker_id, OplogIndex::INITIAL, last_idx)
            .await
    }

    /// Checks whether the oplog exists in the oplog, without opening it
    async fn exists(&self, worker_id: &WorkerId) -> bool;

    /// Scans the oplog for all workers belonging to the given component, in a paginated way.
    ///
    /// Pages can be empty. This operation is slow and is not locking the oplog.
    async fn scan_for_component(
        &self,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<WorkerId>), GolemError>;
}

/// An open oplog providing write access
#[async_trait]
pub trait Oplog: Debug {
    /// Adds a single entry to the oplog (possibly buffered)
    async fn add(&self, entry: OplogEntry);

    /// Drop a chunk of entries from the beginning of the oplog
    ///
    /// This should only be called _after_ `append` succeeded in the layer below this one
    async fn drop_prefix(&self, last_dropped_id: OplogIndex);

    /// Commits the buffered entries to the oplog
    async fn commit(&self);

    /// Returns the current oplog index
    async fn current_oplog_index(&self) -> OplogIndex;

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
        self.commit().await;
        self.current_oplog_index().await
    }

    /// Uploads a big oplog payload and returns a reference to it
    async fn upload_payload(&self, data: &[u8]) -> Result<OplogPayload, String>;

    /// Downloads a big oplog payload by its reference
    async fn download_payload(&self, payload: &OplogPayload) -> Result<Bytes, String>;
}

#[async_trait]
pub trait OplogOps: Oplog {
    async fn add_imported_function_invoked<R: Encode + Sync>(
        &self,
        function_name: String,
        response: &R,
        wrapped_function_type: WrappedFunctionType,
    ) -> Result<OplogEntry, String> {
        let serialized_response = serialize(response)?.to_vec();

        let payload = self.upload_payload(&serialized_response).await?;
        let entry = OplogEntry::ImportedFunctionInvoked {
            timestamp: Timestamp::now_utc(),
            function_name,
            response: payload,
            wrapped_function_type,
        };
        self.add(entry.clone()).await;
        Ok(entry)
    }

    async fn add_exported_function_invoked<R: Encode + Sync>(
        &self,
        function_name: String,
        request: &R,
        idempotency_key: IdempotencyKey,
        calling_convention: Option<CallingConvention>,
    ) -> Result<OplogEntry, String> {
        let serialized_request = serialize(request)?.to_vec();

        let payload = self.upload_payload(&serialized_request).await?;
        let entry = OplogEntry::ExportedFunctionInvoked {
            timestamp: Timestamp::now_utc(),
            function_name,
            request: payload,
            idempotency_key,
            calling_convention,
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

    async fn get_payload_of_entry<T: Decode>(
        &self,
        entry: &OplogEntry,
    ) -> Result<Option<T>, String> {
        match entry {
            OplogEntry::ImportedFunctionInvoked { response, .. } => {
                let response_bytes: Bytes = self.download_payload(response).await?;
                try_deserialize(&response_bytes)
            }
            OplogEntry::ExportedFunctionInvoked { request, .. } => {
                let response_bytes: Bytes = self.download_payload(request).await?;
                try_deserialize(&response_bytes)
            }
            OplogEntry::ExportedFunctionCompleted { response, .. } => {
                let response_bytes: Bytes = self.download_payload(response).await?;
                try_deserialize(&response_bytes)
            }
            _ => Ok(None),
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

#[derive(Debug, Clone)]
struct OpenOplogs {
    oplogs: Arc<DashMap<WorkerId, Weak<dyn Oplog + Send + Sync>>>,
}

impl OpenOplogs {
    fn new() -> Self {
        Self {
            oplogs: Arc::new(DashMap::new()),
        }
    }

    async fn get_or_open(
        &self,
        worker_id: &WorkerId,
        constructor: impl OplogConstructor,
    ) -> Arc<dyn Oplog + Send + Sync> {
        let oplogs_clone = self.oplogs.clone();
        let worker_id_clone = worker_id.clone();
        let entry = self.oplogs.entry(worker_id.clone());
        match entry {
            Entry::Occupied(existing) => match existing.get().upgrade() {
                Some(oplog) => oplog,
                None => {
                    let close = Box::new(move || {
                        oplogs_clone.remove(&worker_id_clone);
                    });
                    let oplog = constructor.create_oplog(close).await;
                    existing.replace_entry(Arc::downgrade(&oplog));
                    oplog
                }
            },
            Entry::Vacant(entry) => {
                let close = Box::new(move || {
                    oplogs_clone.remove(&worker_id_clone);
                });
                let oplog = constructor.create_oplog(close).await;
                entry.insert(Arc::downgrade(&oplog));
                oplog
            }
        }
    }
}

#[async_trait]
trait OplogConstructor {
    async fn create_oplog(
        self,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Arc<dyn Oplog + Send + Sync>;
}
