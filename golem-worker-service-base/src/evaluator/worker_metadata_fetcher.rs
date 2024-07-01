use std::fmt::Display;
use async_trait::async_trait;
use golem_common::model::ComponentId;
use golem_service_base::model::{ComponentMetadata, WorkerId};


#[async_trait]
pub trait ComponentMetadataFetcher {
    async fn get_component_metadata(
        &self,
        component_id: &ComponentId,
    ) -> Result<ComponentMetadata, MetadataFetchError>;
}

pub struct MetadataFetchError(pub String);

impl Display for MetadataFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Worker component metadata fetch error: {}", self.0)
    }
}

pub struct NoopComponentMetadataFetcher;

#[async_trait]
impl ComponentMetadataFetcher for NoopComponentMetadataFetcher {
    async fn get_component_metadata(
        &self,
        _component_id: &ComponentId,
    ) -> Result<ComponentMetadata, MetadataFetchError> {
        Ok(ComponentMetadata {
            exports: vec![],
            producers: vec![],
            memories: vec![],
        })
    }
}
