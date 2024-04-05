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
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use create_template_request::Data;
use golem_api_grpc::proto::golem::template::{
    create_template_request, create_template_response, get_template_metadata_response,
    get_templates_response, update_template_request, update_template_response,
    CreateTemplateRequest, CreateTemplateRequestChunk, CreateTemplateRequestHeader,
    GetLatestTemplateRequest, GetTemplatesRequest, UpdateTemplateRequest,
    UpdateTemplateRequestChunk, UpdateTemplateRequestHeader,
};
use tonic::transport::Channel;
use tracing::{info, Level};

use golem_api_grpc::proto::golem::template::template_service_client::TemplateServiceClient;
use golem_common::model::TemplateId;

use crate::components::rdb::Rdb;
use crate::components::wait_for_startup_grpc;

pub mod docker;
pub mod filesystem;
pub mod provided;
pub mod spawned;

#[async_trait]
pub trait TemplateService {
    async fn client(&self) -> TemplateServiceClient<Channel> {
        new_client(self.public_host(), self.public_grpc_port()).await
    }

    async fn get_or_add_template(&self, local_path: &Path) -> TemplateId {
        let file_name = local_path.file_name().unwrap().to_string_lossy();
        let mut client = self.client().await;
        let response = client
            .get_templates(GetTemplatesRequest {
                project_id: None,
                template_name: Some(file_name.to_string()),
            })
            .await
            .expect("Failed to call get-templates")
            .into_inner();

        match response.result {
            None => {
                panic!("Missing response from golem-template-service for get-templates")
            }
            Some(get_templates_response::Result::Success(result)) => {
                let latest = result
                    .templates
                    .into_iter()
                    .max_by_key(|t| t.versioned_template_id.as_ref().unwrap().version);
                match latest {
                    Some(template) => template
                        .versioned_template_id
                        .expect("versioned_template_id field is missing")
                        .template_id
                        .expect("template_id field is missing")
                        .try_into()
                        .expect("template_id has unexpected format"),
                    None => self.add_template(local_path).await,
                }
            }
            Some(get_templates_response::Result::Error(error)) => {
                panic!("Failed to get templates from golem-template-service: {error:?}");
            }
        }
    }

    async fn add_template(&self, local_path: &Path) -> TemplateId {
        let mut client = self.client().await;
        let file_name = local_path.file_name().unwrap().to_string_lossy();
        let data = std::fs::read(local_path)
            .unwrap_or_else(|_| panic!("Failed to read template from {local_path:?}"));

        let chunks: Vec<CreateTemplateRequest> = vec![
            CreateTemplateRequest {
                data: Some(Data::Header(CreateTemplateRequestHeader {
                    project_id: None,
                    template_name: file_name.to_string(),
                })),
            },
            CreateTemplateRequest {
                data: Some(Data::Chunk(CreateTemplateRequestChunk {
                    template_chunk: data,
                })),
            },
        ];
        let response = client
            .create_template(tokio_stream::iter(chunks))
            .await
            .expect("Failed to create template")
            .into_inner();
        match response.result {
            None => {
                panic!("Missing response from golem-template-service for create-template")
            }
            Some(create_template_response::Result::Success(template)) => {
                info!("Created template {template:?}");
                template
                    .protected_template_id
                    .unwrap()
                    .versioned_template_id
                    .unwrap()
                    .template_id
                    .unwrap()
                    .try_into()
                    .unwrap()
            }
            Some(create_template_response::Result::Error(error)) => {
                panic!("Failed to create template in golem-template-service: {error:?}");
            }
        }
    }

    async fn update_template(&self, template_id: &TemplateId, local_path: &Path) -> i32 {
        let mut client = self.client().await;
        let data = std::fs::read(local_path)
            .unwrap_or_else(|_| panic!("Failed to read template from {local_path:?}"));

        let chunks: Vec<UpdateTemplateRequest> = vec![
            UpdateTemplateRequest {
                data: Some(update_template_request::Data::Header(
                    UpdateTemplateRequestHeader {
                        template_id: Some(template_id.clone().into()),
                    },
                )),
            },
            UpdateTemplateRequest {
                data: Some(update_template_request::Data::Chunk(
                    UpdateTemplateRequestChunk {
                        template_chunk: data,
                    },
                )),
            },
        ];
        let response = client
            .update_template(tokio_stream::iter(chunks))
            .await
            .expect("Failed to update template")
            .into_inner();
        match response.result {
            None => {
                panic!("Missing response from golem-template-service for create-template")
            }
            Some(update_template_response::Result::Success(template)) => {
                info!("Created template {template:?}");
                template
                    .protected_template_id
                    .unwrap()
                    .versioned_template_id
                    .unwrap()
                    .version
            }
            Some(update_template_response::Result::Error(error)) => {
                panic!("Failed to update template in golem-template-service: {error:?}");
            }
        }
    }

    async fn get_latest_version(&self, template_id: &TemplateId) -> i32 {
        let response = self
            .client()
            .await
            .get_latest_template_metadata(GetLatestTemplateRequest {
                template_id: Some(template_id.clone().into()),
            })
            .await
            .expect("Failed to get latest template metadata")
            .into_inner();
        match response.result {
            None => {
                panic!("Missing response from golem-template-service for create-template")
            }
            Some(get_template_metadata_response::Result::Success(template)) => {
                template
                    .template
                    .expect("No template in response")
                    .versioned_template_id
                    .expect("No versioned_template_id field")
                    .version
            }
            Some(get_template_metadata_response::Result::Error(error)) => {
                panic!("Failed to get template metadata from golem-template-service: {error:?}");
            }
        }
    }

    fn private_host(&self) -> &str;
    fn private_http_port(&self) -> u16;
    fn private_grpc_port(&self) -> u16;

    fn public_host(&self) -> &str {
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

async fn new_client(host: &str, grpc_port: u16) -> TemplateServiceClient<Channel> {
    TemplateServiceClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to golem-template-service")
}

async fn wait_for_startup(host: &str, grpc_port: u16) {
    wait_for_startup_grpc(host, grpc_port, "golem-template-service").await
}

fn env_vars(
    http_port: u16,
    grpc_port: u16,
    rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    verbosity: Level,
) -> HashMap<String, String> {
    let log_level = verbosity.as_str().to_lowercase();

    let vars: &[(&str, &str)] = &[
        ("RUST_LOG"                     , &format!("{log_level},cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn")),
        ("RUST_BACKTRACE"               , "1"),
        ("GOLEM__TEMPLATE_STORE__TYPE", "Local"),
        ("GOLEM__TEMPLATE_STORE__CONFIG__OBJECT_PREFIX", ""),
        ("GOLEM__TEMPLATE_STORE__CONFIG__ROOT_PATH", "/tmp/ittest-local-object-store/golem"),
        ("GOLEM__GRPC_PORT", &grpc_port.to_string()),
        ("GOLEM__HTTP_PORT", &http_port.to_string()),
    ];

    let mut vars: HashMap<String, String> =
        HashMap::from_iter(vars.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    vars.extend(rdb.info().env().clone());
    vars
}
