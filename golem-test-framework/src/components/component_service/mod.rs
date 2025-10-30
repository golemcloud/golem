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

pub mod filesystem;
pub mod provided;
pub mod spawned;

use super::cloud_service::CloudService;
use crate::components::rdb::Rdb;
use crate::components::{wait_for_startup_grpc, wait_for_startup_http, EnvVarBuilder};
use crate::config::GolemClientProtocol;
use crate::model::PluginDefinitionCreation;
use anyhow::{anyhow, Context as AnyhowContext};
use async_trait::async_trait;
use async_zip::base::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use futures::{stream, StreamExt, TryStreamExt};
pub use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient as ComponentServiceGrpcClient;
pub use golem_api_grpc::proto::golem::component::v1::plugin_service_client::PluginServiceClient as PluginServiceGrpcClient;
use golem_api_grpc::proto::golem::component::v1::{
    component_error, create_component_request, create_component_response, create_plugin_response,
    delete_plugin_response, download_component_response,
    get_component_metadata_all_versions_response, get_component_metadata_response,
    get_components_response, get_plugin_response, install_plugin_response,
    update_component_request, update_component_response, CreateComponentRequest,
    CreateComponentRequestChunk, CreateComponentRequestHeader, CreatePluginRequest,
    DeletePluginRequest, GetComponentRequest, GetComponentsRequest, GetLatestComponentRequest,
    GetPluginRequest, UpdateComponentRequest, UpdateComponentRequestChunk,
    UpdateComponentRequestHeader,
};
use golem_api_grpc::proto::golem::component::{
    Component, PluginInstallation, VersionedComponentId,
};
use golem_client::api::ComponentClient as ComponentServiceHttpClient;
use golem_client::api::ComponentClientLive as ComponentServiceHttpClientLive;
use golem_client::api::PluginClient as PluginServiceHttpClient;
use golem_client::api::PluginClientLive as PluginServiceHttpClientLive;
use golem_client::model::ComponentQuery;
use golem_client::{Context, Security};
use golem_common::model::agent::extraction::extract_agent_types;
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::plugin::PluginTypeSpecificDefinition;
use golem_common::model::{
    AccountId, ComponentFilePathWithPermissions, ComponentId, ComponentType, ComponentVersion,
    InitialComponentFile, PluginId, PluginInstallationId, ProjectId,
};
use golem_service_base::clients::authorised_request;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::ops::DerefMut;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;
use tokio::fs;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::time::sleep;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tracing::{debug, info, Level};
use url::Url;
use uuid::Uuid;

#[async_trait]
pub trait ComponentService: Send + Sync {
    fn component_directory(&self) -> &Path;

    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService>;

    fn client_protocol(&self) -> GolemClientProtocol;
    async fn base_http_client(&self) -> reqwest::Client;

    async fn component_http_client(&self, token: &Uuid) -> ComponentServiceHttpClientLive {
        let url = format!("http://{}:{}", self.public_host(), self.public_http_port());
        ComponentServiceHttpClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: Url::parse(&url).expect("Failed to parse url"),
                security_token: Security::Bearer(token.to_string()),
            },
        }
    }
    async fn component_grpc_client(&self) -> ComponentServiceGrpcClient<Channel>;

    async fn plugin_http_client(&self, token: &Uuid) -> PluginServiceHttpClientLive {
        let url = format!("http://{}:{}", self.public_host(), self.public_http_port());
        PluginServiceHttpClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: Url::parse(&url).expect("Failed to parse url"),
                security_token: Security::Bearer(token.to_string()),
            },
        }
    }
    async fn plugin_grpc_client(&self) -> PluginServiceGrpcClient<Channel>;

    async fn get_plugin_id(
        &self,
        token: &Uuid,
        owner: AccountId,
        name: &str,
        version: &str,
    ) -> crate::Result<Option<PluginId>> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.plugin_grpc_client().await;

                let request = authorised_request(
                    GetPluginRequest {
                        account_id: Some(owner.into()),
                        name: name.to_string(),
                        version: version.to_string(),
                    },
                    token,
                );

                let response = client.get_plugin(request).await?;
                let converted = response.into_inner().result.and_then(|r| match r {
                    get_plugin_response::Result::Success(result) => result
                        .plugin
                        .map(|p| PluginId::try_from(p.id.unwrap()).map_err(|e| anyhow!(e))),
                    get_plugin_response::Result::Error(error) => Some(Err(anyhow!("{error:?}"))),
                });
                match converted {
                    Some(Ok(inner)) => Ok(Some(inner)),
                    Some(Err(e)) => Err(e),
                    None => Ok(None),
                }
            }
            GolemClientProtocol::Http => {
                let client = self.plugin_http_client(token).await;

                let result = client.get_plugin(&owner.value, name, version).await;

                match result {
                    Ok(def) => Ok(Some(PluginId(def.id))),
                    Err(golem_client::Error::Item(golem_client::api::PluginError::Error404(_))) => {
                        Ok(None)
                    }
                    Err(other) => Err(other)?,
                }
            }
        }
    }

    async fn to_grpc_component(
        &self,
        token: &Uuid,
        component: golem_client::model::Component,
    ) -> crate::Result<Component> {
        let account_id = AccountId {
            value: component.account_id,
        };
        let component = Component {
            versioned_component_id: Some(VersionedComponentId {
                component_id: Some(
                    ComponentId(component.versioned_component_id.component_id).into(),
                ),
                version: component.versioned_component_id.version,
            }),
            component_name: component.component_name,
            component_size: component.component_size,
            metadata: Some(component.metadata.into()),
            project_id: Some(ProjectId(component.project_id).into()),
            account_id: Some(account_id.clone().into()),
            created_at: Some(SystemTime::from(component.created_at).into()),
            component_type: Some(component.component_type as i32),
            files: component
                .files
                .into_iter()
                .map(|file| file.into())
                .collect(),
            installed_plugins: stream::iter(component.installed_plugins)
                .then(async |install| {
                    let plugin_id = self
                        .get_plugin_id(
                            token,
                            account_id.clone(),
                            &install.plugin_name,
                            &install.plugin_version,
                        )
                        .await?
                        .ok_or(anyhow!("Failed to get plugin id during conversion"))?;

                    Ok::<PluginInstallation, anyhow::Error>(PluginInstallation {
                        id: Some(PluginInstallationId(install.id).into()),
                        plugin_id: Some(plugin_id.into()),
                        priority: install.priority,
                        parameters: install.parameters,
                    })
                })
                .try_collect::<Vec<_>>()
                .await?,
            env: component.env,
        };
        Ok(component)
    }

    async fn get_components(
        &self,
        token: &Uuid,
        request: GetComponentsRequest,
    ) -> crate::Result<Vec<Component>> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.component_grpc_client().await;

                let request = authorised_request(request, token);

                match client
                    .get_components(request)
                    .await?
                    .into_inner()
                    .result
                    .ok_or_else(|| anyhow!("get_components: no result"))?
                {
                    get_components_response::Result::Success(result) => Ok(result.components),
                    get_components_response::Result::Error(error) => Err(anyhow!("{error:?}")),
                }
            }
            GolemClientProtocol::Http => {
                let client = self.component_http_client(token).await;

                match client
                    .get_components(
                        request
                            .project_id
                            .map(|pid| pid.value.unwrap().into())
                            .as_ref(),
                        request.component_name.as_deref(),
                    )
                    .await
                {
                    Ok(components) => {
                        stream::iter(components)
                            .then(|c| self.to_grpc_component(token, c))
                            .try_collect()
                            .await
                    }
                    Err(error) => Err(anyhow!("{error:?}")),
                }
            }
        }
    }

    async fn get_component_metadata_all_versions(
        &self,
        token: &Uuid,
        request: GetComponentRequest,
    ) -> crate::Result<Vec<Component>> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.component_grpc_client().await;

                let request = authorised_request(request, token);

                match client
                    .get_component_metadata_all_versions(request)
                    .await?
                    .into_inner()
                    .result
                    .ok_or_else(|| anyhow!("get_component_metadata_all_versions: no result"))?
                {
                    get_component_metadata_all_versions_response::Result::Success(result) => {
                        Ok(result.components)
                    }
                    get_component_metadata_all_versions_response::Result::Error(error) => {
                        Err(anyhow!("{error:?}"))
                    }
                }
            }
            GolemClientProtocol::Http => {
                let client = self.component_http_client(token).await;

                match client
                    .get_component_metadata_all_versions(
                        &request.component_id.unwrap().value.unwrap().into(),
                    )
                    .await
                {
                    Ok(result) => {
                        stream::iter(result)
                            .then(|c| self.to_grpc_component(token, c))
                            .try_collect()
                            .await
                    }
                    Err(error) => Err(anyhow!("{error:?}")),
                }
            }
        }
    }

    async fn get_latest_component_metadata(
        &self,
        token: &Uuid,
        request: GetLatestComponentRequest,
    ) -> crate::Result<Component> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.component_grpc_client().await;

                let request = authorised_request(request, token);

                match client
                    .get_latest_component_metadata(request)
                    .await?
                    .into_inner()
                    .result
                    .ok_or_else(|| anyhow!("get_latest_component_metadata: no result"))?
                {
                    get_component_metadata_response::Result::Success(result) => result
                        .component
                        .ok_or_else(|| anyhow!("get_latest_component_metadata: missing component")),
                    get_component_metadata_response::Result::Error(error) => {
                        Err(anyhow!("{error:?}"))
                    }
                }
            }
            GolemClientProtocol::Http => {
                let client = self.component_http_client(token).await;

                match client
                    .get_latest_component_metadata(
                        &request.component_id.unwrap().value.unwrap().into(),
                    )
                    .await
                {
                    Ok(result) => self.to_grpc_component(token, result).await,
                    Err(error) => Err(anyhow!("{error:?}")),
                }
            }
        }
    }

    async fn get_or_add_component(
        &self,
        token: &Uuid,
        local_path: &Path,
        name: &str,
        component_type: ComponentType,
        files: &[(PathBuf, InitialComponentFile)],
        dynamic_linking: &HashMap<String, DynamicLinkedInstance>,
        unverified: bool,
        env: &HashMap<String, String>,
        project_id: Option<ProjectId>,
    ) -> Component {
        let mut retries = 10;
        loop {
            let latest_component: Option<Component> = match self.client_protocol() {
                GolemClientProtocol::Grpc => {
                    let mut client = self.component_grpc_client().await;

                    let request = authorised_request(
                        GetComponentsRequest {
                            project_id: project_id.clone().map(|pid| pid.into()),
                            component_name: Some(name.to_string()),
                        },
                        token,
                    );

                    match client
                        .get_components(request)
                        .await
                        .expect("Failed to call get-components")
                        .into_inner()
                        .result
                    {
                        None => {
                            panic!(
                                "Missing response from golem-component-service for get-components"
                            )
                        }
                        Some(get_components_response::Result::Success(result)) => {
                            debug!("Response from get_components (GRPC) was {result:?}");
                            result
                                .components
                                .into_iter()
                                .max_by_key(|t| t.versioned_component_id.as_ref().unwrap().version)
                        }
                        Some(get_components_response::Result::Error(error)) => {
                            panic!(
                                "Failed to get components from golem-component-service (GRPC): {error:?}"
                            );
                        }
                    }
                }
                GolemClientProtocol::Http => {
                    let client = self.component_http_client(token).await;

                    match client.get_components(None, Some(name)).await {
                        Ok(result) => {
                            debug!("Response from get_components (HTTP) was {result:?}");
                            let max = result
                                .into_iter()
                                .max_by_key(|component| component.versioned_component_id.version);
                            if let Some(max) = max {
                                Some(self.to_grpc_component(token, max).await.unwrap())
                            } else {
                                None
                            }
                        }
                        Err(error) => {
                            panic!(
                                "Failed to get components from golem-component-service (HTTP): {error:?}"
                            );
                        }
                    }
                }
            };

            if let Some(latest_component) = latest_component {
                return latest_component;
            }

            match self
                .add_component(
                    token,
                    local_path,
                    name,
                    component_type,
                    files,
                    dynamic_linking,
                    unverified,
                    env,
                    project_id.clone(),
                )
                .await
            {
                Ok(component_id) => break component_id,
                Err(AddComponentError::AlreadyExists) => {
                    if retries > 0 {
                        info!("Component with name {name} got created in parallel, retrying get_or_add_component");
                        retries -= 1;
                        sleep(Duration::from_secs(5)).await;
                        continue;
                    } else {
                        panic!(
                            "Component with name {name} already exists in golem-component-service"
                        );
                    }
                }
                Err(AddComponentError::Other(message)) => {
                    panic!("Failed to add component with name {name}: {message}");
                }
            }
        }
    }

    async fn add_component_with_id(
        &self,
        _local_path: &Path,
        _component_id: &ComponentId,
        _component_name: &str,
        _component_type: ComponentType,
        _project_id: Option<ProjectId>,
    ) -> Result<(), AddComponentError> {
        panic!(
            "Adding a component with a specific Component ID is only supported in filesystem mode"
        )
    }

    async fn add_component(
        &self,
        token: &Uuid,
        local_path: &Path,
        name: &str,
        component_type: ComponentType,
        files: &[(PathBuf, InitialComponentFile)],
        dynamic_linking: &HashMap<String, DynamicLinkedInstance>,
        _unverified: bool,
        env: &HashMap<String, String>,
        project_id: Option<ProjectId>,
    ) -> Result<Component, AddComponentError> {
        let agent_types = extract_agent_types(local_path, false, true)
            .await
            .map_err(|err| {
                AddComponentError::Other(format!("Failed analyzing component: {err}"))
            })?;

        let mut file = File::open(local_path).await.map_err(|_| {
            AddComponentError::Other(format!("Failed to read component from {local_path:?}"))
        })?;

        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.component_grpc_client().await;

                let component_type: golem_api_grpc::proto::golem::component::ComponentType =
                    component_type.into();

                let files = files.iter().map(|(_, f)| f.clone().into()).collect();

                let mut chunks: Vec<CreateComponentRequest> = vec![CreateComponentRequest {
                    data: Some(create_component_request::Data::Header(
                        CreateComponentRequestHeader {
                            project_id: project_id.map(|pid| pid.into()),
                            component_name: name.to_string(),
                            component_type: Some(component_type as i32),
                            files,
                            dynamic_linking: HashMap::from_iter(
                                dynamic_linking
                                    .iter()
                                    .map(|(k, v)| (k.clone(), v.clone().into())),
                            ),
                            env: env.clone(),
                            agent_types: agent_types.into_iter().map(|a| a.into()).collect(),
                        },
                    )),
                }];

                loop {
                    let mut buffer = Box::new([0; 4096]);

                    let n = file.read(buffer.deref_mut()).await.map_err(|_| {
                        AddComponentError::Other(format!(
                            "Failed to read component from {local_path:?}"
                        ))
                    })?;

                    if n == 0 {
                        break;
                    } else {
                        chunks.push(CreateComponentRequest {
                            data: Some(create_component_request::Data::Chunk(
                                CreateComponentRequestChunk {
                                    component_chunk: buffer[0..n].to_vec(),
                                },
                            )),
                        });
                    }
                }
                let request = authorised_request(tokio_stream::iter(chunks), token);

                let response = client
                    .create_component(request)
                    .await
                    .map_err(|status| {
                        AddComponentError::Other(format!(
                            "Failed to call create_component: {status:?}"
                        ))
                    })?
                    .into_inner();
                match response.result {
                    None => Err(AddComponentError::Other(
                        "Missing response from golem-component-service for create-component"
                            .to_string(),
                    )),
                    Some(create_component_response::Result::Success(component)) => Ok(component),
                    Some(create_component_response::Result::Error(error)) => match error.error {
                        Some(component_error::Error::AlreadyExists(_)) => {
                            Err(AddComponentError::AlreadyExists)
                        }
                        _ => Err(AddComponentError::Other(format!(
                            "Failed to create component in golem-component-service: {error:?}"
                        ))),
                    },
                }
            }
            GolemClientProtocol::Http => {
                let client = self.component_http_client(token).await;

                let archive = build_ifs_archive(self.component_directory(), Some(files)).await.map_err(|error| {
                    AddComponentError::Other(format!(
                        "Failed to build IFS archive golem-component-service add component: {error:?}"
                    ))
                })?;

                let archive_file = match &archive {
                    Some((_, path)) =>
                        Some(File::open(path).await.map_err(|error| {
                            AddComponentError::Other(format!(
                                "Failed to open IFS archive golem-component-service add component: {error:?}"
                            ))
                        })?),
                    None => None,
                };

                match client
                    .create_component(
                        &ComponentQuery {
                            project_id: project_id.map(|pid| pid.0),
                            component_name: name.to_string(),
                        },
                        file,
                        Some(&component_type),
                        to_http_file_permissions(files).as_ref(),
                        archive_file,
                        to_http_dynamic_linking(Some(dynamic_linking)).as_ref(),
                        Some(&golem_client::model::ComponentEnv {
                            key_values: env.clone(),
                        }),
                        Some(&golem_client::model::AgentTypes { types: agent_types }),
                    )
                    .await
                {
                    Ok(component) => {
                        debug!("Created component (HTTP) {:?}", component);
                        self.to_grpc_component(token, component)
                            .await
                            .map_err(|e| AddComponentError::Other(format!("{e:?}")))
                    }
                    Err(error) => {
                        if let golem_client::Error::Item(
                            golem_client::api::ComponentError::Error409(_),
                        ) = &error
                        {
                            Err(AddComponentError::AlreadyExists)
                        } else {
                            Err(AddComponentError::Other(format!("{error:?}")))
                        }
                    }
                }
            }
        }
    }

    async fn update_component(
        &self,
        token: &Uuid,
        component_id: &ComponentId,
        local_path: &Path,
        component_type: ComponentType,
        files: Option<&[(PathBuf, InitialComponentFile)]>,
        dynamic_linking: Option<&HashMap<String, DynamicLinkedInstance>>,
        env: &HashMap<String, String>,
    ) -> crate::Result<u64> {
        let mut file = File::open(local_path)
            .await
            .unwrap_or_else(|_| panic!("Failed to read component from {local_path:?}"));

        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.component_grpc_client().await;

                let component_type: golem_api_grpc::proto::golem::component::ComponentType =
                    component_type.into();

                let update_files = files.is_some();

                let files: Vec<golem_api_grpc::proto::golem::component::InitialComponentFile> =
                    files
                        .into_iter()
                        .flatten()
                        .map(|(_, f)| f.clone().into())
                        .collect::<Vec<_>>();

                let mut chunks: Vec<UpdateComponentRequest> = vec![UpdateComponentRequest {
                    data: Some(update_component_request::Data::Header(
                        UpdateComponentRequestHeader {
                            component_id: Some(component_id.clone().into()),
                            component_type: Some(component_type as i32),
                            update_files,
                            files,
                            dynamic_linking: HashMap::from_iter(
                                dynamic_linking
                                    .into_iter()
                                    .flatten()
                                    .map(|(k, v)| (k.clone(), v.clone().into())),
                            ),
                            env: env.clone(),
                            agent_types: vec![],
                        },
                    )),
                }];

                loop {
                    let mut buffer = Box::new([0; 4096]);

                    let n = file
                        .read(buffer.deref_mut())
                        .await
                        .unwrap_or_else(|_| panic!("Failed to read template from {local_path:?}"));

                    if n == 0 {
                        break;
                    } else {
                        chunks.push(UpdateComponentRequest {
                            data: Some(update_component_request::Data::Chunk(
                                UpdateComponentRequestChunk {
                                    component_chunk: buffer[0..n].to_vec(),
                                },
                            )),
                        });
                    }
                }
                let request = authorised_request(tokio_stream::iter(chunks), token);

                let response = client
                    .update_component(request)
                    .await
                    .expect("Failed to update component")
                    .into_inner();

                match response.result {
                    None => {
                        panic!("Missing response from golem-component-service for create-component")
                    }
                    Some(update_component_response::Result::Success(component)) => {
                        info!("Updated component (GRPC) {component:?}");
                        Ok(component.versioned_component_id.unwrap().version)
                    }
                    Some(update_component_response::Result::Error(error)) => Err(anyhow!(
                        "Failed to update component in golem-component-service (GRPC): {error:?}"
                    )),
                }
            }
            GolemClientProtocol::Http => {
                let client = self.component_http_client(token).await;

                let archive = match build_ifs_archive(self.component_directory(), files).await {
                    Ok(archive) => archive,
                    Err(error) => panic!(
                        "Failed to build IFS archive in golem-component-service update component: {error:?}"
                    )
                };

                let archive_file = match &archive {
                    Some((_, path)) =>
                        match File::open(path).await {
                            Ok(file) => Some(file),
                            Err(error) => panic!(
                                "Failed to open IFS archive in golem-component-service update component: {error:?}"
                            )
                        }
                    None => None,
                };

                let component_env = golem_client::model::ComponentEnv {
                    key_values: env.clone(),
                };

                match client
                    .update_component(
                        &component_id.0,
                        Some(&component_type),
                        file,
                        files
                            .as_ref()
                            .and_then(|files| to_http_file_permissions(files))
                            .as_ref(),
                        archive_file,
                        to_http_dynamic_linking(dynamic_linking).as_ref(),
                        Some(&component_env),
                        None,
                    )
                    .await
                {
                    Ok(component) => {
                        debug!("Updated component (HTTP) {:?}", component);
                        Ok(component.versioned_component_id.version)
                    }
                    Err(error) => Err(anyhow!(
                        "Failed to update component in golem-component-service (HTTP): {error:?}"
                    )),
                }
            }
        }
    }

    async fn get_latest_version(&self, token: &Uuid, component_id: &ComponentId) -> u64 {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.component_grpc_client().await;

                let request = authorised_request(
                    GetLatestComponentRequest {
                        component_id: Some(component_id.clone().into()),
                    },
                    token,
                );

                let response = client
                    .get_latest_component_metadata(request)
                    .await
                    .expect("Failed to get latest component metadata (GRPC)")
                    .into_inner();
                match response.result {
                    None => {
                        panic!("Missing response from golem-component-service for create-component")
                    }
                    Some(get_component_metadata_response::Result::Success(component)) => {
                        component
                            .component
                            .expect("No component in response")
                            .versioned_component_id
                            .expect("No versioned_component_id field")
                            .version
                    }
                    Some(get_component_metadata_response::Result::Error(error)) => {
                        panic!("Failed to get component metadata from golem-component-service: {error:?}");
                    }
                }
            }
            GolemClientProtocol::Http => {
                let client = self.component_http_client(token).await;

                client
                    .get_latest_component_metadata(&component_id.0)
                    .await
                    .expect("Failed to get latest component metadata (HTTP)")
                    .versioned_component_id
                    .version
            }
        }
    }

    ///
    /// **Arguments**:
    ///
    /// * `account_id`:  Only used for the http client. AccountId of the account the plugin wasm file was uploaded to.
    async fn create_plugin(
        &self,
        token: &Uuid,
        account_id: &AccountId,
        definition: PluginDefinitionCreation,
    ) -> crate::Result<()> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.plugin_grpc_client().await;

                let request = authorised_request(
                    CreatePluginRequest {
                        plugin: Some(definition.into()),
                    },
                    token,
                );

                let response = client.create_plugin(request).await?.into_inner();
                match response.result {
                    None => Err(anyhow!(
                        "Missing response from golem-component-service for create-plugin"
                    )),
                    Some(create_plugin_response::Result::Success(_)) => Ok(()),
                    Some(create_plugin_response::Result::Error(error)) => Err(anyhow!(
                        "Failed to create plugin in golem-component-service: {error:?}"
                    )),
                }
            }
            GolemClientProtocol::Http => {
                let client = self.plugin_http_client(token).await;

                let result = match definition.specs {
                    PluginTypeSpecificDefinition::ComponentTransformer(def) => {
                        let specs =
                            golem_client::model::PluginTypeSpecificCreation::ComponentTransformer(
                                golem_client::model::ComponentTransformerDefinition {
                                    provided_wit_package: def.provided_wit_package,
                                    json_schema: def.json_schema,
                                    validate_url: def.validate_url,
                                    transform_url: def.transform_url,
                                },
                            );

                        client
                            .create_plugin(&golem_client::model::PluginDefinitionCreation {
                                name: definition.name,
                                version: definition.version,
                                description: definition.description,
                                icon: definition.icon,
                                homepage: definition.homepage,
                                scope: definition.scope,
                                specs,
                            })
                            .await
                    }
                    PluginTypeSpecificDefinition::OplogProcessor(def) => {
                        let specs = golem_client::model::PluginTypeSpecificCreation::OplogProcessor(
                            golem_client::model::OplogProcessorDefinition {
                                component_id: def.component_id.0,
                                component_version: def.component_version,
                            },
                        );

                        client
                            .create_plugin(&golem_client::model::PluginDefinitionCreation {
                                name: definition.name,
                                version: definition.version,
                                description: definition.description,
                                icon: definition.icon,
                                homepage: definition.homepage,
                                scope: definition.scope,
                                specs,
                            })
                            .await
                    }
                    golem_common::model::plugin::PluginTypeSpecificDefinition::Library(def) => {
                        // TODO: This round trip trough the blob storage is redundant, but ensure the same api works both grpc and http. Improve this
                        let data = self
                            .plugin_wasm_files_service()
                            .get(account_id, &def.blob_storage_key)
                            .await
                            .map_err(|e| anyhow!(e))?
                            .ok_or(anyhow!("plugin wasm file not found in blob storage"))?;

                        client
                            .create_library_plugin(
                                &definition.name,
                                &definition.version,
                                &definition.description,
                                definition.icon,
                                &definition.homepage,
                                &definition.scope,
                                data,
                            )
                            .await
                    }
                    golem_common::model::plugin::PluginTypeSpecificDefinition::App(def) => {
                        // TODO: This round trip trough the blob storage is redundant, but ensure the same api works both grpc and http. Improve this
                        let data = self
                            .plugin_wasm_files_service()
                            .get(account_id, &def.blob_storage_key)
                            .await
                            .map_err(|e| anyhow!(e))?
                            .expect("plugin wasm file not found in blob storage");

                        client
                            .create_app_plugin(
                                &definition.name,
                                &definition.version,
                                &definition.description,
                                definition.icon,
                                &definition.homepage,
                                &definition.scope,
                                data,
                            )
                            .await
                    }
                };

                match result {
                    Ok(_) => Ok(()),
                    Err(error) => Err(anyhow!(
                        "Failed to create plugin in golem-component-service: {error:?}"
                    )),
                }
            }
        }
    }

    async fn delete_plugin(
        &self,
        token: &Uuid,
        owner: AccountId,
        name: &str,
        version: &str,
    ) -> crate::Result<()> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.plugin_grpc_client().await;

                let request = authorised_request(
                    DeletePluginRequest {
                        account_id: Some(owner.into()),
                        name: name.to_string(),
                        version: version.to_string(),
                    },
                    token,
                );

                let response = client.delete_plugin(request).await?.into_inner();

                match response.result {
                    None => Err(anyhow!(
                        "Missing response from golem-component-service for create-plugin"
                    )),
                    Some(delete_plugin_response::Result::Success(_)) => Ok(()),
                    Some(delete_plugin_response::Result::Error(error)) => Err(anyhow!(
                        "Failed to delete plugin in golem-component-service: {error:?}"
                    )),
                }
            }
            GolemClientProtocol::Http => {
                let client = self.plugin_http_client(token).await;

                let result = client.delete_plugin(&owner.value, name, version).await;

                match result {
                    Ok(_) => Ok(()),
                    Err(error) => Err(anyhow!(
                        "Failed to create plugin in golem-component-service: {error:?}"
                    )),
                }
            }
        }
    }

    async fn install_plugin_to_component(
        &self,
        token: &Uuid,
        component_id: &ComponentId,
        plugin_name: &str,
        plugin_version: &str,
        priority: i32,
        parameters: HashMap<String, String>,
    ) -> crate::Result<PluginInstallationId> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.component_grpc_client().await;

                let request = authorised_request(
                    golem_api_grpc::proto::golem::component::v1::InstallPluginRequest {
                        component_id: Some(component_id.clone().into()),
                        name: plugin_name.to_string(),
                        version: plugin_version.to_string(),
                        priority,
                        parameters,
                    },
                    token,
                );

                let response = client.install_plugin(request).await?.into_inner();

                match response.result {
                    None => Err(anyhow!(
                        "Missing response from golem-component-service for install-plugin"
                    )),
                    Some(install_plugin_response::Result::Success(result)) => Ok(result
                        .installation
                        .ok_or(anyhow!("Missing plugin_installation field"))?
                        .id
                        .ok_or(anyhow!("Missing plugin_installation_id field"))?
                        .try_into()
                        .map_err(|error| {
                            anyhow!("plugin_installation_id has unexpected format: {error}")
                        })?),
                    Some(install_plugin_response::Result::Error(error)) => Err(anyhow!(
                        "Failed to install plugin in golem-component-service (GRPC): {error:?}"
                    )),
                }
            }

            GolemClientProtocol::Http => {
                let client = self.component_http_client(token).await;

                let result = client
                    .install_plugin(
                        &component_id.0,
                        &golem_client::model::PluginInstallationCreation {
                            name: plugin_name.to_string(),
                            version: plugin_version.to_string(),
                            priority,
                            parameters,
                        },
                    )
                    .await;

                match result {
                    Ok(result) => Ok(PluginInstallationId(result.id)),
                    Err(error) => Err(anyhow!(
                        "Failed to install plugin in golem-component-service (HTTP): {error:?}"
                    )),
                }
            }
        }
    }

    async fn get_component_size(
        &self,
        token: &Uuid,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> crate::Result<u64> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.component_grpc_client().await;

                let request = authorised_request(
                    golem_api_grpc::proto::golem::component::v1::DownloadComponentRequest {
                        component_id: Some(component_id.clone().into()),
                        version: Some(component_version),
                    },
                    token,
                );

                let response = client.download_component(request).await?.into_inner();

                let chunks = response.into_stream().try_collect::<Vec<_>>().await?;
                let bytes = chunks
                    .into_iter()
                    .map(|chunk| match chunk.result {
                        None => Err(anyhow!("Empty response")),
                        Some(download_component_response::Result::SuccessChunk(chunk)) => Ok(chunk),
                        Some(download_component_response::Result::Error(error)) => {
                            Err(anyhow!("Failed to download component: {error:?}"))
                        }
                    })
                    .collect::<crate::Result<Vec<Vec<u8>>>>()?;

                let bytes: Vec<u8> = bytes.into_iter().flatten().collect();
                Ok(bytes.len() as u64)
            }
            GolemClientProtocol::Http => {
                let client = self.component_http_client(token).await;

                match client
                    .download_component(&component_id.0, Some(component_version))
                    .await
                {
                    Ok(bytes) => Ok(bytes.len() as u64),
                    Err(error) => Err(anyhow!("{error:?}")),
                }
            }
        }
    }

    fn private_host(&self) -> String;
    fn private_http_port(&self) -> u16;
    fn private_grpc_port(&self) -> u16;

    fn public_host(&self) -> String {
        self.private_host()
    }

    fn public_http_port(&self) -> u16 {
        self.private_http_port()
    }

    fn public_grpc_port(&self) -> u16 {
        self.private_grpc_port()
    }

    async fn kill(&self);
}

async fn new_component_grpc_client(
    host: &str,
    grpc_port: u16,
) -> ComponentServiceGrpcClient<Channel> {
    ComponentServiceGrpcClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to golem-component-service")
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip)
}

async fn new_plugin_grpc_client(host: &str, grpc_port: u16) -> PluginServiceGrpcClient<Channel> {
    PluginServiceGrpcClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to golem-component-service (plugins)")
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip)
}

async fn wait_for_startup(
    protocol: GolemClientProtocol,
    host: &str,
    grpc_port: u16,
    http_port: u16,
    timeout: Duration,
) {
    match protocol {
        GolemClientProtocol::Grpc => {
            wait_for_startup_grpc(host, grpc_port, "golem-component-service", timeout).await
        }
        GolemClientProtocol::Http => {
            wait_for_startup_http(host, http_port, "golem-component-service", timeout).await
        }
    }
}

async fn env_vars(
    http_port: u16,
    grpc_port: u16,
    component_compilation_service: Option<(&str, u16)>,
    rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    verbosity: Level,
    private_rdb_connection: bool,
    cloud_service: &Arc<dyn CloudService>,
) -> HashMap<String, String> {
    let mut builder = EnvVarBuilder::golem_service(verbosity)
        .with_str("GOLEM__COMPONENT_STORE__TYPE", "Local")
        .with_str("GOLEM__COMPONENT_STORE__CONFIG__OBJECT_PREFIX", "")
        .with_str(
            "GOLEM__COMPONENT_STORE__CONFIG__ROOT_PATH",
            "/tmp/ittest-local-object-store/golem",
        )
        .with_str("GOLEM__BLOB_STORAGE__TYPE", "LocalFileSystem")
        .with_str(
            "GOLEM__BLOB_STORAGE__CONFIG__ROOT",
            "/tmp/ittest-local-object-store/golem",
        )
        .with("GOLEM__CLOUD_SERVICE__HOST", cloud_service.private_host())
        .with(
            "GOLEM__CLOUD_SERVICE__PORT",
            cloud_service.private_grpc_port().to_string(),
        )
        .with(
            "GOLEM__CLOUD_SERVICE__ACCESS_TOKEN",
            cloud_service.admin_token().to_string(),
        )
        .with("GOLEM__GRPC_PORT", grpc_port.to_string())
        .with("GOLEM__HTTP_PORT", http_port.to_string())
        .with_all(rdb.info().env("golem_component", private_rdb_connection));

    match component_compilation_service {
        Some((host, port)) => {
            builder = builder
                .with_str("GOLEM__COMPILATION__TYPE", "Enabled")
                .with("GOLEM__COMPILATION__CONFIG__HOST", host.to_string())
                .with("GOLEM__COMPILATION__CONFIG__PORT", port.to_string());
        }
        _ => builder = builder.with_str("GOLEM__COMPILATION__TYPE", "Disabled"),
    };

    builder.build()
}

#[derive(Debug)]
pub enum AddComponentError {
    AlreadyExists,
    Other(String),
}

impl Error for AddComponentError {}

impl Display for AddComponentError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AddComponentError::AlreadyExists => write!(f, "Component already exists"),
            AddComponentError::Other(message) => write!(f, "{message}"),
        }
    }
}

fn to_http_file_permissions(
    files: &[(PathBuf, InitialComponentFile)],
) -> Option<golem_common::model::ComponentFilePathWithPermissionsList> {
    if files.is_empty() {
        None
    } else {
        Some(golem_client::model::ComponentFilePathWithPermissionsList {
            values: files
                .iter()
                .map(|(_source, file)| ComponentFilePathWithPermissions {
                    path: file.path.clone(),
                    permissions: file.permissions,
                })
                .collect(),
        })
    }
}

fn to_http_dynamic_linking(
    dynamic_linking: Option<&HashMap<String, DynamicLinkedInstance>>,
) -> Option<golem_client::model::DynamicLinking> {
    let dynamic_linking = dynamic_linking?;
    if dynamic_linking.is_empty() {
        return None;
    }

    Some(golem_client::model::DynamicLinking {
        dynamic_linking: dynamic_linking
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    match v {
                        DynamicLinkedInstance::WasmRpc(link) => {
                            golem_client::model::DynamicLinkedInstance::WasmRpc(
                                golem_client::model::DynamicLinkedWasmRpc {
                                    targets: link.targets.clone(),
                                },
                            )
                        }
                    },
                )
            })
            .collect(),
    })
}

async fn build_ifs_archive(
    component_directory: &Path,
    files: Option<&[(PathBuf, InitialComponentFile)]>,
) -> crate::Result<Option<(TempDir, PathBuf)>> {
    static ARCHIVE_NAME: &str = "ifs.zip";

    let Some(files) = files else { return Ok(None) };
    if files.is_empty() {
        return Ok(None);
    }

    let temp_dir = tempfile::Builder::new()
        .prefix("golem-test-framework-ifs-zip")
        .tempdir()?;
    let temp_file = File::create(temp_dir.path().join(ARCHIVE_NAME)).await?;
    let mut zip_writer = ZipFileWriter::with_tokio(temp_file);

    for (source_file, ifs_file) in files {
        zip_writer
            .write_entry_whole(
                ZipEntryBuilder::new(ifs_file.path.to_string().into(), Compression::Deflate),
                &(fs::read(&component_directory.join(source_file))
                    .await
                    .with_context(|| format!("source file path: {}", source_file.display()))?),
            )
            .await?;
    }

    zip_writer.close().await?;
    let file_path = temp_dir.path().join(ARCHIVE_NAME);
    Ok(Some((temp_dir, file_path)))
}
