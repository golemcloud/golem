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
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace, ExistsResult};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn delete_object(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError>;

    async fn delete_objects(
        &self,
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
    mutation_lock: Mutex<()>,
}

impl DefaultBlobStoreService {
    pub fn new(blob_storage: Arc<dyn BlobStorage + Send + Sync>) -> Self {
        Self {
            blob_storage,
            mutation_lock: Mutex::new(()),
        }
    }
}

#[async_trait]
impl BlobStoreService for DefaultBlobStoreService {
    async fn clear(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock.lock().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let path = Path::new(&container_name);
        if self
            .blob_storage
            .exists("blob_store", "clear", namespace.clone(), path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
            == ExistsResult::DoesNotExist
        {
            self.blob_storage
                .create_dir("blob_store", "clear", namespace, path)
                .await
                .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
            return Ok(BlobStoreMutation::default());
        }
        let blobs = self
            .blob_storage
            .list_blobs_below("blob_store", "clear", namespace.clone(), path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        let deleted_bytes = blobs
            .iter()
            .fold(0u64, |sum, (_, metadata)| sum.saturating_add(metadata.size));
        self.blob_storage
            .delete_dir("blob_store", "clear", namespace.clone(), path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        // Re-create the empty container directory so the container continues to exist.
        // clear() semantics: remove all objects, keep the container itself.
        self.blob_storage
            .create_dir("blob_store", "clear", namespace, path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        let mutation = BlobStoreMutation {
            bytes_delta: -(deleted_bytes.min(i64::MAX as u64) as i64),
            objects_deleted: blobs.len() as u64,
        };
        Ok(mutation)
    }

    async fn container_exists(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<bool, BlobStoreError> {
        self.blob_storage
            .exists(
                "blob_store",
                "container_exists",
                BlobStorageNamespace::CustomStorage { environment_id },
                Path::new(&container_name),
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
        let _mutation = self.mutation_lock.lock().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let source = Path::new(&source_container_name).join(&source_object_name);
        let destination = Path::new(&destination_container_name).join(&destination_object_name);
        let source_size = self
            .blob_storage
            .get_metadata("blob_store", "copy_object", namespace.clone(), &source)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
            .ok_or_else(|| BlobStoreError::NotFound("Source object does not exist".to_string()))?
            .size;
        let old_destination_size = self
            .blob_storage
            .get_metadata("blob_store", "copy_object", namespace.clone(), &destination)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
            .map_or(0, |metadata| metadata.size);
        let delta = signed_delta(source_size, old_destination_size);
        if !resource_limits.try_record_blob_storage_delta(delta) {
            return Err(BlobStoreError::LimitExceeded(
                "Blob storage byte limit exceeded".to_string(),
            ));
        }
        if let Err(error) = self
            .blob_storage
            .copy(
                "blob_store",
                "copy_object",
                namespace,
                &source,
                &destination,
            )
            .await
        {
            resource_limits.rollback_blob_storage_delta(delta);
            return Err(BlobStoreError::TransientBackend(error.to_string()));
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
        self.blob_storage
            .create_dir(
                "blob_store",
                "create_container",
                BlobStorageNamespace::CustomStorage { environment_id },
                Path::new(&container_name),
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        Ok(BlobStoreMutation::default())
    }

    async fn delete_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock.lock().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let path = Path::new(&container_name);
        if self
            .blob_storage
            .exists("blob_store", "delete_container", namespace.clone(), path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
            == ExistsResult::DoesNotExist
        {
            return Ok(BlobStoreMutation::default());
        }
        let blobs = self
            .blob_storage
            .list_blobs_below("blob_store", "delete_container", namespace.clone(), path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        let deleted_bytes = blobs
            .iter()
            .fold(0u64, |sum, (_, metadata)| sum.saturating_add(metadata.size));
        self.blob_storage
            .delete_dir("blob_store", "delete_container", namespace, path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        Ok(BlobStoreMutation {
            bytes_delta: -(deleted_bytes.min(i64::MAX as u64) as i64),
            objects_deleted: blobs.len() as u64,
        })
    }

    async fn delete_object(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock.lock().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let path = Path::new(&container_name).join(&object_name);
        let old_metadata = self
            .blob_storage
            .get_metadata("blob_store", "delete_object", namespace.clone(), &path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        let old_size = old_metadata.as_ref().map_or(0, |metadata| metadata.size);
        self.blob_storage
            .delete("blob_store", "delete_object", namespace, &path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        Ok(BlobStoreMutation {
            bytes_delta: -(old_size.min(i64::MAX as u64) as i64),
            objects_deleted: u64::from(old_metadata.is_some()),
        })
    }

    async fn delete_objects(
        &self,
        environment_id: EnvironmentId,
        container_name: &str,
        object_names: &[String],
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock.lock().await;
        let paths: HashSet<PathBuf> = object_names
            .iter()
            .map(|object_name| Path::new(container_name).join(object_name))
            .collect();
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let mut deleted_bytes = 0u64;
        let mut existing_paths = Vec::new();
        for path in &paths {
            if let Some(metadata) = self
                .blob_storage
                .get_metadata("blob_store", "delete_objects", namespace.clone(), path)
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
        self.blob_storage
            .delete_many("blob_store", "delete_objects", namespace, &existing_paths)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        Ok(BlobStoreMutation {
            bytes_delta: -(deleted_bytes.min(i64::MAX as u64) as i64),
            objects_deleted: existing_paths.len() as u64,
        })
    }

    async fn get_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<Option<u64>, BlobStoreError> {
        self.blob_storage
            .get_metadata(
                "blob_store",
                "get_container",
                BlobStorageNamespace::CustomStorage { environment_id },
                Path::new(&container_name),
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
        let data = self
            .blob_storage
            .get_raw_slice(
                "blob_store",
                "get_data",
                BlobStorageNamespace::CustomStorage { environment_id },
                &Path::new(&container_name).join(&object_name),
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
        self.blob_storage
            .exists(
                "blob_store",
                "has_object",
                BlobStorageNamespace::CustomStorage { environment_id },
                &Path::new(&container_name).join(&object_name),
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
        self.blob_storage
            .list_dir(
                "blob_store",
                "list_objects",
                BlobStorageNamespace::CustomStorage { environment_id },
                Path::new(&container_name),
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))
            .map(|paths| {
                paths
                    .iter()
                    .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
                    .collect()
            })
    }

    async fn move_object(
        &self,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        let _mutation = self.mutation_lock.lock().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let source = Path::new(&source_container_name).join(&source_object_name);
        let destination = Path::new(&destination_container_name).join(&destination_object_name);
        if source == destination {
            self.blob_storage
                .get_metadata("blob_store", "move_object", namespace, &source)
                .await
                .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
                .ok_or_else(|| {
                    BlobStoreError::NotFound("Source object does not exist".to_string())
                })?;
            return Ok(BlobStoreMutation::default());
        }
        let old_destination = self
            .blob_storage
            .get_metadata("blob_store", "move_object", namespace.clone(), &destination)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        let old_destination_size = old_destination.as_ref().map_or(0, |metadata| metadata.size);
        self.blob_storage
            .r#move(
                "blob_store",
                "move_object",
                namespace,
                &source,
                &destination,
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        Ok(BlobStoreMutation {
            bytes_delta: -(old_destination_size.min(i64::MAX as u64) as i64),
            objects_deleted: u64::from(old_destination.is_some()),
        })
    }

    async fn object_info(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<ObjectMetadata, BlobStoreError> {
        match self
            .blob_storage
            .get_metadata(
                "blob_store",
                "object_info",
                BlobStorageNamespace::CustomStorage { environment_id },
                &Path::new(&container_name).join(&object_name),
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
        let _mutation = self.mutation_lock.lock().await;
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let path = Path::new(container_name).join(object_name);
        let old_size = self
            .blob_storage
            .get_metadata("blob_store", "write_data", namespace.clone(), &path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?
            .map_or(0, |metadata| metadata.size);
        let delta = signed_delta(data.len() as u64, old_size);
        if !resource_limits.try_record_blob_storage_delta(delta) {
            return Err(BlobStoreError::LimitExceeded(
                "Blob storage byte limit exceeded".to_string(),
            ));
        }
        if let Err(error) = self
            .blob_storage
            .put_raw("blob_store", "write_data", namespace, &path, data)
            .await
        {
            resource_limits.rollback_blob_storage_delta(delta);
            return Err(BlobStoreError::TransientBackend(error.to_string()));
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
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use std::path::Path;
    use std::sync::Arc;
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
            .delete_container(environment_id, "container1".to_string())
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

    fn in_memory_blob_store() -> impl BlobStoreService {
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
                limits,
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
            .clear(environment_id, "container".to_string())
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
            .delete_object(environment_id, "container".to_string(), "zero".to_string())
            .await
            .unwrap();
        assert_eq!(zero.bytes_delta, 0);
        assert_eq!(zero.objects_deleted, 1);

        let many = blob_store
            .delete_objects(
                environment_id,
                "container",
                &[
                    "first".to_string(),
                    "first".to_string(),
                    "missing".to_string(),
                ],
            )
            .await
            .unwrap();
        assert_eq!(many.bytes_delta, -4);
        assert_eq!(many.objects_deleted, 1);
        let already_deleted = blob_store
            .delete_objects(
                environment_id,
                "container",
                &["first".to_string(), "missing".to_string()],
            )
            .await
            .unwrap();
        assert_eq!(already_deleted, BlobStoreMutation::default());

        let cleared = blob_store
            .clear(environment_id, "container".to_string())
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
                limits,
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
            .delete_container(environment_id, "container".to_string())
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
}
