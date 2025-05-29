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

use crate::cloud::CloudGolemTypes;
use crate::error::GolemError;
use crate::grpc::authorised_grpc_request;
use crate::services::golem_config::PluginServiceConfig;
use crate::services::plugins::{CachedPlugins, Plugins, PluginsObservations, PluginsUnavailable};
use applying::Apply;
use async_trait::async_trait;
use cloud_api_grpc::proto::golem::cloud::component::v1::plugin_service_client::PluginServiceClient;
use cloud_api_grpc::proto::golem::cloud::component::v1::{
    get_plugin_by_id_response, GetPluginByIdRequest,
};
use cloud_common::model::{CloudPluginOwner, CloudPluginScope};
use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::v1::{
    get_installed_plugins_response, GetInstalledPluginsRequest,
};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::plugin::{PluginDefinition, PluginInstallation};
use golem_common::model::RetryConfig;
use golem_common::model::{AccountId, ComponentId, ComponentVersion, PluginInstallationId};
use http::Uri;
use std::sync::Arc;
use std::time::Duration;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use uuid::Uuid;

pub fn cloud_configured(config: &PluginServiceConfig) -> Arc<dyn Plugins<CloudGolemTypes>> {
    match config {
        PluginServiceConfig::Grpc(config) => {
            let client = CachedPlugins::new(
                CloudGrpcPlugins::new(
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

#[derive(Clone)]
struct CloudGrpcPlugins {
    plugins_client: GrpcClient<PluginServiceClient<Channel>>,
    components_client: GrpcClient<ComponentServiceClient<Channel>>,
    access_token: Uuid,
}

impl CloudGrpcPlugins {
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
impl PluginsObservations for CloudGrpcPlugins {
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
impl Plugins<CloudGolemTypes> for CloudGrpcPlugins {
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
    ) -> Result<PluginDefinition<CloudPluginOwner, CloudPluginScope>, GolemError> {
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
                .apply(convert_grpc_plugin_definition)
                .map_err(GolemError::runtime)?),
            Some(get_plugin_by_id_response::Result::Error(error)) => {
                Err(GolemError::runtime(format!("{error:?}")))?
            }
        }
    }
}

fn convert_grpc_plugin_definition(
    value: cloud_api_grpc::proto::golem::cloud::component::PluginDefinition,
) -> Result<PluginDefinition<CloudPluginOwner, CloudPluginScope>, String> {
    let account_id: AccountId = value.account_id.ok_or("Missing account id")?.into();

    Ok(PluginDefinition {
        id: value.id.ok_or("Missing plugin id")?.try_into()?,
        name: value.name,
        version: value.version,
        description: value.description,
        icon: value.icon,
        homepage: value.homepage,
        specs: value.specs.ok_or("Missing plugin specs")?.try_into()?,
        scope: value.scope.ok_or("Missing plugin scope")?.try_into()?,
        owner: CloudPluginOwner { account_id },
        deleted: value.deleted,
    })
}
