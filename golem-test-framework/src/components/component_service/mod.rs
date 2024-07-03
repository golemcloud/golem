// Copyright 2024 Golem Cloud
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

use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use create_component_request::Data;
use golem_api_grpc::proto::golem::component::{
    component_error, create_component_request, create_component_response,
    get_component_metadata_response, get_components_response, update_component_request,
    update_component_response, CreateComponentRequest, CreateComponentRequestChunk,
    CreateComponentRequestHeader, GetComponentsRequest, GetLatestComponentRequest,
    UpdateComponentRequest, UpdateComponentRequestChunk, UpdateComponentRequestHeader,
};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tonic::transport::Channel;
use tracing::{info, Level};

use golem_api_grpc::proto::golem::component::component_service_client::ComponentServiceClient;
use golem_common::model::ComponentId;

use crate::components::rdb::Rdb;
use crate::components::wait_for_startup_grpc;

pub mod docker;
pub mod filesystem;
pub mod k8s;
pub mod provided;
pub mod spawned;

#[async_trait]
pub trait ComponentService {
    async fn client(&self) -> ComponentServiceClient<Channel> {
        new_client(&self.public_host(), self.public_grpc_port()).await
    }

    async fn get_or_add_component(&self, local_path: &Path) -> ComponentId {
        let mut retries = 3;
        loop {
            let file_name = local_path.file_name().unwrap().to_string_lossy();
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
                    let latest = result
                        .components
                        .into_iter()
                        .max_by_key(|t| t.versioned_component_id.as_ref().unwrap().version);
                    match latest {
                        Some(component) => {
                            break component
                                .versioned_component_id
                                .expect("versioned_component_id field is missing")
                                .component_id
                                .expect("component_id field is missing")
                                .try_into()
                                .expect("component_id has unexpected format")
                        }
                        None => match self.add_component(local_path).await {
                            Ok(component_id) => break component_id,
                            Err(AddComponentError::AlreadyExists) => {
                                if retries > 0 {
                                    info!("Component got created in parallel, retrying get_or_add_component");
                                    retries -= 1;
                                    continue;
                                } else {
                                    panic!("Component already exists in golem-component-service");
                                }
                            }
                            Err(AddComponentError::Other(message)) => {
                                panic!("Failed to add component: {message}");
                            }
                        },
                    }
                }
                Some(get_components_response::Result::Error(error)) => {
                    panic!("Failed to get components from golem-component-service: {error:?}");
                }
            }
        }
    }

    async fn add_component(
        &self,
        local_path: &Path,
    ) -> Result<ComponentId, crate::components::component_service::AddComponentError> {
        let file_name = local_path.file_name().unwrap().to_string_lossy();
        self.add_component_with_name(local_path, &file_name).await
    }

    async fn add_component_with_name(
        &self,
        local_path: &Path,
        name: &str,
    ) -> Result<ComponentId, AddComponentError> {
        let mut client = self.client().await;
        let mut file = File::open(local_path).await.map_err(|_| {
            AddComponentError::Other(format!("Failed to read component from {local_path:?}"))
        })?;

        let mut chunks: Vec<CreateComponentRequest> = vec![CreateComponentRequest {
            data: Some(Data::Header(CreateComponentRequestHeader {
                project_id: None,
                component_name: name.to_string(),
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
                    .protected_component_id
                    .ok_or(AddComponentError::Other(
                        "Missing protected_component_id field".to_string(),
                    ))?
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

    async fn update_component(&self, component_id: &ComponentId, local_path: &Path) -> u64 {
        let mut client = self.client().await;
        let mut file = File::open(local_path)
            .await
            .unwrap_or_else(|_| panic!("Failed to read component from {local_path:?}"));

        let mut chunks: Vec<UpdateComponentRequest> = vec![UpdateComponentRequest {
            data: Some(update_component_request::Data::Header(
                UpdateComponentRequestHeader {
                    component_id: Some(component_id.clone().into()),
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
                component
                    .protected_component_id
                    .unwrap()
                    .versioned_component_id
                    .unwrap()
                    .version
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

    fn kill(&self);
}

async fn new_client(host: &str, grpc_port: u16) -> ComponentServiceClient<Channel> {
    ComponentServiceClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to golem-component-service")
}

async fn wait_for_startup(host: &str, grpc_port: u16, timeout: Duration) {
    wait_for_startup_grpc(host, grpc_port, "golem-component-service", timeout).await
}

fn env_vars(
    http_port: u16,
    grpc_port: u16,
    component_compilation_service: Option<(&str, u16)>,
    rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    verbosity: Level,
) -> HashMap<String, String> {
    let log_level = verbosity.as_str().to_lowercase();
    let vars: &[(&str, &str)] = &[
        ("RUST_LOG"                     , &format!("{log_level},cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn")),
        ("RUST_BACKTRACE"               , "1"),
        ("GOLEM__COMPONENT_STORE__TYPE", "Local"),
        ("GOLEM__COMPONENT_STORE__CONFIG__OBJECT_PREFIX", ""),
        ("GOLEM__COMPONENT_STORE__CONFIG__ROOT_PATH", "/tmp/ittest-local-object-store/golem"),
        ("GOLEM__GRPC_PORT", &grpc_port.to_string()),
        ("GOLEM__HTTP_PORT", &http_port.to_string()),
    ];

    let mut vars: HashMap<String, String> =
        HashMap::from_iter(vars.iter().map(|(k, v)| (k.to_string(), v.to_string())));

    match component_compilation_service {
        Some((host, port)) => {
            vars.insert(
                "GOLEM__COMPILATION__TYPE".to_string(),
                "Enabled".to_string(),
            );
            vars.insert(
                "GOLEM__COMPILATION__CONFIG__HOST".to_string(),
                host.to_string(),
            );
            vars.insert(
                "GOLEM__COMPILATION__CONFIG__PORT".to_string(),
                port.to_string(),
            );
        }
        _ => {
            vars.insert(
                "GOLEM__COMPILATION__TYPE".to_string(),
                "Disabled".to_string(),
            );
        }
    };

    vars.extend(rdb.info().env("golem_component").clone());
    vars
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
