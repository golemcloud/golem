// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::components::rdb::Rdb;
use crate::components::{
    new_reqwest_client, wait_for_startup_grpc, wait_for_startup_http, EnvVarBuilder, GolemEnvVars,
};
use crate::config::GolemClientProtocol;
use anyhow::anyhow;
use async_trait::async_trait;
use async_zip::base::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use futures_util::TryStreamExt;
use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient as ComponentServiceGrpcClient;
use golem_api_grpc::proto::golem::component::v1::plugin_service_client::PluginServiceClient as PluginServiceGrpcClient;
use golem_api_grpc::proto::golem::component::v1::{
    component_error, create_component_request, create_component_response, create_plugin_response,
    download_component_response, get_component_metadata_response, get_components_response,
    install_plugin_response, update_component_request, update_component_response,
    CreateComponentRequest, CreateComponentRequestChunk, CreateComponentRequestHeader,
    CreatePluginRequest, GetComponentsRequest, GetLatestComponentRequest, UpdateComponentRequest,
    UpdateComponentRequestChunk, UpdateComponentRequestHeader,
};
use golem_client::api::ComponentClient as ComponentServiceHttpClient;
use golem_client::api::ComponentClientLive as ComponentServiceHttpClientLive;
use golem_client::api::PluginClient as PluginServiceHttpClient;
use golem_client::api::PluginClientLive as PluginServiceHttpClientLive;
use golem_client::Context;
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::plugin::{
    DefaultPluginOwner, DefaultPluginScope, PluginDefinition, PluginTypeSpecificDefinition,
};
use golem_common::model::{
    ComponentFilePathWithPermissions, ComponentId, ComponentType, ComponentVersion,
    InitialComponentFile, PluginInstallationId,
};
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::fs;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::time::sleep;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tracing::{debug, info, Level};
use url::Url;

pub mod docker;
pub mod filesystem;
pub mod k8s;
pub mod provided;
pub mod spawned;

#[derive(Clone)]
pub enum ComponentServiceClient {
    Grpc(ComponentServiceGrpcClient<Channel>),
    Http(Arc<ComponentServiceHttpClientLive>),
}

#[derive(Clone)]
pub enum PluginServiceClient {
    Grpc(PluginServiceGrpcClient<Channel>),
    Http(Arc<PluginServiceHttpClientLive>),
}

#[async_trait]
pub trait ComponentService {
    fn component_client(&self) -> ComponentServiceClient;
    fn plugin_client(&self) -> PluginServiceClient;

    async fn get_or_add_component(
        &self,
        local_path: &Path,
        name: &str,
        component_type: ComponentType,
        files: &[InitialComponentFile],
        dynamic_linking: &HashMap<String, DynamicLinkedInstance>,
        unverified: bool,
    ) -> ComponentId {
        let mut retries = 5;
        loop {
            let latest_component_id: Option<ComponentId> = match self.component_client() {
                ComponentServiceClient::Grpc(mut client) => {
                    match client
                        .get_components(GetComponentsRequest {
                            project_id: None,
                            component_name: Some(name.to_string()),
                        })
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
                                .map(|component| {
                                    component
                                        .versioned_component_id
                                        .expect("versioned_component_id field is missing")
                                        .component_id
                                        .expect("component_id field is missing")
                                        .try_into()
                                        .expect("component_id has unexpected format")
                                })
                        }
                        Some(get_components_response::Result::Error(error)) => {
                            panic!(
                                "Failed to get components from golem-component-service (GRPC): {error:?}"
                            );
                        }
                    }
                }
                ComponentServiceClient::Http(client) => {
                    match client.get_components(Some(&name)).await {
                        Ok(result) => {
                            debug!("Response from get_components (HTTP) was {result:?}");
                            result
                                .into_iter()
                                .max_by_key(|component| component.versioned_component_id.version)
                                .map(|component| {
                                    ComponentId(component.versioned_component_id.component_id)
                                })
                        }
                        Err(error) => {
                            panic!(
                                "Failed to get components from golem-component-service (HTTP): {error:?}"
                            );
                        }
                    }
                }
            };

            if let Some(latest_component_id) = latest_component_id {
                return latest_component_id;
            }

            match self
                .add_component(
                    local_path,
                    name,
                    component_type,
                    files,
                    dynamic_linking,
                    unverified,
                )
                .await
            {
                Ok(component_id) => break component_id,
                Err(AddComponentError::AlreadyExists) => {
                    if retries > 0 {
                        info!("Component with name {name} got created in parallel, retrying get_or_add_component");
                        retries -= 1;
                        sleep(Duration::from_secs(1)).await;
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
        _component_type: ComponentType,
    ) -> Result<(), AddComponentError> {
        panic!(
            "Adding a component with a specific Component ID is only supported in filesystem mode"
        )
    }

    async fn add_component(
        &self,
        local_path: &Path,
        name: &str,
        component_type: ComponentType,
        files: &[InitialComponentFile],
        dynamic_linking: &HashMap<String, DynamicLinkedInstance>,
        _unverified: bool,
    ) -> Result<ComponentId, AddComponentError> {
        let mut file = File::open(local_path).await.map_err(|_| {
            AddComponentError::Other(format!("Failed to read component from {local_path:?}"))
        })?;

        match self.component_client() {
            ComponentServiceClient::Grpc(mut client) => {
                let component_type: golem_api_grpc::proto::golem::component::ComponentType =
                    component_type.into();

                let files = files.iter().map(|f| f.clone().into()).collect();

                let mut chunks: Vec<CreateComponentRequest> = vec![CreateComponentRequest {
                    data: Some(create_component_request::Data::Header(
                        CreateComponentRequestHeader {
                            project_id: None,
                            component_name: name.to_string(),
                            component_type: Some(component_type as i32),
                            files,
                            dynamic_linking: HashMap::from_iter(
                                dynamic_linking
                                    .iter()
                                    .map(|(k, v)| (k.clone(), v.clone().into())),
                            ),
                        },
                    )),
                }];

                loop {
                    let mut buffer = [0; 4096];

                    let n = file.read(&mut buffer).await.map_err(|_| {
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
                let response = client
                    .create_component(tokio_stream::iter(chunks))
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
                    Some(create_component_response::Result::Success(component)) => {
                        info!("Created component (GRPC) {component:?}");
                        Ok(component
                            .versioned_component_id
                            .ok_or(AddComponentError::Other(
                                "Missing versioned_component_id field".to_string(),
                            ))?
                            .component_id
                            .ok_or(AddComponentError::Other(
                                "Missing component_id field".to_string(),
                            ))?
                            .try_into()
                            .map_err(|error| {
                                AddComponentError::Other(format!(
                                    "component_id has unexpected format: {error}"
                                ))
                            })?)
                    }
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
            ComponentServiceClient::Http(client) => {
                let archive = build_ifs_archive(Some(files)).await.map_err(|error| {
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
                        name,
                        Some(&component_type),
                        file,
                        to_http_file_permissions(files).as_ref(),
                        archive_file,
                        to_http_dynamic_linking(Some(dynamic_linking)).as_ref(),
                    )
                    .await
                {
                    Ok(component) => {
                        debug!("Created component (HTTP) {:?}", component);
                        Ok(ComponentId(component.versioned_component_id.component_id))
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
        component_id: &ComponentId,
        local_path: &Path,
        component_type: ComponentType,
        files: Option<&[InitialComponentFile]>,
        dynamic_linking: Option<&HashMap<String, DynamicLinkedInstance>>,
    ) -> u64 {
        let mut file = File::open(local_path)
            .await
            .unwrap_or_else(|_| panic!("Failed to read component from {local_path:?}"));

        match self.component_client() {
            ComponentServiceClient::Grpc(mut client) => {
                let component_type: golem_api_grpc::proto::golem::component::ComponentType =
                    component_type.into();

                let update_files = files.is_some();

                let files: Vec<golem_api_grpc::proto::golem::component::InitialComponentFile> =
                    files
                        .into_iter()
                        .flatten()
                        .map(|f| f.clone().into())
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
                        },
                    )),
                }];

                loop {
                    let mut buffer = [0; 4096];

                    let n = file
                        .read(&mut buffer)
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
                let response = client
                    .update_component(tokio_stream::iter(chunks))
                    .await
                    .expect("Failed to update component")
                    .into_inner();
                match response.result {
                    None => {
                        panic!("Missing response from golem-component-service for create-component")
                    }
                    Some(update_component_response::Result::Success(component)) => {
                        info!("Updated component (GRPC) {component:?}");
                        component.versioned_component_id.unwrap().version
                    }
                    Some(update_component_response::Result::Error(error)) => {
                        panic!("Failed to update component in golem-component-service (GRPC): {error:?}");
                    }
                }
            }
            ComponentServiceClient::Http(client) => {
                let archive = match build_ifs_archive(files).await {
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
                    )
                    .await
                {
                    Ok(component) => {
                        debug!("Updated component (HTTP) {:?}", component);
                        component.versioned_component_id.version
                    }
                    Err(error) => {
                        panic!("Failed to update component in golem-component-service (HTTP): {error:?}");
                    }
                }
            }
        }
    }

    async fn get_latest_version(&self, component_id: &ComponentId) -> u64 {
        match self.component_client() {
            ComponentServiceClient::Grpc(mut client) => {
                let response = client
                    .get_latest_component_metadata(GetLatestComponentRequest {
                        component_id: Some(component_id.clone().into()),
                    })
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
            ComponentServiceClient::Http(client) => {
                client
                    .get_latest_component_metadata(&component_id.0)
                    .await
                    .expect("Failed to get latest component metadata (HTTP)")
                    .versioned_component_id
                    .version
            }
        }
    }

    async fn create_plugin(
        &self,
        definition: PluginDefinition<DefaultPluginOwner, DefaultPluginScope>,
    ) -> crate::Result<()> {
        match self.plugin_client() {
            PluginServiceClient::Grpc(mut client) => {
                let response = client
                    .create_plugin(CreatePluginRequest {
                        plugin: Some(definition.into()),
                    })
                    .await?
                    .into_inner();
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
            PluginServiceClient::Http(client) => {
                let result = client.create_plugin(
                    &golem_client::model::PluginDefinitionWithoutOwnerDefaultPluginScope {
                        name: definition.name,
                        version: definition.version,
                        description: definition.description,
                        icon: definition.icon,
                        homepage: definition.homepage,
                        specs: match definition.specs {
                            PluginTypeSpecificDefinition::ComponentTransformer(def) => {
                                golem_client::model::PluginTypeSpecificDefinition::ComponentTransformer(
                                    golem_client::model::ComponentTransformerDefinition {
                                        provided_wit_package: def.provided_wit_package,
                                        json_schema: def.json_schema,
                                        validate_url: def.validate_url,
                                        transform_url: def.transform_url,
                                    },
                                )
                            }
                            PluginTypeSpecificDefinition::OplogProcessor(def) => {
                                golem_client::model::PluginTypeSpecificDefinition::OplogProcessor(
                                    golem_client::model::OplogProcessorDefinition {
                                        component_id: def.component_id.0,
                                        component_version: def.component_version,
                                    },
                                )
                            }
                        },
                        scope: definition.scope,
                    },
                ).await;
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
        component_id: &ComponentId,
        plugin_name: &str,
        plugin_version: &str,
        priority: i32,
        parameters: HashMap<String, String>,
    ) -> crate::Result<PluginInstallationId> {
        match self.component_client() {
            ComponentServiceClient::Grpc(mut client) => {
                let response = client
                    .install_plugin(
                        golem_api_grpc::proto::golem::component::v1::InstallPluginRequest {
                            component_id: Some(component_id.clone().into()),
                            name: plugin_name.to_string(),
                            version: plugin_version.to_string(),
                            priority,
                            parameters,
                        },
                    )
                    .await?
                    .into_inner();

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
            ComponentServiceClient::Http(client) => {
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
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> crate::Result<Option<u64>> {
        match self.component_client() {
            ComponentServiceClient::Grpc(mut client) => {
                let response = client
                    .download_component(
                        golem_api_grpc::proto::golem::component::v1::DownloadComponentRequest {
                            component_id: Some(component_id.clone().into()),
                            version: Some(component_version),
                        },
                    )
                    .await?
                    .into_inner();

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
                Ok(Some(bytes.len() as u64))
            }
            ComponentServiceClient::Http(client) => match client
                .download_component(&component_id.0, Some(component_version))
                .await
            {
                Ok(bytes) => Ok(Some(bytes.len() as u64)),
                Err(error) => Err(anyhow!("{error:?}")),
            },
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

fn new_component_http_client(host: &str, http_port: u16) -> Arc<ComponentServiceHttpClientLive> {
    Arc::new(ComponentServiceHttpClientLive {
        context: Context {
            client: new_reqwest_client(),
            base_url: Url::parse(&format!("http://{host}:{http_port}"))
                .expect("Failed to parse url"),
        },
    })
}

async fn new_component_client(
    protocol: GolemClientProtocol,
    host: &str,
    grpc_port: u16,
    http_port: u16,
) -> ComponentServiceClient {
    match protocol {
        GolemClientProtocol::Grpc => {
            ComponentServiceClient::Grpc(new_component_grpc_client(host, grpc_port).await)
        }
        GolemClientProtocol::Http => {
            ComponentServiceClient::Http(new_component_http_client(host, http_port))
        }
    }
}

async fn new_plugin_grpc_client(host: &str, grpc_port: u16) -> PluginServiceGrpcClient<Channel> {
    PluginServiceGrpcClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to golem-component-service (plugins)")
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip)
}

fn new_plugin_http_client(host: &str, http_port: u16) -> Arc<PluginServiceHttpClientLive> {
    Arc::new(PluginServiceHttpClientLive {
        context: Context {
            client: new_reqwest_client(),
            base_url: Url::parse(&format!("http://{host}:{http_port}"))
                .expect("Failed to parse url"),
        },
    })
}

async fn new_plugin_client(
    protocol: GolemClientProtocol,
    host: &str,
    grpc_port: u16,
    http_port: u16,
) -> PluginServiceClient {
    match protocol {
        GolemClientProtocol::Grpc => {
            PluginServiceClient::Grpc(new_plugin_grpc_client(host, grpc_port).await)
        }
        GolemClientProtocol::Http => {
            PluginServiceClient::Http(new_plugin_http_client(host, http_port))
        }
    }
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

#[async_trait]
pub trait ComponentServiceEnvVars {
    async fn env_vars(
        &self,
        http_port: u16,
        grpc_port: u16,
        component_compilation_service: Option<(&str, u16)>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
    ) -> HashMap<String, String>;
}

#[async_trait]
impl ComponentServiceEnvVars for GolemEnvVars {
    async fn env_vars(
        &self,
        http_port: u16,
        grpc_port: u16,
        component_compilation_service: Option<(&str, u16)>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
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
            .with("GOLEM__GRPC_PORT", grpc_port.to_string())
            .with("GOLEM__HTTP_PORT", http_port.to_string())
            .with_all(rdb.info().env("golem_component"));

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
    files: &[InitialComponentFile],
) -> Option<golem_common::model::ComponentFilePathWithPermissionsList> {
    if files.is_empty() {
        None
    } else {
        Some(golem_client::model::ComponentFilePathWithPermissionsList {
            values: files
                .iter()
                .map(|file| ComponentFilePathWithPermissions {
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
    let Some(dynamic_linking) = dynamic_linking else {
        return None;
    };
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
                                    target_interface_name: link.target_interface_name.clone(),
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
    files: Option<&[InitialComponentFile]>,
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

    for file in files {
        zip_writer
            .write_entry_whole(
                ZipEntryBuilder::new(file.key.0.clone().into(), Compression::Deflate),
                &(fs::read(Path::new(file.path.as_path())).await?),
            )
            .await?;
    }

    zip_writer.close().await?;
    let file_path = temp_dir.path().join(ARCHIVE_NAME);
    Ok(Some((temp_dir, file_path)))
}
