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
use crate::components::rdb::Rdb;
use crate::components::redis::Redis;
use crate::components::shard_manager::ShardManager;
use crate::components::{wait_for_startup_grpc, ROOT_TOKEN};

pub mod spawned;

#[async_trait]
pub trait WorkerService {
    fn private_host(&self) -> String;
    fn private_http_port(&self) -> u16;
    fn private_grpc_port(&self) -> u16;
    fn private_custom_request_port(&self) -> u16;

    fn public_host(&self) -> String {
        self.private_host()
    }

    fn public_http_port(&self) -> u16 {
        self.private_http_port()
    }

    fn public_grpc_port(&self) -> u16 {
        self.private_grpc_port()
    }

    fn public_custom_request_port(&self) -> u16 {
        self.private_custom_request_port()
    }

    fn kill(&self);
}

async fn wait_for_startup(host: &str, grpc_port: u16, timeout: Duration) {
    wait_for_startup_grpc(host, grpc_port, "cloud-worker-service", timeout).await
}

fn env_vars(
    http_port: u16,
    grpc_port: u16,
    custom_request_port: u16,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
    cloud: Arc<dyn CloudService + Send + Sync + 'static>,
    rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    verbosity: Level,
) -> HashMap<String, String> {
    let log_level = verbosity.as_str().to_lowercase();

    let vars: &[(&str, &str)] = &[
        ("RUST_LOG"                                   , &format!("{log_level},cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn")),
        ("RUST_BACKTRACE"                             , "1"),
        ("GOLEM__REDIS__HOST"                         , &redis.private_host()),
        ("GOLEM__REDIS__PORT"                         , &redis.private_port().to_string()),
        ("GOLEM__REDIS__DATABASE"                     , "1"),
        ("GOLEM__COMPONENT_SERVICE__HOST"             , &component_service.private_host()),
        ("GOLEM__COMPONENT_SERVICE__PORT"             , &component_service.private_grpc_port().to_string()),
        ("GOLEM__COMPONENT_SERVICE__ACCESS_TOKEN"     , ROOT_TOKEN),
        ("ENVIRONMENT"                                , "local"),
        ("GOLEM__ENVIRONMENT"                         , "local"),
        ("GOLEM__WORKSPACE", "it"),
        ("GOLEM__ROUTING_TABLE__HOST"                 , &shard_manager.private_host()),
        ("GOLEM__ROUTING_TABLE__PORT"                 , &shard_manager.private_grpc_port().to_string()),
        ("GOLEM__CUSTOM_REQUEST_PORT"                 , &custom_request_port.to_string()),
        ("GOLEM__WORKER_GRPC_PORT"                    , &grpc_port.to_string()),
        ("GOLEM__PORT"                                , &http_port.to_string()),
        ("GOLEM__DOMAIN_RECORDS__DOMAIN_ALLOW_LIST", "[]"),
        ("GOLEM__CLOUD_SERVICE__HOST", &cloud.private_host()),
        ("GOLEM__CLOUD_SERVICE__PORT", &cloud.private_grpc_port().to_string()),
        ("GOLEM__CLOUD_SERVICE__ACCESS_TOKEN"     , ROOT_TOKEN),
    ];

    let mut vars: HashMap<String, String> =
        HashMap::from_iter(vars.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    vars.extend(rdb.info().env("worker_service").clone());
    vars
}
