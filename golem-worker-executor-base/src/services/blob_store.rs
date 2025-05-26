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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use bincode::{Decode, Encode};

use golem_common::model::AccountId;

use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace, ExistsResult};
use golem_wasm_rpc_derive::IntoValue;

/// Interface for storing blobs in a persistent storage.
#[async_trait]
pub trait BlobStoreService: Send + Sync {
    async fn clear(&self, account_id: AccountId, container_name: String) -> anyhow::Result<()>;

    async fn container_exists(
        &self,
        account_id: AccountId,
        container_name: String,
    ) -> anyhow::Result<bool>;

    async fn copy_object(
        &self,
        account_id: AccountId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> anyhow::Result<()>;

    async fn create_container(
        &self,
        account_id: AccountId,
        container_name: String,
    ) -> anyhow::Result<()>;

    async fn delete_container(
        &self,
        account_id: AccountId,
        container_name: String,
    ) -> anyhow::Result<()>;

    async fn delete_object(
        &self,
        account_id: AccountId,
        container_name: String,
        object_name: String,
    ) -> anyhow::Result<()>;

    async fn delete_objects(
        &self,
        account_id: AccountId,
        container_name: String,
        object_names: Vec<String>,
    ) -> anyhow::Result<()>;

    async fn get_container(
        &self,
        account_id: AccountId,
        container_name: String,
    ) -> anyhow::Result<Option<u64>>;

    async fn get_data(
        &self,
        account_id: AccountId,
        container_name: String,
        object_name: String,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Vec<u8>>;

    async fn has_object(
        &self,
        account_id: AccountId,
        container_name: String,
        object_name: String,
    ) -> anyhow::Result<bool>;

    async fn list_objects(
        &self,
        account_id: AccountId,
        container_name: String,
    ) -> anyhow::Result<Vec<String>>;

    async fn move_object(
        &self,
        account_id: AccountId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> anyhow::Result<()>;

    async fn object_info(
        &self,
        account_id: AccountId,
        container_name: String,
        object_name: String,
    ) -> anyhow::Result<ObjectMetadata>;

    async fn write_data(
        &self,
        account_id: AccountId,
        container_name: String,
        object_name: String,
        data: Vec<u8>,
    ) -> anyhow::Result<()>;
}

pub struct DefaultBlobStoreService {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
}

impl DefaultBlobStoreService {
    pub fn new(blob_storage: Arc<dyn BlobStorage + Send + Sync>) -> Self {
        Self { blob_storage }
    }
}

#[async_trait]
impl BlobStoreService for DefaultBlobStoreService {
    async fn clear(&self, account_id: AccountId, container_name: String) -> anyhow::Result<()> {
        self.blob_storage
            .delete_dir(
                "blob_store",
                "clear",
                BlobStorageNamespace::CustomStorage(account_id),
                Path::new(&container_name),
            )
            .await
            .map_err(|err| anyhow!(err))?;
        Ok(())
    }

    async fn container_exists(
        &self,
        account_id: AccountId,
        container_name: String,
    ) -> anyhow::Result<bool> {
        self.blob_storage
            .exists(
                "blob_store",
                "container_exists",
                BlobStorageNamespace::CustomStorage(account_id),
                Path::new(&container_name),
            )
            .await
            .map_err(|err| anyhow!(err))
            .map(|result| match result {
                ExistsResult::Directory => true,
                ExistsResult::File => false,
                ExistsResult::DoesNotExist => false,
            })
    }

    async fn copy_object(
        &self,
        account_id: AccountId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> anyhow::Result<()> {
        self.blob_storage
            .copy(
                "blob_store",
                "copy_object",
                BlobStorageNamespace::CustomStorage(account_id),
                &Path::new(&source_container_name).join(&source_object_name),
                &Path::new(&destination_container_name).join(&destination_object_name),
            )
            .await
            .map_err(|err| anyhow!(err))
    }

    async fn create_container(
        &self,
        account_id: AccountId,
        container_name: String,
    ) -> anyhow::Result<()> {
        self.blob_storage
            .create_dir(
                "blob_store",
                "create_container",
                BlobStorageNamespace::CustomStorage(account_id),
                Path::new(&container_name),
            )
            .await
            .map_err(|err| anyhow!(err))?;
        Ok(())
    }

    async fn delete_container(
        &self,
        account_id: AccountId,
        container_name: String,
    ) -> anyhow::Result<()> {
        self.blob_storage
            .delete_dir(
                "blob_store",
                "delete_container",
                BlobStorageNamespace::CustomStorage(account_id),
                Path::new(&container_name),
            )
            .await
            .map_err(|err| anyhow!(err))?;
        Ok(())
    }

    async fn delete_object(
        &self,
        account_id: AccountId,
        container_name: String,
        object_name: String,
    ) -> anyhow::Result<()> {
        self.blob_storage
            .delete_dir(
                "blob_store",
                "delete_object",
                BlobStorageNamespace::CustomStorage(account_id),
                &Path::new(&container_name).join(&object_name),
            )
            .await
            .map_err(|err| anyhow!(err))?;
        Ok(())
    }

    async fn delete_objects(
        &self,
        account_id: AccountId,
        container_name: String,
        object_names: Vec<String>,
    ) -> anyhow::Result<()> {
        let paths: Vec<PathBuf> = object_names
            .iter()
            .map(|object_name| Path::new(&container_name).join(object_name))
            .collect();
        self.blob_storage
            .delete_many(
                "blob_store",
                "delete_objects",
                BlobStorageNamespace::CustomStorage(account_id),
                &paths,
            )
            .await
            .map_err(|err| anyhow!(err))
    }

    async fn get_container(
        &self,
        account_id: AccountId,
        container_name: String,
    ) -> anyhow::Result<Option<u64>> {
        self.blob_storage
            .get_metadata(
                "blob_store",
                "get_container",
                BlobStorageNamespace::CustomStorage(account_id),
                Path::new(&container_name),
            )
            .await
            .map_err(|err| anyhow!(err))
            .map(|result| result.map(|metadata| metadata.last_modified_at.to_millis()))
    }

    async fn get_data(
        &self,
        account_id: AccountId,
        container_name: String,
        object_name: String,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let data = self
            .blob_storage
            .get_raw_slice(
                "blob_store",
                "get_data",
                BlobStorageNamespace::CustomStorage(account_id),
                &Path::new(&container_name).join(&object_name),
                start,
                end,
            )
            .await
            .map_err(|err| anyhow!(err))?;

        match data {
            Some(data) => Ok(data.to_vec()),
            None => anyhow::bail!("Object does not exist"),
        }
    }

    async fn has_object(
        &self,
        account_id: AccountId,
        container_name: String,
        object_name: String,
    ) -> anyhow::Result<bool> {
        self.blob_storage
            .exists(
                "blob_store",
                "has_object",
                BlobStorageNamespace::CustomStorage(account_id),
                &Path::new(&container_name).join(&object_name),
            )
            .await
            .map_err(|err| anyhow!(err))
            .map(|result| match result {
                ExistsResult::Directory => false,
                ExistsResult::File => true,
                ExistsResult::DoesNotExist => false,
            })
    }

    async fn list_objects(
        &self,
        account_id: AccountId,
        container_name: String,
    ) -> anyhow::Result<Vec<String>> {
        self.blob_storage
            .list_dir(
                "blob_store",
                "list_objects",
                BlobStorageNamespace::CustomStorage(account_id),
                Path::new(&container_name),
            )
            .await
            .map_err(|err| anyhow!(err))
            .map(|paths| {
                paths
                    .iter()
                    .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
                    .collect()
            })
    }

    async fn move_object(
        &self,
        account_id: AccountId,
        source_container_name: String,
        source_object_name: String,
        destination_container_name: String,
        destination_object_name: String,
    ) -> anyhow::Result<()> {
        self.blob_storage
            .r#move(
                "blob_store",
                "move_object",
                BlobStorageNamespace::CustomStorage(account_id),
                &Path::new(&source_container_name).join(&source_object_name),
                &Path::new(&destination_container_name).join(&destination_object_name),
            )
            .await
            .map_err(|err| anyhow!(err))
    }

    async fn object_info(
        &self,
        account_id: AccountId,
        container_name: String,
        object_name: String,
    ) -> anyhow::Result<ObjectMetadata> {
        match self
            .blob_storage
            .get_metadata(
                "blob_store",
                "object_info",
                BlobStorageNamespace::CustomStorage(account_id),
                &Path::new(&container_name).join(&object_name),
            )
            .await
            .map_err(|err| anyhow!(err))?
        {
            Some(metadata) => Ok(ObjectMetadata {
                name: object_name,
                container: container_name,
                created_at: metadata.last_modified_at.to_millis(),
                size: metadata.size,
            }),
            None => anyhow::bail!("Object does not exist"),
        }
    }

    async fn write_data(
        &self,
        account_id: AccountId,
        container_name: String,
        object_name: String,
        data: Vec<u8>,
    ) -> anyhow::Result<()> {
        self.blob_storage
            .put_raw(
                "blob_store",
                "write_data",
                BlobStorageNamespace::CustomStorage(account_id),
                &Path::new(&container_name).join(&object_name),
                &data,
            )
            .await
            .map_err(|err| anyhow!(err))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode, IntoValue)]
pub struct ObjectMetadata {
    pub name: String,
    pub container: String,
    pub created_at: u64,
    pub size: u64,
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use std::path::Path;
    use std::sync::Arc;

    use tempfile::TempDir;

    use golem_common::model::AccountId;

    use crate::services::blob_store::{BlobStoreService, DefaultBlobStoreService};
    use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;

    async fn test_container_exists(blob_store: &impl BlobStoreService) {
        let account1 = AccountId {
            value: "account1".to_string(),
        };
        assert!(!blob_store
            .container_exists(account1.clone(), "container1".to_string())
            .await
            .unwrap());
        blob_store
            .create_container(account1.clone(), "container1".to_string())
            .await
            .unwrap();
        assert!(blob_store
            .container_exists(account1.clone(), "container1".to_string())
            .await
            .unwrap());
    }

    async fn test_container_delete(blob_store: &impl BlobStoreService) {
        let account1 = AccountId {
            value: "account1".to_string(),
        };
        blob_store
            .create_container(account1.clone(), "container1".to_string())
            .await
            .unwrap();
        blob_store
            .delete_container(account1.clone(), "container1".to_string())
            .await
            .unwrap();
        assert!(!blob_store
            .container_exists(account1.clone(), "container1".to_string())
            .await
            .unwrap());
    }

    async fn test_container_has_write_read_has(blob_store: &impl BlobStoreService) {
        let account1 = AccountId {
            value: "account1".to_string(),
        };

        blob_store
            .create_container(account1.clone(), "container1".to_string())
            .await
            .unwrap();
        assert!(!blob_store
            .has_object(
                account1.clone(),
                "container1".to_string(),
                "obj1".to_string()
            )
            .await
            .unwrap());

        let original_data = vec![1, 2, 3, 4];
        blob_store
            .write_data(
                account1.clone(),
                "container1".to_string(),
                "obj1".to_string(),
                original_data.clone(),
            )
            .await
            .unwrap();

        let read_data = blob_store
            .get_data(
                account1.clone(),
                "container1".to_string(),
                "obj1".to_string(),
                0,
                4,
            )
            .await
            .unwrap();

        assert_eq!(original_data, read_data);
        assert!(blob_store
            .has_object(
                account1.clone(),
                "container1".to_string(),
                "obj1".to_string()
            )
            .await
            .unwrap());
    }

    async fn test_container_list_copy_move_list(blob_store: &impl BlobStoreService) {
        let account1 = AccountId {
            value: "account1".to_string(),
        };

        blob_store
            .create_container(account1.clone(), "container1".to_string())
            .await
            .unwrap();
        blob_store
            .create_container(account1.clone(), "container2".to_string())
            .await
            .unwrap();

        assert!(blob_store
            .list_objects(account1.clone(), "container1".to_string(),)
            .await
            .unwrap()
            .is_empty());

        let original_data = vec![1, 2, 3, 4];
        blob_store
            .write_data(
                account1.clone(),
                "container1".to_string(),
                "obj1".to_string(),
                original_data.clone(),
            )
            .await
            .unwrap();

        blob_store
            .copy_object(
                account1.clone(),
                "container1".to_string(),
                "obj1".to_string(),
                "container1".to_string(),
                "obj2".to_string(),
            )
            .await
            .unwrap();

        let mut result = blob_store
            .list_objects(account1.clone(), "container1".to_string())
            .await
            .unwrap();

        result.sort();

        assert_eq!(result, vec!["obj1", "obj2"]);

        blob_store
            .move_object(
                account1.clone(),
                "container1".to_string(),
                "obj1".to_string(),
                "container2".to_string(),
                "obj3".to_string(),
            )
            .await
            .unwrap();

        assert_eq!(
            blob_store
                .list_objects(account1.clone(), "container1".to_string(),)
                .await
                .unwrap(),
            vec!["obj2"]
        );

        assert_eq!(
            blob_store
                .list_objects(account1.clone(), "container2".to_string(),)
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
}
