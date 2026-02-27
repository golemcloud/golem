// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::model::Component;
use std::sync::Arc;

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
    async fn get_latest_by_id_in_cache(&self, component_id: ComponentId) -> Option<Component>;

    // Might be outdated. Use get_latest_by_id_uncached if you always need the latest version
    async fn get_latest_by_id(
        &self,
        component_id: ComponentId,
    ) -> Result<Component, ComponentServiceError> {
        match self.get_latest_by_id_in_cache(component_id).await {
            Some(cached) => Ok(cached),
            None => self.get_latest_by_id_uncached(component_id).await,
        }
    }

    async fn get_latest_by_id_uncached(
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

pub struct RemoteComponentService {
    client: Arc<dyn RegistryService>,
    cache: Cache<(ComponentId, ComponentRevision), (), Component, CacheError>,
}

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
    async fn get_latest_by_id_in_cache(&self, component_id: ComponentId) -> Option<Component> {
        let mut keys = self.cache.keys().await;
        keys.retain(|(id, _)| *id == component_id);
        keys.sort_by_key(|(_, revision)| *revision);
        for idx in (0..keys.len()).rev() {
            let key = &keys[idx];
            let entry = self.cache.try_get(key).await;
            if let Some(metadata) = entry {
                return Some(metadata);
            }
        }
        None
    }

    async fn get_latest_by_id_uncached(
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
}
