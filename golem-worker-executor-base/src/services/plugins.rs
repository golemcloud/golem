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
use crate::grpc::authorised_grpc_request;
use crate::services::golem_config::PluginServiceConfig;
use crate::{DefaultGolemTypes, GolemTypes};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::v1::plugin_service_client::PluginServiceClient;
use golem_api_grpc::proto::golem::component::v1::{
    get_installed_plugins_response, get_plugin_by_id_response, GetInstalledPluginsRequest,
    GetPluginByIdRequest,
};
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::plugin::{
    DefaultPluginOwner, DefaultPluginScope, PluginDefinition, PluginInstallation,
};
use golem_common::model::{AccountId, ComponentId, ComponentVersion, PluginInstallationId};
use golem_common::model::{PluginId, RetryConfig};
use http::Uri;
use std::sync::Arc;
use std::time::Duration;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use uuid::Uuid;

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

pub fn default_configured(
    config: &PluginServiceConfig,
) -> (
    Arc<dyn Plugins<DefaultGolemTypes>>,
    Arc<dyn PluginsObservations>,
) {
    match config {
        PluginServiceConfig::Grpc(config) => {
            let client1 = CachedPlugins::new(
                DefaultGrpcPlugins::new(
                    config.uri(),
                    config
                        .access_token
                        .parse::<Uuid>()
                        .expect("Access token must be an UUID"),
                    config.retries.clone(),
                    config.connect_timeout,
                ),
                config.plugin_cache_size,
            );
            let client2 = client1.clone();
            (Arc::new(client1), Arc::new(client2))
        }
        PluginServiceConfig::Local(_) => {
            let client1 = PluginsUnavailable;
            let client2 = client1.clone();
            (Arc::new(client1), Arc::new(client2))
        }
    }
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
struct DefaultGrpcPlugins {
    plugins_client: GrpcClient<PluginServiceClient<Channel>>,
    components_client: GrpcClient<ComponentServiceClient<Channel>>,
    access_token: Uuid,
}

impl DefaultGrpcPlugins {
    pub fn new(
        endpoint: Uri,
        access_token: Uuid,
        retry_config: RetryConfig,
        connect_timeout: Duration,
    ) -> Self {
        Self {
            plugins_client: GrpcClient::new(
                "plugins_service",
                move |channel| {
                    PluginServiceClient::new(channel)
                        .send_compressed(CompressionEncoding::Gzip)
                        .accept_compressed(CompressionEncoding::Gzip)
                },
                endpoint.clone(),
                GrpcClientConfig {
                    retries_on_unavailable: retry_config.clone(),
                    connect_timeout,
                },
            ),
            components_client: GrpcClient::new(
                "component_service",
                move |channel| {
                    ComponentServiceClient::new(channel)
                        .send_compressed(CompressionEncoding::Gzip)
                        .accept_compressed(CompressionEncoding::Gzip)
                },
                endpoint,
                GrpcClientConfig {
                    retries_on_unavailable: retry_config.clone(),
                    ..Default::default()
                },
            ),
            access_token,
        }
    }
}

#[async_trait]
impl PluginsObservations for DefaultGrpcPlugins {
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
impl Plugins<DefaultGolemTypes> for DefaultGrpcPlugins {
    async fn get_plugin_installation(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
        installation_id: &PluginInstallationId,
    ) -> Result<PluginInstallation, GolemError> {
        let response = self
            .components_client
            .call("get_installed_plugins", move |client| {
                let request = authorised_grpc_request(
                    GetInstalledPluginsRequest {
                        component_id: Some(component_id.clone().into()),
                        version: Some(component_version),
                    },
                    &self.access_token,
                );
                Box::pin(client.get_installed_plugins(request))
            })
            .await
            .map_err(|err| {
                GolemError::runtime(format!("Failed to get installed plugins: {err:?}"))
            })?
            .into_inner();
        let installations: Vec<PluginInstallation> = match response.result {
            None => Err(GolemError::runtime("Empty response"))?,
            Some(get_installed_plugins_response::Result::Success(response)) => response
                .installations
                .into_iter()
                .map(|i| i.try_into())
                .collect::<Result<Vec<_>, _>>()
                .map_err(GolemError::runtime)?,
            Some(get_installed_plugins_response::Result::Error(error)) => {
                Err(GolemError::runtime(format!("{error:?}")))?
            }
        };

        let mut result = None;
        for installation in installations {
            self.observe_plugin_installation(
                account_id,
                component_id,
                component_version,
                &installation,
            )
            .await?;

            if installation.id == *installation_id {
                result = Some(installation);
            }
        }

        result.ok_or(GolemError::runtime("Plugin installation not found"))
    }

    async fn get_plugin_definition(
        &self,
        _account_id: &AccountId,
        _component_id: &ComponentId,
        _component_version: ComponentVersion,
        plugin_installation: &PluginInstallation,
    ) -> Result<PluginDefinition<DefaultPluginOwner, DefaultPluginScope>, GolemError> {
        let response = self
            .plugins_client
            .call("get_plugin_by_id", move |client| {
                let request = authorised_grpc_request(
                    GetPluginByIdRequest {
                        id: Some(plugin_installation.plugin_id.clone().into()),
                    },
                    &self.access_token,
                );
                Box::pin(client.get_plugin_by_id(request))
            })
            .await
            .map_err(|err| {
                GolemError::runtime(format!("Failed to get plugin definition: {err:?}"))
            })?
            .into_inner();

        match response.result {
            None => Err(GolemError::runtime("Empty response"))?,
            Some(get_plugin_by_id_response::Result::Success(response)) => Ok(response
                .plugin
                .ok_or("Missing plugin field")
                .map_err(GolemError::runtime)?
                .try_into()
                .map_err(GolemError::runtime)?),
            Some(get_plugin_by_id_response::Result::Error(error)) => {
                Err(GolemError::runtime(format!("{error:?}")))?
            }
        }
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
