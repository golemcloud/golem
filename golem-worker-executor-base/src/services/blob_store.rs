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

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use bincode::{Decode, Encode};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio_stream::StreamExt;
use tonic::metadata::Binary;
use tracing::{error, info};
use golem_api_grpc::proto::golem::workerexecutor::v1::{FileNode, NodeType};
use golem_common::model::{AccountId, ComponentId, OwnedWorkerId, WorkerId};

use crate::storage::blob::{BlobStorage, BlobStorageNamespace, ExistsResult};

/// Interface for storing blobs in a persistent storage.
#[async_trait]
pub trait BlobStoreService {
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

    async fn get_files_metadata(
        &self,
        owned_worker_id: OwnedWorkerId
    ) -> Result<Vec<FileNode>, String>;

    async fn get_file_or_directory(
        &self,
        owned_worker_id: OwnedWorkerId,
        path: String,
    ) -> Result<FileOrDirectoryResponse, String>;

    async fn get_file(
        &self,
        owned_worker_id: OwnedWorkerId,
        path: PathBuf,
    ) -> Result<Vec<u8>, String>;

    async fn get_directory_metadata(
        &self,
        owned_worker_id: OwnedWorkerId,
        path: PathBuf
    ) -> Result<Vec<FileNode>, String>; // Helper method to determine if a path is a directory
    async fn is_directory(&self, path: &PathBuf) -> Result<bool, String>;
}

pub enum FileOrDirectoryResponse {
    FileContent(Vec<u8>),
    DirectoryListing(Vec<FileNode>),
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeTypeSerializeable {
    Directory,
    File,
}

// Enum representing file permissions in kebab-case
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilePermission {
    ReadWrite,
    ReadOnly,
}

// Convert `FilePermission` to a kebab-case string
impl ToString for FilePermission {
    fn to_string(&self) -> String {
        match self {
            FilePermission::ReadWrite => "read-write".to_string(),
            FilePermission::ReadOnly => "read-only".to_string(),
        }
    }
}

// Node Struct with Constructors
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    node_type: NodeTypeSerializeable,
    name: String,
    full_path: PathBuf,
    permission: FilePermission,
    children: Vec<Node>,
}

impl Node {
    pub async fn new_file(name: &str, full_path: &PathBuf) -> Self {
        // Determine file permission based on read-only status



        let permission = if let Ok(metadata) = fs::metadata(full_path).await {
            if metadata.permissions().readonly() {
                FilePermission::ReadOnly
            } else {
                FilePermission::ReadWrite
            }
        } else {
            FilePermission::ReadOnly // Default to read-only on error
        };

        Node {
            node_type: NodeTypeSerializeable::File,
            name: name.to_string(),
            full_path: full_path.clone(),
            permission,
            children: Vec::new(),
        }
    }

    pub fn new_directory(name: &str, full_path: &PathBuf) -> Self {
        Node {
            node_type: NodeTypeSerializeable::Directory,
            name: name.to_string(),
            full_path: full_path.clone(),
            permission: FilePermission::ReadWrite, // Directories are usually writable
            children: Vec::new(),
        }
    }

    pub fn add_child(&mut self, node: Node) -> &mut Self {
        self.children.push(node);
        self
    }

    pub fn display(&self, level: usize) {
        let indent = "  ".repeat(level);
        println!("{}{} ({:?})", indent, self.name, self.node_type);
        for child in &self.children {
            child.display(level + 1);
        }
    }
}

// Convert Node tree to a vector of FileNode structs for the response
pub fn convert_to_file_nodes(node: &Node) -> Vec<FileNode> {
    let mut files = Vec::new();

    // Determine the node type based on the NodeTypeSerializeable
    let node_type = match node.node_type {
        NodeTypeSerializeable::Directory => NodeType::Directory as i32,
        NodeTypeSerializeable::File => NodeType::File as i32,
    };

    // Push the current node as a FileNode with its full path as the name
    files.push(FileNode {
        name: node.full_path.to_str().unwrap().to_string(),
        r#type: node_type,
        permission: node.permission.to_string(),
    });

    // Recursively add child nodes if it's a directory
    if node.node_type == NodeTypeSerializeable::Directory {
        for child in &node.children {
            files.extend(convert_to_file_nodes(child));
        }
    }

    files
}


// pub fn convert_to_file_nodes(node: &Node) -> Vec<FileNode> {
//     let mut files = Vec::new();
//
//     // Determine the node type based on the NodeTypeSerializeable
//     let node_type = match node.node_type {
//         NodeTypeSerializeable::Directory => NodeType::Directory as i32,
//         NodeTypeSerializeable::File => NodeType::File as i32,
//     };
//
//     // Push the current node as a FileNode with its full path as the name
//     files.push(FileNode {
//         name: node.full_path.to_str().unwrap().to_string(),
//         r#type: node_type,
//         permission: node.node_type.to_string(),
//     });
//
//     // Recursively add child nodes if it's a directory
//     if node.node_type == NodeTypeSerializeable::Directory {
//         for child in &node.children {
//             files.extend(convert_to_file_nodes(child));
//         }
//     }
//
//     files
// }



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
            .map_err(|err| anyhow!(err))
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

    async fn get_files_metadata(&self, owned_worker_id: OwnedWorkerId) -> Result<Vec<FileNode>, String> {
        let path = PathBuf::from(format!(
            "/worker_executor_store/compressed_oplog/-1/{}/{}/{}",
            owned_worker_id.worker_id.component_id,
            owned_worker_id.worker_id.component_id,
            owned_worker_id.worker_id.worker_name
        ));
        info!("--------------------------{}", path.display());

        // Start building nodes from the root path

        self.get_directory_metadata(owned_worker_id,path).await
        // match build_node(path.file_name().unwrap().to_str().unwrap().to_string(), path).await {
        //     Ok(root) => {
        //         let files = convert_to_file_nodes(&root);
        //         Ok(files)
        //     }
        //     Err(err) => {
        //         error!("Failed to retrieve file metadata: {:?}", err);
        //         Err(format!("Failed to get files metadata: {:?}", err))
        //     }
        // }
    }

    async fn get_file_or_directory(
        &self,
        owned_worker_id: OwnedWorkerId,
        path: String,
    ) -> Result<FileOrDirectoryResponse, String> {
        let path = PathBuf::from(path);

        // Check if the path is a directory asynchronously
        if self.is_directory(&path).await? {
            // If it's a directory, retrieve directory metadata
            let directory_metadata = self.get_directory_metadata(owned_worker_id, path).await?;
            Ok(FileOrDirectoryResponse::DirectoryListing(directory_metadata))
        } else {
            // If it's a file, retrieve file content
            let file_content = self.get_file(owned_worker_id, path).await?;
            Ok(FileOrDirectoryResponse::FileContent(file_content))
        }
    }

    async fn get_file(
        &self,
        owned_worker_id: OwnedWorkerId,
        path: PathBuf,
    ) -> Result<Vec<u8>, String> {
        let path = path.as_path();
        let bytes = self
            .blob_storage
            .get_file(path)
            .await
            .map_err(|err| format!("Failed to retrieve file content for worker {:?} at path {:?}: {:?}", owned_worker_id, path, err))?;

        Ok(bytes)
    }

    async fn get_directory_metadata(
        &self,
        owned_worker_id: OwnedWorkerId,
        path: PathBuf,
    ) -> Result<Vec<FileNode>, String> {
        // Implement your logic here to fetch directory metadata
        // let path = PathBuf::from(format!(
        //     "/worker_executor_store/compressed_oplog/-1/{}/{}/{}",
        //     owned_worker_id.worker_id.component_id,
        //     owned_worker_id.worker_id.component_id,
        //     owned_worker_id.worker_id.worker_name
        // ));
        // info!("--------------------------{}", path.display());

        // Start building nodes from the root path
        match build_node(path.file_name().unwrap().to_str().unwrap().to_string(), path).await {
            Ok(root) => {
                let files = convert_to_file_nodes(&root);
                Ok(files)
            }
            Err(err) => {
                error!("Failed to retrieve file metadata: {:?}", err);
                Err(format!("Failed to get files metadata: {:?}", err))
            }
        }

    }

    // Helper method to determine if a path is a directory
    async fn is_directory(&self, path: &PathBuf) -> Result<bool, String> {
        tokio::fs::metadata(path)
            .await
            .map(|metadata| metadata.is_dir())
            .map_err(|e| format!("Failed to check if path is directory: {:?}", e))
    }
}

// Function to build the directory tree asynchronously
pub fn build_node(name: String, path: PathBuf) -> BoxFuture<'static, io::Result<Node>> {
    Box::pin(async move {
        let metadata = fs::metadata(&path).await?;
        let node_type = if metadata.is_dir() {
            NodeTypeSerializeable::Directory
        } else {
            NodeTypeSerializeable::File
        };

        let node = match node_type {
            NodeTypeSerializeable::Directory => {
                let mut directory_node = Node::new_directory(&name, &path);

                let mut read_dir = fs::read_dir(&path).await?;
                while let Some(entry) = read_dir.next_entry().await? {
                    let child_name = entry.file_name().into_string().unwrap_or_default();
                    let child_path = entry.path();
                    let child_node = build_node(child_name, child_path).await?;
                    directory_node.add_child(child_node);
                }
                directory_node
            }
            NodeTypeSerializeable::File => Node::new_file(&name, &path).await,
        };

        Ok(node)
    })
}


#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
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
    use crate::storage::blob::fs::FileSystemBlobStorage;
    use crate::storage::blob::memory::InMemoryBlobStorage;

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
