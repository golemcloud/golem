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

use crate::error::GolemError;
use crate::GolemTypes;
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::plugin::{PluginDefinition, PluginInstallation};
use golem_common::model::PluginId;
use golem_common::model::{AccountId, ComponentId, ComponentVersion, PluginInstallationId};

/// Part of the `Plugins` service for recording observed information as a way to pre-cache
/// data. It is in a separate trait because it does not have to be parametric for the Owner/Scope
/// types.
#[async_trait]
pub trait PluginsObservations: Send + Sync {
    /// Observes a known plugin installation; as getting component metadata returns the active set
    /// of installed plugins in its result, it is an opportunity to cache this information and
    /// use it in further calls to `get`.
    ///
    /// Calling this method is completely optional and only serves performance improvement purposes.
    /// `get` must always work even if `observe_plugin_installation` was never called.
    async fn observe_plugin_installation(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        plugin_installation: &PluginInstallation,
    ) -> Result<(), GolemError>;
}

#[async_trait]
pub trait Plugins<T: GolemTypes>: PluginsObservations {
    /// Gets a plugin installation and the plugin definition it refers to for a given plugin
    /// installation id belonging to a specific component version
    async fn get(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        installation_id: &PluginInstallationId,
    ) -> Result<
        (
            PluginInstallation,
            PluginDefinition<T::PluginOwner, T::PluginScope>,
        ),
        GolemError,
    > {
        let plugin_installation = self
            .get_plugin_installation(account_id, component_id, component_version, installation_id)
            .await?;
        let plugin_definition = self
            .get_plugin_definition(
                account_id,
                component_id,
                component_version,
                &plugin_installation,
            )
            .await?;
        Ok((plugin_installation, plugin_definition))
    }

    async fn get_plugin_installation(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        installation_id: &PluginInstallationId,
    ) -> Result<PluginInstallation, GolemError>;

    async fn get_plugin_definition(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        plugin_installation: &PluginInstallation,
    ) -> Result<PluginDefinition<T::PluginOwner, T::PluginScope>, GolemError>;
}

#[allow(clippy::type_complexity)]
pub struct CachedPlugins<T: GolemTypes, Inner: Plugins<T>> {
    inner: Inner,
    cached_plugin_installations: Cache<
        (
            AccountId,
            ComponentId,
            ComponentVersion,
            PluginInstallationId,
        ),
        (),
        PluginInstallation,
        GolemError,
    >,
    cached_plugin_definitions: Cache<
        (AccountId, PluginId),
        (),
        PluginDefinition<T::PluginOwner, T::PluginScope>,
        GolemError,
    >,
}

impl<T: GolemTypes, Inner: Plugins<T> + Clone> Clone for CachedPlugins<T, Inner> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            cached_plugin_installations: self.cached_plugin_installations.clone(),
            cached_plugin_definitions: self.cached_plugin_definitions.clone(),
        }
    }
}

impl<T: GolemTypes, Inner: Plugins<T>> CachedPlugins<T, Inner> {
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
impl<T: GolemTypes, Inner: Plugins<T>> PluginsObservations for CachedPlugins<T, Inner> {
    async fn observe_plugin_installation(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        plugin_installation: &PluginInstallation,
    ) -> Result<(), GolemError> {
        let key = (
            account_id.clone(),
            component_id.clone(),
            component_version,
            plugin_installation.id.clone(),
        );
        let installation = plugin_installation.clone();
        let _ = self
            .cached_plugin_installations
            .get_or_insert_simple(&key, || Box::pin(async move { Ok(installation) }))
            .await;
        Ok(())
    }
}

#[async_trait]
impl<T: GolemTypes, Inner: Plugins<T> + Clone + 'static> Plugins<T> for CachedPlugins<T, Inner> {
    async fn get_plugin_installation(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        installation_id: &PluginInstallationId,
    ) -> Result<PluginInstallation, GolemError> {
        let key = (
            account_id.clone(),
            component_id.clone(),
            component_version,
            installation_id.clone(),
        );
        let inner = self.inner.clone();
        let account_id = account_id.clone();
        let component_id = component_id.clone();
        let installation_id = installation_id.clone();
        self.cached_plugin_installations
            .get_or_insert_simple(&key, || {
                Box::pin(async move {
                    inner
                        .get_plugin_installation(
                            &account_id,
                            &component_id,
                            component_version,
                            &installation_id,
                        )
                        .await
                })
            })
            .await
    }

    async fn get_plugin_definition(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        plugin_installation: &PluginInstallation,
    ) -> Result<PluginDefinition<T::PluginOwner, T::PluginScope>, GolemError> {
        let key = (account_id.clone(), plugin_installation.plugin_id.clone());
        let inner = self.inner.clone();
        let account_id = account_id.clone();
        let component_id = component_id.clone();
        let plugin_installation = plugin_installation.clone();
        self.cached_plugin_definitions
            .get_or_insert_simple(&key, || {
                Box::pin(async move {
                    inner
                        .get_plugin_definition(
                            &account_id,
                            &component_id,
                            component_version,
                            &plugin_installation,
                        )
                        .await
                })
            })
            .await
    }
}

#[derive(Clone)]
pub struct PluginsUnavailable;

#[async_trait]
impl PluginsObservations for PluginsUnavailable {
    async fn observe_plugin_installation(
        &self,
        _account_id: &AccountId,
        _component_id: &ComponentId,
        _component_version: ComponentVersion,
        _plugin_installation: &PluginInstallation,
    ) -> Result<(), GolemError> {
        Ok(())
    }
}

#[async_trait]
impl<T: GolemTypes> Plugins<T> for PluginsUnavailable {
    async fn get_plugin_installation(
        &self,
        _account_id: &AccountId,
        _component_id: &ComponentId,
        _component_version: ComponentVersion,
        _installation_id: &PluginInstallationId,
    ) -> Result<PluginInstallation, GolemError> {
        Err(GolemError::runtime("Not available"))
    }

    async fn get_plugin_definition(
        &self,
        _account_id: &AccountId,
        _component_id: &ComponentId,
        _component_version: ComponentVersion,
        _plugin_installation: &PluginInstallation,
    ) -> Result<PluginDefinition<T::PluginOwner, T::PluginScope>, GolemError> {
        Err(GolemError::runtime("Not available"))
    }
}
