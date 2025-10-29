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

use super::cloud_service::CloudService;
use crate::components::component_service::ComponentService;
use crate::components::redis::Redis;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_service::WorkerService;
use crate::components::{wait_for_startup_grpc, EnvVarBuilder};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tonic::codec::CompressionEncoding;
use tonic::transport::{Channel, Endpoint};
use tracing::Level;

pub mod provided;
pub mod spawned;

#[async_trait]
pub trait WorkerExecutor: Send + Sync {
    async fn client(&self) -> crate::Result<WorkerExecutorClient<Channel>>;

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
    async fn restart(&self);
}

async fn new_client(host: &str, grpc_port: u16) -> crate::Result<WorkerExecutorClient<Channel>> {
    Ok(
        WorkerExecutorClient::connect(format!("http://{host}:{grpc_port}"))
            .await?
            .send_compressed(CompressionEncoding::Gzip)
            .accept_compressed(CompressionEncoding::Gzip),
    )
}

fn new_client_lazy(host: &str, grpc_port: u16) -> crate::Result<WorkerExecutorClient<Channel>> {
    Ok(WorkerExecutorClient::new(
        Endpoint::try_from(format!("http://{host}:{grpc_port}"))?.connect_lazy(),
    )
    .send_compressed(CompressionEncoding::Gzip)
    .accept_compressed(CompressionEncoding::Gzip))
}

async fn wait_for_startup(host: &str, grpc_port: u16, timeout: Duration) {
    wait_for_startup_grpc(host, grpc_port, "golem-worker-executor", timeout).await
}

async fn env_vars(
    http_port: u16,
    grpc_port: u16,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
    worker_service: Arc<dyn WorkerService + 'static>,
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    cloud_service: &Arc<dyn CloudService>,
    verbosity: Level,
) -> HashMap<String, String> {
    EnvVarBuilder::golem_service(verbosity)
        .with_str("ENVIRONMENT", "local")
        .with_str("WASMTIME_BACKTRACE_DETAILS", "1")
        .with_str("GOLEM__KEY_VALUE_STORAGE__TYPE", "Redis")
        .with_str("GOLEM__INDEXED_STORAGE__TYPE", "KVStoreRedis")
        .with_str(
            "GOLEM__BLOB_STORAGE__CONFIG__ROOT",
            "/tmp/ittest-local-object-store/golem",
        )
        .with_str(
            "GOLEM__KEY_VALUE_STORAGE__CONFIG__HOST",
            &redis.private_host(),
        )
        .with_str(
            "GOLEM__KEY_VALUE_STORAGE__CONFIG__PORT",
            &redis.private_port().to_string(),
        )
        .with_str("GOLEM__KEY_VALUE_STORAGE__CONFIG__PREFIX", redis.prefix())
        .with_str("GOLEM__BLOB_STORAGE__TYPE", "LocalFileSystem")
        .with_str(
            "GOLEM__PUBLIC_WORKER_API__HOST",
            &worker_service.private_host(),
        )
        .with(
            "GOLEM__PUBLIC_WORKER_API__PORT",
            worker_service.private_grpc_port().to_string(),
        )
        .with(
            "GOLEM__PUBLIC_WORKER_API__ACCESS_TOKEN",
            cloud_service.admin_token().to_string(),
        )
        .with_str("GOLEM__COMPONENT_SERVICE__TYPE", "Grpc")
        .with_str(
            "GOLEM__COMPONENT_SERVICE__CONFIG__HOST",
            &component_service.private_host(),
        )
        .with(
            "GOLEM__COMPONENT_SERVICE__CONFIG__PORT",
            component_service.private_grpc_port().to_string(),
        )
        .with(
            "GOLEM__COMPONENT_SERVICE__CONFIG__ACCESS_TOKEN",
            cloud_service.admin_token().to_string(),
        )
        .with_str("GOLEM__PLUGIN_SERVICE__TYPE", "Grpc")
        .with_str(
            "GOLEM__PLUGIN_SERVICE__CONFIG__HOST",
            &component_service.private_host(),
        )
        .with(
            "GOLEM__PLUGIN_SERVICE__CONFIG__PORT",
            component_service.private_grpc_port().to_string(),
        )
        .with(
            "GOLEM__PLUGIN_SERVICE__CONFIG__ACCESS_TOKEN",
            cloud_service.admin_token().to_string(),
        )
        .with_str("GOLEM__COMPILED_COMPONENT_SERVICE__TYPE", "Enabled")
        .with_str("GOLEM__SHARD_MANAGER_SERVICE__TYPE", "Grpc")
        .with_str(
            "GOLEM__SHARD_MANAGER_SERVICE__CONFIG__HOST",
            &shard_manager.private_host(),
        )
        .with(
            "GOLEM__SHARD_MANAGER_SERVICE__CONFIG__PORT",
            shard_manager.private_grpc_port().to_string(),
        )
        .with_str(
            "GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MAX_ATTEMPTS",
            "5",
        )
        .with_str(
            "GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MIN_DELAY",
            "100ms",
        )
        .with_str(
            "GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MAX_DELAY",
            "2s",
        )
        .with_str(
            "GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MULTIPLIER",
            "2",
        )
        .with_str("GOLEM__RESOURCE_LIMITS__TYPE", "Disabled")
        .with_str("GOLEM__LIMITS__FUEL_TO_BORROW", "100000")
        .with_str("GOLEM__PROJECT_SERVICE__TYPE", "Grpc")
        .with(
            "GOLEM__PROJECT_SERVICE__CONFIG__HOST",
            cloud_service.private_host(),
        )
        .with(
            "GOLEM__PROJECT_SERVICE__CONFIG__PORT",
            cloud_service.private_grpc_port().to_string(),
        )
        .with(
            "GOLEM__PROJECT_SERVICE__CONFIG__ACCESS_TOKEN",
            cloud_service.admin_token().to_string(),
        )
        .with("GOLEM__PORT", grpc_port.to_string())
        .with("GOLEM__HTTP_PORT", http_port.to_string())
        .build()
}
