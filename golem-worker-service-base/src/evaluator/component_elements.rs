use crate::evaluator::component_metadata_fetch::{ComponentMetadataService, MetadataFetchError};
use crate::evaluator::{Fqn, Function};
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, SimpleCache};
use golem_common::model::{ComponentId, ComponentVersion};
use golem_service_base::model::{ComponentMetadata, WorkerId};
use rib::ParsedFunctionName;

use std::sync::Arc;

// ComponentElements is geared to be used in the evaluation context of
// of Rib. It has more specific data coresponding to function calls
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
            .into_iter()
            .map(|f| Function {
                fqn: Fqn {
                    parsed_function_name: ParsedFunctionName::global(f.name),
                },
                arguments: f.parameters.clone(),
                return_type: f.results.clone(),
            })
            .collect::<Vec<Function>>();

        let function_of_interfaces = component_metadata
            .instances()
            .into_iter()
            .flat_map(|i| {
                i.functions.into_iter().map(move |f| Function {
                    fqn: Fqn {
                        parsed_function_name: ParsedFunctionName::parse(format!(
                            "{}.{{{}}}",
                            i.name,
                            f.name
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
    component_metadata_service: Arc<dyn ComponentMetadataService + Sync + Send>,
    component_elements_cache:
        Cache<(ComponentId, ComponentVersion), (), ComponentElements, MetadataFetchError>,
    currently_running_version_cache: Cache<WorkerId, (), ComponentVersion, MetadataFetchError>,
}

impl DefaultComponentElementsFetch {
    pub(crate) fn new(
        metadata_fetcher: Arc<dyn ComponentMetadataService + Sync + Send>,
        max_cache_size: usize,
    ) -> Self {
        DefaultComponentElementsFetch {
            component_metadata_service: metadata_fetcher,
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
        worker_id: &WorkerId,
    ) -> Result<ComponentVersion, MetadataFetchError> {
        self.currently_running_version_cache
            .get_or_insert_simple(&worker_id.clone(), || {
                let component_metadata_service = self.component_metadata_service.clone();
                let worker_id = worker_id.clone();
                Box::pin(async move {
                    component_metadata_service
                        .get_active_component_in_worker(&worker_id)
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
            .get_or_insert_simple(&(component_id.clone(), version), || {
                let component_metadata_service = self.component_metadata_service.clone();
                let component_id = component_id.clone();

                Box::pin(async move {
                    let component = component_metadata_service
                        .get_component_metadata(&component_id, version)
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
        let latest_component_version_details = self
            .component_metadata_service
            .get_latest_component_metadata(&worker_id.component_id)
            .await?;

        let _ = self
            .currently_running_version_cache
            .get_or_insert_simple(&worker_id.clone(), || {
                Box::pin(async move { Ok(latest_component_version_details.version) })
            })
            .await;

        // Caching the component_element details of the above version
        // If there is a winner here due to concurrent calls, we don't update it.
        // Incase there is a discrepancy between the association of worker-id -> version details
        // and the component_elements cache, it will get fixed in the next call.
        // -------------------------------------------------------------------------------------------
        // The Race condition of worker-executor getting updated with another version of component-id
        // -------------------------------------------------------------------------------------------
        // If worker-executor is updated with another version of component-id after the successfully updating
        // any cache related to evaluator, then the actual function invocation will fail
        // since Rib-evaluation-context has stale information and in that race condition,
        // we invalidate this cache and keep retrying for a configurable number of times.
        self.component_elements_cache
            .get_or_insert_simple(
                &(
                    worker_id.component_id.clone(),
                    latest_component_version_details.version,
                ),
                || {
                    let metadata = latest_component_version_details.metadata.clone();
                    Box::pin(
                        async move { Ok(ComponentElements::from_component_metadata(metadata)) },
                    )
                },
            )
            .await
    }
}

#[async_trait]
pub(crate) trait ComponentElementsFetch {
    async fn get_component_elements(
        &self,
        worker_id: &WorkerId,
    ) -> Result<ComponentElements, MetadataFetchError>;

    fn invalidate_cached_current_running_version(&self, worker_id: &WorkerId);
}

#[async_trait]
impl ComponentElementsFetch for DefaultComponentElementsFetch {
    async fn get_component_elements(
        &self,
        worker_id: &WorkerId,
    ) -> Result<ComponentElements, MetadataFetchError> {
        let current_version = self
            .get_currently_running_version_from_cache(worker_id)
            .await;

        match current_version {
            Ok(current_version) => {
                self.get_component_elements_from_cache(&worker_id.component_id, current_version)
                    .await
            }
            Err(e) => match e {
                MetadataFetchError::WorkerNotFound => {
                    self.get_component_elements_from_cache_latest_version(worker_id)
                        .await
                }
                MetadataFetchError::Internal(_) => Err(MetadataFetchError::Internal(
                    "Failed to get current version".to_string(),
                )),
            },
        }
    }

    fn invalidate_cached_current_running_version(&self, worker_id: &WorkerId) {
        self.currently_running_version_cache.remove(worker_id);
    }
}
