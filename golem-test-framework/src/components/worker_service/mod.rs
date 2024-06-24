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
use tonic::Streaming;
use tracing::Level;

use golem_api_grpc::proto::golem::worker::worker_service_client::WorkerServiceClient;
use golem_api_grpc::proto::golem::worker::{
    ConnectWorkerRequest, DeleteWorkerRequest, DeleteWorkerResponse, GetWorkerMetadataRequest,
    GetWorkerMetadataResponse, GetWorkersMetadataRequest, GetWorkersMetadataResponse,
    InterruptWorkerRequest, InterruptWorkerResponse, InvokeAndAwaitRequest, InvokeAndAwaitResponse,
    InvokeRequest, InvokeResponse, LaunchNewWorkerRequest, LaunchNewWorkerResponse, LogEvent,
    ResumeWorkerRequest, ResumeWorkerResponse, UpdateWorkerRequest, UpdateWorkerResponse,
};

use crate::components::component_service::ComponentService;
use crate::components::rdb::Rdb;
use crate::components::shard_manager::ShardManager;
use crate::components::wait_for_startup_grpc;

pub mod docker;
pub mod forwarding;
pub mod k8s;
pub mod provided;
pub mod spawned;

#[async_trait]
pub trait WorkerService {
    async fn client(&self) -> WorkerServiceClient<Channel> {
        new_client(&self.public_host(), self.public_grpc_port()).await
    }

    // Overridable client functions - using these instead of client() allows
    // testing worker executors directly without the need to start a worker service,
    // when the `WorkerService` implementation is `ForwardingWorkerService`.
    async fn create_worker(&self, request: LaunchNewWorkerRequest) -> LaunchNewWorkerResponse {
        self.client()
            .await
            .launch_new_worker(request)
            .await
            .expect("Failed to call golem-worker-service")
            .into_inner()
    }

    async fn delete_worker(&self, request: DeleteWorkerRequest) -> DeleteWorkerResponse {
        self.client()
            .await
            .delete_worker(request)
            .await
            .expect("Failed to call golem-worker-service")
            .into_inner()
    }

    async fn get_worker_metadata(
        &self,
        request: GetWorkerMetadataRequest,
    ) -> GetWorkerMetadataResponse {
        self.client()
            .await
            .get_worker_metadata(request)
            .await
            .expect("Failed to call golem-worker-service")
            .into_inner()
    }

    async fn get_workers_metadata(
        &self,
        request: GetWorkersMetadataRequest,
    ) -> GetWorkersMetadataResponse {
        self.client()
            .await
            .get_workers_metadata(request)
            .await
            .expect("Failed to call golem-worker-service")
            .into_inner()
    }

    async fn invoke(&self, request: InvokeRequest) -> InvokeResponse {
        self.client()
            .await
            .invoke(request)
            .await
            .expect("Failed to call golem-worker-service")
            .into_inner()
    }

    async fn invoke_and_await(&self, request: InvokeAndAwaitRequest) -> InvokeAndAwaitResponse {
        self.client()
            .await
            .invoke_and_await(request)
            .await
            .expect("Failed to call golem-worker-service")
            .into_inner()
    }

    async fn connect_worker(&self, request: ConnectWorkerRequest) -> Streaming<LogEvent> {
        self.client()
            .await
            .connect_worker(request)
            .await
            .expect("Failed to call golem-worker-service")
            .into_inner()
    }

    async fn resume_worker(&self, request: ResumeWorkerRequest) -> ResumeWorkerResponse {
        self.client()
            .await
            .resume_worker(request)
            .await
            .expect("Failed to call golem-worker-service")
            .into_inner()
    }

    async fn interrupt_worker(&self, request: InterruptWorkerRequest) -> InterruptWorkerResponse {
        self.client()
            .await
            .interrupt_worker(request)
            .await
            .expect("Failed to call golem-worker-service")
            .into_inner()
    }

    async fn update_worker(&self, request: UpdateWorkerRequest) -> UpdateWorkerResponse {
        self.client()
            .await
            .update_worker(request)
            .await
            .expect("Failed to call golem-worker-service")
            .into_inner()
    }

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

async fn new_client(host: &str, grpc_port: u16) -> WorkerServiceClient<Channel> {
    WorkerServiceClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to golem-worker-service")
}

async fn wait_for_startup(host: &str, grpc_port: u16, timeout: Duration) {
    wait_for_startup_grpc(host, grpc_port, "golem-worker-service", timeout).await
}

fn env_vars(
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
        ("GOLEM__COMPONENT_SERVICE__HOST"             , &component_service.private_host()),
        ("GOLEM__COMPONENT_SERVICE__PORT"             , &component_service.private_grpc_port().to_string()),
        ("GOLEM__COMPONENT_SERVICE__ACCESS_TOKEN"     , "5C832D93-FF85-4A8F-9803-513950FDFDB1"),
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
    vars.extend(rdb.info().env("golem_worker").clone());
    vars
}
