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

use crate::components::component_service::ComponentService;
use crate::components::rdb::Rdb;
use crate::components::shard_manager::ShardManager;
use crate::components::{
    new_reqwest_client, wait_for_startup_grpc, wait_for_startup_http, EnvVarBuilder, GolemEnvVars,
};
use crate::config::GolemClientProtocol;
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::SplitStream;
use futures_util::{SinkExt, StreamExt};
use golem_api_grpc::proto::golem::common::{
    AccountId, Empty, FilterComparator, PluginInstallationId, StringFilterComparator,
};
use golem_api_grpc::proto::golem::component::ComponentFilePermissions;
use golem_api_grpc::proto::golem::worker::v1::worker_service_client::WorkerServiceClient as WorkerServiceGrpcClient;
use golem_api_grpc::proto::golem::worker::v1::{
    delete_worker_response, get_file_contents_response, get_oplog_response,
    get_worker_metadata_response, get_workers_metadata_response, interrupt_worker_response,
    invoke_and_await_json_response, invoke_and_await_response, invoke_response,
    launch_new_worker_response, list_directory_response, resume_worker_response,
    search_oplog_response, update_worker_response, ConnectWorkerRequest, DeleteWorkerRequest,
    DeleteWorkerResponse, ForkWorkerRequest, ForkWorkerResponse, GetFileContentsRequest,
    GetOplogRequest, GetOplogResponse, GetOplogSuccessResponse, GetWorkerMetadataRequest,
    GetWorkerMetadataResponse, GetWorkersMetadataRequest, GetWorkersMetadataResponse,
    GetWorkersMetadataSuccessResponse, InterruptWorkerRequest, InterruptWorkerResponse,
    InvokeAndAwaitJsonRequest, InvokeAndAwaitJsonResponse, InvokeAndAwaitRequest,
    InvokeAndAwaitResponse, InvokeJsonRequest, InvokeRequest, InvokeResponse,
    LaunchNewWorkerRequest, LaunchNewWorkerResponse, LaunchNewWorkerSuccessResponse,
    ListDirectoryRequest, ListDirectoryResponse, ListDirectorySuccessResponse, ResumeWorkerRequest,
    ResumeWorkerResponse, SearchOplogRequest, SearchOplogResponse, SearchOplogSuccessResponse,
    UpdateWorkerRequest, UpdateWorkerResponse,
};
use golem_api_grpc::proto::golem::worker::worker_filter::Filter;
use golem_api_grpc::proto::golem::worker::{
    file_system_node, update_record, Cursor, DirectoryFileSystemNode, FailedUpdate,
    FileFileSystemNode, FileSystemNode, IdempotencyKey, IndexedResourceMetadata, InvocationContext,
    InvokeParameters, InvokeResult, LogEvent, OplogCursor, OplogEntry, OplogEntryWithIndex,
    PendingUpdate, ResourceMetadata, SuccessfulUpdate, TargetWorkerId, UpdateMode, UpdateRecord,
    WorkerCreatedAtFilter, WorkerEnvFilter, WorkerMetadata, WorkerNameFilter, WorkerStatusFilter,
    WorkerVersionFilter,
};
use golem_client::api::WorkerClient as WorkerServiceHttpClient;
use golem_client::api::WorkerClientLive as WorkerServiceHttpClientLive;
use golem_client::Context;
use golem_common::model::WorkerEvent;
use golem_wasm_rpc::{Value, ValueAndType};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::net::TcpStream;
use tokio::{task, time};
use tokio_tungstenite::tungstenite::protocol::frame::Payload;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use tonic::codec::CompressionEncoding;
use tonic::transport::{Channel, Endpoint};
use tonic::Streaming;
use tracing::Level;
use url::Url;
use uuid::Uuid;

pub mod docker;
pub mod forwarding;
pub mod k8s;
pub mod provided;
pub mod spawned;

#[derive(Clone)]
pub enum WorkerServiceClient {
    Grpc(WorkerServiceGrpcClient<Channel>),
    Http(Arc<WorkerServiceHttpClientLive>),
}

#[async_trait]
pub trait WorkerService {
    fn client(&self) -> WorkerServiceClient;

    // Overridable client functions - using these instead of client() allows
    // testing worker executors directly without the need to start a worker service,
    // when the `WorkerService` implementation is `ForwardingWorkerService`.
    async fn create_worker(
        &self,
        request: LaunchNewWorkerRequest,
    ) -> crate::Result<LaunchNewWorkerResponse> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => {
                Ok(client.launch_new_worker(request).await?.into_inner())
            }
            WorkerServiceClient::Http(client) => {
                match client
                    .launch_new_worker(
                        &request.component_id.unwrap().value.unwrap().into(),
                        &golem_client::model::WorkerCreationRequest {
                            name: request.name,
                            args: request.args,
                            env: request.env,
                        },
                    )
                    .await
                {
                    Ok(result) => Ok(LaunchNewWorkerResponse {
                        result: Some(launch_new_worker_response::Result::Success(
                            LaunchNewWorkerSuccessResponse {
                                worker_id: Some(result.worker_id.into()),
                                component_version: result.component_version,
                            },
                        )),
                    }),
                    Err(err) => Err(anyhow!("{err:?}")),
                }
            }
        }
    }

    async fn delete_worker(
        &self,
        request: DeleteWorkerRequest,
    ) -> crate::Result<DeleteWorkerResponse> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => {
                Ok(client.delete_worker(request).await?.into_inner())
            }
            WorkerServiceClient::Http(client) => {
                match client
                    .delete_worker(
                        &request
                            .worker_id
                            .as_ref()
                            .unwrap()
                            .component_id
                            .unwrap()
                            .value
                            .unwrap()
                            .into(),
                        &request.worker_id.unwrap().name,
                    )
                    .await
                {
                    Ok(_) => Ok(DeleteWorkerResponse {
                        result: Some(delete_worker_response::Result::Success(Empty {})),
                    }),
                    Err(err) => Err(anyhow!("{err:?}")),
                }
            }
        }
    }

    async fn get_worker_metadata(
        &self,
        request: GetWorkerMetadataRequest,
    ) -> crate::Result<GetWorkerMetadataResponse> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => {
                Ok(client.get_worker_metadata(request).await?.into_inner())
            }
            WorkerServiceClient::Http(client) => {
                match client
                    .get_worker_metadata(
                        &request
                            .worker_id
                            .as_ref()
                            .unwrap()
                            .component_id
                            .unwrap()
                            .value
                            .unwrap()
                            .into(),
                        &request.worker_id.unwrap().name,
                    )
                    .await
                {
                    Ok(result) => Ok(GetWorkerMetadataResponse {
                        result: Some(get_worker_metadata_response::Result::Success(
                            http_worker_metadata_to_grpc(result),
                        )),
                    }),
                    Err(err) => Err(anyhow!("{err:?}")),
                }
            }
        }
    }

    async fn get_workers_metadata(
        &self,
        request: GetWorkersMetadataRequest,
    ) -> crate::Result<GetWorkersMetadataResponse> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => {
                Ok(client.get_workers_metadata(request).await?.into_inner())
            }
            WorkerServiceClient::Http(client) => {
                match client
                    .get_workers_metadata(
                        &request.component_id.unwrap().value.unwrap().into(),
                        request
                            .filter
                            .and_then(|filter| filter.filter)
                            .map(grpc_filter_to_http_filter)
                            .as_deref(),
                        request
                            .cursor
                            .map(|cursor| format!("{}/{}", cursor.layer, cursor.cursor))
                            .as_deref(),
                        Some(request.count),
                        Some(request.precise),
                    )
                    .await
                {
                    Ok(result) => Ok(GetWorkersMetadataResponse {
                        result: Some(get_workers_metadata_response::Result::Success(
                            GetWorkersMetadataSuccessResponse {
                                workers: result
                                    .workers
                                    .into_iter()
                                    .map(http_worker_metadata_to_grpc)
                                    .collect(),
                                cursor: result.cursor.map(|cursor| Cursor {
                                    layer: cursor.layer,
                                    cursor: cursor.cursor,
                                }),
                            },
                        )),
                    }),
                    Err(err) => Err(anyhow!("{err:?}")),
                }
            }
        }
    }

    async fn invoke(
        &self,
        worker_id: TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        invoke_parameters: Option<Vec<ValueAndType>>,
        context: Option<InvocationContext>,
    ) -> crate::Result<InvokeResponse> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => Ok(client
                .invoke(InvokeRequest {
                    worker_id: Some(worker_id),
                    idempotency_key,
                    function,
                    invoke_parameters: invoke_parameters_to_grpc(invoke_parameters),
                    context,
                })
                .await?
                .into_inner()),
            WorkerServiceClient::Http(client) => {
                match client
                    .invoke_function(
                        &worker_id.component_id.unwrap().value.unwrap().into(),
                        &worker_id.name.unwrap(),
                        idempotency_key.map(|key| key.value).as_deref(),
                        &function,
                        &invoke_parameters_to_http(invoke_parameters),
                    )
                    .await
                {
                    Ok(_) => Ok(InvokeResponse {
                        result: Some(invoke_response::Result::Success(Empty {})),
                    }),
                    Err(error) => Err(anyhow!("{error:?}")),
                }
            }
        }
    }

    async fn invoke_json(&self, request: InvokeJsonRequest) -> crate::Result<InvokeResponse> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => {
                Ok(client.invoke_json(request).await?.into_inner())
            }
            WorkerServiceClient::Http(client) => {
                match client
                    .invoke_function(
                        &request
                            .worker_id
                            .as_ref()
                            .unwrap()
                            .component_id
                            .unwrap()
                            .value
                            .unwrap()
                            .into(),
                        &request.worker_id.unwrap().name.unwrap(),
                        request.idempotency_key.map(|key| key.value).as_deref(),
                        &request.function,
                        &invoke_json_parameters_to_http(Some(request.invoke_parameters)),
                    )
                    .await
                {
                    Ok(_) => Ok(InvokeResponse {
                        result: Some(invoke_response::Result::Success(Empty {})),
                    }),
                    Err(error) => Err(anyhow!("{error:?}")),
                }
            }
        }
    }

    async fn invoke_and_await(
        &self,
        worker_id: TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        invoke_parameters: Option<Vec<ValueAndType>>,
        context: Option<InvocationContext>,
    ) -> crate::Result<InvokeAndAwaitResponse> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => Ok(client
                .invoke_and_await(InvokeAndAwaitRequest {
                    worker_id: Some(worker_id),
                    idempotency_key,
                    function,
                    invoke_parameters: invoke_parameters_to_grpc(invoke_parameters),
                    context,
                })
                .await?
                .into_inner()),
            WorkerServiceClient::Http(client) => {
                match client
                    .invoke_and_await_function(
                        &worker_id.component_id.unwrap().value.unwrap().into(),
                        &worker_id.name.unwrap(),
                        idempotency_key.map(|key| key.value).as_deref(),
                        &function,
                        &invoke_parameters_to_http(invoke_parameters),
                    )
                    .await
                {
                    Ok(result) => Ok(InvokeAndAwaitResponse {
                        result: Some(invoke_and_await_response::Result::Success(InvokeResult {
                            result: vec![Value::try_from(result.result).unwrap().into()],
                        })),
                    }),
                    Err(error) => Err(anyhow!("{error:?}")),
                }
            }
        }
    }

    async fn invoke_and_await_json(
        &self,
        request: InvokeAndAwaitJsonRequest,
    ) -> crate::Result<InvokeAndAwaitJsonResponse> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => {
                Ok(client.invoke_and_await_json(request).await?.into_inner())
            }
            WorkerServiceClient::Http(client) => {
                match client
                    .invoke_and_await_function(
                        &request
                            .worker_id
                            .as_ref()
                            .unwrap()
                            .component_id
                            .unwrap()
                            .value
                            .unwrap()
                            .into(),
                        &request.worker_id.unwrap().name.unwrap(),
                        request.idempotency_key.map(|key| key.value).as_deref(),
                        &request.function,
                        &invoke_json_parameters_to_http(Some(request.invoke_parameters)),
                    )
                    .await
                {
                    Ok(result) => Ok(InvokeAndAwaitJsonResponse {
                        result: Some(invoke_and_await_json_response::Result::Success(
                            serde_json::to_string(&result.result)?,
                        )),
                    }),
                    Err(error) => Err(anyhow!("{error:?}")),
                }
            }
        }
    }

    async fn connect_worker(
        &self,
        request: ConnectWorkerRequest,
    ) -> crate::Result<Box<dyn WorkerLogEventStream>> {
        match self.client() {
            WorkerServiceClient::Grpc(client) => Ok(Box::new(
                GrpcWorkerLogEventStream::new(client, request).await?,
            )),
            WorkerServiceClient::Http(client) => Ok(Box::new(
                HttpWorkerLogEventStream::new(client, request).await?,
            )),
        }
    }

    async fn resume_worker(
        &self,
        request: ResumeWorkerRequest,
    ) -> crate::Result<ResumeWorkerResponse> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => {
                Ok(client.resume_worker(request).await?.into_inner())
            }
            WorkerServiceClient::Http(client) => match client
                .resume_worker(
                    &request
                        .worker_id
                        .as_ref()
                        .unwrap()
                        .component_id
                        .unwrap()
                        .value
                        .unwrap()
                        .into(),
                    &request.worker_id.unwrap().name,
                )
                .await
            {
                Ok(_) => Ok(ResumeWorkerResponse {
                    result: Some(resume_worker_response::Result::Success(Empty {})),
                }),
                Err(error) => Err(anyhow!("{error:?}")),
            },
        }
    }

    async fn interrupt_worker(
        &self,
        request: InterruptWorkerRequest,
    ) -> crate::Result<InterruptWorkerResponse> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => {
                Ok(client.interrupt_worker(request).await?.into_inner())
            }
            WorkerServiceClient::Http(client) => match client
                .interrupt_worker(
                    &request
                        .worker_id
                        .as_ref()
                        .unwrap()
                        .component_id
                        .unwrap()
                        .value
                        .unwrap()
                        .into(),
                    &request.worker_id.unwrap().name,
                    Some(request.recover_immediately),
                )
                .await
            {
                Ok(_) => Ok(InterruptWorkerResponse {
                    result: Some(interrupt_worker_response::Result::Success(Empty {})),
                }),
                Err(error) => Err(anyhow!("{error:?}")),
            },
        }
    }

    async fn update_worker(
        &self,
        request: UpdateWorkerRequest,
    ) -> crate::Result<UpdateWorkerResponse> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => {
                Ok(client.update_worker(request).await?.into_inner())
            }
            WorkerServiceClient::Http(client) => match client
                .update_worker(
                    &request
                        .worker_id
                        .as_ref()
                        .unwrap()
                        .component_id
                        .unwrap()
                        .value
                        .unwrap()
                        .into(),
                    &request.worker_id.unwrap().name,
                    &golem_client::model::UpdateWorkerRequest {
                        mode: match UpdateMode::try_from(request.mode)? {
                            UpdateMode::Automatic => {
                                golem_client::model::WorkerUpdateMode::Automatic
                            }
                            UpdateMode::Manual => golem_client::model::WorkerUpdateMode::Manual,
                        },
                        target_version: request.target_version,
                    },
                )
                .await
            {
                Ok(_) => Ok(UpdateWorkerResponse {
                    result: Some(update_worker_response::Result::Success(Empty {})),
                }),
                Err(error) => Err(anyhow!("{error:?}")),
            },
        }
    }

    async fn get_oplog(&self, request: GetOplogRequest) -> crate::Result<GetOplogResponse> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => {
                Ok(client.get_oplog(request).await?.into_inner())
            }
            WorkerServiceClient::Http(client) => match client
                .get_oplog(
                    &request
                        .worker_id
                        .as_ref()
                        .unwrap()
                        .component_id
                        .unwrap()
                        .value
                        .unwrap()
                        .into(),
                    &request.worker_id.unwrap().name,
                    Some(request.from_oplog_index),
                    request.count,
                    request
                        .cursor
                        .map(|cursor| golem_client::model::OplogCursor {
                            current_component_version: cursor.current_component_version,
                            next_oplog_index: cursor.next_oplog_index,
                        })
                        .as_ref(),
                    None,
                )
                .await
            {
                Ok(result) => Ok(GetOplogResponse {
                    result: Some(get_oplog_response::Result::Success(
                        GetOplogSuccessResponse {
                            entries: result
                                .entries
                                .into_iter()
                                .map(|entry| OplogEntry::try_from(entry.entry).unwrap())
                                .collect(),
                            next: result.next.map(|cursor| OplogCursor {
                                next_oplog_index: cursor.next_oplog_index,
                                current_component_version: cursor.current_component_version,
                            }),
                            first_index_in_chunk: result.first_index_in_chunk,
                            last_index: result.last_index,
                        },
                    )),
                }),
                Err(error) => Err(anyhow!("{error:?}")),
            },
        }
    }

    async fn search_oplog(
        &self,
        request: SearchOplogRequest,
    ) -> crate::Result<SearchOplogResponse> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => {
                Ok(client.search_oplog(request).await?.into_inner())
            }
            WorkerServiceClient::Http(client) => {
                match client
                    .get_oplog(
                        &request
                            .worker_id
                            .as_ref()
                            .unwrap()
                            .component_id
                            .unwrap()
                            .value
                            .unwrap()
                            .into(),
                        &request.worker_id.unwrap().name,
                        None,
                        request.count,
                        request
                            .cursor
                            .map(|cursor| golem_client::model::OplogCursor {
                                current_component_version: cursor.current_component_version,
                                next_oplog_index: cursor.next_oplog_index,
                            })
                            .as_ref(),
                        Some(request.query).as_deref(),
                    )
                    .await
                {
                    Ok(result) => Ok(SearchOplogResponse {
                        result: Some(search_oplog_response::Result::Success(
                            SearchOplogSuccessResponse {
                                entries: result
                                    .entries
                                    .into_iter()
                                    .map(|entry| OplogEntryWithIndex {
                                        oplog_index: entry.oplog_index,
                                        entry: Some(OplogEntry::try_from(entry.entry).unwrap()),
                                    })
                                    .collect(),
                                next: result.next.map(|cursor| OplogCursor {
                                    next_oplog_index: cursor.next_oplog_index,
                                    current_component_version: cursor.current_component_version,
                                }),
                                last_index: result.last_index,
                            },
                        )),
                    }),
                    Err(error) => Err(anyhow!("{error:?}")),
                }
            }
        }
    }

    async fn list_directory(
        &self,
        request: ListDirectoryRequest,
    ) -> crate::Result<ListDirectoryResponse> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => {
                Ok(client.list_directory(request).await?.into_inner())
            }
            WorkerServiceClient::Http(client) => {
                match client
                    .get_files(
                        &request
                            .worker_id
                            .as_ref()
                            .unwrap()
                            .component_id
                            .unwrap()
                            .value
                            .unwrap()
                            .into(),
                        &request.worker_id.unwrap().name.unwrap(),
                        &request.path,
                    )
                    .await
                {
                    Ok(result) => Ok(ListDirectoryResponse {
                        result: Some(list_directory_response::Result::Success(
                            ListDirectorySuccessResponse {
                                nodes: result.nodes.into_iter().map(|node|
                                    FileSystemNode {
                                        value: Some(
                                            match node.kind {
                                                golem_client::model::FlatComponentFileSystemNodeKind::Directory => {
                                                    file_system_node::Value::File(FileFileSystemNode {
                                                        name: node.name,
                                                        last_modified: node.last_modified,
                                                        size: node.size.unwrap(),
                                                        permissions: match node.permissions.unwrap() {
                                                            golem_client::model::ComponentFilePermissions::ReadOnly => {
                                                                ComponentFilePermissions::ReadOnly.into()
                                                            }
                                                            golem_client::model::ComponentFilePermissions::ReadWrite => {
                                                                ComponentFilePermissions::ReadWrite.into()
                                                            }
                                                        },
                                                    })
                                                }
                                                golem_client::model::FlatComponentFileSystemNodeKind::File => {
                                                    file_system_node::Value::Directory(DirectoryFileSystemNode {
                                                        name: node.name,
                                                        last_modified: node.last_modified,
                                                    })
                                                }
                                            }
                                        ),
                                    }
                                ).collect(),
                            },
                        )),
                    }),
                    Err(error) => Err(anyhow!("{error:?}")),
                }
            }
        }
    }

    async fn get_file_contents(&self, request: GetFileContentsRequest) -> crate::Result<Bytes> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => {
                let mut stream = client.get_file_contents(request).await?.into_inner();
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
            WorkerServiceClient::Http(client) => {
                match client
                    .get_file_content(
                        &request
                            .worker_id
                            .as_ref()
                            .unwrap()
                            .component_id
                            .unwrap()
                            .value
                            .unwrap()
                            .into(),
                        &request.worker_id.unwrap().name.unwrap(),
                        &request.file_path,
                    )
                    .await
                {
                    Ok(result) => Ok(result),
                    Err(error) => Err(anyhow!("{error:?}")),
                }
            }
        }
    }

    async fn fork_worker(
        &self,
        fork_worker_request: ForkWorkerRequest,
    ) -> crate::Result<ForkWorkerResponse> {
        match self.client() {
            WorkerServiceClient::Grpc(mut client) => {
                Ok(client.fork_worker(fork_worker_request).await?.into_inner())
            }
            WorkerServiceClient::Http(_client) => {
                panic!("Fork worker is not available on HTTP API");
            }
        }
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

async fn new_grpc_client(host: &str, grpc_port: u16) -> WorkerServiceGrpcClient<Channel> {
    let endpoint = Endpoint::new(format!("http://{host}:{grpc_port}"))
        .expect("Failed to create worker service endpoint")
        .connect_timeout(Duration::from_secs(10));
    let channel = endpoint
        .connect()
        .await
        .expect("Failed to connect to Worker service");
    WorkerServiceGrpcClient::new(channel)
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip)
}

fn new_http_client(host: &str, http_port: u16) -> Arc<WorkerServiceHttpClientLive> {
    Arc::new(WorkerServiceHttpClientLive {
        context: Context {
            client: new_reqwest_client(),
            base_url: Url::parse(&format!("http://{host}:{http_port}"))
                .expect("Failed to parse url"),
        },
    })
}

async fn new_client(
    protocol: GolemClientProtocol,
    host: &str,
    grpc_port: u16,
    http_port: u16,
) -> WorkerServiceClient {
    match protocol {
        GolemClientProtocol::Grpc => {
            WorkerServiceClient::Grpc(new_grpc_client(host, grpc_port).await)
        }
        GolemClientProtocol::Http => WorkerServiceClient::Http(new_http_client(host, http_port)),
    }
}

async fn wait_for_startup(
    protocol: GolemClientProtocol,
    host: &str,
    grpc_port: u16,
    http_port: u16,
    timeout: Duration,
) {
    match protocol {
        GolemClientProtocol::Grpc => {
            wait_for_startup_grpc(host, grpc_port, "golem-worker-service", timeout).await
        }
        GolemClientProtocol::Http => {
            wait_for_startup_http(host, http_port, "golem-worker-service", timeout).await
        }
    }
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

fn http_worker_metadata_to_grpc(
    worker_metadata: golem_client::model::WorkerMetadata,
) -> WorkerMetadata {
    WorkerMetadata {
        worker_id: Some(worker_metadata.worker_id.into()),
        account_id: Some(AccountId {
            name: "1".to_string(),
        }),
        args: worker_metadata.args,
        env: worker_metadata.env,
        status: worker_metadata.status.into(),
        component_version: worker_metadata.component_version,
        retry_count: worker_metadata.retry_count,
        pending_invocation_count: worker_metadata.pending_invocation_count,
        updates: worker_metadata
            .updates
            .into_iter()
            .map(|record| match record {
                golem_client::model::UpdateRecord::PendingUpdate(
                    golem_client::model::PendingUpdate {
                        timestamp,
                        target_version,
                    },
                ) => UpdateRecord {
                    timestamp: Some(SystemTime::from(timestamp).into()),
                    target_version,
                    update: Some(update_record::Update::Pending(PendingUpdate {})),
                },
                golem_client::model::UpdateRecord::SuccessfulUpdate(
                    golem_client::model::SuccessfulUpdate {
                        timestamp,
                        target_version,
                    },
                ) => UpdateRecord {
                    timestamp: Some(SystemTime::from(timestamp).into()),
                    target_version,
                    update: Some(update_record::Update::Successful(SuccessfulUpdate {})),
                },
                golem_client::model::UpdateRecord::FailedUpdate(
                    golem_client::model::FailedUpdate {
                        timestamp,
                        target_version,
                        details,
                    },
                ) => UpdateRecord {
                    timestamp: Some(SystemTime::from(timestamp).into()),
                    target_version,
                    update: Some(update_record::Update::Failed(FailedUpdate { details })),
                },
            })
            .collect(),
        created_at: Some(SystemTime::from(worker_metadata.created_at).into()),
        last_error: worker_metadata.last_error,
        component_size: worker_metadata.component_size,
        total_linear_memory_size: worker_metadata.total_linear_memory_size,
        owned_resources: worker_metadata
            .owned_resources
            .into_iter()
            .map(|(k, v)| {
                (
                    k.parse().unwrap(),
                    ResourceMetadata {
                        created_at: Some(SystemTime::from(v.created_at).into()),
                        indexed: v.indexed.map(|indexed| IndexedResourceMetadata {
                            resource_name: indexed.resource_name,
                            resource_params: indexed.resource_params,
                        }),
                    },
                )
            })
            .collect(),
        active_plugins: worker_metadata
            .active_plugins
            .into_iter()
            .map(|id| PluginInstallationId {
                value: Some(id.into()),
            })
            .collect(),
    }
}

fn grpc_filter_comparator_to_http(comparator: i32) -> &'static str {
    match FilterComparator::try_from(comparator).unwrap() {
        FilterComparator::Equal => "==",
        FilterComparator::NotEqual => "!=",
        FilterComparator::Less => "<",
        FilterComparator::LessEqual => "<=",
        FilterComparator::Greater => ">",
        FilterComparator::GreaterEqual => ">=",
    }
}

fn grpc_string_filter_comparator_to_http(comparator: i32) -> &'static str {
    match StringFilterComparator::try_from(comparator).unwrap() {
        StringFilterComparator::StringEqual => "==",
        StringFilterComparator::StringNotEqual => "!=",
        StringFilterComparator::StringLike => "like",
        StringFilterComparator::StringNotLike => "notlike",
    }
}

fn grpc_filter_to_http_filter(filter: Filter) -> Vec<String> {
    fn convert_filter(filter: Filter, allow_and: bool) -> Vec<String> {
        match filter {
            Filter::Name(WorkerNameFilter { comparator, value }) => {
                vec![format!(
                    "name {} {}",
                    grpc_string_filter_comparator_to_http(comparator),
                    value
                )]
            }
            Filter::Version(WorkerVersionFilter { comparator, value }) => {
                vec![format!(
                    "version {} {}",
                    grpc_filter_comparator_to_http(comparator),
                    value
                )]
            }
            Filter::Status(WorkerStatusFilter { comparator, value }) => {
                vec![format!(
                    "status {} {}",
                    grpc_filter_comparator_to_http(comparator),
                    value
                )]
            }
            Filter::CreatedAt(WorkerCreatedAtFilter { comparator, value }) => {
                vec![format!(
                    "name {} {}",
                    grpc_filter_comparator_to_http(comparator),
                    value.unwrap()
                )]
            }
            Filter::Env(WorkerEnvFilter {
                name,
                comparator,
                value,
            }) => {
                vec![format!(
                    "env.{} {} {}",
                    name,
                    grpc_string_filter_comparator_to_http(comparator),
                    value
                )]
            }
            Filter::And(and_filter) => {
                if !allow_and {
                    panic!("'And' filters are only supported on the root level on the HTTP API")
                }
                and_filter
                    .filters
                    .into_iter()
                    .filter_map(|filter| filter.filter)
                    .flat_map(|filter| convert_filter(filter, false))
                    .collect()
            }
            Filter::Or(_) => {
                panic!("Or filters are not supported for HTTP client")
            }
            Filter::Not(_) => {
                panic!("Not filters are not supported for HTTP client")
            }
        }
    }

    convert_filter(filter, true)
}

fn invoke_parameters_to_http(
    parameters: Option<Vec<ValueAndType>>,
) -> golem_client::model::InvokeParameters {
    golem_client::model::InvokeParameters {
        params: match parameters {
            Some(parameters) => parameters
                .into_iter()
                .map(|p| p.try_into().unwrap())
                .collect(),
            None => vec![],
        },
    }
}

fn invoke_json_parameters_to_http(
    parameters: Option<Vec<String>>,
) -> golem_client::model::InvokeParameters {
    golem_client::model::InvokeParameters {
        params: match parameters {
            Some(parameters) => parameters
                .into_iter()
                .map(|p| serde_json::from_str(&p).unwrap())
                .collect(),
            None => vec![],
        },
    }
}

fn invoke_parameters_to_grpc(parameters: Option<Vec<ValueAndType>>) -> Option<InvokeParameters> {
    parameters.map(|parameters| InvokeParameters {
        params: parameters
            .into_iter()
            .map(|param| param.value.into())
            .collect(),
    })
}

#[async_trait]
pub trait WorkerLogEventStream: Send {
    async fn message(&mut self) -> crate::Result<Option<LogEvent>>;
}

pub struct GrpcWorkerLogEventStream {
    streaming: Streaming<LogEvent>,
}

impl GrpcWorkerLogEventStream {
    async fn new(
        mut client: WorkerServiceGrpcClient<Channel>,
        request: ConnectWorkerRequest,
    ) -> crate::Result<Self> {
        Ok(Self {
            streaming: client.connect_worker(request).await?.into_inner(),
        })
    }
}

#[async_trait]
impl WorkerLogEventStream for GrpcWorkerLogEventStream {
    async fn message(&mut self) -> crate::Result<Option<LogEvent>> {
        Ok(self.streaming.message().await?)
    }
}

struct HttpWorkerLogEventStream {
    read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
}

impl HttpWorkerLogEventStream {
    async fn new(
        client: Arc<WorkerServiceHttpClientLive>,
        request: ConnectWorkerRequest,
    ) -> crate::Result<Self> {
        let url = format!(
            "ws://{}:{}/v1/components/{}/workers/{}/connect",
            client.context.base_url.host().unwrap(),
            client.context.base_url.port_or_known_default().unwrap(),
            Uuid::from(
                request
                    .worker_id
                    .as_ref()
                    .unwrap()
                    .component_id
                    .unwrap()
                    .value
                    .unwrap()
            ),
            request.worker_id.unwrap().name,
        );

        let (stream, _) = tokio_tungstenite::connect_async_tls_with_config(
            url,
            None,
            false,
            Some(Connector::Plain),
        )
        .await?;
        let (mut write, read) = stream.split();

        static PING_HELLO: &str = "hello";
        task::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                match write
                    .send(Message::Ping(Payload::from(PING_HELLO.as_bytes())))
                    .await
                {
                    Ok(_) => {}
                    Err(error) => break error,
                };
            }
        });

        Ok(Self { read })
    }
}

#[async_trait]
impl WorkerLogEventStream for HttpWorkerLogEventStream {
    async fn message(&mut self) -> crate::Result<Option<LogEvent>> {
        match self.read.next().await {
            Some(Ok(message)) => match message {
                Message::Text(payload) => Ok(Some(
                    serde_json::from_str::<WorkerEvent>(payload.as_str())?
                        .try_into()
                        .map_err(|error: String| anyhow!(error))?,
                )),
                Message::Binary(payload) => Ok(Some(
                    serde_json::from_slice::<WorkerEvent>(payload.as_slice())?
                        .try_into()
                        .map_err(|error: String| anyhow!(error))?,
                )),
                Message::Ping(_) => self.message().await,
                Message::Pong(_) => self.message().await,
                Message::Close(_) => Ok(None),
                Message::Frame(_) => {
                    panic!("Raw frames should not be received")
                }
            },
            Some(Err(error)) => Err(anyhow!(error)),
            None => Ok(None),
        }
    }
}
