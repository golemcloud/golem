use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::time::Instant;
use tracing::{debug, info};
use wasmtime::Engine;
use golem_api_grpc::proto::golem::workerexecutor::v1::FileNode;
use golem_common::model::{AccountId, ComponentId, OwnedWorkerId};
use crate::error::GolemError;
use crate::services::blob_store::{BlobStoreService, DefaultBlobStoreService, FileOrDirectoryResponse, ObjectMetadata};
use crate::services::compiled_component::{CompiledComponentService, CompiledComponentServiceDisabled, DefaultCompiledComponentService};
use crate::services::golem_config::CompiledComponentServiceConfig;
use crate::storage::blob::{BlobStorage, BlobStorageNamespace};

/// Struct representing the Initial File System (IFS) for a component
pub struct InitialFileSystem {
    pub data: Vec<u8>,
}

/// Service for storing initial file system (IFS) data for WebAssembly components
// #[async_trait]
// pub trait InitialFileSystemService {
//     async fn get(
//         &self,
//         component_id: &ComponentId,
//         component_version: u64,
//     ) -> Result<Option<InitialFileSystem>, GolemError>;
//
//     async fn put(
//         &self,
//         component_id: &ComponentId,
//         component_version: u64,
//         initial_file: &Vec<u8>,
//         path: &Path
//     ) -> Result<(), GolemError>;
//
//     async fn set_permissions(
//         &self,
//         path: &Path,
//     ) -> Result<(), GolemError>;
//
// }

pub struct DefaultInitialFileSystemService {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
}

impl DefaultInitialFileSystemService {
    pub fn new(blob_storage: Arc<dyn BlobStorage + Send + Sync>) -> Self {
        Self { blob_storage }
    }

    fn key(component_id: &ComponentId, component_version: u64) -> PathBuf {
        Path::new(&component_id.to_string()).join(format!("{component_version}.ifs"))
    }
}

#[async_trait]
impl BlobStoreService for DefaultInitialFileSystemService {
    async fn clear(&self, account_id: AccountId, container_name: String) -> anyhow::Result<()> {
        todo!()
    }

    async fn container_exists(&self, account_id: AccountId, container_name: String) -> anyhow::Result<bool> {
        todo!()
    }

    async fn copy_object(&self, account_id: AccountId, source_container_name: String, source_object_name: String, destination_container_name: String, destination_object_name: String) -> anyhow::Result<()> {
        todo!()
    }

    async fn create_container(&self, account_id: AccountId, container_name: String) -> anyhow::Result<()> {
        todo!()
    }

    async fn delete_container(&self, account_id: AccountId, container_name: String) -> anyhow::Result<()> {
        todo!()
    }

    async fn delete_object(&self, account_id: AccountId, container_name: String, object_name: String) -> anyhow::Result<()> {
        todo!()
    }

    async fn delete_objects(&self, account_id: AccountId, container_name: String, object_names: Vec<String>) -> anyhow::Result<()> {
        todo!()
    }

    async fn get_container(&self, account_id: AccountId, container_name: String) -> anyhow::Result<Option<u64>> {
        todo!()
    }

    async fn get_data(&self, account_id: AccountId, container_name: String, object_name: String, start: u64, end: u64) -> anyhow::Result<Vec<u8>> {
        todo!()
    }

    async fn has_object(&self, account_id: AccountId, container_name: String, object_name: String) -> anyhow::Result<bool> {
        todo!()
    }

    async fn list_objects(&self, account_id: AccountId, container_name: String) -> anyhow::Result<Vec<String>> {
        todo!()
    }

    async fn move_object(&self, account_id: AccountId, source_container_name: String, source_object_name: String, destination_container_name: String, destination_object_name: String) -> anyhow::Result<()> {
        todo!()
    }

    async fn object_info(&self, account_id: AccountId, container_name: String, object_name: String) -> anyhow::Result<ObjectMetadata> {
        todo!()
    }

    async fn write_data(&self, account_id: AccountId, container_name: String, object_name: String, data: Vec<u8>) -> anyhow::Result<()> {
        todo!()
    }

    async fn get_files_metadata(&self, owned_worker_id: OwnedWorkerId) -> Result<Vec<FileNode>, String> {
        todo!()
    }

    async fn get_file_or_directory(&self, owned_worker_id: OwnedWorkerId, path: String) -> Result<FileOrDirectoryResponse, String> {
        todo!()
    }

    async fn get_file(&self, owned_worker_id: OwnedWorkerId, path: PathBuf) -> Result<std::io::Result<Vec<u8>>, String> {
        todo!()
    }

    async fn get_directory_metadata(&self, owned_worker_id: OwnedWorkerId, path: PathBuf) -> Result<Vec<FileNode>, String> {
        todo!()
    }

    async fn initialize_worker_ifs(&self, owned_worker_id: OwnedWorkerId) -> Result<(), String> {
        todo!()
    }

    async fn setup_ifs_source(&self, component_id: ComponentId) -> Result<String, String> {
        todo!()
    }

    async fn generate_path(&self, component_id: ComponentId) -> Result<String, String> {
        todo!()
    }

    async fn save_ifs_zip(&self, initial_file_system: Vec<u8>, component_id: ComponentId, version: u64) -> Result<String, String> {
        todo!()
    }

    async fn decompress_ifs(&self, component_id: ComponentId, version: u64) -> Result<(), String> {
        todo!()
    }
    // async fn get(
    //     &self,
    //     component_id: &ComponentId,
    //     component_version: u64,
    // ) -> Result<Option<InitialFileSystem>, GolemError> {
    //     match self
    //         .blob_storage
    //         .get_raw(
    //             "initial_file_system",
    //             "get",
    //             BlobStorageNamespace::InitialFileSystem(AccountId{value: component_id.to_string()}),
    //             &Self::key(component_id, component_version),
    //         )
    //         .await
    //     {
    //         Ok(None) => Ok(None),
    //         Ok(Some(bytes)) => {
    //             let start = Instant::now();
    //             let end = Instant::now();
    //             let load_time = end.duration_since(start);
    //             debug!(
    //                 "Loaded initial file system for {} in {}ms",
    //                 component_id,
    //                 load_time.as_millis(),
    //             );
    //
    //             Ok(Some(InitialFileSystem { data: Vec::from(bytes) }))
    //         }
    //         Err(err) => Err(GolemError::component_download_failed(
    //             component_id.clone(),
    //             component_version,
    //             format!("Could not download initial file system: {err}"),
    //         )),
    //     }
    // }
    //
    // async fn put(
    //     &self,
    //     component_id: &ComponentId,
    //     component_version: u64,
    //     initial_file: &Vec<u8>,
    //     path: &Path
    // ) -> Result<(), GolemError> {
    //     info!("Saving initial file system for component: {}", component_id);
    //     info!("Saving to {}", path.display());
    //
    //     self.blob_storage
    //         .put_raw(
    //             "initial_file_system",
    //             "put",
    //             BlobStorageNamespace::InitialFileSystem(AccountId{value: component_id.to_string()}),
    //             &path,
    //             &initial_file,
    //         )
    //         .await
    //         .map_err(|err| {
    //             GolemError::component_download_failed(
    //                 component_id.clone(),
    //                 component_version,
    //                 format!("Could not store initial file system: {err}"),
    //             )
    //         })
    // }

    async fn set_permissions(&self, path: &Path) -> Result<(), GolemError> {
        self.blob_storage.set_permissions(path).await.map_err(|err| {GolemError::PermissionsNotSet })
    }
}
pub struct InitialFileSystemServiceDisabled {}

impl Default for InitialFileSystemServiceDisabled {
    fn default() -> Self {
        Self::new()
    }
}

impl InitialFileSystemServiceDisabled {
    pub fn new() -> Self {
        InitialFileSystemServiceDisabled {}
    }
}

// #[async_trait]
// impl InitialFileSystemService for InitialFileSystemServiceDisabled {
//     async fn get(
//         &self,
//         _component_id: &ComponentId,
//         _component_version: u64,
//     ) -> Result<Option<InitialFileSystem>, GolemError> {
//         Ok(None)
//     }
//
//     async fn put(
//         &self,
//         component_id: &ComponentId,
//         component_version: u64,
//         initial_file: &Vec<u8>,
//         path: &Path
//     ) -> Result<(), GolemError> {
//         Ok(())
//     }
//
//     async fn set_permissions(&self, path: &Path) -> Result<(), GolemError> {
//         todo!()
//     }
// }

pub fn configured(
    config: &CompiledComponentServiceConfig,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
) -> Arc<dyn BlobStoreService + Send + Sync> {
    match config {
        CompiledComponentServiceConfig::Enabled(_) => {
            Arc::new(DefaultBlobStoreService::new(blob_storage))
        }
        CompiledComponentServiceConfig::Disabled(_) => {
            Arc::new(DefaultBlobStoreService::new(blob_storage))
        }
    }
}