use crate::evaluator::component_metadata_fetch::{ComponentMetadataFetch, MetadataFetchError};
use crate::evaluator::{Fqn, Function};
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, SimpleCache};
use golem_common::model::{ComponentId, ComponentVersion};
use golem_service_base::model::{ComponentMetadata, WorkerId};
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
                fqn: Fqn {
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
                    fqn: Fqn {
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
    component_elements_cache:
        Cache<(ComponentId, ComponentVersion), (), ComponentElements, MetadataFetchError>,
    currently_running_version_cache: Cache<WorkerId, (), ComponentVersion, MetadataFetchError>,
}

impl DefaultComponentElementsFetch {
    pub(crate) fn new(
        metadata_fetcher: Arc<dyn ComponentMetadataFetch + Sync + Send>,
        max_cache_size: usize,
    ) -> Self {
        DefaultComponentElementsFetch {
            component_metadata_fetch: metadata_fetcher,
            component_elements_cache: Cache::new(
                Some(max_cache_size),
                golem_common::cache::FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "worker_gateway_component_info",
            ),
            currently_running_version_cache: Cache::new(
                Some(max_cache_size),
                golem_common::cache::FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "worker_gateway_running_version_info",
            ),
        }
    }

    pub(crate) async fn get_currently_running_version_from_cache(
        &self,
        worker_id: WorkerId,
    ) -> Result<ComponentVersion, MetadataFetchError> {
        self.currently_running_version_cache
            .get_or_insert_simple(&worker_id.clone(), || {
                let component_metadata_service = self.component_metadata_fetch.clone();
                Box::pin(async move {
                    component_metadata_service
                        .get_currently_running_component(&worker_id)
                        .await
                })
            })
            .await
    }

    pub(crate) async fn get_component_elements_from_cache(
        &self,
        component_id: &ComponentId,
        version: ComponentVersion,
    ) -> Result<ComponentElements, MetadataFetchError> {
        self.component_elements_cache
            .get_or_insert_simple(&(component_id.clone(), version.clone()), || {
                let component_metadata_service = self.component_metadata_fetch.clone();
                Box::pin(async move {
                    let component = component_metadata_service
                        .get_latest_version_details(&component_id)
                        .await?;
                    Ok(ComponentElements::from_component_metadata(
                        component.metadata,
                    ))
                })
            })
            .await
    }

    pub(crate) async fn get_component_elements_from_cache_latest_version(
        &self,
        worker_id: &WorkerId,
    ) -> Result<ComponentElements, MetadataFetchError> {
        let latest_version = self
            .component_metadata_fetch
            .get_latest_version_details(&worker_id.component_id)
            .await?;

        let result = self
            .component_elements_cache
            .get_or_insert_simple(
                &(
                    worker_id.component_id.clone(),
                    latest_version.versioned_component_id.version.clone(),
                ),
                || {
                    let metadata = latest_version.metadata.clone();
                    Box::pin(
                        async move { Ok(ComponentElements::from_component_metadata(metadata)) },
                    )
                },
            )
            .await?;

        let _ = self
            .currently_running_version_cache
            .get_or_insert_simple(&worker_id.clone(), || {
                Box::pin(async move { Ok(latest_version.versioned_component_id.version) })
            })
            .await;

        Ok(result)
    }
}

// A service that will give richer data
// compared to ComponentMetadataFetch service
// which is required for the evaluator
#[async_trait]
pub(crate) trait ComponentElementsFetch {
    async fn get_component_elements(
        &self,
        worker_id: WorkerId,
    ) -> Result<ComponentElements, MetadataFetchError>;

    fn invalidate_cached_current_running_version(&self, worker_id: WorkerId);
}

#[async_trait]
impl ComponentElementsFetch for DefaultComponentElementsFetch {
    async fn get_component_elements(
        &self,
        worker_id: WorkerId,
    ) -> Result<ComponentElements, MetadataFetchError> {
        let current_version = self
            .get_currently_running_version_from_cache(worker_id.clone())
            .await;

        match current_version {
            Ok(current_version) => {
                self.get_component_elements_from_cache(&worker_id.component_id, current_version)
                    .await
            }
            Err(e) => match e {
                MetadataFetchError::WorkerNotFound => {
                    self.get_component_elements_from_cache_latest_version(&worker_id)
                        .await
                }
                MetadataFetchError::Internal(_) => Err(MetadataFetchError::Internal(
                    "Failed to get current version".to_string(),
                )),
            },
        }
    }

    fn invalidate_cached_current_running_version(&self, component_id: &ComponentId) {
        self.component_metadata_fetch
            .invalidate_cached_latest_version(component_id);
    }
}
