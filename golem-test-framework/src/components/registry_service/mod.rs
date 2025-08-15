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

pub mod spawned;

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
use golem_client::api::{ComponentClient as ComponentServiceHttpClient, RegistryServiceClientLive};
use golem_client::api::ComponentClientLive as ComponentServiceHttpClientLive;
use golem_client::api::PluginClient as PluginServiceHttpClient;
use golem_client::api::PluginClientLive as PluginServiceHttpClientLive;
use golem_client::{Context, Security};
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::plugin::PluginTypeSpecificDefinition;
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
use golem_common::model::component::InitialComponentFile;
use golem_common::model::auth::TokenSecret;
use golem_common::model::account::AccountId;

#[async_trait]
pub trait RegistryService: Send + Sync {
    fn http_host(&self) -> String;
    fn http_port(&self) -> u16;


    fn grpc_host(&self) -> String;
    fn gprc_port(&self) -> u16;

    fn admin_account_id(&self) -> AccountId;
    fn admin_account_email(&self) -> String;
    fn admin_account_token(&self) -> TokenSecret;

    async fn kill(&mut self);

    async fn base_http_client(&self) -> reqwest::Client;

    async fn client(&self, token: &TokenSecret) -> RegistryServiceClientLive {
        let url = format!("http://{}:{}", self.http_host(), self.http_port());
        RegistryServiceClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: Url::parse(&url).expect("Failed to parse url"),
                security_token: Security::Bearer(token.0.to_string()),
            },
        }
    }

}
