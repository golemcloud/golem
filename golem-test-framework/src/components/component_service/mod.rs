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

use anyhow::anyhow;
use async_trait::async_trait;
use create_component_request::Data;
use golem_api_grpc::proto::golem::component::v1::{
    component_error, create_component_request, create_component_response, create_plugin_response,
    get_component_metadata_response, get_components_response, install_plugin_response,
    update_component_request, update_component_response, CreateComponentRequest,
    CreateComponentRequestChunk, CreateComponentRequestHeader, CreatePluginRequest,
    GetComponentsRequest, GetLatestComponentRequest, UpdateComponentRequest,
    UpdateComponentRequestChunk, UpdateComponentRequestHeader,
};
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::time::sleep;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tracing::{debug, info, Level};

use crate::components::rdb::Rdb;
use crate::components::{wait_for_startup_grpc, EnvVarBuilder, GolemEnvVars};
use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::v1::plugin_service_client::PluginServiceClient;
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::plugin::{DefaultPluginOwner, DefaultPluginScope, PluginDefinition};
use golem_common::model::{ComponentId, ComponentType, InitialComponentFile, PluginInstallationId};

pub mod docker;
pub mod filesystem;
pub mod k8s;
pub mod provided;
pub mod spawned;

#[async_trait]
pub trait ComponentService {
    async fn client(&self) -> ComponentServiceClient<Channel>;
    async fn plugins_client(&self) -> PluginServiceClient<Channel>;

    async fn get_or_add_component(
        &self,
        local_path: &Path,
        component_type: ComponentType,
    ) -> ComponentId {
        let mut retries = 5;
        loop {
            let mut file_name: String = local_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();
            if component_type == ComponentType::Ephemeral {
                file_name = format!("{}-ephemeral", file_name);
            }

            let mut client = self.client().await;
            let response = client
                .get_components(GetComponentsRequest {
                    project_id: None,
                    component_name: Some(file_name.to_string()),
                })
                .await
                .expect("Failed to call get-components")
                .into_inner();

            match response.result {
                None => {
                    panic!("Missing response from golem-component-service for get-components")
                }
                Some(get_components_response::Result::Success(result)) => {
                    debug!("Response from get_components was {result:?}");
                    let latest = result
                        .components
                        .into_iter()
                        .max_by_key(|t| t.versioned_component_id.as_ref().unwrap().version);
                    match latest {
                        Some(component)
                            if Into::<ComponentType>::into(component.component_type())
                                == component_type =>
                        {
                            break component
                                .versioned_component_id
                                .expect("versioned_component_id field is missing")
                                .component_id
                                .expect("component_id field is missing")
                                .try_into()
                                .expect("component_id has unexpected format")
                        }
                        _ => {
                            match self
                                .add_component_with_name(local_path, &file_name, component_type)
                                .await
                            {
                                Ok(component_id) => break component_id,
                                Err(AddComponentError::AlreadyExists) => {
                                    if retries > 0 {
                                        info!("Component with name {file_name} got created in parallel, retrying get_or_add_component");
                                        retries -= 1;
                                        sleep(Duration::from_secs(1)).await;
                                        continue;
                                    } else {
                                        panic!("Component with name {file_name} already exists in golem-component-service");
                                    }
                                }
                                Err(AddComponentError::Other(message)) => {
                                    panic!(
                                        "Failed to add component with name {file_name}: {message}"
                                    );
                                }
                            }
                        }
                    }
                }
                Some(get_components_response::Result::Error(error)) => {
                    panic!("Failed to get components from golem-component-service: {error:?}");
                }
            }
        }
    }

    // Forward to get_or_add_component. This method is only used in tests for adding a 'broken' component using the
    // filesystem component service, which will skip verification here.
    async fn get_or_add_component_unverified(
        &self,
        local_path: &Path,
        component_type: ComponentType,
    ) -> ComponentId {
        self.get_or_add_component(local_path, component_type).await
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
        component_type: ComponentType,
    ) -> Result<ComponentId, AddComponentError> {
        let file_name = local_path.file_name().unwrap().to_string_lossy();
        self.add_component_with_name(local_path, &file_name, component_type)
            .await
    }

    async fn add_component_with_name(
        &self,
        local_path: &Path,
        name: &str,
        component_type: ComponentType,
    ) -> Result<ComponentId, AddComponentError> {
        self.add_component_with_files(local_path, name, component_type, &[], &HashMap::new())
            .await
    }

    async fn add_component_with_files(
        &self,
        local_path: &Path,
        name: &str,
        component_type: ComponentType,
        files: &[InitialComponentFile],
        dynamic_linking: &HashMap<String, DynamicLinkedInstance>,
    ) -> Result<ComponentId, AddComponentError> {
        let mut client = self.client().await;
        let mut file = File::open(local_path).await.map_err(|_| {
            AddComponentError::Other(format!("Failed to read component from {local_path:?}"))
        })?;

        let component_type: golem_api_grpc::proto::golem::component::ComponentType =
            component_type.into();

        let files = files.iter().map(|f| f.clone().into()).collect();

        let mut chunks: Vec<CreateComponentRequest> = vec![CreateComponentRequest {
            data: Some(Data::Header(CreateComponentRequestHeader {
                project_id: None,
                component_name: name.to_string(),
                component_type: Some(component_type as i32),
                files,
                dynamic_linking: HashMap::from_iter(
                    dynamic_linking
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone().into())),
                ),
            })),
        }];

        loop {
            let mut buffer = [0; 4096];

            let n = file.read(&mut buffer).await.map_err(|_| {
                AddComponentError::Other(format!("Failed to read component from {local_path:?}"))
            })?;

            if n == 0 {
                break;
            } else {
                chunks.push(CreateComponentRequest {
                    data: Some(Data::Chunk(CreateComponentRequestChunk {
                        component_chunk: buffer[0..n].to_vec(),
                    })),
                });
            }
        }
        let response = client
            .create_component(tokio_stream::iter(chunks))
            .await
            .map_err(|status| {
                AddComponentError::Other(format!("Failed to call create_component: {status:?}"))
            })?
            .into_inner();
        match response.result {
            None => Err(AddComponentError::Other(
                "Missing response from golem-component-service for create-component".to_string(),
            )),
            Some(create_component_response::Result::Success(component)) => {
                info!("Created component {component:?}");
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

    async fn update_component(
        &self,
        component_id: &ComponentId,
        local_path: &Path,
        component_type: ComponentType,
    ) -> u64 {
        self.update_component_with_files(
            component_id,
            local_path,
            component_type,
            &None,
            &HashMap::new(),
        )
        .await
    }

    async fn update_component_with_files(
        &self,
        component_id: &ComponentId,
        local_path: &Path,
        component_type: ComponentType,
        files: &Option<Vec<InitialComponentFile>>,
        dynamic_linking: &HashMap<String, DynamicLinkedInstance>,
    ) -> u64 {
        let mut client = self.client().await;
        let mut file = File::open(local_path)
            .await
            .unwrap_or_else(|_| panic!("Failed to read component from {local_path:?}"));

        let component_type: golem_api_grpc::proto::golem::component::ComponentType =
            component_type.into();

        let update_files = files.is_some();

        let files: Vec<golem_api_grpc::proto::golem::component::InitialComponentFile> = files
            .iter()
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
                            .iter()
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
                info!("Created component {component:?}");
                component.versioned_component_id.unwrap().version
            }
            Some(update_component_response::Result::Error(error)) => {
                panic!("Failed to update component in golem-component-service: {error:?}");
            }
        }
    }

    async fn get_latest_version(&self, component_id: &ComponentId) -> u64 {
        let response = self
            .client()
            .await
            .get_latest_component_metadata(GetLatestComponentRequest {
                component_id: Some(component_id.clone().into()),
            })
            .await
            .expect("Failed to get latest component metadata")
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

    async fn create_plugin(
        &self,
        definition: PluginDefinition<DefaultPluginOwner, DefaultPluginScope>,
    ) -> crate::Result<()> {
        let mut client = self.plugins_client().await;
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

    async fn install_plugin_to_component(
        &self,
        component_id: &ComponentId,
        plugin_name: &str,
        plugin_version: &str,
        priority: i32,
        parameters: HashMap<String, String>,
    ) -> crate::Result<PluginInstallationId> {
        let mut client = self.client().await;
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
                "Failed to install plugin in golem-component-service: {error:?}"
            )),
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

async fn new_client(host: &str, grpc_port: u16) -> ComponentServiceClient<Channel> {
    ComponentServiceClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to golem-component-service")
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip)
}

async fn new_plugins_client(host: &str, grpc_port: u16) -> PluginServiceClient<Channel> {
    PluginServiceClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to golem-component-service (plugins)")
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip)
}

async fn wait_for_startup(host: &str, grpc_port: u16, timeout: Duration) {
    wait_for_startup_grpc(host, grpc_port, "golem-component-service", timeout).await
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
