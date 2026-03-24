// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::error::SharedError;
use golem_common::model::resource_definition::{
    ResourceDefinition, ResourceDefinitionId, ResourceName,
};
use golem_common::{IntoAnyhow, SafeDisplay};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use std::sync::Arc;

#[derive(Debug, Clone, thiserror::Error)]
pub enum FetchError {
    #[error("Resource definition not found")]
    NotFound,
    #[error(transparent)]
    InternalError(SharedError),
}

impl IntoAnyhow for FetchError {
    fn into_anyhow(self) -> ::anyhow::Error {
        anyhow::Error::from(self).context("FetchError")
    }
}

impl SafeDisplay for FetchError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::NotFound => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

impl From<RegistryServiceError> for FetchError {
    fn from(err: RegistryServiceError) -> Self {
        match err {
            RegistryServiceError::NotFound(_) => FetchError::NotFound,
            other => FetchError::InternalError(SharedError::new(other)),
        }
    }
}

#[async_trait]
pub trait ResourceDefinitionFetcher: Send + Sync {
    /// Always fetches from the source. Never cached.
    async fn fetch_by_id(&self, id: ResourceDefinitionId)
        -> Result<ResourceDefinition, FetchError>;

    /// Resolves a name to a definition. May return a cached result.
    async fn resolve_by_name(
        &self,
        environment_id: EnvironmentId,
        name: ResourceName,
    ) -> Result<ResourceDefinition, FetchError>;

    /// Invalidates any cached entry for this (environment_id, name).
    async fn invalidate(&self, environment_id: EnvironmentId, name: ResourceName);

    /// Invalidates all cached entries.
    async fn invalidate_all(&self);
}

type DefinitionCache = Cache<(EnvironmentId, ResourceName), (), ResourceDefinition, FetchError>;

pub struct GrpcResourceDefinitionFetcher {
    registry_service: Arc<dyn RegistryService>,
    cache: DefinitionCache,
}

impl GrpcResourceDefinitionFetcher {
    pub fn new(
        registry_service: Arc<dyn RegistryService>,
        config: &crate::shard_manager_config::ResourceDefinitionFetcherConfig,
    ) -> Self {
        let cache = Cache::new(
            Some(config.cache_max_capacity),
            FullCacheEvictionMode::LeastRecentlyUsed(1),
            BackgroundEvictionMode::OlderThan {
                ttl: config.cache_ttl,
                period: config.cache_eviction_period,
            },
            "quota_resource_definitions",
        );
        Self {
            registry_service,
            cache,
        }
    }
}

#[async_trait]
impl ResourceDefinitionFetcher for GrpcResourceDefinitionFetcher {
    async fn fetch_by_id(
        &self,
        id: ResourceDefinitionId,
    ) -> Result<ResourceDefinition, FetchError> {
        self.registry_service
            .get_resource_definition_by_id(id)
            .await
            .map_err(FetchError::from)
    }

    async fn resolve_by_name(
        &self,
        environment_id: EnvironmentId,
        name: ResourceName,
    ) -> Result<ResourceDefinition, FetchError> {
        let key = (environment_id, name.clone());
        let registry_service = self.registry_service.clone();
        self.cache
            .get_or_insert_simple(&key, async || {
                registry_service
                    .get_resource_definition_by_name(environment_id, name)
                    .await
                    .map_err(FetchError::from)
            })
            .await
    }

    async fn invalidate(&self, environment_id: EnvironmentId, name: ResourceName) {
        self.cache.remove(&(environment_id, name)).await;
    }

    async fn invalidate_all(&self) {
        for key in self.cache.keys().await {
            self.cache.remove(&key).await;
        }
    }
}
