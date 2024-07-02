use crate::service::worker::WorkerServiceError;
use async_trait::async_trait;
use golem_common::model::ComponentId;
use golem_common::model::ComponentVersion;
use golem_service_base::model::{Component, ComponentMetadata, WorkerId};
use std::fmt::Display;

// Service to fetch the component metadata given a component-id
// This is different to ComponentMetadataFetch which gives richer data called ComponentElements
// that's more useful to evaluator.
// Outside modules/crates should use this service, while ComponentElementsFetch is visible only to the base crate
#[async_trait]
pub trait ComponentMetadataFetch {
    async fn get_latest_version_details(
        &self,
        component_id: &ComponentId,
    ) -> Result<Component, MetadataFetchError>;

    async fn get_currently_running_component(
        &self,
        worker_id: &WorkerId,
    ) -> Result<ComponentVersion, MetadataFetchError>;
}

#[derive(Clone)]
pub enum MetadataFetchError {
    WorkerNotFound,
    Internal(String),
}

impl Display for MetadataFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Worker component metadata fetch error: {}", self.0)
    }
}

pub struct NoopComponentMetadataFetch;

#[async_trait]
impl ComponentMetadataFetch for NoopComponentMetadataFetch {
    async fn get_latest_version_details(
        &self,
        _component_id: &ComponentId,
    ) -> Result<Component, MetadataFetchError> {
        Err(MetadataFetchError::Internal("Not implemented".to_string()))
    }

    async fn get_currently_running_component(
        &self,
        _worker_id: &WorkerId,
    ) -> Result<Component, MetadataFetchError> {
        Err(MetadataFetchError::Internal("Not implemented".to_string()))
    }
}
