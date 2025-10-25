// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub mod forwarding;
pub mod provided;
pub mod spawned;

use super::cloud_service::CloudService;
use crate::components::component_service::ComponentService;
use crate::components::rdb::Rdb;
use crate::components::shard_manager::ShardManager;
use crate::components::{wait_for_startup_grpc, wait_for_startup_http, EnvVarBuilder};
use crate::config::GolemClientProtocol;
use anyhow::{anyhow, Context as AnyhowContext};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::SplitStream;
use futures::{SinkExt, StreamExt};
use golem_api_grpc::proto::golem::common::{
    AccountId, Empty, FilterComparator, PluginInstallationId, StringFilterComparator,
};
use golem_api_grpc::proto::golem::component::ComponentFilePermissions;
pub use golem_api_grpc::proto::golem::worker::v1::worker_service_client::WorkerServiceClient as WorkerServiceGrpcClient;
use golem_api_grpc::proto::golem::worker::v1::{
    cancel_invocation_response, delete_worker_response, get_file_contents_response,
    get_file_system_node_response, get_oplog_response, get_worker_metadata_response,
    get_workers_metadata_response, interrupt_worker_response, invoke_and_await_json_response,
    invoke_and_await_response, invoke_and_await_typed_response, invoke_response,
    launch_new_worker_response, resume_worker_response, revert_worker_response,
    search_oplog_response, update_worker_response, CancelInvocationRequest,
    CancelInvocationResponse, ConnectWorkerRequest, DeleteWorkerRequest, DeleteWorkerResponse,
    ForkWorkerRequest, ForkWorkerResponse, GetFileContentsRequest, GetFileSystemNodeRequest,
    GetFileSystemNodeResponse, GetOplogRequest, GetOplogResponse, GetOplogSuccessResponse,
    GetWorkerMetadataRequest, GetWorkerMetadataResponse, GetWorkersMetadataRequest,
    GetWorkersMetadataResponse, GetWorkersMetadataSuccessResponse, InterruptWorkerRequest,
    InterruptWorkerResponse, InvokeAndAwaitJsonRequest, InvokeAndAwaitJsonResponse,
    InvokeAndAwaitRequest, InvokeAndAwaitResponse, InvokeAndAwaitTypedResponse, InvokeJsonRequest,
    InvokeRequest, InvokeResponse, LaunchNewWorkerRequest, LaunchNewWorkerResponse,
    LaunchNewWorkerSuccessResponse, ListFileSystemNodeResponse, ResumeWorkerRequest,
    ResumeWorkerResponse, RevertWorkerRequest, RevertWorkerResponse, SearchOplogRequest,
    SearchOplogResponse, SearchOplogSuccessResponse, UpdateWorkerRequest, UpdateWorkerResponse,
};
use golem_api_grpc::proto::golem::worker::worker_filter::Filter;
use golem_api_grpc::proto::golem::worker::{
    file_system_node, update_record, Cursor, DirectoryFileSystemNode, FailedUpdate,
    FileFileSystemNode, FileSystemNode, IdempotencyKey, InvocationContext, InvokeParameters,
    InvokeResult, InvokeResultTyped, LogEvent, OplogCursor, OplogEntry, OplogEntryWithIndex,
    PendingUpdate, SuccessfulUpdate, UpdateMode, UpdateRecord, WorkerCreatedAtFilter,
    WorkerEnvFilter, WorkerId, WorkerMetadata, WorkerNameFilter, WorkerStatusFilter,
    WorkerVersionFilter, WorkerWasiConfigVarsFilter,
};
use golem_client::api::ApiDefinitionClient as ApiDefinitionServiceHttpClient;
use golem_client::api::ApiDefinitionClientLive as ApiDefinitionServiceHttpClientLive;
use golem_client::api::ApiDeploymentClient as ApiDeploymentServiceHttpClient;
use golem_client::api::ApiDeploymentClientLive as ApiDeploymentServiceHttpClientLive;
use golem_client::api::ApiSecurityClient as ApiSecurityServiceHttpClient;
use golem_client::api::ApiSecurityClientLive as ApiSecurityServiceHttpClientLive;
use golem_client::api::WorkerClient as WorkerServiceHttpClient;
use golem_client::api::WorkerClientLive as WorkerServiceHttpClientLive;
use golem_client::model::{
    ApiDeployment, ApiDeploymentRequest, HttpApiDefinitionRequest, HttpApiDefinitionResponseData,
    OpenApiHttpApiDefinitionResponse, SecuritySchemeData,
};
use golem_client::{Context, Security};
use golem_common::model::worker::WasiConfigVars;
use golem_common::model::WorkerEvent;
use golem_common::model::{ProjectId, PromiseId};
use golem_service_base::clients::authorised_request;
use golem_wasm::{Value, ValueAndType};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::net::TcpStream;
use tokio::{task, time};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::Payload;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use tonic::codec::CompressionEncoding;
use tonic::transport::{Channel, Endpoint};
use tonic::Streaming;
use tracing::Level;
use url::Url;
use uuid::Uuid;

#[async_trait]
pub trait WorkerService: Send + Sync {
    fn component_service(&self) -> &Arc<dyn ComponentService>;

    fn client_protocol(&self) -> GolemClientProtocol;
    async fn base_http_client(&self) -> reqwest::Client;

    async fn worker_http_client(&self, token: &Uuid) -> WorkerServiceHttpClientLive {
        let url = format!("http://{}:{}", self.public_host(), self.public_http_port());
        WorkerServiceHttpClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: Url::parse(&url).expect("Failed to parse url"),
                security_token: Security::Bearer(token.to_string()),
            },
        }
    }
    async fn worker_grpc_client(&self) -> WorkerServiceGrpcClient<Channel>;

    async fn api_definition_http_client(&self, token: &Uuid) -> ApiDefinitionServiceHttpClientLive {
        let url = format!("http://{}:{}", self.public_host(), self.public_http_port());
        ApiDefinitionServiceHttpClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: Url::parse(&url).expect("Failed to parse url"),
                security_token: Security::Bearer(token.to_string()),
            },
        }
    }

    async fn api_deployment_http_client(&self, token: &Uuid) -> ApiDeploymentServiceHttpClientLive {
        let url = format!("http://{}:{}", self.public_host(), self.public_http_port());
        ApiDeploymentServiceHttpClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: Url::parse(&url).expect("Failed to parse url"),
                security_token: Security::Bearer(token.to_string()),
            },
        }
    }

    async fn api_security_http_client(&self, token: &Uuid) -> ApiSecurityServiceHttpClientLive {
        let url = format!("http://{}:{}", self.public_host(), self.public_http_port());
        ApiSecurityServiceHttpClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: Url::parse(&url).expect("Failed to parse url"),
                security_token: Security::Bearer(token.to_string()),
            },
        }
    }

    async fn complete_promise(
        &self,
        token: &Uuid,
        promise_id: PromiseId,
        data: Vec<u8>,
    ) -> crate::Result<()> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                use golem_api_grpc::proto::golem::worker::v1::CompletePromiseRequest;
                use golem_api_grpc::proto::golem::worker::CompleteParameters;

                let mut client = self.worker_grpc_client().await;
                let request = CompletePromiseRequest {
                    worker_id: Some(promise_id.worker_id.into()),
                    complete_parameters: Some(CompleteParameters {
                        oplog_idx: promise_id.oplog_idx.into(),
                        data,
                    }),
                };
                let request = authorised_request(request, token);

                client.complete_promise(request).await?;
                Ok(())
            }
            GolemClientProtocol::Http => {
                use golem_client::model::CompleteParameters;

                let client = self.worker_http_client(token).await;
                client
                    .complete_promise(
                        &promise_id.worker_id.component_id.0,
                        &promise_id.worker_id.worker_name,
                        &CompleteParameters {
                            oplog_idx: promise_id.oplog_idx.into(),
                            data,
                        },
                    )
                    .await?;

                Ok(())
            }
        }
    }

    // Overridable client functions - using these instead of client() allows
    // testing worker executors directly without the need to start a worker service,
    // when the `WorkerService` implementation is `ForwardingWorkerService`.
    async fn create_worker(
        &self,
        token: &Uuid,
        request: LaunchNewWorkerRequest,
    ) -> crate::Result<LaunchNewWorkerResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);

                Ok(client.launch_new_worker(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;
                let result = client
                    .launch_new_worker(
                        &request.component_id.unwrap().value.unwrap().into(),
                        &golem_client::model::WorkerCreationRequest {
                            name: request.name,
                            args: request.args,
                            env: request.env,
                            wasi_config_vars: request
                                .wasi_config_vars
                                .expect("no wasi_config_vars field")
                                .into(),
                        },
                    )
                    .await?;

                Ok(LaunchNewWorkerResponse {
                    result: Some(launch_new_worker_response::Result::Success(
                        LaunchNewWorkerSuccessResponse {
                            worker_id: Some(result.worker_id.into()),
                            component_version: result.component_version,
                        },
                    )),
                })
            }
        }
    }

    async fn delete_worker(
        &self,
        token: &Uuid,
        request: DeleteWorkerRequest,
    ) -> crate::Result<DeleteWorkerResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);
                Ok(client.delete_worker(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;

                client
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
                    .await?;

                Ok(DeleteWorkerResponse {
                    result: Some(delete_worker_response::Result::Success(Empty {})),
                })
            }
        }
    }

    async fn get_worker_metadata(
        &self,
        token: &Uuid,
        request: GetWorkerMetadataRequest,
    ) -> crate::Result<GetWorkerMetadataResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);
                Ok(client.get_worker_metadata(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;

                let result = client
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
                    .await?;

                Ok(GetWorkerMetadataResponse {
                    result: Some(get_worker_metadata_response::Result::Success(
                        http_worker_metadata_to_grpc(result),
                    )),
                })
            }
        }
    }

    async fn get_workers_metadata(
        &self,
        token: &Uuid,
        request: GetWorkersMetadataRequest,
    ) -> crate::Result<GetWorkersMetadataResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);
                Ok(client.get_workers_metadata(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;
                let result = client
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
                    .await?;

                Ok(GetWorkersMetadataResponse {
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
                })
            }
        }
    }

    async fn invoke(
        &self,
        token: &Uuid,
        worker_id: WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        invoke_parameters: Vec<ValueAndType>,
        context: Option<InvocationContext>,
    ) -> crate::Result<InvokeResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(
                    InvokeRequest {
                        worker_id: Some(worker_id),
                        idempotency_key,
                        function,
                        invoke_parameters: invoke_parameters_to_grpc(invoke_parameters),
                        context,
                    },
                    token,
                );

                let response = client.invoke(request).await?.into_inner();
                Ok(response)
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;

                client
                    .invoke_function(
                        &worker_id.component_id.unwrap().value.unwrap().into(),
                        &worker_id.name,
                        idempotency_key.map(|key| key.value).as_deref(),
                        &function,
                        &invoke_parameters_to_http(invoke_parameters),
                    )
                    .await?;

                Ok(InvokeResponse {
                    result: Some(invoke_response::Result::Success(Empty {})),
                })
            }
        }
    }

    async fn invoke_json(
        &self,
        token: &Uuid,
        request: InvokeJsonRequest,
    ) -> crate::Result<InvokeResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);
                Ok(client.invoke_json(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;
                client
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
                        &request.worker_id.unwrap().name,
                        request.idempotency_key.map(|key| key.value).as_deref(),
                        &request.function,
                        &invoke_json_parameters_to_http(request.invoke_parameters),
                    )
                    .await?;
                Ok(InvokeResponse {
                    result: Some(invoke_response::Result::Success(Empty {})),
                })
            }
        }
    }

    async fn invoke_and_await(
        &self,
        token: &Uuid,
        worker_id: WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        invoke_parameters: Vec<ValueAndType>,
        context: Option<InvocationContext>,
    ) -> crate::Result<InvokeAndAwaitResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(
                    InvokeAndAwaitRequest {
                        worker_id: Some(worker_id),
                        idempotency_key,
                        function,
                        invoke_parameters: invoke_parameters_to_grpc(invoke_parameters),
                        context,
                    },
                    token,
                );

                let response = client.invoke_and_await(request).await?.into_inner();
                Ok(response)
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;
                let result = client
                    .invoke_and_await_function(
                        &worker_id.component_id.unwrap().value.unwrap().into(),
                        &worker_id.name,
                        idempotency_key.map(|key| key.value).as_deref(),
                        &function,
                        &invoke_parameters_to_http(invoke_parameters),
                    )
                    .await?;

                Ok(InvokeAndAwaitResponse {
                    result: Some(invoke_and_await_response::Result::Success(InvokeResult {
                        result: result.result.map(|result| {
                            let value: Value = result.into();
                            value.into()
                        }),
                    })),
                })
            }
        }
    }

    async fn invoke_and_await_typed(
        &self,
        token: &Uuid,
        worker_id: WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        invoke_parameters: Vec<ValueAndType>,
        context: Option<InvocationContext>,
    ) -> crate::Result<InvokeAndAwaitTypedResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(
                    InvokeAndAwaitRequest {
                        worker_id: Some(worker_id),
                        idempotency_key,
                        function,
                        invoke_parameters: invoke_parameters_to_grpc(invoke_parameters),
                        context,
                    },
                    token,
                );

                Ok(client.invoke_and_await_typed(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;
                let result = client
                    .invoke_and_await_function(
                        &worker_id.component_id.unwrap().value.unwrap().into(),
                        &worker_id.name,
                        idempotency_key.map(|key| key.value).as_deref(),
                        &function,
                        &invoke_parameters_to_http(invoke_parameters),
                    )
                    .await?;

                Ok(InvokeAndAwaitTypedResponse {
                    result: Some(invoke_and_await_typed_response::Result::Success(
                        InvokeResultTyped {
                            result: result.result.map(|vnt| vnt.into()),
                        },
                    )),
                })
            }
        }
    }

    async fn invoke_and_await_json(
        &self,
        token: &Uuid,
        request: InvokeAndAwaitJsonRequest,
    ) -> crate::Result<InvokeAndAwaitJsonResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);
                Ok(client.invoke_and_await_json(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;
                let result = client
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
                        &request.worker_id.unwrap().name,
                        request.idempotency_key.map(|key| key.value).as_deref(),
                        &request.function,
                        &invoke_json_parameters_to_http(request.invoke_parameters),
                    )
                    .await?;
                Ok(InvokeAndAwaitJsonResponse {
                    result: Some(invoke_and_await_json_response::Result::Success(
                        serde_json::to_string(&result.result)?,
                    )),
                })
            }
        }
    }

    async fn connect_worker(
        &self,
        token: &Uuid,
        request: ConnectWorkerRequest,
    ) -> crate::Result<Box<dyn WorkerLogEventStream>> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);

                Ok(Box::new(
                    GrpcWorkerLogEventStream::new(client, request).await?,
                ))
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;
                Ok(Box::new(
                    HttpWorkerLogEventStream::new(Arc::new(client), request).await?,
                ))
            }
        }
    }

    async fn resume_worker(
        &self,
        token: &Uuid,
        request: ResumeWorkerRequest,
    ) -> crate::Result<ResumeWorkerResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);
                Ok(client.resume_worker(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;
                client
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
                    .await?;

                Ok(ResumeWorkerResponse {
                    result: Some(resume_worker_response::Result::Success(Empty {})),
                })
            }
        }
    }

    async fn interrupt_worker(
        &self,
        token: &Uuid,
        request: InterruptWorkerRequest,
    ) -> crate::Result<InterruptWorkerResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);
                Ok(client.interrupt_worker(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;
                client
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
                    .await?;

                Ok(InterruptWorkerResponse {
                    result: Some(interrupt_worker_response::Result::Success(Empty {})),
                })
            }
        }
    }

    async fn update_worker(
        &self,
        token: &Uuid,
        request: UpdateWorkerRequest,
    ) -> crate::Result<UpdateWorkerResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);
                Ok(client.update_worker(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;
                client
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
                    .await?;
                Ok(UpdateWorkerResponse {
                    result: Some(update_worker_response::Result::Success(Empty {})),
                })
            }
        }
    }

    async fn get_oplog(
        &self,
        token: &Uuid,
        request: GetOplogRequest,
    ) -> crate::Result<GetOplogResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);
                Ok(client.get_oplog(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;

                let result = client
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
                    .await?;

                Ok(GetOplogResponse {
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
                })
            }
        }
    }

    async fn search_oplog(
        &self,
        token: &Uuid,
        request: SearchOplogRequest,
    ) -> crate::Result<SearchOplogResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);
                Ok(client.search_oplog(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;

                let result = client
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
                    .await?;

                Ok(SearchOplogResponse {
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
                })
            }
        }
    }

    async fn get_file_system_node(
        &self,
        token: &Uuid,
        request: GetFileSystemNodeRequest,
    ) -> crate::Result<GetFileSystemNodeResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);
                Ok(client.get_file_system_node(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;

                let result = client
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
                        &request.worker_id.unwrap().name,
                        &request.path,
                    )
                    .await?;

                Ok(GetFileSystemNodeResponse {
                    result: Some(get_file_system_node_response::Result::Success(
                        ListFileSystemNodeResponse {
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
                })
            }
        }
    }

    async fn get_file_contents(
        &self,
        token: &Uuid,
        request: GetFileContentsRequest,
    ) -> crate::Result<Bytes> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);
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
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;

                let result = client
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
                        &request.worker_id.unwrap().name,
                        &request.file_path,
                    )
                    .await?;

                Ok(result)
            }
        }
    }

    async fn fork_worker(
        &self,
        token: &Uuid,
        fork_worker_request: ForkWorkerRequest,
    ) -> crate::Result<ForkWorkerResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(fork_worker_request, token);
                Ok(client.fork_worker(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                panic!("Fork worker is not available on HTTP API");
            }
        }
    }

    async fn revert_worker(
        &self,
        token: &Uuid,
        request: RevertWorkerRequest,
    ) -> crate::Result<RevertWorkerResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);
                Ok(client.revert_worker(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;

                client
                    .revert_worker(
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
                        &match request.target.as_ref().and_then(|target| target.target) {
                            Some(golem_api_grpc::proto::golem::common::revert_worker_target::Target::RevertToOplogIndex(target)) => {
                                golem_client::model::RevertWorkerTarget::RevertToOplogIndex(golem_client::model::RevertToOplogIndex {
                                    last_oplog_index: target.last_oplog_index as u64,
                                })
                            }
                            Some(golem_api_grpc::proto::golem::common::revert_worker_target::Target::RevertLastInvocations(target)) => {
                                golem_client::model::RevertWorkerTarget::RevertLastInvocations(golem_client::model::RevertLastInvocations {
                                    number_of_invocations: target.number_of_invocations as u64,
                                })
                            }
                            _ => Err(anyhow!("RevertWorkerRequest.target is required"))?,
                        },
                    )
                    .await?;

                Ok(RevertWorkerResponse {
                    result: Some(revert_worker_response::Result::Success(Empty {})),
                })
            }
        }
    }

    async fn cancel_invocation(
        &self,
        token: &Uuid,
        request: CancelInvocationRequest,
    ) -> crate::Result<CancelInvocationResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.worker_grpc_client().await;
                let request = authorised_request(request, token);
                Ok(client.cancel_invocation(request).await?.into_inner())
            }
            GolemClientProtocol::Http => {
                let client = self.worker_http_client(token).await;
                let response = client
                    .cancel_invocation(
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
                        &request.idempotency_key.as_ref().unwrap().value,
                    )
                    .await?;

                Ok(CancelInvocationResponse {
                    result: Some(cancel_invocation_response::Result::Success(
                        response.canceled,
                    )),
                })
            }
        }
    }

    async fn create_api_definition(
        &self,
        token: &Uuid,
        project_id: &ProjectId,
        request: &HttpApiDefinitionRequest,
    ) -> crate::Result<HttpApiDefinitionResponseData> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("create_api_definition"),
            GolemClientProtocol::Http => {
                let client = self.api_definition_http_client(token).await;

                let result = client
                    .create_definition_json(&project_id.0, request)
                    .await?;

                Ok(result)
            }
        }
    }

    async fn create_api_definition_from_yaml(
        &self,
        token: &Uuid,
        project_id: &ProjectId,
        open_api_yaml: &str,
    ) -> crate::Result<HttpApiDefinitionResponseData> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("create_api_definition"),
            GolemClientProtocol::Http => {
                let client = self.api_definition_http_client(token).await;

                let result = client
                    .import_open_api_yaml(&project_id.0, &serde_yaml::from_str(open_api_yaml)?)
                    .await?;

                Ok(result)
            }
        }
    }

    async fn create_api_definition_from_json(
        &self,
        token: &Uuid,
        project_id: &ProjectId,
        open_api_json: &str,
    ) -> crate::Result<HttpApiDefinitionResponseData> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("create_api_definition"),
            GolemClientProtocol::Http => {
                let client = self.api_definition_http_client(token).await;

                let result = client
                    .import_open_api_json(&project_id.0, &serde_json::from_str(open_api_json)?)
                    .await?;

                Ok(result)
            }
        }
    }

    async fn update_api_definition(
        &self,
        token: &Uuid,
        project_id: &ProjectId,
        request: &HttpApiDefinitionRequest,
    ) -> crate::Result<HttpApiDefinitionResponseData> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("update_api_definition"),
            GolemClientProtocol::Http => {
                let client = self.api_definition_http_client(token).await;

                let result = client
                    .update_definition_json(&project_id.0, &request.id, &request.version, request)
                    .await?;

                Ok(result)
            }
        }
    }

    async fn get_api_definition(
        &self,
        token: &Uuid,
        project_id: &ProjectId,
        api_definition_id: &str,
        api_definition_version: &str,
    ) -> crate::Result<HttpApiDefinitionResponseData> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("get_api_definition"),
            GolemClientProtocol::Http => {
                let client = self.api_definition_http_client(token).await;

                let result = client
                    .get_definition(&project_id.0, api_definition_id, api_definition_version)
                    .await?;

                Ok(result)
            }
        }
    }

    async fn get_api_definition_versions(
        &self,
        token: &Uuid,
        project_id: &ProjectId,
        api_definition_id: &str,
    ) -> crate::Result<Vec<HttpApiDefinitionResponseData>> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("get_api_definition_versions"),
            GolemClientProtocol::Http => {
                let client = self.api_definition_http_client(token).await;

                let result = client
                    .list_definitions(&project_id.0, Some(api_definition_id))
                    .await?;

                Ok(result)
            }
        }
    }

    async fn get_all_api_definitions(
        &self,
        token: &Uuid,
        project_id: &ProjectId,
    ) -> crate::Result<Vec<HttpApiDefinitionResponseData>> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("get_all_api_definitions"),
            GolemClientProtocol::Http => {
                let client = self.api_definition_http_client(token).await;

                let result = client.list_definitions(&project_id.0, None).await?;

                Ok(result)
            }
        }
    }

    async fn delete_api_definition(
        &self,
        token: &Uuid,
        project_id: &ProjectId,
        api_definition_id: &str,
        api_definition_version: &str,
    ) -> crate::Result<()> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("delete_api_definition"),
            GolemClientProtocol::Http => {
                let client = self.api_definition_http_client(token).await;

                client
                    .delete_definition(&project_id.0, api_definition_id, api_definition_version)
                    .await?;

                Ok(())
            }
        }
    }

    async fn create_or_update_api_deployment(
        &self,
        token: &Uuid,
        request: ApiDeploymentRequest,
    ) -> crate::Result<ApiDeployment> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("create_api_deployment"),
            GolemClientProtocol::Http => {
                let client = self.api_deployment_http_client(token).await;

                let result = client.deploy(&request).await?;

                Ok(result)
            }
        }
    }

    async fn get_api_deployment(
        &self,
        token: &Uuid,
        project_id: &ProjectId,
        site: &str,
    ) -> crate::Result<ApiDeployment> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("get_api_deployment"),
            GolemClientProtocol::Http => {
                let client = self.api_deployment_http_client(token).await;

                let result = client.get_deployment(&project_id.0, site).await?;

                Ok(result)
            }
        }
    }

    async fn list_api_deployments(
        &self,
        token: &Uuid,
        project_id: &ProjectId,
        api_definition_id: Option<&str>,
    ) -> crate::Result<Vec<ApiDeployment>> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("list_api_deployments"),
            GolemClientProtocol::Http => {
                let client = self.api_deployment_http_client(token).await;

                let result = client
                    .list_deployments(&project_id.0, api_definition_id)
                    .await?;

                Ok(result)
            }
        }
    }

    async fn delete_api_deployment(
        &self,
        token: &Uuid,
        project_id: &ProjectId,
        site: &str,
    ) -> crate::Result<()> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("delete_api_deployment"),
            GolemClientProtocol::Http => {
                let client = self.api_deployment_http_client(token).await;

                client.delete_deployment(&project_id.0, site).await?;

                Ok(())
            }
        }
    }

    async fn export_openapi_spec(
        &self,
        token: &Uuid,
        project_id: &ProjectId,
        api_definition_id: &str,
        api_definition_version: &str,
    ) -> crate::Result<OpenApiHttpApiDefinitionResponse> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("export_openapi_spec"),
            GolemClientProtocol::Http => {
                let client = self.api_definition_http_client(token).await;

                let result = client
                    .export_definition(&project_id.0, api_definition_id, api_definition_version)
                    .await?;

                Ok(result)
            }
        }
    }

    async fn undeploy_api(
        &self,
        token: &Uuid,
        project_id: &ProjectId,
        site: &str,
        id: &str,
        version: &str,
    ) -> crate::Result<()> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("undeploy_api"),
            GolemClientProtocol::Http => {
                let client = self.api_deployment_http_client(token).await;

                client
                    .undeploy_api(&project_id.0, site, id, version)
                    .await?;

                Ok(())
            }
        }
    }

    async fn create_api_security_scheme(
        &self,
        token: &Uuid,
        project_id: &ProjectId,
        request: SecuritySchemeData,
    ) -> crate::Result<SecuritySchemeData> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("create_api_security_scheme"),
            GolemClientProtocol::Http => {
                let client = self.api_security_http_client(token).await;

                let result = client.create(&project_id.0, &request).await?;

                Ok(result)
            }
        }
    }

    async fn get_api_security_scheme(
        &self,
        token: &Uuid,
        project_id: &ProjectId,
        security_scheme_id: &str,
    ) -> crate::Result<SecuritySchemeData> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => not_available_on_grpc_api("get_api_security_scheme"),
            GolemClientProtocol::Http => {
                let client = self.api_security_http_client(token).await;

                let result = client.get(&project_id.0, security_scheme_id).await?;

                Ok(result)
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

async fn new_worker_grpc_client(host: &str, grpc_port: u16) -> WorkerServiceGrpcClient<Channel> {
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

async fn env_vars(
    http_port: u16,
    grpc_port: u16,
    custom_request_port: u16,
    component_service: &Arc<dyn ComponentService>,
    shard_manager: &Arc<dyn ShardManager>,
    rdb: &Arc<dyn Rdb>,
    verbosity: Level,
    rdb_private_connection: bool,
    cloud_service: &Arc<dyn CloudService>,
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
        .with(
            "GOLEM__COMPONENT_SERVICE__ACCESS_TOKEN",
            cloud_service.admin_token().to_string(),
        )
        .with_str("GOLEM__ENVIRONMENT", "local")
        .with("GOLEM__ROUTING_TABLE__HOST", shard_manager.private_host())
        .with(
            "GOLEM__ROUTING_TABLE__PORT",
            shard_manager.private_grpc_port().to_string(),
        )
        .with(
            "GOLEM__CUSTOM_REQUEST_PORT",
            custom_request_port.to_string(),
        )
        .with("GOLEM__CLOUD_SERVICE__HOST", cloud_service.private_host())
        .with(
            "GOLEM__CLOUD_SERVICE__PORT",
            cloud_service.private_grpc_port().to_string(),
        )
        .with(
            "GOLEM__CLOUD_SERVICE__ACCESS_TOKEN",
            cloud_service.admin_token().to_string(),
        )
        .with("GOLEM__WORKER_GRPC_PORT", grpc_port.to_string())
        .with("GOLEM__PORT", http_port.to_string())
        .with("GOLEM__ENGINE__ENABLE_FS_CACHE", "true".to_string())
        .with_all(rdb.info().env("golem_worker", rdb_private_connection))
        .build()
}

fn http_worker_metadata_to_grpc(
    worker_metadata: golem_client::model::WorkerMetadata,
) -> WorkerMetadata {
    let mut owned_resources = Vec::new();
    for instance in worker_metadata.exported_resource_instances {
        owned_resources.push(golem_api_grpc::proto::golem::worker::ResourceDescription {
            resource_id: instance.key,
            resource_name: instance.description.resource_name,
            resource_owner: instance.description.resource_owner,
            created_at: Some(instance.description.created_at.into()),
        });
    }

    WorkerMetadata {
        worker_id: Some(worker_metadata.worker_id.into()),
        created_by: Some(AccountId {
            name: "1".to_string(),
        }),
        project_id: Some(ProjectId(worker_metadata.project_id).into()),
        args: worker_metadata.args,
        env: worker_metadata.env,
        wasi_config_vars: Some(WasiConfigVars(worker_metadata.wasi_config_vars).into()),
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
        owned_resources,
        active_plugins: worker_metadata
            .active_plugins
            .into_iter()
            .map(|id| PluginInstallationId {
                value: Some(id.into()),
            })
            .collect(),
        skipped_regions: worker_metadata
            .skipped_regions
            .into_iter()
            .map(|region| region.into())
            .collect(),
        deleted_regions: worker_metadata
            .deleted_regions
            .into_iter()
            .map(|region| region.into())
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
        StringFilterComparator::StartsWith => "startswith",
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
            Filter::WasiConfigVars(WorkerWasiConfigVarsFilter {
                name,
                comparator,
                value,
            }) => {
                vec![format!(
                    "config.{} {} {}",
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
    parameters: Vec<ValueAndType>,
) -> golem_client::model::InvokeParameters {
    golem_client::model::InvokeParameters {
        params: parameters
            .into_iter()
            .map(|p| p.try_into().unwrap())
            .collect(),
    }
}

fn invoke_json_parameters_to_http(
    parameters: Vec<String>,
) -> golem_client::model::InvokeParameters {
    golem_client::model::InvokeParameters {
        params: parameters
            .into_iter()
            .map(|p| serde_json::from_str(&p).unwrap())
            .collect(),
    }
}

fn invoke_parameters_to_grpc(parameters: Vec<ValueAndType>) -> Option<InvokeParameters> {
    Some(InvokeParameters {
        params: parameters
            .into_iter()
            .map(|param| param.value.into())
            .collect(),
    })
}

fn not_available_on_grpc_api<T>(endpoint: &str) -> crate::Result<T> {
    Err(anyhow!("not available on GRPC API: {endpoint}"))
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
        request: tonic::Request<ConnectWorkerRequest>,
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

        let mut connection_request = url
            .into_client_request()
            .context("Failed to create request")?;

        {
            let headers = connection_request.headers_mut();

            if let Some(bearer_token) = client.context.bearer_token() {
                headers.insert("Authorization", format!("Bearer {bearer_token}").parse()?);
            }
        }

        let (stream, _) = tokio_tungstenite::connect_async_tls_with_config(
            connection_request,
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
