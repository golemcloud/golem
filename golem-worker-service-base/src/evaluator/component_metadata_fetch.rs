use async_trait::async_trait;
use golem_common::model::ComponentId;
use golem_common::model::ComponentVersion;
use golem_service_base::model::{ComponentMetadata, WorkerId};
use std::fmt::Display;

#[async_trait]
pub trait ComponentMetadataService {
    async fn get_latest_component_metadata(
        &self,
        component_id: &ComponentId,
    ) -> Result<ComponentDetails, MetadataFetchError>;

    async fn get_component_metadata(
        &self,
        component_id: &ComponentId,
        version: ComponentVersion
    ) -> Result<ComponentDetails, MetadataFetchError>;

    async fn get_active_component_in_worker(
        &self,
        worker_id: &WorkerId,
    ) -> Result<ComponentVersion, MetadataFetchError>;
}

pub struct ComponentDetails {
    pub version: ComponentVersion,
    pub metadata: ComponentMetadata,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MetadataFetchError {
    WorkerNotFound,
    Internal(String),
}

impl Display for MetadataFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetadataFetchError::WorkerNotFound => write!(f, "Worker not found"),
            MetadataFetchError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

pub struct NoopComponentMetadataService;

#[async_trait]
impl ComponentMetadataService for NoopComponentMetadataService {
    async fn get_latest_component_metadata(
        &self,
        _component_id: &ComponentId,
    ) -> Result<ComponentDetails, MetadataFetchError> {
        Err(MetadataFetchError::Internal("Not implemented".to_string()))
    }

    async fn get_component_metadata(&self, component_id: &ComponentId, version: ComponentVersion) -> Result<ComponentDetails, MetadataFetchError> {
        Err(MetadataFetchError::Internal("Not implemented".to_string()))
    }

    async fn get_active_component_in_worker(
        &self,
        _worker_id: &WorkerId,
    ) -> Result<ComponentVersion, MetadataFetchError> {
        Err(MetadataFetchError::Internal("Not implemented".to_string()))
    }
}
