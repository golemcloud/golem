// Copyright 2024-2025 Golem Cloud
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
use bytes::Bytes;
use tonic::codec::CompressionEncoding;
use tonic::transport::{Channel, Endpoint};
use tonic::Streaming;
use tracing::Level;

use anyhow::anyhow;
use golem_api_grpc::proto::golem::worker::v1::worker_service_client::WorkerServiceClient;
use golem_api_grpc::proto::golem::worker::v1::{
    get_file_contents_response, ConnectWorkerRequest, DeleteWorkerRequest, DeleteWorkerResponse,
    ForkWorkerRequest, ForkWorkerResponse, GetFileContentsRequest, GetOplogRequest,
    GetOplogResponse, GetWorkerMetadataRequest, GetWorkerMetadataResponse,
    GetWorkersMetadataRequest, GetWorkersMetadataResponse, InterruptWorkerRequest,
    InterruptWorkerResponse, InvokeAndAwaitJsonRequest, InvokeAndAwaitJsonResponse,
    InvokeAndAwaitRequest, InvokeAndAwaitResponse, InvokeJsonRequest, InvokeRequest,
    InvokeResponse, LaunchNewWorkerRequest, LaunchNewWorkerResponse, ListDirectoryRequest,
    ListDirectoryResponse, ResumeWorkerRequest, ResumeWorkerResponse, SearchOplogRequest,
    SearchOplogResponse, UpdateWorkerRequest, UpdateWorkerResponse,
};
use golem_api_grpc::proto::golem::worker::LogEvent;

use crate::components::component_service::ComponentService;
use crate::components::rdb::Rdb;
use crate::components::shard_manager::ShardManager;
use crate::components::{wait_for_startup_grpc, EnvVarBuilder, GolemEnvVars};

pub mod docker;
pub mod forwarding;
pub mod k8s;
pub mod provided;
pub mod spawned;

#[async_trait]
pub trait WorkerService {
    async fn client(&self) -> crate::Result<WorkerServiceClient<Channel>>;

    // Overridable client functions - using these instead of client() allows
    // testing worker executors directly without the need to start a worker service,
    // when the `WorkerService` implementation is `ForwardingWorkerService`.
    async fn create_worker(
        &self,
        request: LaunchNewWorkerRequest,
    ) -> crate::Result<LaunchNewWorkerResponse> {
        Ok(self
            .client()
            .await?
            .launch_new_worker(request)
            .await?
            .into_inner())
    }

    async fn delete_worker(
        &self,
        request: DeleteWorkerRequest,
    ) -> crate::Result<DeleteWorkerResponse> {
        Ok(self
            .client()
            .await?
            .delete_worker(request)
            .await?
            .into_inner())
    }

    async fn get_worker_metadata(
        &self,
        request: GetWorkerMetadataRequest,
    ) -> crate::Result<GetWorkerMetadataResponse> {
        Ok(self
            .client()
            .await?
            .get_worker_metadata(request)
            .await?
            .into_inner())
    }

    async fn get_workers_metadata(
        &self,
        request: GetWorkersMetadataRequest,
    ) -> crate::Result<GetWorkersMetadataResponse> {
        Ok(self
            .client()
            .await?
            .get_workers_metadata(request)
            .await?
            .into_inner())
    }

    async fn invoke(&self, request: InvokeRequest) -> crate::Result<InvokeResponse> {
        Ok(self.client().await?.invoke(request).await?.into_inner())
    }

    async fn invoke_json(&self, request: InvokeJsonRequest) -> crate::Result<InvokeResponse> {
        Ok(self
            .client()
            .await?
            .invoke_json(request)
            .await?
            .into_inner())
    }

    async fn invoke_and_await(
        &self,
        request: InvokeAndAwaitRequest,
    ) -> crate::Result<InvokeAndAwaitResponse> {
        Ok(self
            .client()
            .await?
            .invoke_and_await(request)
            .await?
            .into_inner())
    }

    async fn invoke_and_await_json(
        &self,
        request: InvokeAndAwaitJsonRequest,
    ) -> crate::Result<InvokeAndAwaitJsonResponse> {
        Ok(self
            .client()
            .await?
            .invoke_and_await_json(request)
            .await?
            .into_inner())
    }

    async fn connect_worker(
        &self,
        request: ConnectWorkerRequest,
    ) -> crate::Result<Streaming<LogEvent>> {
        Ok(self
            .client()
            .await?
            .connect_worker(request)
            .await?
            .into_inner())
    }

    async fn resume_worker(
        &self,
        request: ResumeWorkerRequest,
    ) -> crate::Result<ResumeWorkerResponse> {
        Ok(self
            .client()
            .await?
            .resume_worker(request)
            .await?
            .into_inner())
    }

    async fn interrupt_worker(
        &self,
        request: InterruptWorkerRequest,
    ) -> crate::Result<InterruptWorkerResponse> {
        Ok(self
            .client()
            .await?
            .interrupt_worker(request)
            .await?
            .into_inner())
    }

    async fn update_worker(
        &self,
        request: UpdateWorkerRequest,
    ) -> crate::Result<UpdateWorkerResponse> {
        Ok(self
            .client()
            .await?
            .update_worker(request)
            .await?
            .into_inner())
    }

    async fn get_oplog(&self, request: GetOplogRequest) -> crate::Result<GetOplogResponse> {
        Ok(self.client().await?.get_oplog(request).await?.into_inner())
    }

    async fn search_oplog(
        &self,
        request: SearchOplogRequest,
    ) -> crate::Result<SearchOplogResponse> {
        Ok(self
            .client()
            .await?
            .search_oplog(request)
            .await?
            .into_inner())
    }

    async fn list_directory(
        &self,
        request: ListDirectoryRequest,
    ) -> crate::Result<ListDirectoryResponse> {
        Ok(self
            .client()
            .await?
            .list_directory(request)
            .await?
            .into_inner())
    }

    async fn get_file_contents(&self, request: GetFileContentsRequest) -> crate::Result<Bytes> {
        let mut stream = self
            .client()
            .await?
            .get_file_contents(request)
            .await?
            .into_inner();

        let mut bytes = Vec::new();
        while let Some(chunk) = stream.message().await? {
            match chunk.result {
                Some(get_file_contents_response::Result::Success(data)) => {
                    bytes.extend_from_slice(&data);
                }
                Some(get_file_contents_response::Result::Error(err)) => {
                    return Err(anyhow!("Error from get_file_contents: {err:?}"));
                }
                None => {
                    return Err(anyhow!("Unexpected response from get_file_contents"));
                }
            }
        }
        Ok(Bytes::from(bytes))
    }

    async fn fork_worker(
        &self,
        fork_worker_request: ForkWorkerRequest,
    ) -> crate::Result<ForkWorkerResponse> {
        let response = self
            .client()
            .await?
            .fork_worker(fork_worker_request)
            .await?;

        Ok(response.into_inner())
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

    async fn kill(&self);
}

async fn new_client(
    host: &str,
    grpc_port: u16,
) -> Result<WorkerServiceClient<Channel>, tonic::transport::Error> {
    let endpoint = Endpoint::new(format!("http://{host}:{grpc_port}"))?
        .connect_timeout(Duration::from_secs(10));
    let channel = endpoint.connect().await?;
    Ok(WorkerServiceClient::new(channel)
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip))
}

async fn wait_for_startup(host: &str, grpc_port: u16, timeout: Duration) {
    wait_for_startup_grpc(host, grpc_port, "golem-worker-service", timeout).await
}

#[async_trait]
pub trait WorkerServiceEnvVars {
    async fn env_vars(
        &self,
        http_port: u16,
        grpc_port: u16,
        custom_request_port: u16,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        verbosity: Level,
    ) -> HashMap<String, String>;
}

#[async_trait]
impl WorkerServiceEnvVars for GolemEnvVars {
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
        EnvVarBuilder::golem_service(verbosity)
            .with_str("GOLEM__BLOB_STORAGE__TYPE", "LocalFileSystem")
            .with_str(
                "GOLEM__BLOB_STORAGE__CONFIG__ROOT",
                "/tmp/ittest-local-object-store/golem",
            )
            .with(
                "GOLEM__COMPONENT_SERVICE__HOST",
                component_service.private_host(),
            )
            .with(
                "GOLEM__COMPONENT_SERVICE__PORT",
                component_service.private_grpc_port().to_string(),
            )
            .with_str(
                "GOLEM__COMPONENT_SERVICE__ACCESS_TOKEN",
                "5C832D93-FF85-4A8F-9803-513950FDFDB1",
            )
            .with_str("ENVIRONMENT", "local")
            .with_str("GOLEM__ENVIRONMENT", "ittest")
            .with("GOLEM__ROUTING_TABLE__HOST", shard_manager.private_host())
            .with(
                "GOLEM__ROUTING_TABLE__PORT",
                shard_manager.private_grpc_port().to_string(),
            )
            .with(
                "GOLEM__CUSTOM_REQUEST_PORT",
                custom_request_port.to_string(),
            )
            .with("GOLEM__WORKER_GRPC_PORT", grpc_port.to_string())
            .with("GOLEM__PORT", http_port.to_string())
            .with_all(rdb.info().env("golem_worker"))
            .build()
    }
}
