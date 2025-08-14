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
use golem_client::api::ComponentClient as ComponentServiceHttpClient;
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

#[async_trait]
pub trait RegistryService: Send + Sync {
    fn http_host(&self) -> String;
    fn http_port(&self) -> u16;


    fn grpc_host(&self) -> String;
    fn gprc_port(&self) -> u16;

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

    async fn kill(&self);
}

async fn wait_for_startup(
    protocol: GolemClientProtocol,
    host: &str,
    grpc_port: u16,
    http_port: u16,
    timeout: Duration,
) {
    wait_for_startup_http(host, http_port, "golem-registry-service", timeout).await
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
