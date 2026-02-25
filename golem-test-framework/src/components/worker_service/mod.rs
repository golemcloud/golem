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

pub mod provided;
pub mod spawned;

use super::rdb::Rdb;
use super::registry_service::RegistryService;
use super::shard_manager::ShardManager;
use super::{
    wait_for_startup_grpc, wait_for_startup_http, wait_for_startup_http_any_response, EnvVarBuilder,
};
use async_trait::async_trait;
use golem_client::api::{AgentClientLive, WorkerClientLive};
use golem_client::{Context, Security};
use golem_common::model::auth::TokenSecret;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::Level;
use url::Url;

#[async_trait]
pub trait WorkerService: Send + Sync {
    fn http_host(&self) -> String;
    fn http_port(&self) -> u16;

    fn grpc_host(&self) -> String;
    fn gprc_port(&self) -> u16;

    fn custom_request_host(&self) -> String;
    fn custom_request_port(&self) -> u16;

    async fn kill(&self);

    async fn base_http_client(&self) -> reqwest::Client;

    async fn worker_http_client(&self, token: &TokenSecret) -> WorkerClientLive {
        let url = format!("http://{}:{}", self.http_host(), self.http_port());
        WorkerClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: Url::parse(&url).expect("Failed to parse url"),
                security_token: Security::Bearer(token.secret().to_string()),
            },
        }
    }

    async fn agent_http_client(&self, token: &TokenSecret) -> AgentClientLive {
        let url = format!("http://{}:{}", self.http_host(), self.http_port());
        AgentClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: Url::parse(&url).expect("Failed to parse url"),
                security_token: Security::Bearer(token.secret().to_string()),
            },
        }
    }
}

async fn wait_for_startup(
    host: &str,
    grpc_port: u16,
    http_port: u16,
    custom_request_port: u16,
    timeout: Duration,
) {
    wait_for_startup_grpc(host, grpc_port, "golem-worker-service", timeout).await;
    wait_for_startup_http(host, http_port, "golem-worker-service", timeout).await;
    wait_for_startup_http_any_response(host, custom_request_port, "golem-worker-service", timeout)
        .await;
}

async fn env_vars(
    http_port: u16,
    grpc_port: u16,
    custom_request_port: u16,
    shard_manager: &Arc<dyn ShardManager>,
    rdb: &Arc<dyn Rdb>,
    verbosity: Level,
    rdb_private_connection: bool,
    registry_service: &Arc<dyn RegistryService>,
    enable_fs_cache: bool,
    otlp: bool,
) -> HashMap<String, String> {
    EnvVarBuilder::golem_service(verbosity)
        .with_str("GOLEM__BLOB_STORAGE__TYPE", "LocalFileSystem")
        .with_str(
            "GOLEM__BLOB_STORAGE__CONFIG__ROOT",
            "/tmp/ittest-local-object-store/golem",
        )
        .with(
            "GOLEM__REGISTRY_SERVICE__HOST",
            registry_service.grpc_host(),
        )
        .with(
            "GOLEM__REGISTRY_SERVICE__PORT",
            registry_service.grpc_port().to_string(),
        )
        .with_str("GOLEM__ENVIRONMENT", "local")
        .with("GOLEM__ROUTING_TABLE__HOST", shard_manager.grpc_host())
        .with(
            "GOLEM__ROUTING_TABLE__PORT",
            shard_manager.grpc_port().to_string(),
        )
        .with(
            "GOLEM__CUSTOM_REQUEST_PORT",
            custom_request_port.to_string(),
        )
        .with("GOLEM__GRPC__PORT", grpc_port.to_string())
        .with("GOLEM__PORT", http_port.to_string())
        .with(
            "GOLEM__ENGINE__ENABLE_FS_CACHE",
            enable_fs_cache.to_string(),
        )
        .with_all(rdb.info().env("golem_worker", rdb_private_connection))
        .with_optional_otlp("worker_service", otlp)
        .build()
}
