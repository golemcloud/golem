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

use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::cache::SimpleCache;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::model::component::ComponentDto;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::{error_forwarding, SafeDisplay};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::model::auth::AuthCtx;
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
    async fn get_by_version(
        &self,
        component_id: &ComponentId,
        version: ComponentRevision,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, ComponentServiceError>;

    async fn get_latest_by_id(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, ComponentServiceError>;

    async fn get_all_versions(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<ComponentDto>, ComponentServiceError>;
}

pub struct CachedComponentService {
    inner: Arc<dyn ComponentService>,
    cache: Cache<(ComponentId, ComponentRevision), (), ComponentDto, Arc<ComponentServiceError>>,
}

impl CachedComponentService {
    pub fn new(inner: Arc<dyn ComponentService>, cache_capacity: usize) -> Self {
        Self {
            inner,
            cache: Cache::new(
                Some(cache_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "component-metadata-cache",
            ),
        }
    }
}

#[async_trait]
impl ComponentService for CachedComponentService {
    async fn get_by_version(
        &self,
        component_id: &ComponentId,
        version: ComponentRevision,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, ComponentServiceError> {
        let inner_clone = self.inner.clone();
        self.cache
            .get_or_insert_simple(&(component_id.clone(), version), || async {
                inner_clone
                    .get_by_version(component_id, version, auth_ctx)
                    .await
                    .map_err(Arc::new)
            })
            .await
            .map_err(|e| match &*e {
                ComponentServiceError::InternalError(inner) => {
                    ComponentServiceError::InternalError(anyhow!("Cached error: {inner}"))
                }
                ComponentServiceError::ComponentNotFound => {
                    ComponentServiceError::ComponentNotFound
                }
            })
    }

    async fn get_latest_by_id(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, ComponentServiceError> {
        self.inner.get_latest_by_id(component_id, auth_ctx).await
    }

    async fn get_all_versions(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<ComponentDto>, ComponentServiceError> {
        self.inner.get_all_versions(component_id, auth_ctx).await
    }
}

pub struct RemoteComponentService {
    client: Arc<dyn RegistryService>,
}

impl RemoteComponentService {
    pub fn new(client: Arc<dyn RegistryService>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ComponentService for RemoteComponentService {
    async fn get_by_version(
        &self,
        component_id: &ComponentId,
        version: ComponentRevision,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, ComponentServiceError> {
        self.client
            .get_component_metadata(component_id, version, auth_ctx)
            .await
            .map_err(|e| match e {
                RegistryServiceError::NotFound(_) => ComponentServiceError::ComponentNotFound,
                other => other.into(),
            })
    }

    async fn get_latest_by_id(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, ComponentServiceError> {
        self.client
            .get_latest_component_metadata(component_id, auth_ctx)
            .await
            .map_err(|e| match e {
                RegistryServiceError::NotFound(_) => ComponentServiceError::ComponentNotFound,
                other => other.into(),
            })
    }

    async fn get_all_versions(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<ComponentDto>, ComponentServiceError> {
        self.client
            .get_all_component_versions(component_id, auth_ctx)
            .await
            .map_err(|e| match e {
                RegistryServiceError::NotFound(_) => ComponentServiceError::ComponentNotFound,
                other => other.into(),
            })
    }
}
