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

use async_trait::async_trait;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::types::ObjectMetadata;
use golem_service_base::storage::blob::{
    BlobStorage, BlobStorageLabelledApi, BlobStorageNamespace, ExistsResult, join_blob_path,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    /// Transient backend failure (network, timeout, etc.)
    TransientBackend(String),
    /// Other/unknown error
    Other(String),
}

impl std::fmt::Display for BlobStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(msg) => write!(f, "Not found: {msg}"),
            Self::AlreadyExists(msg) => write!(f, "Already exists: {msg}"),
            Self::PermissionDenied(msg) => write!(f, "Permission denied: {msg}"),
            Self::InvalidInput(msg) => write!(f, "Invalid input: {msg}"),
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
    ) -> Result<(), BlobStoreError>;

    async fn container_exists(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<bool, BlobStoreError>;

    async fn copy_object(
        &self,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<(), BlobStoreError>;

    async fn create_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<(), BlobStoreError>;

    async fn delete_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<(), BlobStoreError>;

    async fn delete_object(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<(), BlobStoreError>;

    async fn delete_objects(
        &self,
        environment_id: EnvironmentId,
        container_name: &str,
        object_names: &[String],
    ) -> Result<(), BlobStoreError>;

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
    ) -> Result<(), BlobStoreError>;

    async fn object_info(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<ObjectMetadata, BlobStoreError>;

    async fn write_data(
        &self,
        environment_id: EnvironmentId,
        container_name: &str,
        object_name: &str,
        data: &[u8],
    ) -> Result<(), BlobStoreError>;
}

pub struct DefaultBlobStoreService {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
}

impl DefaultBlobStoreService {
    pub fn new(blob_storage: Arc<dyn BlobStorage + Send + Sync>) -> Self {
        Self { blob_storage }
    }

    fn object_name(path: &Path) -> Result<String, BlobStoreError> {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(ToString::to_string)
            .ok_or_else(|| {
                BlobStoreError::InvalidInput(format!("Invalid blob object path: {path:?}"))
            })
    }
}

#[async_trait]
impl BlobStoreService for DefaultBlobStoreService {
    async fn clear(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<(), BlobStoreError> {
        let namespace = BlobStorageNamespace::CustomStorage { environment_id };
        let path = Path::new(&container_name);
        let blob_storage = self.blob_storage.with("blob_store", "clear");
        blob_storage
            .delete_dir(namespace.clone(), path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        // Re-create the empty container directory so the container continues to exist.
        // clear() semantics: remove all objects, keep the container itself.
        blob_storage
            .create_dir(namespace, path)
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        Ok(())
    }

    async fn container_exists(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<bool, BlobStoreError> {
        self.blob_storage
            .with("blob_store", "container_exists")
            .exists(
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
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<(), BlobStoreError> {
        self.blob_storage
            .with("blob_store", "copy_object")
            .copy(
                BlobStorageNamespace::CustomStorage { environment_id },
                &join_blob_path(&source_container_name, &source_object_name),
                &join_blob_path(&destination_container_name, &destination_object_name),
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))
    }

    async fn create_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<(), BlobStoreError> {
        self.blob_storage
            .with("blob_store", "create_container")
            .create_dir(
                BlobStorageNamespace::CustomStorage { environment_id },
                Path::new(&container_name),
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        Ok(())
    }

    async fn delete_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<(), BlobStoreError> {
        self.blob_storage
            .with("blob_store", "delete_container")
            .delete_dir(
                BlobStorageNamespace::CustomStorage { environment_id },
                Path::new(&container_name),
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        Ok(())
    }

    async fn delete_object(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<(), BlobStoreError> {
        self.blob_storage
            .with("blob_store", "delete_object")
            .delete(
                BlobStorageNamespace::CustomStorage { environment_id },
                &join_blob_path(&container_name, &object_name),
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))?;
        Ok(())
    }

    async fn delete_objects(
        &self,
        environment_id: EnvironmentId,
        container_name: &str,
        object_names: &[String],
    ) -> Result<(), BlobStoreError> {
        let paths: Vec<PathBuf> = object_names
            .iter()
            .map(|object_name| join_blob_path(container_name, object_name))
            .collect();
        self.blob_storage
            .with("blob_store", "delete_objects")
            .delete_many(
                BlobStorageNamespace::CustomStorage { environment_id },
                &paths,
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))
    }

    async fn get_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<Option<u64>, BlobStoreError> {
        self.blob_storage
            .with("blob_store", "get_container")
            .get_metadata(
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
            .with("blob_store", "get_data")
            .get_raw_slice(
                BlobStorageNamespace::CustomStorage { environment_id },
                &join_blob_path(&container_name, &object_name),
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
            .with("blob_store", "has_object")
            .exists(
                BlobStorageNamespace::CustomStorage { environment_id },
                &join_blob_path(&container_name, &object_name),
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
            .with("blob_store", "list_objects")
            .list_dir(
                BlobStorageNamespace::CustomStorage { environment_id },
                Path::new(&container_name),
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))
            .and_then(|paths| paths.iter().map(|path| Self::object_name(path)).collect())
    }

    async fn move_object(
        &self,
        environment_id: EnvironmentId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> Result<(), BlobStoreError> {
        self.blob_storage
            .with("blob_store", "move_object")
            .r#move(
                BlobStorageNamespace::CustomStorage { environment_id },
                &join_blob_path(&source_container_name, &source_object_name),
                &join_blob_path(&destination_container_name, &destination_object_name),
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))
    }

    async fn object_info(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<ObjectMetadata, BlobStoreError> {
        match self
            .blob_storage
            .with("blob_store", "object_info")
            .get_metadata(
                BlobStorageNamespace::CustomStorage { environment_id },
                &join_blob_path(&container_name, &object_name),
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
        environment_id: EnvironmentId,
        container_name: &str,
        object_name: &str,
        data: &[u8],
    ) -> Result<(), BlobStoreError> {
        self.blob_storage
            .with("blob_store", "write_data")
            .put_raw(
                BlobStorageNamespace::CustomStorage { environment_id },
                &join_blob_path(container_name, object_name),
                data,
            )
            .await
            .map_err(|err| BlobStoreError::TransientBackend(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use crate::services::blob_store::{BlobStoreService, DefaultBlobStoreService};
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::TempDir;
    use test_r::test;

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
            .write_data(environment_id, "container1", "obj1", &original_data)
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
            .write_data(environment_id, "container1", "obj1", &original_data)
            .await
            .unwrap();

        blob_store
            .copy_object(
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

    async fn test_empty_container_name(blob_store: &impl BlobStoreService) {
        let environment_id = EnvironmentId::new();
        blob_store
            .write_data(environment_id, "", "x", b"data")
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
                .write_data(environment_id, container_name, "x", b"data")
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
    async fn test_empty_container_name_in_memory() {
        test_empty_container_name(&in_memory_blob_store()).await;
    }

    #[test]
    async fn test_container_name_edge_cases_local() {
        let tempdir = TempDir::new().unwrap();
        let blob_store = fs_blob_store(tempdir.path()).await;
        test_empty_container_name(&blob_store).await;
        test_container_name_spellings(&blob_store).await;
    }
}
