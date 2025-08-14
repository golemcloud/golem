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

use super::{ComponentServiceGrpcClient, RegistryService};
use super::PluginServiceGrpcClient;
use crate::components::rdb::{DbInfo, Rdb};
use crate::components::{new_reqwest_client, wait_for_startup_http, ChildProcessLogger};
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::OnceCell;
use tonic::transport::Channel;
use tracing::info;
use tracing::Level;
use tokio::task::JoinSet;
use golem_registry_service::config::RegistryServiceConfig;
use golem_service_base::config::{BlobStorageConfig, LocalFileSystemBlobStorageConfig};
use crate::components::blob_storage::BlobStorageInfo;

pub struct SpawnedRegistyService {
    join_set: Option<JoinSet<anyhow::Result<()>>>,
    run_details: golem_registry_service::RunDetails,
    base_http_client: OnceCell<reqwest::Client>,
}

impl SpawnedRegistyService {
    pub async fn new(
        db_info: &DbInfo,
        blob_storage_info: &BlobStorageInfo,
    ) -> anyhow::Result<Self> {
        info!("Starting golem-registry-service process");

        let mut join_set = JoinSet::new();

        let config= make_config(db_info, blob_storage_info);

        let prometheus_registry = prometheus::Registry::new();

        let service = golem_registry_service::RegistryService::new(
            config,
            prometheus_registry
        ).await?;

        let run_details = service.start(&mut join_set).await?;

        wait_for_startup_http("localhost", run_details.http_port, "registry-service", Duration::from_secs(10)).await;

        Ok(Self {
            run_details,
            join_set: Some(join_set),
            base_http_client: OnceCell::new()
        })
    }
}

#[async_trait]
impl RegistryService for SpawnedRegistyService {
    fn http_host(&self) -> String {
        "localhost".to_string()
    }
    fn http_port(&self) -> u16 {
        self.run_details.http_port
    }


    fn grpc_host(&self) -> String {
        "localhost".to_string()
    }
    fn gprc_port(&self) -> u16 {
        // self.run_details.grpc_port
        todo!()
    }

    async fn kill(&mut self) {
        if let Some(mut join_set) = self.join_set.take() {
            join_set.abort_all();
            join_set.join_all().await;
        };
    }

    async fn base_http_client(&self) -> reqwest::Client {
        self.base_http_client
            .get_or_init(async || new_reqwest_client())
            .await
            .clone()
    }

}

fn make_config(
    db_info: &DbInfo,
    blob_storage_info: &BlobStorageInfo,
) -> RegistryServiceConfig {
    RegistryServiceConfig {
        db: db_info.config("golem_component", false),
        blob_storage: blob_storage_info.config(),
        grpc_port: 0,
        http_port: 0,
        ..Default::default()
    }
}
