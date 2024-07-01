use golem_service_base::model::ComponentMetadata;
use rib::ParsedFunctionName;
use crate::evaluator::{FQN, Function};

// Static details of an evaluation context that doesn't change corresponding to input request details
// except for worker_id
// Here the lowest
#[derive(Debug, Clone)]
pub struct StaticSymbolTable {
    pub functions: Vec<Function>
}

impl StaticSymbolTable {
    pub fn empty() -> Self {
        StaticSymbolTable {
            functions: vec![],
        }
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

        StaticSymbolTable {
            functions: function_of_interfaces
                .into_iter()
                .chain(functions)
                .collect(),
        }
    }
}

pub(crate) mod cached {
    use std::sync::Arc;
    use async_trait::async_trait;
    use golem_common::cache::{BackgroundEvictionMode, Cache, SimpleCache};
    use golem_common::model::ComponentId;
    use golem_service_base::model::{ComponentMetadata, WorkerId};
    use crate::evaluator::symbol_table::StaticSymbolTable;
    use crate::evaluator::worker_metadata_fetcher::{MetadataFetchError, ComponentMetadataFetcher};

    // The logic shouldn't be visible outside the crate
    pub(crate) struct DefaultSymbolTableFetch {
        metadata_fetcher: Arc<dyn ComponentMetadataFetcher + Sync + Send>,
        cache: Cache<ComponentId, (), StaticSymbolTable, MetadataFetchError>,
    }

    impl DefaultSymbolTableFetch {
        pub fn new(metadata_fetcher:  Arc<dyn ComponentMetadataFetcher + Sync + Send>,) -> Self {
            DefaultSymbolTableFetch {
                metadata_fetcher,
                cache: Cache::new(
                    Some(10000),
                    golem_common::cache::FullCacheEvictionMode::LeastRecentlyUsed(1),
                    BackgroundEvictionMode::None,
                    "worker_gateway",
                )
            }
        }
    }

    #[async_trait]
    pub(crate) trait StaticSymbolTableFetch {
        async fn get_static_symbol_table(
            &self,
            component_id: &ComponentId,
        ) -> Result<StaticSymbolTable, MetadataFetchError>;

        fn invalidate_in_memory_symbol_table(&self, component_id: &ComponentId);
    }

    #[async_trait]
    impl StaticSymbolTableFetch for DefaultSymbolTableFetch {
        async fn get_static_symbol_table(
            &self,
            component_id: &ComponentId,
        ) -> Result<StaticSymbolTable, MetadataFetchError> {
            self.cache
                .get_or_insert_simple(component_id, || {
                    let metadata_fetcher = self.metadata_fetcher.clone();
                    Box::pin(async move {
                        let component_metadata= metadata_fetcher.get_component_metadata(component_id).await?;
                        Ok(StaticSymbolTable::from_component_metadata(component_metadata))
                    })
                })
                .await
        }

        fn invalidate_in_memory_symbol_table(&self, component_id: &ComponentId) {
            self.cache.remove(component_id);
        }
    }
}