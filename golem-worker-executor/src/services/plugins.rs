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

use super::golem_config::PluginServiceConfig;
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::component::{
    ComponentId, ComponentRevision, InstalledPlugin, PluginPriority,
};
use golem_common::model::plugin_registration::PluginRegistrationId;
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::plugin_registration::PluginRegistration;
use std::sync::Arc;

#[async_trait]
pub trait PluginsService: Send + Sync {
    /// Gets a plugin installation and the plugin definition it refers to for a given plugin
    /// installation id belonging to a specific component version
    async fn get(
        &self,
        component_id: &ComponentId,
        component_version: ComponentRevision,
        plugin_priority: PluginPriority,
    ) -> Result<(InstalledPlugin, PluginRegistration), WorkerExecutorError> {
        let plugin_installation = self
            .get_plugin_installation(component_id, component_version, plugin_priority)
            .await?;
        let plugin_definition = self
            .get_plugin_definition(&plugin_installation.plugin_registration_id)
            .await?;
        Ok((plugin_installation, plugin_definition))
    }

    async fn get_plugin_installation(
        &self,
        component_id: &ComponentId,
        component_version: ComponentRevision,
        plugin_priority: PluginPriority,
    ) -> Result<InstalledPlugin, WorkerExecutorError>;

    async fn get_plugin_definition(
        &self,
        plugin_id: &PluginRegistrationId,
    ) -> Result<PluginRegistration, WorkerExecutorError>;
}

pub fn configured(
    registry_service: Arc<dyn RegistryService>,
    config: &PluginServiceConfig,
) -> Arc<dyn PluginsService> {
    let client = CachedPlugins::new(
        PluginsRegistryService::new(registry_service),
        config.plugin_cache_size,
    );
    Arc::new(client)
}

#[allow(clippy::type_complexity)]
pub struct CachedPlugins<Inner: PluginsService> {
    inner: Inner,
    cached_plugin_installations: Cache<
        (ComponentId, ComponentRevision, PluginPriority),
        (),
        InstalledPlugin,
        WorkerExecutorError,
    >,
    cached_plugin_definitions:
        Cache<PluginRegistrationId, (), PluginRegistration, WorkerExecutorError>,
}

impl<Inner: PluginsService + Clone> Clone for CachedPlugins<Inner> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            cached_plugin_installations: self.cached_plugin_installations.clone(),
            cached_plugin_definitions: self.cached_plugin_definitions.clone(),
        }
    }
}

impl<Inner: PluginsService> CachedPlugins<Inner> {
    pub fn new(inner: Inner, plugin_cache_capacity: usize) -> Self {
        Self {
            inner,
            cached_plugin_installations: Cache::new(
                Some(plugin_cache_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "plugin_installations",
            ),
            cached_plugin_definitions: Cache::new(
                Some(plugin_cache_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "plugin_definitions",
            ),
        }
    }
}

#[async_trait]
impl<Inner: PluginsService + Clone + 'static> PluginsService for CachedPlugins<Inner> {
    async fn get_plugin_installation(
        &self,
        component_id: &ComponentId,
        component_version: ComponentRevision,
        plugin_priority: PluginPriority,
    ) -> Result<InstalledPlugin, WorkerExecutorError> {
        let key = (component_id.clone(), component_version, plugin_priority);
        let inner = self.inner.clone();
        let component_id = component_id.clone();
        self.cached_plugin_installations
            .get_or_insert_simple(&key, || {
                Box::pin(async move {
                    inner
                        .get_plugin_installation(&component_id, component_version, plugin_priority)
                        .await
                })
            })
            .await
    }

    async fn get_plugin_definition(
        &self,
        plugin_id: &PluginRegistrationId,
    ) -> Result<PluginRegistration, WorkerExecutorError> {
        let inner = self.inner.clone();
        self.cached_plugin_definitions
            .get_or_insert_simple(plugin_id, || {
                Box::pin(async move { inner.get_plugin_definition(plugin_id).await })
            })
            .await
    }
}

#[derive(Clone)]
struct PluginsRegistryService {
    client: Arc<dyn RegistryService>,
}

impl PluginsRegistryService {
    pub fn new(client: Arc<dyn RegistryService>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl PluginsService for PluginsRegistryService {
    async fn get_plugin_installation(
        &self,
        component_id: &ComponentId,
        component_version: ComponentRevision,
        plugin_priority: PluginPriority,
    ) -> Result<InstalledPlugin, WorkerExecutorError> {
        let component = self
            .client
            .get_component_metadata(component_id, component_version, &AuthCtx::System)
            .await
            .map_err(|e| WorkerExecutorError::runtime(format!("Failed getting component: {e}")))?;
        component
            .installed_plugins
            .into_iter()
            .find(|ip| ip.priority == plugin_priority)
            .ok_or(WorkerExecutorError::runtime(
                "failed to find plugin with priority in component",
            ))
    }

    async fn get_plugin_definition(
        &self,
        plugin_id: &PluginRegistrationId,
    ) -> Result<PluginRegistration, WorkerExecutorError> {
        self.client
            .get_plugin_registration_by_id(plugin_id, &AuthCtx::System)
            .await
            .map_err(|e| {
                WorkerExecutorError::runtime(format!("Failed getting plugin registration: {e}"))
            })
    }
}
