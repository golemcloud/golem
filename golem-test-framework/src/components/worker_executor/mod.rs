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
use tonic::transport::Channel;
use tracing::Level;

use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;

use crate::components::component_service::ComponentService;
use crate::components::redis::Redis;
use crate::components::shard_manager::ShardManager;
use crate::components::wait_for_startup_grpc;
use crate::components::worker_service::WorkerService;

pub mod docker;
pub mod k8s;
pub mod provided;
pub mod spawned;

#[async_trait]
pub trait WorkerExecutor {
    async fn client(&self) -> WorkerExecutorClient<Channel> {
        new_client(&self.public_host(), self.public_grpc_port()).await
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

    fn kill(&self);
    async fn restart(&self);
}

async fn new_client(host: &str, grpc_port: u16) -> WorkerExecutorClient<Channel> {
    WorkerExecutorClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to golem-worker-executor")
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
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    verbosity: Level,
) -> HashMap<String, String> {
    let log_level = verbosity.as_str().to_lowercase();

    let vars: &[(&str, &str)] = &[
        ("RUST_LOG"                                      , &format!("{log_level},cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn")),
        ("ENVIRONMENT"                    , "local"),
        ("WASMTIME_BACKTRACE_DETAILS"                    , "1"),
        ("RUST_BACKTRACE"                                , "1"),
        ("GOLEM__REDIS__HOST"                            , &redis.private_host()),
        ("GOLEM__REDIS__PORT"                            , &redis.private_port().to_string()),
        ("GOLEM__REDIS__KEY_PREFIX"                      , redis.prefix()),
        ("GOLEM__PUBLIC_WORKER_API__HOST"                , &worker_service.private_host()),
        ("GOLEM__PUBLIC_WORKER_API__PORT"                , &worker_service.private_grpc_port().to_string()),
        ("GOLEM__PUBLIC_WORKER_API__ACCESS_TOKEN"        , "2A354594-7A63-4091-A46B-CC58D379F677"),
        ("GOLEM__COMPONENT_SERVICE__CONFIG__HOST"        , &component_service.private_host()),
        ("GOLEM__COMPONENT_SERVICE__CONFIG__PORT"        , &component_service.private_grpc_port().to_string()),
        ("GOLEM__COMPONENT_SERVICE__CONFIG__ACCESS_TOKEN", "2A354594-7A63-4091-A46B-CC58D379F677"),
        ("GOLEM__COMPILED_COMPONENT_SERVICE__TYPE"       , "Disabled"),
        ("GOLEM__BLOB_STORE_SERVICE__TYPE"               , "InMemory"),
        ("GOLEM__SHARD_MANAGER_SERVICE__TYPE"            , "Grpc"),
        ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__HOST"    , &shard_manager.private_host()),
        ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__PORT"    , &shard_manager.private_grpc_port().to_string()),
        ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MAX_ATTEMPTS"    , "5"),
        ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MIN_DELAY"    , "100ms"),
        ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MAX_DELAY"    , "2s"),
        ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MULTIPLIER"    , "2"),
        ("GOLEM__PORT"                                   , &grpc_port.to_string()),
        ("GOLEM__HTTP_PORT"                              , &http_port.to_string()),
    ];

    HashMap::from_iter(vars.iter().map(|(k, v)| (k.to_string(), v.to_string())))
}
