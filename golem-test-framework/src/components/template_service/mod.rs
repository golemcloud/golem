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

use golem_api_grpc::proto::golem::template::template_service_client::TemplateServiceClient;

use crate::components::rdb::Rdb;
use crate::components::wait_for_startup_grpc;

pub mod docker;
pub mod provided;
pub mod spawned;

#[async_trait]
pub trait TemplateService {
    async fn client(&self) -> TemplateServiceClient<Channel> {
        new_client(self.host(), self.grpc_port()).await
    }

    fn host(&self) -> &str;
    fn http_port(&self) -> u16;
    fn grpc_port(&self) -> u16;
    fn kill(&self);
}

async fn new_client(host: &str, grpc_port: u16) -> TemplateServiceClient<Channel> {
    TemplateServiceClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to golem-template-service")
}

async fn wait_for_startup(host: &str, grpc_port: u16) {
    wait_for_startup_grpc(host, grpc_port, "golem-template-service").await
}

fn env_vars(
    http_port: u16,
    grpc_port: u16,
    rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    verbosity: Level,
) -> HashMap<String, String> {
    let log_level = verbosity.as_str().to_lowercase();

    let vars: &[(&str, &str)] = &[
        ("RUST_LOG"                     , &format!("{log_level},cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn")),
        ("RUST_BACKTRACE"               , "1"),
        ("GOLEM__TEMPLATE_STORE__TYPE", "Local"),
        ("GOLEM__TEMPLATE_STORE__CONFIG__OBJECT_PREFIX", ""),
        ("GOLEM__TEMPLATE_STORE__CONFIG__ROOT_PATH", "/tmp/ittest-local-object-store/golem"),
        ("GOLEM__GRPC_PORT", &grpc_port.to_string()),
        ("GOLEM__HTTP_PORT", &http_port.to_string()),
    ];

    let mut vars: HashMap<String, String> =
        HashMap::from_iter(vars.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    vars.extend(rdb.info().env().clone());
    vars
}
