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

use super::redis::Redis;
use super::registry_service::RegistryService;
use super::shard_manager::ShardManager;
use super::worker_service::WorkerService;
use super::{wait_for_startup_grpc, EnvVarBuilder};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::Level;

pub mod provided;
pub mod spawned;

#[async_trait]
pub trait WorkerExecutor: Send + Sync {
    fn grpc_host(&self) -> String;

    fn grpc_port(&self) -> u16;

    async fn kill(&self);

    async fn restart(&self);

    async fn is_running(&self) -> bool;
}

async fn wait_for_startup(host: &str, grpc_port: u16, timeout: Duration) {
    wait_for_startup_grpc(host, grpc_port, "golem-worker-executor", timeout).await
}

async fn env_vars(
    http_port: u16,
    grpc_port: u16,
    shard_manager: &Arc<dyn ShardManager>,
    worker_service: &Arc<dyn WorkerService>,
    redis: &Arc<dyn Redis>,
    registry_service: &Arc<dyn RegistryService>,
    verbosity: Level,
    otlp: bool,
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
            &worker_service.grpc_host(),
        )
        .with(
            "GOLEM__PUBLIC_WORKER_API__PORT",
            worker_service.gprc_port().to_string(),
        )
        .with_str(
            "GOLEM__REGISTRY_SERVICE__HOST",
            &registry_service.grpc_host(),
        )
        .with(
            "GOLEM__REGISTRY_SERVICE__PORT",
            registry_service.grpc_port().to_string(),
        )
        .with_str("GOLEM__COMPILED_COMPONENT_SERVICE__TYPE", "Enabled")
        .with_str("GOLEM__SHARD_MANAGER_SERVICE__TYPE", "Grpc")
        .with_str(
            "GOLEM__SHARD_MANAGER_SERVICE__CONFIG__HOST",
            &shard_manager.grpc_host(),
        )
        .with(
            "GOLEM__SHARD_MANAGER_SERVICE__CONFIG__PORT",
            shard_manager.grpc_port().to_string(),
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
        .with_str("GOLEM__LIMITS__FUEL_TO_BORROW", "100000")
        .with_str("GOLEM__AGENT_DEPLOYMENTS_SERVICE__CACHE_CAPACITY", "0")
        .with_str(
            "GOLEM__AGENT_DEPLOYMENTS_SERVICE__USE_HTTPS_FOR_WEBHOOK_URL",
            "false",
        )
        .with("GOLEM__GRPC__PORT", grpc_port.to_string())
        .with("GOLEM__HTTP_PORT", http_port.to_string())
        .with_optional_otlp("worker_executor", otlp)
        .build()
}
