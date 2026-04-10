// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::rdb::{DbInfo, Rdb};
use super::registry_service::RegistryService;
use super::shard_manager::ShardManager;
use super::worker_service::WorkerService;
use super::{EnvVarBuilder, wait_for_startup_grpc};
use async_trait::async_trait;
use std::collections::HashMap;
use std::process::Child;
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

async fn wait_for_startup(
    host: &str,
    grpc_port: u16,
    timeout: Duration,
    child: Option<&mut Child>,
) {
    wait_for_startup_grpc(host, grpc_port, "golem-worker-executor", timeout, child).await
}

async fn env_vars(
    http_port: u16,
    grpc_port: u16,
    shard_manager: &Arc<dyn ShardManager>,
    worker_service: &Arc<dyn WorkerService>,
    rdb: &Arc<dyn Rdb>,
    registry_service: &Arc<dyn RegistryService>,
    environment_state_cache_capacity: Option<usize>,
    verbosity: Level,
    otlp: bool,
) -> HashMap<String, String> {
    let mut env = EnvVarBuilder::golem_service(verbosity)
        .with_str("ENVIRONMENT", "local")
        .with_str("WASMTIME_BACKTRACE_DETAILS", "1")
        .with_str(
            "GOLEM__BLOB_STORAGE__CONFIG__ROOT",
            "/tmp/ittest-local-object-store/golem",
        )
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
        .with_str("GOLEM__SHARD_MANAGER__HOST", &shard_manager.grpc_host())
        .with(
            "GOLEM__SHARD_MANAGER__PORT",
            shard_manager.grpc_port().to_string(),
        )
        .with_str("GOLEM__SHARD_MANAGER__RETRIES__MAX_ATTEMPTS", "5")
        .with_str("GOLEM__SHARD_MANAGER__RETRIES__MIN_DELAY", "100ms")
        .with_str("GOLEM__SHARD_MANAGER__RETRIES__MAX_DELAY", "2s")
        .with_str("GOLEM__SHARD_MANAGER__RETRIES__MULTIPLIER", "2")
        .with_str("GOLEM__LIMITS__FUEL_TO_BORROW", "100000")
        .with_str(
            "GOLEM__AGENT_WEBHOOKS_SERVICE__USE_HTTPS_FOR_WEBHOOK_URL",
            "false",
        )
        .with_str("GOLEM__QUOTA_SERVICE__INLINE_WAIT_THRESHOLD", "30s")
        .with_str("GOLEM__QUOTA_SERVICE__RENEWAL_INTERVAL", "1s")
        .with("GOLEM__GRPC__PORT", grpc_port.to_string())
        .with("GOLEM__HTTP_PORT", http_port.to_string())
        .with_optional_otlp("worker_executor", otlp)
        .build();

    if let Some(environment_state_cache_capacity) = environment_state_cache_capacity {
        env.insert(
            "GOLEM__ENVIRONMENT_STATE_SERVICE__CACHE_CAPACITY".to_string(),
            environment_state_cache_capacity.to_string(),
        );
    }

    let db_env = rdb.info().env("golem_worker_executor", false);
    let db_type = db_env
        .get("GOLEM__DB__TYPE")
        .map(String::as_str)
        .expect("worker executor storage requires GOLEM__DB__TYPE");

    env.insert(
        "GOLEM__KEY_VALUE_STORAGE__TYPE".to_string(),
        db_type.to_string(),
    );

    match rdb.info() {
        DbInfo::Postgres(_) => {
            env.insert(
                "GOLEM__INDEXED_STORAGE__TYPE".to_string(),
                "Postgres".to_string(),
            );
            for (key, value) in &db_env {
                if let Some(rest) = key.strip_prefix("GOLEM__DB__CONFIG__") {
                    let indexed_value = if rest == "SCHEMA" {
                        format!("{value}_indexed")
                    } else {
                        value.clone()
                    };
                    env.insert(
                        format!("GOLEM__INDEXED_STORAGE__CONFIG__{rest}"),
                        indexed_value,
                    );
                }
            }
        }
        DbInfo::Sqlite(_) => {
            env.insert(
                "GOLEM__INDEXED_STORAGE__TYPE".to_string(),
                "KVStoreSqlite".to_string(),
            );
        }
        DbInfo::Mysql(_) => {
            panic!("Mysql-backed worker executor storage is not supported in the test framework");
        }
    }

    for (key, value) in db_env {
        if key == "GOLEM__DB__TYPE" {
            continue;
        }

        if let Some(rest) = key.strip_prefix("GOLEM__DB__") {
            env.insert(format!("GOLEM__KEY_VALUE_STORAGE__{rest}"), value);
        }
    }

    env
}
