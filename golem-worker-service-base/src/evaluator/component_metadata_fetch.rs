use async_trait::async_trait;
use golem_common::model::ComponentId;
use golem_service_base::model::ComponentMetadata;
use std::fmt::Display;

#[async_trait]
pub trait ComponentMetadataFetch {
    async fn get_component_metadata(
        &self,
        component_id: &ComponentId,
    ) -> Result<ComponentMetadata, MetadataFetchError>;
}

#[derive(Clone)]
pub struct MetadataFetchError(pub String);

impl Display for MetadataFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Worker component metadata fetch error: {}", self.0)
    }
}

pub struct NoopComponentMetadataFetch;

#[async_trait]
impl ComponentMetadataFetch for NoopComponentMetadataFetch {
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
