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
use golem_common::model::plugin_registration::{PluginRegistrationDto, PluginRegistrationId};
use golem_common::model::component::{ComponentId, ComponentRevision, InstalledPlugin, PluginPriority};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::sync::Arc;
use uuid::Uuid;
use golem_service_base::model::plugin_registration::PluginRegistration;

#[async_trait]
pub trait PluginsService: Send + Sync {
    // /// Observes a known plugin installation; as getting component metadata returns the active set
    // /// of installed plugins in its result, it is an opportunity to cache this information and
    // /// use it in further calls to `get`.
    // ///
    // /// Calling this method is completely optional and only serves performance improvement purposes.
    // /// `get` must always work even if `observe_plugin_installation` was never called.
    // async fn observe_plugin_installation(
    //     &self,
    //     component_id: &ComponentId,
    //     component_version: ComponentRevision,
    //     plugin_priority: &i32,
    // ) -> Result<(), WorkerExecutorError>;

    /// Gets a plugin installation and the plugin definition it refers to for a given plugin
    /// installation id belonging to a specific component version
    async fn get(
        &self,
        component_id: &ComponentId,
        component_version: ComponentRevision,
        plugin_priority: PluginPriority
    ) -> Result<(InstalledPlugin, PluginRegistration), WorkerExecutorError> {
        let plugin_installation = self
            .get_plugin_installation(component_id, component_version, plugin_priority)
            .await?;
        let plugin_definition = self.get_plugin_definition(&plugin_installation.plugin_id).await?;
        Ok((plugin_installation, plugin_definition))
    }

    async fn get_plugin_installation(
        &self,
        component_id: &ComponentId,
        component_version: ComponentRevision,
        plugin_priority: PluginPriority
    ) -> Result<InstalledPlugin, WorkerExecutorError>;

    async fn get_plugin_definition(
        &self,
        plugin_id: &PluginRegistrationId
    ) -> Result<PluginRegistration, WorkerExecutorError>;
}

pub fn configured(config: &PluginServiceConfig) -> Arc<dyn PluginsService> {
    match config {
        PluginServiceConfig::Grpc(config) => {
            let client = CachedPlugins::new(
                self::grpc::PluginsGrpc::new(
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
            Arc::new(client)
        }
        PluginServiceConfig::Local(_) => Arc::new(PluginsUnavailable),
    }
}

#[allow(clippy::type_complexity)]
pub struct CachedPlugins<Inner: PluginsService> {
    inner: Inner,
    cached_plugin_installations: Cache<
        (
            ComponentId,
            ComponentRevision,
            i32,
        ),
        (),
        InstalledPlugin,
        WorkerExecutorError,
    >,
    cached_plugin_definitions:
        Cache<PluginRegistrationId, (), PluginRegistrationDto, WorkerExecutorError>,
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
    // async fn observe_plugin_installation(
    //     &self,
    //     component_id: &ComponentId,
    //     component_version: ComponentRevision,
    //     plugin_priority: &i32,
    // ) -> Result<(), WorkerExecutorError> {
    //     let key = (
    //         component_id.clone(),
    //         component_version,
    //         plugin_priority,
    //     );
    //     let installation = plugin_installation.clone();
    //     let _ = self
    //         .cached_plugin_installations
    //         .get_or_insert_simple(&key, || Box::pin(async move { Ok(installation) }))
    //         .await;
    //     Ok(())
    // }

    async fn get_plugin_installation(
        &self,
        component_id: &ComponentId,
        component_version: ComponentRevision,
        plugin_priority: i32
    ) -> Result<InstalledPlugin, WorkerExecutorError> {
        let key = (
            component_id.clone(),
            component_version,
            plugin_priority,
        );
        let inner = self.inner.clone();
        let component_id = component_id.clone();
        self.cached_plugin_installations
            .get_or_insert_simple(&key, || {
                Box::pin(async move {
                    inner
                        .get_plugin_installation(
                            &component_id,
                            component_version,
                            plugin_priority,
                        )
                        .await
                })
            })
            .await
    }

    async fn get_plugin_definition(
        &self,
        plugin_id: &PluginRegistrationId
    ) -> Result<PluginRegistration, WorkerExecutorError> {
        let inner = self.inner.clone();
        self.cached_plugin_definitions
            .get_or_insert_simple(plugin_id, || {
                Box::pin(async move {
                    inner
                        .get_plugin_definition(
                            plugin_id
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
impl PluginsService for PluginsUnavailable {
    // async fn observe_plugin_installation(
    //     &self,
    //     _account_id: &AccountId,
    //     _component_id: &ComponentId,
    //     _component_version: ComponentRevision,
    //     _plugin_installation: &PluginInstallation,
    // ) -> Result<(), WorkerExecutorError> {
    //     Ok(())
    // }

    async fn get_plugin_installation(
        &self,
        _component_id: &ComponentId,
        _component_version: ComponentRevision,
        _plugin_priority: i32
    ) -> Result<InstalledPlugin, WorkerExecutorError> {
        Err(WorkerExecutorError::runtime("Not available"))
    }

    async fn get_plugin_definition(
        &self,
        _plugin_id: &PluginRegistrationId
    ) -> Result<PluginRegistration, WorkerExecutorError> {
        Err(WorkerExecutorError::runtime("Not available"))
    }
}

mod grpc {
    use golem_service_base::grpc::authorised_grpc_request;
    use crate::services::plugins::{PluginsService};
    use async_trait::async_trait;
    use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
    use golem_api_grpc::proto::golem::component::v1::plugin_service_client::PluginServiceClient;
    use golem_api_grpc::proto::golem::component::v1::{
        get_installed_plugins_response, GetInstalledPluginsRequest,
    };
    use golem_api_grpc::proto::golem::component::v1::{
        get_plugin_registration_by_id_response, GetPluginRegistrationByIdRequest,
    };
    use golem_common::client::{GrpcClient, GrpcClientConfig};
    use golem_common::model::RetryConfig;
    use golem_common::model::component::{ComponentId, ComponentRevision, InstalledPlugin, PluginPriority};
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use http::Uri;
    use std::time::Duration;
    use tonic::codec::CompressionEncoding;
    use tonic::transport::Channel;
    use uuid::Uuid;
    use golem_common::model::plugin_registration::PluginRegistrationDto;
    use golem_common::model::plugin_registration::PluginRegistrationId;
    use golem_service_base::model::plugin_registration::PluginRegistration;

    #[derive(Clone)]
    pub struct PluginsGrpc {
        plugins_client: GrpcClient<PluginServiceClient<Channel>>,
        components_client: GrpcClient<ComponentServiceClient<Channel>>,
        access_token: Uuid,
    }

    impl PluginsGrpc {
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
    impl PluginsService for PluginsGrpc {
        // async fn observe_plugin_installation(
        //     &self,
        //     _account_id: &AccountId,
        //     _component_id: &ComponentId,
        //     _component_version: ComponentRevision,
        //     _plugin_installation: &PluginInstallation,
        // ) -> Result<(), WorkerExecutorError> {
        //     Ok(())
        // }

        async fn get_plugin_installation(
            &self,
            component_id: &ComponentId,
            component_version: ComponentRevision,
            plugin_priority: PluginPriority
        ) -> Result<PluginRegistration, WorkerExecutorError> {
            let response = self
                .components_client
                .call("get_installed_plugins", move |client| {
                    let request = authorised_grpc_request(
                        GetInstalledPluginsRequest {
                            component_id: Some(component_id.clone().into()),
                            version: Some(component_version.0),
                        },
                        &self.access_token,
                    );
                    Box::pin(client.get_installed_plugins(request))
                })
                .await
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "Failed to get installed plugins: {err:?}"
                    ))
                })?
                .into_inner();
            let installations: Vec<InstalledPlugin> = match response.result {
                None => Err(WorkerExecutorError::runtime("Empty response"))?,
                Some(get_installed_plugins_response::Result::Success(response)) => response
                    .installations
                    .into_iter()
                    .map(|i| i.try_into())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(WorkerExecutorError::runtime)?,
                Some(get_installed_plugins_response::Result::Error(error)) => {
                    Err(WorkerExecutorError::runtime(format!("{error:?}")))?
                }
            };

            let mut result = None;
            for installation in installations {
                self.observe_plugin_installation(
                    component_id,
                    component_version,
                    &installation,
                )
                .await?;

                if installation.priority == *plugin_priority {
                    result = Some(installation);
                }
            }

            result.ok_or(WorkerExecutorError::runtime(
                "Plugin installation not found",
            ))
        }

        async fn get_plugin_definition(
            &self,
            plugin_id: &PluginRegistrationId
        ) -> Result<PluginRegistration, WorkerExecutorError> {
            let response = self
                .plugins_client
                .call("get_plugin_by_id", move |client| {
                    let request = authorised_grpc_request(
                        GetPluginRegistrationByIdRequest {
                            id: Some(plugin_id.clone().into()),
                        },
                        &self.access_token,
                    );
                    Box::pin(client.get_plugin_by_id(request))
                })
                .await
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "Failed to get plugin definition: {err:?}"
                    ))
                })?
                .into_inner();

            match response.result {
                None => Err(WorkerExecutorError::runtime("Empty response"))?,
                Some(get_plugin_registration_by_id_response::Result::Success(response)) => Ok(response
                    .plugin
                    .ok_or("Missing plugin field")
                    .map_err(WorkerExecutorError::runtime)?
                    .try_into()
                    .map_err(WorkerExecutorError::runtime)?),
                Some(get_plugin_registration_by_id_response::Result::Error(error)) => {
                    Err(WorkerExecutorError::runtime(format!("{error:?}")))?
                }
            }
        }
    }
}
