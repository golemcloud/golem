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

use async_trait::async_trait;
use tonic::transport::Channel;
use tracing::Level;

use golem_api_grpc::proto::golem::worker::worker_service_client::WorkerServiceClient;

use crate::components::rdb::Rdb;
use crate::components::redis::Redis;
use crate::components::shard_manager::ShardManager;
use crate::components::template_service::TemplateService;
use crate::components::wait_for_startup_grpc;

pub mod docker;
pub mod provided;
pub mod spawned;

#[async_trait]
pub trait WorkerService {
    async fn client(&self) -> WorkerServiceClient<Channel> {
        new_client(self.public_host(), self.public_grpc_port()).await
    }

    fn private_host(&self) -> &str;
    fn private_http_port(&self) -> u16;
    fn private_grpc_port(&self) -> u16;
    fn private_custom_request_port(&self) -> u16;

    fn public_host(&self) -> &str {
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

async fn new_client(host: &str, grpc_port: u16) -> WorkerServiceClient<Channel> {
    WorkerServiceClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to golem-worker-service")
}

async fn wait_for_startup(host: &str, grpc_port: u16) {
    wait_for_startup_grpc(host, grpc_port, "golem-worker-service").await
}

fn env_vars(
    http_port: u16,
    grpc_port: u16,
    custom_request_port: u16,
    template_service: Arc<dyn TemplateService + Send + Sync + 'static>,
    shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
    rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    verbosity: Level,
) -> HashMap<String, String> {
    let log_level = verbosity.as_str().to_lowercase();

    let vars: &[(&str, &str)] = &[
        ("RUST_LOG"                                   , &format!("{log_level},cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn")),
        ("RUST_BACKTRACE"                             , "1"),
        ("GOLEM__REDIS__HOST"                         , redis.private_host()),
        ("GOLEM__REDIS__PORT"                         , &redis.private_port().to_string()),
        ("GOLEM__REDIS__DATABASE"                     , "1"),
        ("GOLEM__TEMPLATE_SERVICE__HOST"              , template_service.private_host()),
        ("GOLEM__TEMPLATE_SERVICE__PORT"              , &template_service.private_grpc_port().to_string()),
        ("GOLEM__TEMPLATE_SERVICE__ACCESS_TOKEN"      , "5C832D93-FF85-4A8F-9803-513950FDFDB1"),
        ("ENVIRONMENT"                                , "local"),
        ("GOLEM__ENVIRONMENT"                         , "ittest"),
        ("GOLEM__ROUTING_TABLE__HOST"                 , &shard_manager.private_host()),
        ("GOLEM__ROUTING_TABLE__PORT"                 , &shard_manager.private_grpc_port().to_string()),
        ("GOLEM__CUSTOM_REQUEST_PORT"                 , &custom_request_port.to_string()),
        ("GOLEM__WORKER_GRPC_PORT"                    , &grpc_port.to_string()),
        ("GOLEM__PORT"                                , &http_port.to_string()),

    ];

    let mut vars: HashMap<String, String> =
        HashMap::from_iter(vars.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    vars.extend(rdb.info().env().clone());
    vars
}
