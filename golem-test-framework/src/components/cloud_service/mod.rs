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

use crate::components::rdb::Rdb;
use crate::components::{
    new_reqwest_client, wait_for_startup_grpc, wait_for_startup_http, EnvVarBuilder,
};
use crate::config::GolemClientProtocol;
use crate::model::PluginDefinitionCreation;
use anyhow::{anyhow, Context as AnyhowContext};
use async_trait::async_trait;
use async_zip::base::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use futures_util::{stream, StreamExt, TryStreamExt};
use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient as ComponentServiceGrpcClient;
use golem_api_grpc::proto::golem::component::v1::plugin_service_client::PluginServiceClient as PluginServiceGrpcClient;
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
use golem_api_grpc::proto::golem::project::v1::cloud_project_service_client::CloudProjectServiceClient;
use golem_api_grpc::proto::golem::project::v1::{get_default_project_response, GetDefaultProjectRequest};
use golem_client::api::{ComponentClient as ComponentServiceHttpClient, ProjectClient};
use golem_client::api::ComponentClientLive as ComponentServiceHttpClientLive;
use golem_client::api::PluginClient as PluginServiceHttpClient;
use golem_client::api::PluginClientLive as PluginServiceHttpClientLive;
use golem_client::model::ComponentQuery;
use golem_client::{Context, Security};
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::plugin::PluginTypeSpecificDefinition;
use golem_common::model::{
    AccountId, ComponentFilePathWithPermissions, ComponentId, ComponentType, ComponentVersion,
    InitialComponentFile, PluginId, PluginInstallationId,
};
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use uuid::Uuid;
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
use super::ADMIN_TOKEN;
use golem_common::model::ProjectId;

pub mod docker;
pub mod provided;
pub mod spawned;
pub mod k8s;

#[derive(Clone)]
pub enum ProjectServiceClient {
    Grpc(CloudProjectServiceClient<Channel>),
    Http(Arc<golem_client::api::ProjectClientLive>),
}

#[async_trait]
pub trait CloudServiceInternal: Send + Sync {
    fn project_client(&self) -> ProjectServiceClient;
}

#[async_trait]
pub trait CloudService: CloudServiceInternal {
    async fn get_default_project(&self) -> crate::Result<ProjectId> {
        match self.project_client() {
            ProjectServiceClient::Grpc(mut client) => {
                let result = client
                    .get_default_project(GetDefaultProjectRequest { })
                    .await?
                    .into_inner()
                    .result
                    .ok_or_else(|| anyhow!("get_default_project: no result"))?;

                match result {
                    get_default_project_response::Result::Success(result) => Ok(result.id.unwrap().try_into().unwrap()),
                    get_default_project_response::Result::Error(error) => Err(anyhow!("{error:?}"))
                }
            }
            ProjectServiceClient::Http(client) => Ok(ProjectId(client.get_default_project().await?.project_id))
        }
    }

    fn admin_token(&self) -> Uuid {
        ADMIN_TOKEN.clone()
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

async fn new_project_grpc_client(
    host: &str,
    grpc_port: u16,
) -> CloudProjectServiceClient<Channel> {
    CloudProjectServiceClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to golem-cloud-service")
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip)
}

fn new_project_http_client(host: &str, http_port: u16) -> Arc<golem_client::api::ProjectClientLive> {
    Arc::new(golem_client::api::ProjectClientLive {
        context: Context {
            client: new_reqwest_client(),
            base_url: Url::parse(&format!("http://{host}:{http_port}"))
                .expect("Failed to parse url"),
            security_token: Security::Bearer(ADMIN_TOKEN.to_string())
        },
    })
}

async fn new_project_client(
    protocol: GolemClientProtocol,
    host: &str,
    grpc_port: u16,
    http_port: u16,
) -> ProjectServiceClient {
    match protocol {
        GolemClientProtocol::Grpc => {
            ProjectServiceClient::Grpc(new_project_grpc_client(host, grpc_port).await)
        }
        GolemClientProtocol::Http => {
            ProjectServiceClient::Http(new_project_http_client(host, http_port))
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
            wait_for_startup_grpc(host, grpc_port, "cloud-service", timeout).await
        }
        GolemClientProtocol::Http => {
            wait_for_startup_http(host, http_port, "cloud-service", timeout).await
        }
    }
}

async fn env_vars(
    http_port: u16,
    grpc_port: u16,
    rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    verbosity: Level,
    private_rdb_connection: bool,
) -> HashMap<String, String> {
    EnvVarBuilder::golem_service(verbosity)
        .with("GOLEM__ACCOUNTS__ROOT__TOKEN", ADMIN_TOKEN.to_string())
        .with("GOLEM__GRPC_PORT", grpc_port.to_string())
        .with("GOLEM__HTTP_PORT", http_port.to_string())
        .with_all(rdb.info().env("cloud_service", private_rdb_connection))
        .build()
}
