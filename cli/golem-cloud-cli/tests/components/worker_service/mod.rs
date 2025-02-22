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
use golem_test_framework::components::component_service::ComponentService;
use golem_test_framework::components::rdb::Rdb;
use golem_test_framework::components::shard_manager::ShardManager;
use golem_test_framework::components::worker_service::WorkerServiceEnvVars;
use tracing::Level;

use crate::components::rdb::CloudDbInfo;
use crate::components::{CloudEnvVars, ROOT_TOKEN};

#[async_trait]
impl WorkerServiceEnvVars for CloudEnvVars {
    async fn env_vars(
        &self,
        http_port: u16,
        grpc_port: u16,
        custom_request_port: u16,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
    ) -> HashMap<String, String> {
        let log_level = verbosity.as_str().to_lowercase();

        let vars: &[(&str, &str)] = &[
            ("RUST_LOG"                                   , &format!("{log_level},cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn")),
            ("RUST_BACKTRACE"                             , "1"),
            ("GOLEM__REDIS__HOST"                         , &self.redis.private_host()),
            ("GOLEM__REDIS__PORT"                         , &self.redis.private_port().to_string()),
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
            ("GOLEM__CLOUD_SERVICE__HOST", &self.cloud_service.private_host()),
            ("GOLEM__CLOUD_SERVICE__PORT", &self.cloud_service.private_grpc_port().to_string()),
            ("GOLEM__CLOUD_SERVICE__ACCESS_TOKEN"     , ROOT_TOKEN),
        ];

        let mut vars: HashMap<String, String> =
            HashMap::from_iter(vars.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        vars.extend(rdb.info().cloud_env("worker_service").await.clone());
        vars
    }
}
