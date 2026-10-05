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

use crate::services::resource_limits::AtomicResourceEntry;
use async_trait::async_trait;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::types::ObjectMetadata;
use golem_service_base::storage::blob::{
    BlobStorage, BlobStorageLabelledApi, BlobStorageNamespace, ExistsResult,
    blob_file_name_to_string, blob_path_to_string, join_blob_path,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use tokio::sync::Mutex;

fn signed_delta(new_size: u64, old_size: u64) -> i64 {
    if new_size >= old_size {
        new_size.saturating_sub(old_size).min(i64::MAX as u64) as i64
    } else {
        -(old_size.saturating_sub(new_size).min(i64::MAX as u64) as i64)
    }
}

/// Typed errors for blob store operations, enabling semantic retry classification
#[derive(Debug, Clone)]
pub enum BlobStoreError {
    /// The requested object or container was not found
    NotFound(String),
    /// The container or object already exists
    AlreadyExists(String),
    /// Permission denied
    PermissionDenied(String),
    /// Invalid input (bad name, bad range, etc.)
    InvalidInput(String),
    /// The account's blob storage byte limit would be exceeded
    LimitExceeded(String),
    /// Transient backend failure (network, timeout, etc.)
    TransientBackend(String),
    /// Other/unknown error
    Other(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlobStoreMutation {
    pub bytes_delta: i64,
    pub objects_deleted: u64,
}

impl std::fmt::Display for BlobStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(msg) => write!(f, "Not found: {msg}"),
            Self::AlreadyExists(msg) => write!(f, "Already exists: {msg}"),
            Self::PermissionDenied(msg) => write!(f, "Permission denied: {msg}"),
            Self::InvalidInput(msg) => write!(f, "Invalid input: {msg}"),
            Self::LimitExceeded(msg) => write!(f, "Limit exceeded: {msg}"),
            Self::TransientBackend(msg) => write!(f, "Backend error: {msg}"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

/// Interface for storing blobs in a persistent storage.
#[async_trait]
pub trait BlobStoreService: Send + Sync {
    async fn clear(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn container_exists(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<bool, BlobStoreError>;

    async fn copy_object(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn create_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn delete_container(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn delete_object(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn delete_objects(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: &str,
        object_names: &[String],
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn get_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<Option<u64>, BlobStoreError>;

    async fn get_data(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>, BlobStoreError>;

    async fn has_object(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<bool, BlobStoreError>;

    async fn list_objects(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<Vec<String>, BlobStoreError>;

    async fn move_object(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn object_info(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<ObjectMetadata, BlobStoreError>;

    async fn write_data(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: &str,
        object_name: &str,
        data: &[u8],
    ) -> Result<BlobStoreMutation, BlobStoreError>;
}

pub struct DefaultBlobStoreService {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    mutation_locks: StdMutex<HashMap<EnvironmentId, Weak<Mutex<()>>>>,
}

impl DefaultBlobStoreService {
    pub fn new(blob_storage: Arc<dyn BlobStorage + Send + Sync>) -> Self {
        Self {
            blob_storage,
            mutation_locks: StdMutex::new(HashMap::new()),
        }
    }

    fn mutation_lock(&self, environment_id: EnvironmentId) -> Arc<Mutex<()>> {
        let mut locks = self.mutation_locks.lock().unwrap();
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&environment_id).and_then(Weak::upgrade) {
            return lock;
        }

        let lock = Arc::new(Mutex::new(()));
        locks.insert(environment_id, Arc::downgrade(&lock));
        lock
    }

    fn container_path(container_name: &str) -> Result<PathBuf, BlobStoreError> {
        join_blob_path(container_name, "")
            .map_err(|err| BlobStoreError::InvalidInput(err.to_string()))
    }

    fn object_path(container_name: &str, object_name: &str) -> Result<PathBuf, BlobStoreError> {
        join_blob_path(container_name, object_name)
            .map_err(|err| BlobStoreError::InvalidInput(err.to_string()))
    }

    fn blob_path_identity(path: &Path) -> Result<String, BlobStoreError> {
        blob_path_to_string(path).map_err(|err| BlobStoreError::InvalidInput(err.to_string()))
    }

    fn object_name(path: &Path) -> Result<String, BlobStoreError> {
        blob_file_name_to_string(path).map_err(|err| BlobStoreError::InvalidInput(err.to_string()))
    }
}

#[async_trait]
impl BlobStoreService for DefaultBlobStoreService {
    async fn clear(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let path = Self::container_path(&container_name)?;
        let blob_storage = self.blob_storage.with("blob_store", "clear");
        if blob_storage
            .exists(namespace.clone(), &path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
            == ExistsResult::DoesNotExist
        {
            blob_storage
                .create_dir(namespace, &path)
                .await
                .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
            return Ok(BlobStoreMutation::default());
        }
        let blobs = blob_storage
            .list_blobs_below(namespace.clone(), &path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        let deleted_bytes = blobs
            .iter()
            .fold(0u64, |sum, (_, metadata)| sum.saturating_add(metadata.size));
        if !blob_storage
            .delete_dir(namespace.clone(), &path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
        {
            return Ok(BlobStoreMutation::default());
        }
        // Re-create the empty container directory so the container continues to exist.
        // clear() semantics: remove all objects, keep the container itself.
        blob_storage
            .create_dir(namespace, &path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        let mutation = BlobStoreMutation {
            bytes_delta: -(deleted_bytes.min(i64::MAX as u64) as i64),
            objects_deleted: blobs.len() as u64,
        };
        let recorded = resource_limits.try_record_blob_storage_delta(mutation.bytes_delta);
        debug_assert!(recorded);
        Ok(mutation)
    }

    async fn container_exists(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<bool, BlobStoreError> {
        let path = Self::container_path(&container_name)?;
        self.blob_storage
            .with("blob_store", "container_exists")
            .exists(
                BlobStorageNamespace::CustomStorage { environment_id },
                &path,
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))
            .map(|result| match result {
                ExistsResult::Directory => true,
                ExistsResult::File => false,
                ExistsResult::DoesNotExist => false,
            })
    }

    async fn copy_object(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let source = Self::object_path(&source_container_name, &source_object_name)?;
        let destination = Self::object_path(&destination_container_name, &destination_object_name)?;
        let blob_storage = self.blob_storage.with("blob_store", "copy_object");
        let source_size = blob_storage
            .get_metadata(namespace.clone(), &source)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
            .ok_or_else(|| BlobStoreError::NotFound("Source object does not exist".to_string()))?
            .size;
        let old_destination_size = blob_storage
            .get_metadata(namespace.clone(), &destination)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
            .map_or(0, |metadata| metadata.size);
        let delta = signed_delta(source_size, old_destination_size);
        if delta > 0 && !resource_limits.try_record_blob_storage_delta(delta) {
            return Err(BlobStoreError::LimitExceeded(
                "Blob storage byte limit exceeded".to_string(),
            ));
        }
        if let Err(error) = blob_storage.copy(namespace, &source, &destination).await {
            if delta > 0 {
                resource_limits.rollback_blob_storage_delta(delta);
            }
            return Err(BlobStoreError::TransientBackend(error.to_string()));
        }
        if delta < 0 {
            let recorded = resource_limits.try_record_blob_storage_delta(delta);
            debug_assert!(recorded);
        }
        Ok(BlobStoreMutation {
            bytes_delta: delta,
            objects_deleted: 0,
        })
    }

    async fn create_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        let path = Self::container_path(&container_name)?;
        self.blob_storage
            .with("blob_store", "create_container")
            .create_dir(
                BlobStorageNamespace::CustomStorage { environment_id },
                &path,
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        Ok(BlobStoreMutation::default())
    }

    async fn delete_container(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let path = Self::container_path(&container_name)?;
        let blob_storage = self.blob_storage.with("blob_store", "delete_container");
        if blob_storage
            .exists(namespace.clone(), &path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
            == ExistsResult::DoesNotExist
        {
            return Ok(BlobStoreMutation::default());
        }
        let blobs = blob_storage
            .list_blobs_below(namespace.clone(), &path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        let deleted_bytes = blobs
            .iter()
            .fold(0u64, |sum, (_, metadata)| sum.saturating_add(metadata.size));
        if !blob_storage
            .delete_dir(namespace, &path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
        {
            return Ok(BlobStoreMutation::default());
        }
        let mutation = BlobStoreMutation {
            bytes_delta: -(deleted_bytes.min(i64::MAX as u64) as i64),
            objects_deleted: blobs.len() as u64,
        };
        let recorded = resource_limits.try_record_blob_storage_delta(mutation.bytes_delta);
        debug_assert!(recorded);
        Ok(mutation)
    }

    async fn delete_object(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let path = Self::object_path(&container_name, &object_name)?;
        let blob_storage = self.blob_storage.with("blob_store", "delete_object");
        let old_metadata = blob_storage
            .get_metadata(namespace.clone(), &path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        let old_size = old_metadata.as_ref().map_or(0, |metadata| metadata.size);
        blob_storage
            .delete(namespace, &path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        let mutation = BlobStoreMutation {
            bytes_delta: -(old_size.min(i64::MAX as u64) as i64),
            objects_deleted: u64::from(old_metadata.is_some()),
        };
        let recorded = resource_limits.try_record_blob_storage_delta(mutation.bytes_delta);
        debug_assert!(recorded);
        Ok(mutation)
    }

    async fn delete_objects(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: &str,
        object_names: &[String],
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        Self::container_path(container_name)?;
        let paths: HashMap<String, PathBuf> = object_names
            .iter()
            .map(|object_name| {
                let path = Self::object_path(container_name, object_name)?;
                Ok((Self::blob_path_identity(&path)?, path))
            })
            .collect::<Result<_, _>>()?;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let mut deleted_bytes = 0u64;
        let mut existing_paths = Vec::new();
        let blob_storage = self.blob_storage.with("blob_store", "delete_objects");
        for path in paths.values() {
            if let Some(metadata) = blob_storage
                .get_metadata(namespace.clone(), path)
                .await
                .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
            {
                deleted_bytes = deleted_bytes.saturating_add(metadata.size);
                existing_paths.push(path.clone());
            }
        }
        if existing_paths.is_empty() {
            return Ok(BlobStoreMutation::default());
        }
        blob_storage
            .delete_many(namespace, &existing_paths)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        let mutation = BlobStoreMutation {
            bytes_delta: -(deleted_bytes.min(i64::MAX as u64) as i64),
            objects_deleted: existing_paths.len() as u64,
        };
        let recorded = resource_limits.try_record_blob_storage_delta(mutation.bytes_delta);
        debug_assert!(recorded);
        Ok(mutation)
    }

    async fn get_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<Option<u64>, BlobStoreError> {
        let path = Self::container_path(&container_name)?;
        self.blob_storage
            .with("blob_store", "get_container")
            .get_metadata(
                BlobStorageNamespace::CustomStorage { environment_id },
                &path,
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))
            .map(|result| result.map(|metadata| metadata.last_modified_at.to_millis()))
    }

    async fn get_data(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>, BlobStoreError> {
        let path = Self::object_path(&container_name, &object_name)?;
        let data = self
            .blob_storage
            .with("blob_store", "get_data")
            .get_raw_slice(
                BlobStorageNamespace::CustomStorage { environment_id },
                &path,
                start,
                end,
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;

        match data {
            Some(data) => Ok(data.to_vec()),
            None => Err(BlobStoreError::NotFound(
                "Object does not exist".to_string(),
            )),
        }
    }

    async fn has_object(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<bool, BlobStoreError> {
        let path = Self::object_path(&container_name, &object_name)?;
        self.blob_storage
            .with("blob_store", "has_object")
            .exists(
                BlobStorageNamespace::CustomStorage { environment_id },
                &path,
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))
            .map(|result| match result {
                ExistsResult::Directory => false,
                ExistsResult::File => true,
                ExistsResult::DoesNotExist => false,
            })
    }

    async fn list_objects(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<Vec<String>, BlobStoreError> {
        let path = Self::container_path(&container_name)?;
        self.blob_storage
            .with("blob_store", "list_objects")
            .list_dir(
                BlobStorageNamespace::CustomStorage { environment_id },
                &path,
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))
            .and_then(|paths| paths.iter().map(|path| Self::object_name(path)).collect())
    }

    async fn move_object(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let source = Self::object_path(&source_container_name, &source_object_name)?;
        let destination = Self::object_path(&destination_container_name, &destination_object_name)?;
        let blob_storage = self.blob_storage.with("blob_store", "move_object");
        if Self::blob_path_identity(&source)? == Self::blob_path_identity(&destination)? {
            blob_storage
                .get_metadata(namespace, &source)
                .await
                .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
                .ok_or_else(|| {
                    BlobStoreError::NotFound("Source object does not exist".to_string())
                })?;
            return Ok(BlobStoreMutation::default());
        }
        let old_destination = blob_storage
            .get_metadata(namespace.clone(), &destination)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        let old_destination_size = old_destination.as_ref().map_or(0, |metadata| metadata.size);
        blob_storage
            .r#move(namespace, &source, &destination)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        let mutation = BlobStoreMutation {
            bytes_delta: -(old_destination_size.min(i64::MAX as u64) as i64),
            objects_deleted: u64::from(old_destination.is_some()),
        };
        let recorded = resource_limits.try_record_blob_storage_delta(mutation.bytes_delta);
        debug_assert!(recorded);
        Ok(mutation)
    }

    async fn object_info(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<ObjectMetadata, BlobStoreError> {
        let path = Self::object_path(&container_name, &object_name)?;
        match self
            .blob_storage
            .with("blob_store", "object_info")
            .get_metadata(
                BlobStorageNamespace::CustomStorage { environment_id },
                &path,
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
        {
            Some(metadata) => Ok(ObjectMetadata {
                name: object_name,
                container: container_name,
                created_at: metadata.last_modified_at.to_millis(),
                size: metadata.size,
            }),
            None => Err(BlobStoreError::NotFound(
                "Object does not exist".to_string(),
            )),
        }
    }

    async fn write_data(
        &self,
        resource_limits: Arc<AtomicResourceEntry>,
        environment_id: EnvironmentId,
        container_name: &str,
        object_name: &str,
        data: &[u8],
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock(environment_id).lock_owned().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let path = Self::object_path(container_name, object_name)?;
        let blob_storage = self.blob_storage.with("blob_store", "write_data");
        let old_size = blob_storage
            .get_metadata(namespace.clone(), &path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
            .map_or(0, |metadata| metadata.size);
        let delta = signed_delta(data.len() as u64, old_size);
        if delta > 0 && !resource_limits.try_record_blob_storage_delta(delta) {
            return Err(BlobStoreError::LimitExceeded(
                "Blob storage byte limit exceeded".to_string(),
            ));
        }
        if let Err(error) = blob_storage.put_raw(namespace, &path, data).await {
            if delta > 0 {
                resource_limits.rollback_blob_storage_delta(delta);
            }
            return Err(BlobStoreError::TransientBackend(error.to_string()));
        }
        if delta < 0 {
            let recorded = resource_limits.try_record_blob_storage_delta(delta);
            debug_assert!(recorded);
        }
        Ok(BlobStoreMutation {
            bytes_delta: delta,
            objects_deleted: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::services::blob_store::{
        BlobStoreError, BlobStoreMutation, BlobStoreService, DefaultBlobStoreService,
    };
    use crate::services::resource_limits::AtomicResourceEntry;
    use anyhow::Error;
    use async_trait::async_trait;
    use bytes::Bytes;
    use futures::stream::BoxStream;
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::replayable_stream::ErasedReplayableStream;
    use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use golem_service_base::storage::blob::{
        BlobMetadata, BlobRangeStream, BlobStorage, BlobStorageNamespace, ExistsResult,
    };
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tempfile::TempDir;
    use test_r::test;

    fn unlimited_limits() -> Arc<AtomicResourceEntry> {
        Arc::new(AtomicResourceEntry::new(
            u64::MAX,
            usize::MAX,
            usize::MAX,
            u64::MAX,
            u64::MAX,
        ))
    }

    #[derive(Debug)]
    struct FailingPutBlobStorage {
        inner: InMemoryBlobStorage,
        fail_next_put: AtomicBool,
        block_next_put: AtomicBool,
        put_started: tokio::sync::Notify,
        release_put: tokio::sync::Notify,
    }

    impl FailingPutBlobStorage {
        fn new() -> Self {
            Self {
                inner: InMemoryBlobStorage::new(),
                fail_next_put: AtomicBool::new(false),
                block_next_put: AtomicBool::new(false),
                put_started: tokio::sync::Notify::new(),
                release_put: tokio::sync::Notify::new(),
            }
        }

        fn fail_next_put(&self) {
            self.fail_next_put.store(true, Ordering::Release);
        }

        fn block_next_put(&self) {
            self.block_next_put.store(true, Ordering::Release);
        }

        async fn wait_for_blocked_put(&self) {
            self.put_started.notified().await;
        }

        fn release_blocked_put(&self) {
            self.release_put.notify_one();
        }
    }

    #[async_trait]
    impl BlobStorage for FailingPutBlobStorage {
        async fn get_raw(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &Path,
        ) -> Result<Option<Vec<u8>>, Error> {
            self.inner
                .get_raw(target_label, op_label, namespace, path)
                .await
        }

        async fn get_stream(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &Path,
        ) -> Result<Option<BoxStream<'static, Result<Bytes, Error>>>, Error> {
            self.inner
                .get_stream(target_label, op_label, namespace, path)
                .await
        }

        async fn get_range_stream(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &Path,
            offset: u64,
            length: u64,
        ) -> Result<Option<BlobRangeStream>, Error> {
            self.inner
                .get_range_stream(target_label, op_label, namespace, path, offset, length)
                .await
        }

        async fn get_metadata(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &Path,
        ) -> Result<Option<BlobMetadata>, Error> {
            self.inner
                .get_metadata(target_label, op_label, namespace, path)
                .await
        }

        async fn put_raw(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &Path,
            data: &[u8],
        ) -> Result<(), Error> {
            if self.fail_next_put.swap(false, Ordering::AcqRel) {
                return Err(anyhow::anyhow!("injected put failure"));
            }
            if self.block_next_put.swap(false, Ordering::AcqRel) {
                self.put_started.notify_one();
                self.release_put.notified().await;
            }
            self.inner
                .put_raw(target_label, op_label, namespace, path, data)
                .await
        }

        async fn put_stream(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &Path,
            stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
        ) -> Result<(), Error> {
            self.inner
                .put_stream(target_label, op_label, namespace, path, stream)
                .await
        }

        async fn delete(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &Path,
        ) -> Result<(), Error> {
            self.inner
                .delete(target_label, op_label, namespace, path)
                .await
        }

        async fn create_dir(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &Path,
        ) -> Result<(), Error> {
            self.inner
                .create_dir(target_label, op_label, namespace, path)
                .await
        }

        async fn list_dir(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &Path,
        ) -> Result<Vec<PathBuf>, Error> {
            self.inner
                .list_dir(target_label, op_label, namespace, path)
                .await
        }

        async fn delete_dir(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &Path,
        ) -> Result<bool, Error> {
            self.inner
                .delete_dir(target_label, op_label, namespace, path)
                .await
        }

        async fn exists(
            &self,
            target_label: &'static str,
            op_label: &'static str,
            namespace: BlobStorageNamespace,
            path: &Path,
        ) -> Result<ExistsResult, Error> {
            self.inner
                .exists(target_label, op_label, namespace, path)
                .await
        }
    }

    async fn test_container_exists(blob_store: &impl BlobStoreService) {
        let environment_id = EnvironmentId::new();
        assert!(
            !blob_store
                .container_exists(environment_id, "container1".to_string())
                .await
                .unwrap()
        );
        blob_store
            .create_container(environment_id, "container1".to_string())
            .await
            .unwrap();
        assert!(
            blob_store
                .container_exists(environment_id, "container1".to_string())
                .await
                .unwrap()
        );
    }

    async fn test_container_delete(blob_store: &impl BlobStoreService) {
        let environment_id = EnvironmentId::new();
        blob_store
            .create_container(environment_id, "container1".to_string())
            .await
            .unwrap();
        blob_store
            .delete_container(unlimited_limits(), environment_id, "container1".to_string())
            .await
            .unwrap();
        assert!(
            !blob_store
                .container_exists(environment_id, "container1".to_string())
                .await
                .unwrap()
        );
    }

    async fn test_container_has_write_read_has(blob_store: &impl BlobStoreService) {
        let environment_id = EnvironmentId::new();

        blob_store
            .create_container(environment_id, "container1".to_string())
            .await
            .unwrap();
        assert!(
            !blob_store
                .has_object(environment_id, "container1".to_string(), "obj1".to_string())
                .await
                .unwrap()
        );

        let original_data = vec![1, 2, 3, 4];
        blob_store
            .write_data(
                unlimited_limits(),
                environment_id,
                "container1",
                "obj1",
                &original_data,
            )
            .await
            .unwrap();

        let read_data = blob_store
            .get_data(
                environment_id,
                "container1".to_string(),
                "obj1".to_string(),
                0,
                4,
            )
            .await
            .unwrap();

        assert_eq!(original_data, read_data);
        assert!(
            blob_store
                .has_object(environment_id, "container1".to_string(), "obj1".to_string())
                .await
                .unwrap()
        );
    }

    async fn test_container_list_copy_move_list(blob_store: &impl BlobStoreService) {
        let environment_id = EnvironmentId::new();

        blob_store
            .create_container(environment_id, "container1".to_string())
            .await
            .unwrap();
        blob_store
            .create_container(environment_id, "container2".to_string())
            .await
            .unwrap();

        assert!(
            blob_store
                .list_objects(environment_id, "container1".to_string(),)
                .await
                .unwrap()
                .is_empty()
        );

        let original_data = vec![1, 2, 3, 4];
        blob_store
            .write_data(
                unlimited_limits(),
                environment_id,
                "container1",
                "obj1",
                &original_data,
            )
            .await
            .unwrap();

        blob_store
            .copy_object(
                unlimited_limits(),
                environment_id,
                "container1".to_string(),
                "obj1".to_string(),
                "container1".to_string(),
                "obj2".to_string(),
            )
            .await
            .unwrap();

        let mut result = blob_store
            .list_objects(environment_id, "container1".to_string())
            .await
            .unwrap();

        result.sort();

        assert_eq!(result, vec!["obj1", "obj2"]);

        blob_store
            .move_object(
                unlimited_limits(),
                environment_id,
                "container1".to_string(),
                "obj1".to_string(),
                "container2".to_string(),
                "obj3".to_string(),
            )
            .await
            .unwrap();

        assert_eq!(
            blob_store
                .list_objects(environment_id, "container1".to_string(),)
                .await
                .unwrap(),
            vec!["obj2"]
        );

        assert_eq!(
            blob_store
                .list_objects(environment_id, "container2".to_string(),)
                .await
                .unwrap(),
            vec!["obj3"]
        );
    }

    async fn test_empty_container_name(blob_store: &impl BlobStoreService) {
        let environment_id = EnvironmentId::new();
        blob_store
            .write_data(unlimited_limits(), environment_id, "", "x", b"data")
            .await
            .unwrap();
        assert!(
            blob_store
                .has_object(environment_id, String::new(), "x".to_string())
                .await
                .unwrap()
        );
    }

    async fn test_container_name_spellings(blob_store: &impl BlobStoreService) {
        for container_name in ["c/", "./c"] {
            let environment_id = EnvironmentId::new();
            blob_store
                .write_data(
                    unlimited_limits(),
                    environment_id,
                    container_name,
                    "x",
                    b"data",
                )
                .await
                .unwrap();
            assert_eq!(
                blob_store
                    .list_objects(environment_id, container_name.to_string())
                    .await
                    .unwrap(),
                vec!["x"]
            );
        }
    }

    async fn test_contract_object_name(blob_store: &impl BlobStoreService) {
        let environment_id = EnvironmentId::new();
        let object_name = r"animals\cat.png";
        blob_store
            .write_data(
                unlimited_limits(),
                environment_id,
                "photos",
                object_name,
                b"data",
            )
            .await
            .unwrap();
        assert!(
            blob_store
                .has_object(
                    environment_id,
                    "photos".to_string(),
                    object_name.to_string()
                )
                .await
                .unwrap()
        );
        assert_eq!(
            blob_store
                .list_objects(environment_id, "photos".to_string())
                .await
                .unwrap(),
            vec![object_name]
        );
    }

    async fn test_absolute_object_name_is_invalid(blob_store: &impl BlobStoreService) {
        let error = blob_store
            .write_data(
                unlimited_limits(),
                EnvironmentId::new(),
                "photos",
                "/cat.png",
                b"data",
            )
            .await
            .unwrap_err();
        assert!(matches!(error, BlobStoreError::InvalidInput(_)));
    }

    async fn test_invalid_container_name_is_permanent(blob_store: &impl BlobStoreService) {
        for container_name in ["/photos", "../photos"] {
            let environment_id = EnvironmentId::new();
            let error = blob_store
                .create_container(environment_id, container_name.to_string())
                .await
                .unwrap_err();
            assert!(matches!(error, BlobStoreError::InvalidInput(_)));

            let error = blob_store
                .delete_objects(unlimited_limits(), environment_id, container_name, &[])
                .await
                .unwrap_err();
            assert!(matches!(error, BlobStoreError::InvalidInput(_)));
        }
    }

    fn in_memory_blob_store() -> DefaultBlobStoreService {
        let blob_storage = Arc::new(InMemoryBlobStorage::new());
        DefaultBlobStoreService::new(blob_storage)
    }

    async fn fs_blob_store(path: &Path) -> impl BlobStoreService {
        let blob_storage = Arc::new(FileSystemBlobStorage::new(path).await.unwrap());
        DefaultBlobStoreService::new(blob_storage)
    }

    #[test]
    async fn test_container_exists_in_memory() {
        let blob_store = in_memory_blob_store();
        test_container_exists(&blob_store).await;
    }

    #[test]
    async fn test_container_exists_local() {
        let tempdir = TempDir::new().unwrap();
        let blob_store = fs_blob_store(tempdir.path()).await;
        test_container_exists(&blob_store).await;
    }

    #[test]
    async fn test_container_delete_in_memory() {
        let blob_store = in_memory_blob_store();
        test_container_delete(&blob_store).await;
    }

    #[test]
    async fn test_container_delete_local() {
        let tempdir = TempDir::new().unwrap();
        let blob_store = fs_blob_store(tempdir.path()).await;
        test_container_delete(&blob_store).await;
    }

    #[test]
    async fn test_container_has_write_read_has_in_memory() {
        let blob_store = in_memory_blob_store();
        test_container_has_write_read_has(&blob_store).await;
    }

    #[test]
    async fn test_container_has_write_read_has_local() {
        let tempdir = TempDir::new().unwrap();
        let blob_store = fs_blob_store(tempdir.path()).await;
        test_container_has_write_read_has(&blob_store).await;
    }

    #[test]
    async fn test_container_list_copy_move_list_in_memory() {
        let blob_store = in_memory_blob_store();
        test_container_list_copy_move_list(&blob_store).await;
    }

    #[test]
    async fn test_container_list_copy_move_list_local() {
        let tempdir = TempDir::new().unwrap();
        let blob_store = fs_blob_store(tempdir.path()).await;
        test_container_list_copy_move_list(&blob_store).await;
    }

    #[test]
    async fn test_empty_container_name_in_memory() {
        test_empty_container_name(&in_memory_blob_store()).await;
    }

    #[test]
    async fn test_empty_container_name_local() {
        let tempdir = TempDir::new().unwrap();
        let blob_store = fs_blob_store(tempdir.path()).await;
        test_empty_container_name(&blob_store).await;
    }

    #[test]
    async fn test_container_name_spellings_in_memory() {
        test_container_name_spellings(&in_memory_blob_store()).await;
    }

    #[test]
    async fn test_container_name_spellings_local() {
        let tempdir = TempDir::new().unwrap();
        let blob_store = fs_blob_store(tempdir.path()).await;
        test_container_name_spellings(&blob_store).await;
    }

    #[test]
    async fn test_contract_object_name_in_memory() {
        test_contract_object_name(&in_memory_blob_store()).await;
    }

    #[test]
    async fn test_contract_object_name_local() {
        let tempdir = TempDir::new().unwrap();
        test_contract_object_name(&fs_blob_store(tempdir.path()).await).await;
    }

    #[test]
    async fn test_absolute_object_name_is_invalid_in_memory() {
        test_absolute_object_name_is_invalid(&in_memory_blob_store()).await;
    }

    #[test]
    async fn test_absolute_object_name_is_invalid_local() {
        let tempdir = TempDir::new().unwrap();
        test_absolute_object_name_is_invalid(&fs_blob_store(tempdir.path()).await).await;
    }

    #[test]
    async fn test_invalid_container_name_is_permanent_in_memory() {
        test_invalid_container_name_is_permanent(&in_memory_blob_store()).await;
    }

    #[test]
    async fn test_invalid_container_name_is_permanent_local() {
        let tempdir = TempDir::new().unwrap();
        test_invalid_container_name_is_permanent(&fs_blob_store(tempdir.path()).await).await;
    }

    #[test]
    async fn custom_storage_quota_uses_net_mutation_deltas() {
        let blob_store = in_memory_blob_store();
        let environment_id = EnvironmentId::new();
        let limits = unlimited_limits();
        limits.set_available_blob_storage_bytes(5);
        blob_store
            .create_container(environment_id, "container".to_string())
            .await
            .unwrap();

        let created = blob_store
            .write_data(
                limits.clone(),
                environment_id,
                "container",
                "source",
                &[1, 2, 3, 4],
            )
            .await
            .unwrap();
        assert_eq!(created.bytes_delta, 4);

        let rejected = blob_store
            .write_data(
                limits.clone(),
                environment_id,
                "container",
                "source",
                &[0; 6],
            )
            .await;
        assert!(matches!(rejected, Err(BlobStoreError::LimitExceeded(_))));

        let replaced = blob_store
            .write_data(
                limits.clone(),
                environment_id,
                "container",
                "source",
                &[1, 2],
            )
            .await
            .unwrap();
        assert_eq!(replaced.bytes_delta, -2);

        let copied = blob_store
            .copy_object(
                limits.clone(),
                environment_id,
                "container".to_string(),
                "source".to_string(),
                "container".to_string(),
                "destination".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(copied.bytes_delta, 2);

        let moved = blob_store
            .move_object(
                limits.clone(),
                environment_id,
                "container".to_string(),
                "source".to_string(),
                "container".to_string(),
                "destination".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(moved.bytes_delta, -2);

        let cleared = blob_store
            .clear(limits, environment_id, "container".to_string())
            .await
            .unwrap();
        assert_eq!(cleared.bytes_delta, -2);
        assert_eq!(cleared.objects_deleted, 1);
    }

    async fn test_all_deletion_and_replacement_deltas(blob_store: &impl BlobStoreService) {
        let environment_id = EnvironmentId::new();
        let limits = unlimited_limits();
        blob_store
            .create_container(environment_id, "container".to_string())
            .await
            .unwrap();

        for (name, data) in [
            ("zero", &[][..]),
            ("first", &[1, 2, 3, 4]),
            ("second", &[5, 6, 7]),
        ] {
            blob_store
                .write_data(limits.clone(), environment_id, "container", name, data)
                .await
                .unwrap();
        }

        let zero = blob_store
            .delete_object(
                limits.clone(),
                environment_id,
                "container".to_string(),
                "zero".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(zero.bytes_delta, 0);
        assert_eq!(zero.objects_deleted, 1);

        let many = blob_store
            .delete_objects(
                limits.clone(),
                environment_id,
                "container",
                &[
                    "first".to_string(),
                    "./first".to_string(),
                    "missing".to_string(),
                ],
            )
            .await
            .unwrap();
        assert_eq!(many.bytes_delta, -4);
        assert_eq!(many.objects_deleted, 1);
        let already_deleted = blob_store
            .delete_objects(
                limits.clone(),
                environment_id,
                "container",
                &["first".to_string(), "missing".to_string()],
            )
            .await
            .unwrap();
        assert_eq!(already_deleted, BlobStoreMutation::default());

        let cleared = blob_store
            .clear(limits.clone(), environment_id, "container".to_string())
            .await
            .unwrap();
        assert_eq!(cleared.bytes_delta, -3);
        assert_eq!(cleared.objects_deleted, 1);

        blob_store
            .write_data(
                limits.clone(),
                environment_id,
                "container",
                "source",
                &[1, 2, 3, 4, 5],
            )
            .await
            .unwrap();
        let same_path = blob_store
            .move_object(
                limits.clone(),
                environment_id,
                "container".to_string(),
                "source".to_string(),
                "container".to_string(),
                "source".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(same_path, BlobStoreMutation::default());
        assert!(
            blob_store
                .has_object(
                    environment_id,
                    "container".to_string(),
                    "source".to_string(),
                )
                .await
                .unwrap()
        );
        let aliased_same_path = blob_store
            .move_object(
                limits.clone(),
                environment_id,
                "./container".to_string(),
                "source".to_string(),
                "container".to_string(),
                "source".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(aliased_same_path, BlobStoreMutation::default());
        assert!(
            blob_store
                .has_object(
                    environment_id,
                    "container".to_string(),
                    "source".to_string(),
                )
                .await
                .unwrap()
        );
        blob_store
            .write_data(
                limits.clone(),
                environment_id,
                "container",
                "destination",
                &[8, 9],
            )
            .await
            .unwrap();
        let copy = blob_store
            .copy_object(
                limits.clone(),
                environment_id,
                "container".to_string(),
                "source".to_string(),
                "container".to_string(),
                "destination".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(copy.bytes_delta, 3);

        let moved = blob_store
            .move_object(
                limits.clone(),
                environment_id,
                "container".to_string(),
                "source".to_string(),
                "container".to_string(),
                "destination".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(moved.bytes_delta, -5);

        let deleted = blob_store
            .delete_container(limits, environment_id, "container".to_string())
            .await
            .unwrap();
        assert_eq!(deleted.bytes_delta, -5);
        assert_eq!(deleted.objects_deleted, 1);
    }

    #[test]
    async fn all_deletion_and_replacement_deltas_in_memory() {
        test_all_deletion_and_replacement_deltas(&in_memory_blob_store()).await;
    }

    #[test]
    async fn all_deletion_and_replacement_deltas_local() {
        let tempdir = TempDir::new().unwrap();
        test_all_deletion_and_replacement_deltas(&fs_blob_store(tempdir.path()).await).await;
    }

    async fn test_root_container_mutations_do_not_release_quota(
        blob_store: &impl BlobStoreService,
    ) {
        let environment_id = EnvironmentId::new();
        let limits = unlimited_limits();
        limits.set_available_blob_storage_bytes(3);
        blob_store
            .create_container(environment_id, "container".to_string())
            .await
            .unwrap();
        blob_store
            .write_data(
                limits.clone(),
                environment_id,
                "container",
                "object",
                &[1, 2, 3],
            )
            .await
            .unwrap();

        assert_eq!(
            blob_store
                .clear(limits.clone(), environment_id, String::new())
                .await
                .unwrap(),
            BlobStoreMutation::default()
        );
        assert_eq!(
            blob_store
                .delete_container(limits.clone(), environment_id, ".".to_string())
                .await
                .unwrap(),
            BlobStoreMutation::default()
        );
        assert!(
            blob_store
                .has_object(
                    environment_id,
                    "container".to_string(),
                    "object".to_string(),
                )
                .await
                .unwrap()
        );
        assert!(matches!(
            blob_store
                .write_data(limits, environment_id, "container", "another", &[4],)
                .await,
            Err(BlobStoreError::LimitExceeded(_))
        ));
    }

    #[test]
    async fn root_container_mutations_do_not_release_quota_in_memory() {
        test_root_container_mutations_do_not_release_quota(&in_memory_blob_store()).await;
    }

    #[test]
    async fn root_container_mutations_do_not_release_quota_local() {
        let tempdir = TempDir::new().unwrap();
        test_root_container_mutations_do_not_release_quota(&fs_blob_store(tempdir.path()).await)
            .await;
    }

    #[test]
    async fn move_to_new_destination_reports_no_deleted_object() {
        let blob_store = in_memory_blob_store();
        let environment_id = EnvironmentId::new();
        blob_store
            .create_container(environment_id, "container".to_string())
            .await
            .unwrap();
        blob_store
            .write_data(
                unlimited_limits(),
                environment_id,
                "container",
                "source",
                &[1, 2, 3],
            )
            .await
            .unwrap();

        let moved = blob_store
            .move_object(
                unlimited_limits(),
                environment_id,
                "container".to_string(),
                "source".to_string(),
                "container".to_string(),
                "new-destination".to_string(),
            )
            .await
            .unwrap();

        assert_eq!(moved.bytes_delta, 0);
        assert_eq!(moved.objects_deleted, 0);
    }

    #[test]
    async fn concurrent_replacements_keep_accounting_equal_to_physical_size() {
        let blob_store = in_memory_blob_store();
        let environment_id = EnvironmentId::new();
        let limits = unlimited_limits();
        limits.set_available_blob_storage_bytes(10);
        blob_store
            .create_container(environment_id, "container".to_string())
            .await
            .unwrap();
        blob_store
            .write_data(
                limits.clone(),
                environment_id,
                "container",
                "object",
                &[0; 4],
            )
            .await
            .unwrap();

        let (first, second) = tokio::join!(
            blob_store.write_data(
                limits.clone(),
                environment_id,
                "container",
                "object",
                &[1; 2],
            ),
            blob_store.write_data(
                limits.clone(),
                environment_id,
                "container",
                "object",
                &[2; 5],
            ),
        );
        first.unwrap();
        second.unwrap();

        let physical_size = blob_store
            .object_info(
                environment_id,
                "container".to_string(),
                "object".to_string(),
            )
            .await
            .unwrap()
            .size;
        assert_eq!(limits.blob_storage_delta_for_test(), physical_size as i64);

        let first_lock = blob_store.mutation_lock(environment_id);
        let same_environment_lock = blob_store.mutation_lock(environment_id);
        let other_environment_lock = blob_store.mutation_lock(EnvironmentId::new());
        assert!(Arc::ptr_eq(&first_lock, &same_environment_lock));
        assert!(!Arc::ptr_eq(&first_lock, &other_environment_lock));
    }

    #[test]
    async fn environment_locks_serialize_only_matching_environments() {
        let storage = Arc::new(FailingPutBlobStorage::new());
        let blob_store = Arc::new(DefaultBlobStoreService::new(storage.clone()));
        let first_environment = EnvironmentId::new();
        let other_environment = EnvironmentId::new();
        for environment_id in [first_environment, other_environment] {
            blob_store
                .create_container(environment_id, "container".to_string())
                .await
                .unwrap();
        }

        storage.block_next_put();
        let first = tokio::spawn({
            let blob_store = blob_store.clone();
            async move {
                blob_store
                    .write_data(
                        unlimited_limits(),
                        first_environment,
                        "container",
                        "first",
                        &[1],
                    )
                    .await
            }
        });
        storage.wait_for_blocked_put().await;

        let mut same_environment = tokio::spawn({
            let blob_store = blob_store.clone();
            async move {
                blob_store
                    .write_data(
                        unlimited_limits(),
                        first_environment,
                        "container",
                        "second",
                        &[2],
                    )
                    .await
            }
        });
        let other_environment_result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            tokio::spawn({
                let blob_store = blob_store.clone();
                async move {
                    blob_store
                        .write_data(
                            unlimited_limits(),
                            other_environment,
                            "container",
                            "other",
                            &[3],
                        )
                        .await
                }
            }),
        )
        .await
        .expect("another environment must not wait for the blocked mutation")
        .unwrap();
        other_environment_result.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut same_environment)
                .await
                .is_err(),
            "the same environment must wait for the blocked mutation"
        );

        storage.release_blocked_put();
        first.await.unwrap().unwrap();
        same_environment.await.unwrap().unwrap();
    }

    #[test]
    async fn failed_writes_restore_reservations_without_publishing_credit() {
        let storage = Arc::new(FailingPutBlobStorage::new());
        let blob_store = DefaultBlobStoreService::new(storage.clone());
        let environment_id = EnvironmentId::new();
        let limits = unlimited_limits();
        limits.set_available_blob_storage_bytes(10);
        blob_store
            .create_container(environment_id, "container".to_string())
            .await
            .unwrap();
        blob_store
            .write_data(
                limits.clone(),
                environment_id,
                "container",
                "object",
                &[0; 5],
            )
            .await
            .unwrap();

        storage.fail_next_put();
        let failed_growth = blob_store
            .write_data(
                limits.clone(),
                environment_id,
                "container",
                "another",
                &[1; 3],
            )
            .await;
        assert!(matches!(
            failed_growth,
            Err(BlobStoreError::TransientBackend(_))
        ));
        assert_eq!(limits.blob_storage_delta_for_test(), 5);

        storage.fail_next_put();
        let failed = blob_store
            .write_data(
                limits.clone(),
                environment_id,
                "container",
                "object",
                &[1; 2],
            )
            .await;
        assert!(matches!(failed, Err(BlobStoreError::TransientBackend(_))));
        assert_eq!(limits.blob_storage_delta_for_test(), 5);

        let rejected = blob_store
            .write_data(limits, environment_id, "container", "another", &[2; 6])
            .await;
        assert!(matches!(rejected, Err(BlobStoreError::LimitExceeded(_))));
    }
}
