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
use golem_api_grpc::proto::golem::componentcompilation::{
    component_compilation_response, ComponentCompilationRequest,
};
use tonic::transport::Channel;
use tracing::Level;

use crate::components::component_service::ComponentService;
use crate::components::wait_for_startup_grpc;
use golem_api_grpc::proto::golem::componentcompilation::component_compilation_service_client::ComponentCompilationServiceClient;
use golem_common::model::ComponentId;

pub mod docker;
pub mod k8s;
pub mod provided;
pub mod spawned;

#[async_trait]
pub trait ComponentCompilationService {
    async fn client(&self) -> ComponentCompilationServiceClient<Channel> {
        new_client(&self.public_host(), self.public_grpc_port()).await
    }

    async fn enqueue_compilation(&self, component_id: &ComponentId, component_version: u64) {
        let response = self
            .client()
            .await
            .enqueue_compilation(ComponentCompilationRequest {
                component_id: Some(component_id.clone().into()),
                component_version,
            })
            .await
            .expect("Failed to enqueue component compilation")
            .into_inner();
        match response.result {
            None => {
                panic!("Missing response from golem-component-service for component compilation")
            }
            Some(component_compilation_response::Result::Success(_)) => (),
            Some(component_compilation_response::Result::Failure(error)) => {
                panic!("Failed to enqueue component compilation in golem-component-compilation-service: {error:?}");
            }
        }
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
}

async fn new_client(host: &str, grpc_port: u16) -> ComponentCompilationServiceClient<Channel> {
    ComponentCompilationServiceClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to golem-component-compilation-service")
}

async fn wait_for_startup(host: &str, grpc_port: u16, timeout: Duration) {
    wait_for_startup_grpc(
        host,
        grpc_port,
        "golem-component-compilation-service",
        timeout,
    )
    .await
}

fn env_vars(
    http_port: u16,
    grpc_port: u16,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    verbosity: Level,
) -> HashMap<String, String> {
    let log_level = verbosity.as_str().to_lowercase();

    let vars: &[(&str, &str)] = &[
        ("RUST_LOG", &format!("{log_level},cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn")),
        ("RUST_BACKTRACE", "1"),
        ("GOLEM__COMPILED_COMPONENT_SERVICE__TYPE", "Enabled"),
        ("GOLEM__BLOB_STORAGE__TYPE", "LocalFileSystem"),
        ("GOLEM__BLOB_STORAGE__CONFIG__ROOT", "/tmp/ittest-local-object-store/golem"),
        ("GOLEM__COMPONENT_SERVICE__ACCESS_TOKEN", "2A354594-7A63-4091-A46B-CC58D379F677"),
        ("GOLEM__COMPONENT_SERVICE__HOST", &component_service.private_host()),
        ("GOLEM__COMPONENT_SERVICE__PORT", &component_service.private_grpc_port().to_string()),
        ("GOLEM__GRPC_PORT", &grpc_port.to_string()),
        ("GOLEM__HTTP_PORT", &http_port.to_string()),
    ];

    HashMap::from_iter(vars.iter().map(|(k, v)| (k.to_string(), v.to_string())))
}
