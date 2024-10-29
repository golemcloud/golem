use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::time::Instant;
use tracing::{debug, info};
use wasmtime::Engine;
use golem_common::model::{AccountId, ComponentId};
use crate::error::GolemError;
use crate::services::compiled_component::{CompiledComponentService, CompiledComponentServiceDisabled, DefaultCompiledComponentService};
use crate::services::golem_config::CompiledComponentServiceConfig;
use crate::storage::blob::{BlobStorage, BlobStorageNamespace};

/// Struct representing the Initial File System (IFS) for a component
pub struct InitialFileSystem {
    pub data: Vec<u8>,
}

/// Service for storing initial file system (IFS) data for WebAssembly components
#[async_trait]
pub trait InitialFileSystemService {
    async fn get(
        &self,
        component_id: &ComponentId,
        component_version: u64,
    ) -> Result<Option<InitialFileSystem>, GolemError>;

    async fn put(
        &self,
        component_id: &ComponentId,
        component_version: u64,
        initial_file: &Vec<u8>,
        path: &Path
    ) -> Result<(), GolemError>;

    async fn set_permissions(
        &self,
        path: &Path,
    ) -> Result<(), GolemError>;

}

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
impl InitialFileSystemService for DefaultInitialFileSystemService {
    async fn get(
        &self,
        component_id: &ComponentId,
        component_version: u64,
    ) -> Result<Option<InitialFileSystem>, GolemError> {
        match self
            .blob_storage
            .get_raw(
                "initial_file_system",
                "get",
                BlobStorageNamespace::InitialFileSystem(AccountId{value: component_id.to_string()}),
                &Self::key(component_id, component_version),
            )
            .await
        {
            Ok(None) => Ok(None),
            Ok(Some(bytes)) => {
                let start = Instant::now();
                let end = Instant::now();
                let load_time = end.duration_since(start);
                debug!(
                    "Loaded initial file system for {} in {}ms",
                    component_id,
                    load_time.as_millis(),
                );

                Ok(Some(InitialFileSystem { data: Vec::from(bytes) }))
            }
            Err(err) => Err(GolemError::component_download_failed(
                component_id.clone(),
                component_version,
                format!("Could not download initial file system: {err}"),
            )),
        }
    }

    async fn put(
        &self,
        component_id: &ComponentId,
        component_version: u64,
        initial_file: &Vec<u8>,
        path: &Path
    ) -> Result<(), GolemError> {
        info!("Saving initial file system for component: {}", component_id);
        info!("Saving to {}", path.display());

        self.blob_storage
            .put_raw(
                "initial_file_system",
                "put",
                BlobStorageNamespace::InitialFileSystem(AccountId{value: component_id.to_string()}),
                // &Self::key(component_id, component_version),
                &path,
                &initial_file,
            )
            .await
            .map_err(|err| {
                GolemError::component_download_failed(
                    component_id.clone(),
                    component_version,
                    format!("Could not store initial file system: {err}"),
                )
            })
    }

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

#[async_trait]
impl InitialFileSystemService for InitialFileSystemServiceDisabled {
    async fn get(
        &self,
        _component_id: &ComponentId,
        _component_version: u64,
    ) -> Result<Option<InitialFileSystem>, GolemError> {
        Ok(None)
    }

    async fn put(
        &self,
        component_id: &ComponentId,
        component_version: u64,
        initial_file: &Vec<u8>,
        path: &Path
    ) -> Result<(), GolemError> {
        Ok(())
    }

    async fn set_permissions(&self, path: &Path) -> Result<(), GolemError> {
        todo!()
    }
}

pub fn configured(
    config: &CompiledComponentServiceConfig,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
) -> Arc<dyn InitialFileSystemService + Send + Sync> {
    match config {
        CompiledComponentServiceConfig::Enabled(_) => {
            Arc::new(DefaultInitialFileSystemService::new(blob_storage))
        }
        CompiledComponentServiceConfig::Disabled(_) => {
            Arc::new(InitialFileSystemServiceDisabled::new())
        }
    }
}