use std::sync::Arc;

use crate::services::blob_store::{BlobStoreService, DefaultBlobStoreService};
use crate::services::golem_config::CompiledComponentServiceConfig;
use crate::storage::blob::BlobStorage;

/// Struct representing the Initial File System (IFS) for a component
pub struct InitialFileSystem {
    pub data: Vec<u8>,
}

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