use crate::evaluator::component_metadata_fetch::{ComponentMetadataFetch, MetadataFetchError};
use crate::evaluator::{Function, FQN};
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, SimpleCache};
use golem_common::model::ComponentId;
use golem_service_base::model::ComponentMetadata;
use rib::ParsedFunctionName;

use std::sync::Arc;
#[derive(PartialEq, Debug, Clone)]
pub struct ComponentElements {
    pub functions: Vec<Function>,
}

impl ComponentElements {
    pub fn empty() -> Self {
        ComponentElements { functions: vec![] }
    }

    pub fn from_component_metadata(component_metadata: ComponentMetadata) -> Self {
        let top_level_functions = component_metadata.functions();

        let functions = top_level_functions
            .iter()
            .map(|f| Function {
                fqn: FQN {
                    parsed_function_name: ParsedFunctionName::global(f.name.clone()),
                },
                arguments: f.parameters.clone(),
                return_type: f.results.clone(),
            })
            .collect::<Vec<Function>>();

        let function_of_interfaces = component_metadata
            .instances()
            .iter()
            .flat_map(|i| {
                i.functions.iter().map(move |f| Function {
                    fqn: FQN {
                        parsed_function_name: ParsedFunctionName::parse(format!(
                            "{}.{{{}}}",
                            i.name.clone(),
                            f.name.clone()
                        ))
                        .unwrap(),
                    },
                    arguments: f.parameters.clone(),
                    return_type: f.results.clone(),
                })
            })
            .collect::<Vec<Function>>();

        ComponentElements {
            functions: function_of_interfaces
                .into_iter()
                .chain(functions)
                .collect(),
        }
    }
}

// The logic shouldn't be visible outside the crate
pub(crate) struct DefaultComponentElementsFetch {
    component_metadata_fetch: Arc<dyn ComponentMetadataFetch + Sync + Send>,
    component_elements_cache: Cache<ComponentId, (), ComponentElements, MetadataFetchError>,
}

impl DefaultComponentElementsFetch {
    pub(crate) fn new(metadata_fetcher: Arc<dyn ComponentMetadataFetch + Sync + Send>) -> Self {
        DefaultComponentElementsFetch {
            component_metadata_fetch: metadata_fetcher,
            component_elements_cache: Cache::new(
                Some(10000),
                golem_common::cache::FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "worker_gateway",
            ),
        }
    }
}

// A service that will give richer data
// compared to ComponentMetadataFetch service
// which is required for the evaluator
#[async_trait]
pub(crate) trait ComponentElementsFetch {
    async fn get_component_elements(
        &self,
        component_id: ComponentId,
    ) -> Result<ComponentElements, MetadataFetchError>;

    fn invalidate_cached_component_elements(&self, component_id: &ComponentId);
}

#[async_trait]
impl ComponentElementsFetch for DefaultComponentElementsFetch {
    async fn get_component_elements(
        &self,
        component_id: ComponentId,
    ) -> Result<ComponentElements, MetadataFetchError> {
        self.component_elements_cache
            .get_or_insert_simple(&component_id.clone(), || {
                let metadata_fetcher = self.component_metadata_fetch.clone();
                Box::pin(async move {
                    let component_metadata = metadata_fetcher
                        .get_component_metadata(&component_id)
                        .await?;
                    Ok(ComponentElements::from_component_metadata(
                        component_metadata,
                    ))
                })
            })
            .await
    }

    fn invalidate_cached_component_elements(&self, component_id: &ComponentId) {
        self.component_elements_cache.remove(component_id);
    }
}
