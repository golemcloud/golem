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
use std::future::Future;
use std::hash::Hash;
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
    ) -> Result<Option<Arc<ToolDeploymentState>>, ComponentServiceError> {
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

/// Metadata that depends on the current deployment, keyed by the invalidation generation that
/// was current when the lookup started.
type CurrentCache<K, T> = Cache<(K, u64), (), Arc<Current<T>>, RegistryServiceError>;

type ToolDeploymentKey = (EnvironmentId, ComponentId, ComponentRevision);

pub struct RemoteComponentService {
    client: Arc<dyn RegistryService>,
    cache: Cache<(ComponentId, ComponentRevision), (), Component, CacheError>,
    current_cache: Option<CurrentCache<ComponentId, Component>>,
    tool_deployment_cache:
        Option<CurrentCache<ToolDeploymentKey, Option<Arc<ToolDeploymentState>>>>,
    generation: AtomicU64,
    max_age: Duration,
}

struct Current<T> {
    value: T,
    fetched_at: Instant,
}

const CURRENT_LOOKUP_RETRY_WINDOW: Duration = Duration::from_secs(30);

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
            current_cache: Self::current_cache(config, "current_component_metadata"),
            tool_deployment_cache: Self::current_cache(config, "current_tool_deployment_state"),
            generation: AtomicU64::new(0),
            max_age: config.current_component_cache_ttl,
        }
    }

    fn current_cache<K, T>(
        config: &ComponentServiceConfig,
        name: &'static str,
    ) -> Option<CurrentCache<K, T>>
    where
        K: Eq + Hash + Clone + Send + Sync + 'static,
        T: Send + Sync + 'static,
    {
        (config.component_cache_max_capacity > 0 && !config.current_component_cache_ttl.is_zero())
            .then(|| {
                Cache::new(
                    Some(config.component_cache_max_capacity),
                    FullCacheEvictionMode::LeastRecentlyUsed(1),
                    BackgroundEvictionMode::None,
                    name,
                )
            })
    }

    async fn evict_older_generations<K, T>(cache: &Option<CurrentCache<K, T>>, generation: u64)
    where
        K: Eq + Hash + Clone + Send + Sync + 'static,
        T: Send + Sync + 'static,
    {
        if let Some(cache) = cache {
            for key in cache.keys().await {
                if key.1 < generation {
                    cache.remove_if_cached(&key, |_| true).await;
                }
            }
        }
    }

    /// Reads current-deployment metadata through `cache`. A result counts only when no
    /// invalidation happened while it was fetched and it is younger than the configured TTL.
    /// Returns `None` when no such result arrived within the retry window.
    async fn get_current<K, T, F, Fut>(
        &self,
        cache: &Option<CurrentCache<K, T>>,
        key: K,
        fetch: F,
    ) -> Result<Option<Arc<Current<T>>>, RegistryServiceError>
    where
        K: Eq + Hash + Clone + Send + Sync + 'static,
        T: Send + Sync + 'static,
        F: Fn() -> Fut + Clone + Send + 'static,
        Fut: Future<Output = Result<T, RegistryServiceError>> + Send + 'static,
    {
        // Invalidations are global, so unrelated deployments can invalidate several reads
        // in a row. Bound retries by elapsed time, not by the number of deployments.
        let retry_deadline = Instant::now() + CURRENT_LOOKUP_RETRY_WINDOW;
        while Instant::now() < retry_deadline {
            tokio::task::yield_now().await;
            let generation = self.generation.load(Ordering::SeqCst);
            let fetch = fetch.clone();
            let fetch = move || async move {
                let fetched_at = Instant::now();
                fetch()
                    .await
                    .map(|value| Arc::new(Current { value, fetched_at }))
            };
            let cache_key = (key.clone(), generation);
            let result = match cache {
                Some(cache) => cache.get_or_insert_simple_spawned(&cache_key, fetch).await,
                None => fetch().await,
            };
            if self.generation.load(Ordering::SeqCst) != generation {
                continue;
            }
            let current = result?;
            if let Some(cache) = cache
                && current.fetched_at.elapsed() >= self.max_age
            {
                cache
                    .remove_if_cached(&cache_key, |value| Arc::ptr_eq(value, &current))
                    .await;
                continue;
            }
            if self.generation.load(Ordering::SeqCst) == generation {
                return Ok(Some(current));
            }
        }
        Ok(None)
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
#[golem_async_trait::region_simple]
impl ComponentService for RemoteComponentService {
    async fn invalidate_current(&self) {
        // Pending lookups may not yet identify their environment. Generation keys
        // keep their late fills from replacing or being read as current metadata.
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        Self::evict_older_generations(&self.current_cache, generation).await;
        Self::evict_older_generations(&self.tool_deployment_cache, generation).await;
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
                .map(|c| c.value.clone())
        } else {
            None
        }
    }

    async fn get_current_by_id(
        &self,
        component_id: ComponentId,
    ) -> Result<Component, ComponentServiceError> {
        let client = self.client.clone();
        let current = self
            .get_current(&self.current_cache, component_id, move || {
                let client = client.clone();
                async move { client.get_deployed_component_metadata(component_id).await }
            })
            .await
            .map_err(|error| match error {
                RegistryServiceError::NotFound(_) => ComponentServiceError::ComponentNotFound,
                other => other.into(),
            })?
            .ok_or_else(|| {
                ComponentServiceError::InternalError(anyhow!(
                    "Could not obtain fresh deployed component metadata for {component_id} after repeated invalidation or expiry"
                ))
            })?;
        self.store_component_in_cache(current.value.clone()).await;
        Ok(current.value.clone())
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
    ) -> Result<Option<Arc<ToolDeploymentState>>, ComponentServiceError> {
        let client = self.client.clone();
        let current = self
            .get_current(
                &self.tool_deployment_cache,
                (environment_id, component_id, component_revision),
                move || {
                    let client = client.clone();
                    async move {
                        client
                            .get_tool_deployment_state(
                                environment_id,
                                component_id,
                                component_revision,
                            )
                            .await
                            .map(|state| state.map(Arc::new))
                    }
                },
            )
            .await?
            .ok_or_else(|| {
                ComponentServiceError::InternalError(anyhow!(
                    "Could not obtain fresh tool deployment state for component {component_id} revision {component_revision} after repeated invalidation or expiry"
                ))
            })?;
        Ok(current.value.clone())
    }
}

#[cfg(test)]
pub(crate) mod tests;
