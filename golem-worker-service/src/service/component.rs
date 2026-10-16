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

use crate::config::ComponentServiceConfig;
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::component::ComponentId;
use golem_common::model::component::ComponentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::tool::ToolDeploymentState;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::model::component::Component;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::Instant;

#[derive(Debug, thiserror::Error)]
pub enum ComponentServiceError {
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
    #[error("Component not found")]
    ComponentNotFound,
}

impl SafeDisplay for ComponentServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalError(_) => "Internal error".to_string(),
            Self::ComponentNotFound => "Component not found".to_string(),
        }
    }
}

error_forwarding!(ComponentServiceError, RegistryServiceError);

#[async_trait]
pub trait ComponentService: Send + Sync {
    async fn invalidate_current(&self);

    async fn get_current_by_id_in_cache(&self, component_id: ComponentId) -> Option<Component>;

    // Might be outdated. Use get_current_by_id_uncached if you always need the deployed version.
    async fn get_current_by_id(
        &self,
        component_id: ComponentId,
    ) -> Result<Component, ComponentServiceError> {
        match self.get_current_by_id_in_cache(component_id).await {
            Some(cached) => Ok(cached),
            None => self.get_current_by_id_uncached(component_id).await,
        }
    }

    async fn get_current_by_id_uncached(
        &self,
        component_id: ComponentId,
    ) -> Result<Component, ComponentServiceError>;

    async fn get_revision(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Component, ComponentServiceError>;

    async fn get_all_revisions(
        &self,
        component_id: ComponentId,
    ) -> Result<Vec<Component>, ComponentServiceError>;

    async fn get_tool_deployment_state(
        &self,
        _environment_id: EnvironmentId,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
    ) -> Result<Option<ToolDeploymentState>, ComponentServiceError> {
        Err(ComponentServiceError::InternalError(anyhow!(
            "tool deployment metadata is not supported by this component service"
        )))
    }
}

// The error is not actually cached, just something that can be cloned and returned
// from the cache
#[derive(Clone)]
enum CacheError {
    // Note: Don't cache not founds as ids/revisions might be created later
    NotFound,
    Error,
}

impl From<CacheError> for ComponentServiceError {
    fn from(value: CacheError) -> Self {
        match value {
            CacheError::NotFound => ComponentServiceError::ComponentNotFound,
            CacheError::Error => {
                ComponentServiceError::InternalError(anyhow!("Cached request failed"))
            }
        }
    }
}

type CurrentComponentCache =
    Cache<(ComponentId, u64), (), Arc<CurrentComponent>, RegistryServiceError>;

pub struct RemoteComponentService {
    client: Arc<dyn RegistryService>,
    cache: Cache<(ComponentId, ComponentRevision), (), Component, CacheError>,
    current_cache: Option<CurrentComponentCache>,
    generation: AtomicU64,
    max_age: Duration,
}

struct CurrentComponent {
    component: Component,
    fetched_at: Instant,
}

const MAX_CURRENT_LOOKUP_ATTEMPTS: usize = 3;

impl RemoteComponentService {
    pub fn new(client: Arc<dyn RegistryService>, config: &ComponentServiceConfig) -> Self {
        Self {
            client,
            cache: Cache::new(
                Some(config.component_cache_max_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "component_metadata",
            ),
            current_cache: (config.component_cache_max_capacity > 0
                && !config.current_component_cache_ttl.is_zero())
            .then(|| {
                Cache::new(
                    Some(config.component_cache_max_capacity),
                    FullCacheEvictionMode::LeastRecentlyUsed(1),
                    BackgroundEvictionMode::None,
                    "current_component_metadata",
                )
            }),
            generation: AtomicU64::new(0),
            max_age: config.current_component_cache_ttl,
        }
    }

    async fn store_component_in_cache(&self, component: Component) {
        let _ = self
            .cache
            .get_or_insert_simple(&(component.id, component.revision), async move || {
                Ok(component)
            })
            .await;
    }
}

#[async_trait]
impl ComponentService for RemoteComponentService {
    async fn invalidate_current(&self) {
        // Pending lookups may not yet identify their environment. Generation keys
        // keep their late fills from replacing or being read as current metadata.
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(cache) = &self.current_cache {
            for key in cache.keys().await {
                if key.1 < generation {
                    cache.remove_if_cached(&key, |_| true).await;
                }
            }
        }
    }

    async fn get_current_by_id_in_cache(&self, component_id: ComponentId) -> Option<Component> {
        let generation = self.generation.load(Ordering::SeqCst);
        let component = self
            .current_cache
            .as_ref()?
            .try_get(&(component_id, generation))
            .await;
        if self.generation.load(Ordering::SeqCst) == generation {
            component
                .filter(|c| c.fetched_at.elapsed() < self.max_age)
                .map(|c| c.component.clone())
        } else {
            None
        }
    }

    async fn get_current_by_id(
        &self,
        component_id: ComponentId,
    ) -> Result<Component, ComponentServiceError> {
        for _ in 0..MAX_CURRENT_LOOKUP_ATTEMPTS {
            let generation = self.generation.load(Ordering::SeqCst);
            let client = self.client.clone();
            let key = (component_id, generation);
            let fetch = move || async move {
                let fetched_at = Instant::now();
                client
                    .get_deployed_component_metadata(component_id)
                    .await
                    .map(|component| {
                        Arc::new(CurrentComponent {
                            component,
                            fetched_at,
                        })
                    })
            };
            let result = match &self.current_cache {
                Some(cache) => cache.get_or_insert_simple_spawned(&key, fetch).await,
                None => fetch().await,
            };
            if self.generation.load(Ordering::SeqCst) != generation {
                continue;
            }
            let current = result.map_err(|error| match error {
                RegistryServiceError::NotFound(_) => ComponentServiceError::ComponentNotFound,
                other => other.into(),
            })?;
            self.store_component_in_cache(current.component.clone())
                .await;
            if let Some(cache) = &self.current_cache
                && current.fetched_at.elapsed() >= self.max_age
            {
                cache
                    .remove_if_cached(&key, |value| Arc::ptr_eq(value, &current))
                    .await;
                continue;
            }
            if self.generation.load(Ordering::SeqCst) == generation {
                return Ok(current.component.clone());
            }
        }
        Err(ComponentServiceError::InternalError(anyhow!(
            "Could not obtain fresh deployed component metadata for {component_id} after repeated invalidation or expiry"
        )))
    }

    async fn get_current_by_id_uncached(
        &self,
        component_id: ComponentId,
    ) -> Result<Component, ComponentServiceError> {
        let component = self
            .client
            .get_deployed_component_metadata(component_id)
            .await
            .map_err(|err| match err {
                RegistryServiceError::NotFound(_) => ComponentServiceError::ComponentNotFound,
                other => other.into(),
            })?;

        self.store_component_in_cache(component.clone()).await;

        Ok(component)
    }

    async fn get_revision(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Component, ComponentServiceError> {
        let component = self
            .cache
            .get_or_insert_simple(&(component_id, component_revision), async move || {
                self.client
                    .get_component_metadata(component_id, component_revision)
                    .await
                    .map_err(|e| match e {
                        RegistryServiceError::NotFound(_) => CacheError::NotFound,
                        e => {
                            tracing::warn!("Fetching component metadata failed: {e}");
                            CacheError::Error
                        }
                    })
            })
            .await?;
        Ok(component)
    }

    async fn get_all_revisions(
        &self,
        component_id: ComponentId,
    ) -> Result<Vec<Component>, ComponentServiceError> {
        let results = self
            .client
            .get_all_deployed_component_revisions(component_id)
            .await
            .map_err(|e| match e {
                RegistryServiceError::NotFound(_) => ComponentServiceError::ComponentNotFound,
                other => other.into(),
            })?;

        for result in &results {
            self.store_component_in_cache(result.clone()).await;
        }

        Ok(results)
    }

    async fn get_tool_deployment_state(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Option<ToolDeploymentState>, ComponentServiceError> {
        Ok(self
            .client
            .get_tool_deployment_state(environment_id, component_id, component_revision)
            .await?)
    }
}

#[cfg(test)]
pub(crate) mod tests;
