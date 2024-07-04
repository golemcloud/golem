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
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tracing::Level;

use crate::components::cloud_service::CloudService;
use crate::components::component_service::ComponentService;
use crate::components::redis::Redis;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_service::WorkerService;
use crate::components::{wait_for_startup_grpc, ROOT_TOKEN};

pub mod spawned;

#[async_trait]
pub trait WorkerExecutor {
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
    async fn restart(&self);
}

async fn wait_for_startup(host: &str, grpc_port: u16, timeout: Duration) {
    wait_for_startup_grpc(host, grpc_port, "golem-worker-executor", timeout).await
}

fn env_vars(
    http_port: u16,
    grpc_port: u16,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
    worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
    cloud: Arc<dyn CloudService + Send + Sync + 'static>,
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    verbosity: Level,
) -> HashMap<String, String> {
    let log_level = verbosity.as_str().to_lowercase();

    let vars: &[(&str, &str)] = &[
        ("RUST_LOG"                                      , &format!("{log_level},cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn")),
        ("ENVIRONMENT"                    , "local"),
        ("WASMTIME_BACKTRACE_DETAILS"                    , "1"),
        ("RUST_BACKTRACE"                                , "1"),
        ("GOLEM__KEY_VALUE_STORAGE__TYPE"                , "Redis"),
        ("GOLEM__INDEXED_STORAGE__TYPE"                  , "KVStoreRedis"),
        ("GOLEM__BLOB_STORAGE__CONFIG__ROOT", "/tmp/ittest-local-object-store/golem"),
        ("GOLEM__KEY_VALUE_STORAGE__CONFIG__HOST"        , &redis.private_host()),
        ("GOLEM__KEY_VALUE_STORAGE__CONFIG__PORT"        , &redis.private_port().to_string()),
        ("GOLEM__KEY_VALUE_STORAGE__CONFIG__PREFIX"      , redis.prefix()),
        ("GOLEM__BLOB_STORAGE__TYPE"                     , "LocalFileSystem"),
        ("GOLEM__PUBLIC_WORKER_API__HOST"                , &worker_service.private_host()),
        ("GOLEM__PUBLIC_WORKER_API__PORT"                , &worker_service.private_grpc_port().to_string()),
        ("GOLEM__PUBLIC_WORKER_API__ACCESS_TOKEN"        , ROOT_TOKEN),
        ("GOLEM__COMPONENT_SERVICE__CONFIG__HOST"        , &component_service.private_host()),
        ("GOLEM__COMPONENT_SERVICE__CONFIG__PORT"        , &component_service.private_grpc_port().to_string()),
        ("GOLEM__COMPONENT_SERVICE__CONFIG__ACCESS_TOKEN", ROOT_TOKEN),
        ("GOLEM__COMPILED_COMPONENT_SERVICE__TYPE"       , "Enabled"),
        ("GOLEM__SHARD_MANAGER_SERVICE__TYPE"            , "Grpc"),
        ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__HOST"    , &shard_manager.private_host()),
        ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__PORT"    , &shard_manager.private_grpc_port().to_string()),
        ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MAX_ATTEMPTS"    , "5"),
        ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MIN_DELAY"    , "100ms"),
        ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MAX_DELAY"    , "2s"),
        ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MULTIPLIER"    , "2"),
        ("GOLEM__PORT"                                   , &grpc_port.to_string()),
        ("GOLEM__HTTP_PORT"                              , &http_port.to_string()),
        ("GOLEM__RESOURCE_LIMITS__CONFIG__HOST", &cloud.private_host()),
        ("GOLEM__RESOURCE_LIMITS__CONFIG__PORT", &cloud.private_grpc_port().to_string()),
        ("GOLEM__RESOURCE_LIMITS__CONFIG__ACCESS_TOKEN"     , ROOT_TOKEN),
    ];

    HashMap::from_iter(vars.iter().map(|(k, v)| (k.to_string(), v.to_string())))
}
