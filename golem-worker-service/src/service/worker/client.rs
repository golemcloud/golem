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

use super::WorkerResult;
use super::{
    AllExecutors, CallWorkerExecutorError, HasWorkerExecutorClients, RandomExecutor,
    ResponseMapResult, RoutingLogic, WorkerServiceError, WorkerStream,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::TryStreamExt;
use futures::{Stream, StreamExt};
use golem_api_grpc::proto::golem::worker::{InvocationContext, InvokeResult, LogEvent};
use golem_api_grpc::proto::golem::workerexecutor;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    ActivatePluginRequest, CancelInvocationRequest, CompletePromiseRequest, ConnectWorkerRequest,
    CreateWorkerRequest, DeactivatePluginRequest, ForkWorkerRequest, InterruptWorkerRequest,
    InvokeAndAwaitWorkerJsonRequest, InvokeAndAwaitWorkerRequest, ResumeWorkerRequest,
    RevertWorkerRequest, SearchOplogResponse, UpdateWorkerRequest,
};
use golem_common::client::MultiTargetGrpcClient;
use golem_common::model::account::AccountId;
use golem_common::model::component::{
    ComponentFilePath, ComponentId, ComponentRevision, PluginPriority,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{OplogCursor, PublicOplogEntry};
use golem_common::model::oplog::{OplogIndex, PublicOplogEntryWithIndex};
use golem_common::model::worker::WorkerUpdateMode;
use golem_common::model::worker::{RevertWorkerTarget, WorkerMetadataDto};
use golem_common::model::RetryConfig;
use golem_common::model::{
    FilterComparator, IdempotencyKey, PromiseId, ScanCursor, WorkerFilter, WorkerId, WorkerStatus,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::{ComponentFileSystemNode, GetOplogResponse};
use golem_service_base::service::routing_table::{HasRoutingTableService, RoutingTableService};
use golem_wasm::analysis::AnalysedFunctionResult;
use golem_wasm::protobuf::Val as ProtoVal;
use golem_wasm::ValueAndType;
use std::collections::BTreeMap;
use std::pin::Pin;
use std::{collections::HashMap, sync::Arc};
use tonic::transport::Channel;
use tonic::Code;
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;

#[async_trait]
pub trait WorkerClient: Send + Sync {
    async fn create(
        &self,
        worker_id: &WorkerId,
        component_version: ComponentRevision,
        environment_variables: HashMap<String, String>,
        wasi_config_vars: BTreeMap<String, String>,
        ignore_already_existing: bool,
        account_id: AccountId,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<WorkerId>;

    async fn connect(
        &self,
        worker_id: &WorkerId,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<WorkerStream<LogEvent>>;

    async fn delete(
        &self,
        worker_id: &WorkerId,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    /// Invokes a worker using raw `Val` parameter values and awaits its results returning
    /// it as a `TypeAnnotatedValue`.
    async fn invoke_and_await_typed(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Option<ValueAndType>>;

    /// Invokes a worker using raw `Val` parameter values and awaits its results returning
    /// a `Val` values (without type information)
    async fn invoke_and_await(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<InvokeResult>;

    /// Invokes a worker using JSON value encoding represented by raw strings and awaits its results
    /// returning it as a `TypeAnnotatedValue`. The input parameter JSONs cannot be converted to `Val`
    /// without type information so they get forwarded to the executor.
    async fn invoke_and_await_json(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<String>,
        invocation_context: Option<InvocationContext>,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Option<ValueAndType>>;

    /// Enqueues an invocation for the worker without awaiting its results, using raw `Val`
    /// parameters.
    async fn invoke(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    /// Enqueues an invocation for the worker without awaiting its results, using JSON value
    /// encoding represented as raw strings. Without type information these representations cannot
    /// be converted to `Val` so they get forwarded as-is to the executor.
    async fn invoke_json(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<String>,
        invocation_context: Option<InvocationContext>,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: u64,
        data: Vec<u8>,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool>;

    async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<WorkerMetadataDto>;

    async fn find_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<WorkerMetadataDto>)>;

    async fn resume(
        &self,
        worker_id: &WorkerId,
        force: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn update(
        &self,
        worker_id: &WorkerId,
        update_mode: WorkerUpdateMode,
        target_version: ComponentRevision,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn get_oplog(
        &self,
        worker_id: &WorkerId,
        from_oplog_index: OplogIndex,
        cursor: Option<OplogCursor>,
        count: u64,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError>;

    async fn search_oplog(
        &self,
        worker_id: &WorkerId,
        cursor: Option<OplogCursor>,
        count: u64,
        query: String,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError>;

    async fn get_file_system_node(
        &self,
        worker_id: &WorkerId,
        path: ComponentFilePath,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<ComponentFileSystemNode>>;

    async fn get_file_contents(
        &self,
        worker_id: &WorkerId,
        path: ComponentFilePath,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>>;

    async fn activate_plugin(
        &self,
        worker_id: &WorkerId,
        plugin_priority: PluginPriority,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn deactivate_plugin(
        &self,
        worker_id: &WorkerId,
        plugin_priority: PluginPriority,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn fork_worker(
        &self,
        source_worker_id: &WorkerId,
        target_worker_id: &WorkerId,
        oplog_index_cut_off: OplogIndex,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn revert_worker(
        &self,
        worker_id: &WorkerId,
        target: RevertWorkerTarget,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn cancel_invocation(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool>;
}

pub struct TypedResult {
    pub result: ValueAndType,
    pub function_result_types: Vec<AnalysedFunctionResult>,
}

#[derive(Clone)]
pub struct WorkerExecutorWorkerClient {
    worker_executor_clients: MultiTargetGrpcClient<WorkerExecutorClient<OtelGrpcService<Channel>>>,
    // NOTE: unlike other retries, reaching max_attempts for the worker executor
    //       (with retryable errors) does not end the retry loop,
    //       rather it emits a warn log and resets the retry state.
    worker_executor_retries: RetryConfig,
    routing_table_service: Arc<dyn RoutingTableService + Send + Sync>,
}

impl WorkerExecutorWorkerClient {
    pub fn new(
        worker_executor_clients: MultiTargetGrpcClient<
            WorkerExecutorClient<OtelGrpcService<Channel>>,
        >,
        worker_executor_retries: RetryConfig,
        routing_table_service: Arc<dyn RoutingTableService>,
    ) -> Self {
        Self {
            worker_executor_clients,
            worker_executor_retries,
            routing_table_service,
        }
    }

    async fn find_running_metadata_internal(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<WorkerMetadataDto>> {
        let component_id = *component_id;
        let result = self.call_worker_executor(
            AllExecutors,
            "get_running_workers_metadata",
            move |worker_executor_client| {
                let component_id: golem_api_grpc::proto::golem::component::ComponentId =
                    component_id.into();

                Box::pin(
                    worker_executor_client.get_running_workers_metadata(
                        workerexecutor::v1::GetRunningWorkersMetadataRequest {
                            component_id: Some(component_id),
                            filter: filter.clone().map(|f| f.into()),
                            auth_ctx: Some(auth_ctx.clone().into())
                        }
                    )
                )
            },
            |responses| {
                responses.into_iter().map(|response| {
                    match response.into_inner() {
                        workerexecutor::v1::GetRunningWorkersMetadataResponse {
                            result:
                            Some(workerexecutor::v1::get_running_workers_metadata_response::Result::Success(workerexecutor::v1::GetRunningWorkersMetadataSuccessResponse {
                                                                                                                workers
                                                                                                            })),
                        } => {
                            let workers: Vec<WorkerMetadataDto> = workers.into_iter().map(|w| w.try_into()).collect::<Result<Vec<_>, _>>().map_err(|_| WorkerExecutorError::unknown("Convert response error"))?;
                            Ok(workers)
                        }
                        workerexecutor::v1::GetRunningWorkersMetadataResponse {
                            result:
                            Some(workerexecutor::v1::get_running_workers_metadata_response::Result::Failure(err)),
                        } => Err(err.into()),
                        workerexecutor::v1::GetRunningWorkersMetadataResponse { .. } => {
                            Err("Empty response".into())
                        }
                    }
                }).collect::<Result<Vec<_>, ResponseMapResult>>()
            },
            WorkerServiceError::InternalCallError,
        ).await?;

        Ok(result.into_iter().flatten().collect())
    }

    async fn find_metadata_internal(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<WorkerMetadataDto>)> {
        let component_id = *component_id;
        let result = self
            .call_worker_executor(
                RandomExecutor,
                "get_workers_metadata",
                move |worker_executor_client| {
                    Box::pin(worker_executor_client.get_workers_metadata(
                        workerexecutor::v1::GetWorkersMetadataRequest {
                            component_id: Some(component_id.into()),
                            filter: filter.clone().map(|f| f.into()),
                            cursor: Some(cursor.clone().into()),
                            count,
                            precise,
                            environment_id: Some(environment_id.into()),
                            auth_ctx: Some(auth_ctx.clone().into()),
                        },
                    ))
                },
                |response| match response.into_inner() {
                    workerexecutor::v1::GetWorkersMetadataResponse {
                        result:
                            Some(workerexecutor::v1::get_workers_metadata_response::Result::Success(
                                workerexecutor::v1::GetWorkersMetadataSuccessResponse {
                                    workers,
                                    cursor,
                                },
                            )),
                    } => {
                        let workers = workers
                            .into_iter()
                            .map(|w| w.try_into())
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|err| {
                                WorkerExecutorError::unknown(format!(
                                    "Unexpected worker metadata in response: {err}"
                                ))
                            })?;
                        Ok((cursor.map(|c| c.into()), workers))
                    }
                    workerexecutor::v1::GetWorkersMetadataResponse {
                        result:
                            Some(workerexecutor::v1::get_workers_metadata_response::Result::Failure(
                                err,
                            )),
                    } => Err(err.into()),
                    workerexecutor::v1::GetWorkersMetadataResponse { .. } => {
                        Err("Empty response".into())
                    }
                },
                WorkerServiceError::InternalCallError,
            )
            .await?;

        Ok(result)
    }
}

impl HasRoutingTableService for WorkerExecutorWorkerClient {
    fn routing_table_service(&self) -> &Arc<dyn RoutingTableService + Send + Sync> {
        &self.routing_table_service
    }
}

impl HasWorkerExecutorClients for WorkerExecutorWorkerClient {
    fn worker_executor_clients(
        &self,
    ) -> &MultiTargetGrpcClient<WorkerExecutorClient<OtelGrpcService<Channel>>> {
        &self.worker_executor_clients
    }

    fn worker_executor_retry_config(&self) -> &RetryConfig {
        &self.worker_executor_retries
    }
}

#[async_trait]
impl WorkerClient for WorkerExecutorWorkerClient {
    async fn create(
        &self,
        worker_id: &WorkerId,
        component_version: ComponentRevision,
        environment_variables: HashMap<String, String>,
        config_vars: BTreeMap<String, String>,
        ignore_already_existing: bool,
        account_id: AccountId,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<WorkerId> {
        let worker_id_clone = worker_id.clone();
        let account_id_clone = account_id;
        self.call_worker_executor(
            worker_id.clone(),
            "create_worker",
            move |worker_executor_client| {
                let worker_id = worker_id_clone.clone();
                Box::pin(worker_executor_client.create_worker(CreateWorkerRequest {
                    worker_id: Some(worker_id.into()),
                    component_version: component_version.0,
                    env: environment_variables.clone(),
                    component_owner_account_id: Some(account_id_clone.into()),
                    environment_id: Some(environment_id.into()),
                    wasi_config_vars: Some(config_vars.clone().into()),
                    ignore_already_existing,
                    auth_ctx: Some(auth_ctx.clone().into()),
                }))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::CreateWorkerResponse {
                    result: Some(workerexecutor::v1::create_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::CreateWorkerResponse {
                    result: Some(workerexecutor::v1::create_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::CreateWorkerResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;

        Ok(worker_id.clone())
    }

    async fn connect(
        &self,
        worker_id: &WorkerId,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<WorkerStream<LogEvent>> {
        let worker_id_clone = worker_id.clone();
        let account_id_clone = account_id;
        let worker_id_err = worker_id.clone();
        let stream = self
            .call_worker_executor(
                worker_id.clone(),
                "connect_worker",
                move |worker_executor_client| {
                    Box::pin(worker_executor_client.connect_worker(ConnectWorkerRequest {
                        worker_id: Some(worker_id_clone.clone().into()),
                        component_owner_account_id: Some(account_id_clone.into()),
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into()),
                    }))
                },
                |response| Ok(WorkerStream::new(response.into_inner())),
                |error| match error {
                    CallWorkerExecutorError::FailedToConnectToPod(status)
                        if status.code() == Code::NotFound =>
                    {
                        WorkerServiceError::WorkerNotFound(worker_id_err.clone())
                    }
                    _ => WorkerServiceError::InternalCallError(error),
                },
            )
            .await?;

        Ok(stream)
    }

    async fn delete(
        &self,
        worker_id: &WorkerId,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let worker_id_clone = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            "delete_worker",
            move |worker_executor_client| {
                Box::pin(worker_executor_client.delete_worker(
                    workerexecutor::v1::DeleteWorkerRequest {
                        worker_id: Some(golem_api_grpc::proto::golem::worker::WorkerId::from(
                            worker_id_clone.clone(),
                        )),
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into()),
                    },
                ))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::DeleteWorkerResponse {
                    result: Some(workerexecutor::v1::delete_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::DeleteWorkerResponse {
                    result: Some(workerexecutor::v1::delete_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::DeleteWorkerResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;

        Ok(())
    }

    async fn invoke_and_await_typed(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Option<ValueAndType>> {
        let worker_id = worker_id.clone();
        let worker_id_clone = worker_id.clone();

        let invoke_response = self.call_worker_executor(
            worker_id.clone(),
            "invoke_and_await_worker_typed",
            move |worker_executor_client| {
                Box::pin(worker_executor_client.invoke_and_await_worker_typed(
                    InvokeAndAwaitWorkerRequest {
                        worker_id: Some(worker_id_clone.clone().into()),
                        name: function_name.clone(),
                        input: params.clone(),
                        idempotency_key: idempotency_key.clone().map(|v| v.into()),
                        component_owner_account_id: Some(account_id.into()),
                        context: invocation_context.clone(),
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into())
                    }
                )
                )
            },
            move |response| {
                match response.into_inner() {
                    workerexecutor::v1::InvokeAndAwaitWorkerResponseTyped {
                        result:
                        Some(workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Success(
                                 workerexecutor::v1::InvokeAndAwaitWorkerSuccessTyped {
                                     output
                                 },
                             )),
                    } => {
                        match output {
                            Some(vnt) => ValueAndType::try_from(vnt).map(Some).map_err(|err| WorkerExecutorError::unknown(err).into()),
                            None => Ok(None),
                        }
                    }
                    workerexecutor::v1::InvokeAndAwaitWorkerResponseTyped {
                        result:
                        Some(workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Failure(err)),
                    } => {
                        Err(err.into())
                    }
                    workerexecutor::v1::InvokeAndAwaitWorkerResponseTyped { .. } => {
                        Err("Empty response".into())
                    }
                }
            },
            WorkerServiceError::InternalCallError,
        ).await?;

        Ok(invoke_response)
    }

    async fn invoke_and_await(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<InvokeResult> {
        let worker_id = worker_id.clone();
        let worker_id_clone = worker_id.clone();

        let invoke_response = self.call_worker_executor(
            worker_id.clone(),
            "invoke_and_await_worker",
            move |worker_executor_client| {
                Box::pin(worker_executor_client.invoke_and_await_worker(
                    InvokeAndAwaitWorkerRequest {
                        worker_id: Some(worker_id_clone.clone().into()),
                        name: function_name.clone(),
                        input: params.clone(),
                        idempotency_key: idempotency_key.clone().map(|k| k.into()),
                        component_owner_account_id: Some(account_id.into()),
                        context: invocation_context.clone(),
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into())
                    }
                )
                )
            },
            move |response| {
                match response.into_inner() {
                    workerexecutor::v1::InvokeAndAwaitWorkerResponse {
                        result:
                        Some(workerexecutor::v1::invoke_and_await_worker_response::Result::Success(
                                 workerexecutor::v1::InvokeAndAwaitWorkerSuccess {
                                     output,
                                 },
                             )),
                    } => {
                        Ok(InvokeResult { result: output })
                    }
                    workerexecutor::v1::InvokeAndAwaitWorkerResponse {
                        result:
                        Some(workerexecutor::v1::invoke_and_await_worker_response::Result::Failure(err)),
                    } => {
                        Err(err.into())
                    }
                    workerexecutor::v1::InvokeAndAwaitWorkerResponse { .. } => {
                        Err("Empty response".into())
                    }
                }
            },
            WorkerServiceError::InternalCallError,
        ).await?;

        Ok(invoke_response)
    }

    async fn invoke_and_await_json(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<String>,
        invocation_context: Option<InvocationContext>,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Option<ValueAndType>> {
        let worker_id = worker_id.clone();
        let worker_id_clone = worker_id.clone();

        let invoke_response = self.call_worker_executor(
            worker_id.clone(),
            "invoke_and_await_worker_json",
            move |worker_executor_client| {
                Box::pin(worker_executor_client.invoke_and_await_worker_json(
                    InvokeAndAwaitWorkerJsonRequest {
                        worker_id: Some(worker_id_clone.clone().into()),
                        name: function_name.clone(),
                        input: params.clone(),
                        idempotency_key: idempotency_key.clone().map(|v| v.into()),
                        component_owner_account_id: Some(account_id.into()),
                        context: invocation_context.clone(),
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into())
                    }
                )
                )
            },
            move |response| {
                match response.into_inner() {
                    workerexecutor::v1::InvokeAndAwaitWorkerResponseTyped {
                        result:
                        Some(workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Success(
                                 workerexecutor::v1::InvokeAndAwaitWorkerSuccessTyped {
                                     output
                                 },
                             )),
                    } => {
                        match output {
                            Some(vnt) => {
                                ValueAndType::try_from(vnt).map(Some).map_err(|err| WorkerExecutorError::unknown(err).into())
                            }
                            None => Ok(None),
                        }
                    }
                    workerexecutor::v1::InvokeAndAwaitWorkerResponseTyped {
                        result:
                        Some(workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Failure(err)),
                    } => {
                        Err(err.into())
                    }
                    workerexecutor::v1::InvokeAndAwaitWorkerResponseTyped { .. } => {
                        Err("Empty response".into())
                    }
                }
            },
            WorkerServiceError::InternalCallError,
        ).await?;

        Ok(invoke_response)
    }

    async fn invoke(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            "invoke_worker",
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                Box::pin(worker_executor_client.invoke_worker(
                    workerexecutor::v1::InvokeWorkerRequest {
                        worker_id: Some(worker_id.into()),
                        idempotency_key: idempotency_key.clone().map(|k| k.into()),
                        name: function_name.clone(),
                        input: params.clone(),
                        component_owner_account_id: Some(account_id.into()),
                        context: invocation_context.clone(),
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into()),
                    },
                ))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::InvokeWorkerResponse {
                    result: Some(workerexecutor::v1::invoke_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::InvokeWorkerResponse {
                    result: Some(workerexecutor::v1::invoke_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::InvokeWorkerResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;
        Ok(())
    }

    async fn invoke_json(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<String>,
        invocation_context: Option<InvocationContext>,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            "invoke_worker_json",
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                Box::pin(worker_executor_client.invoke_worker_json(
                    workerexecutor::v1::InvokeJsonWorkerRequest {
                        worker_id: Some(worker_id.into()),
                        idempotency_key: idempotency_key.clone().map(|k| k.into()),
                        name: function_name.clone(),
                        input: params.clone(),
                        component_owner_account_id: Some(account_id.into()),
                        context: invocation_context.clone(),
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into()),
                    },
                ))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::InvokeWorkerResponse {
                    result: Some(workerexecutor::v1::invoke_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::InvokeWorkerResponse {
                    result: Some(workerexecutor::v1::invoke_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::InvokeWorkerResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;
        Ok(())
    }

    async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: u64,
        data: Vec<u8>,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool> {
        let promise_id = PromiseId {
            worker_id: worker_id.clone(),
            oplog_idx: OplogIndex::from_u64(oplog_id),
        };

        let result = self
            .call_worker_executor(
                worker_id.clone(),
                "complete_promise",
                move |worker_executor_client| {
                    let promise_id = promise_id.clone();
                    let data = data.clone();
                    Box::pin(
                        worker_executor_client
                            .complete_promise(CompletePromiseRequest {
                                promise_id: Some(promise_id.into()),
                                data,
                                environment_id: Some(environment_id.into()),
                                auth_ctx: Some(auth_ctx.clone().into())
                            })
                    )
                },
                |response| {
                    match response.into_inner() {
                        workerexecutor::v1::CompletePromiseResponse {
                            result:
                            Some(workerexecutor::v1::complete_promise_response::Result::Success(
                                     success,
                                 )),
                        } => Ok(success.completed),
                        workerexecutor::v1::CompletePromiseResponse {
                            result:
                            Some(workerexecutor::v1::complete_promise_response::Result::Failure(
                                     err,
                                 )),
                        } => Err(err.into()),
                        workerexecutor::v1::CompletePromiseResponse { .. } => {
                            Err("Empty response".into())
                        }
                    }
                },
                WorkerServiceError::InternalCallError,
            )
            .await?;
        Ok(result)
    }

    async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            "interrupt_worker",
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                Box::pin(
                    worker_executor_client.interrupt_worker(InterruptWorkerRequest {
                        worker_id: Some(worker_id.into()),
                        recover_immediately,
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into()),
                    }),
                )
            },
            |response| match response.into_inner() {
                workerexecutor::v1::InterruptWorkerResponse {
                    result: Some(workerexecutor::v1::interrupt_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::InterruptWorkerResponse {
                    result:
                        Some(workerexecutor::v1::interrupt_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::InterruptWorkerResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;

        Ok(())
    }

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<WorkerMetadataDto> {
        let worker_id = worker_id.clone();
        let metadata = self.call_worker_executor(
            worker_id.clone(),
            "get_metadata",
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                Box::pin(worker_executor_client.get_worker_metadata(
                    workerexecutor::v1::GetWorkerMetadataRequest {
                        worker_id: Some(golem_api_grpc::proto::golem::worker::WorkerId::from(worker_id.clone())),
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into())
                    }
                ))
            },
            |response| {
                match response.into_inner() {
                    workerexecutor::v1::GetWorkerMetadataResponse {
                        result:
                        Some(workerexecutor::v1::get_worker_metadata_response::Result::Success(metadata)),
                    } => {
                        Ok(metadata.try_into().unwrap())
                    }
                    workerexecutor::v1::GetWorkerMetadataResponse {
                        result:
                        Some(workerexecutor::v1::get_worker_metadata_response::Result::Failure(err)),
                    } => {
                        Err(err.into())
                    }
                    workerexecutor::v1::GetWorkerMetadataResponse { .. } => {
                        Err("Empty response".into())
                    }
                }
            },
            WorkerServiceError::InternalCallError,
        ).await?;

        Ok(metadata)
    }

    async fn find_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<WorkerMetadataDto>)> {
        if filter.as_ref().is_some_and(is_filter_with_running_status) {
            let result = self
                .find_running_metadata_internal(component_id, filter, auth_ctx)
                .await?;

            Ok((None, result.into_iter().take(count as usize).collect()))
        } else {
            self.find_metadata_internal(
                component_id,
                filter,
                cursor,
                count,
                precise,
                environment_id,
                auth_ctx,
            )
            .await
        }
    }

    async fn resume(
        &self,
        worker_id: &WorkerId,
        force: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            "resume_worker",
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                Box::pin(worker_executor_client.resume_worker(ResumeWorkerRequest {
                    worker_id: Some(worker_id.into()),
                    force: Some(force),
                    environment_id: Some(environment_id.into()),
                    auth_ctx: Some(auth_ctx.clone().into()),
                }))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::ResumeWorkerResponse {
                    result: Some(workerexecutor::v1::resume_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::ResumeWorkerResponse {
                    result: Some(workerexecutor::v1::resume_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::ResumeWorkerResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;
        Ok(())
    }

    async fn update(
        &self,
        worker_id: &WorkerId,
        update_mode: WorkerUpdateMode,
        target_version: ComponentRevision,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            "update_worker",
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                Box::pin(worker_executor_client.update_worker(UpdateWorkerRequest {
                    worker_id: Some(worker_id.into()),
                    mode: golem_api_grpc::proto::golem::worker::UpdateMode::from(update_mode)
                        as i32,
                    target_version: target_version.0,
                    environment_id: Some(environment_id.into()),
                    auth_ctx: Some(auth_ctx.clone().into()),
                }))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::UpdateWorkerResponse {
                    result: Some(workerexecutor::v1::update_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::UpdateWorkerResponse {
                    result: Some(workerexecutor::v1::update_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::UpdateWorkerResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;
        Ok(())
    }

    async fn get_oplog(
        &self,
        worker_id: &WorkerId,
        from_oplog_index: OplogIndex,
        cursor: Option<OplogCursor>,
        count: u64,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            "get_oplog",
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                Box::pin(
                    worker_executor_client.get_oplog(workerexecutor::v1::GetOplogRequest {
                        worker_id: Some(worker_id.into()),
                        from_oplog_index: from_oplog_index.into(),
                        cursor: cursor.clone().map(|c| c.into()),
                        count,
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into()),
                    }),
                )
            },
            |response| match response.into_inner() {
                workerexecutor::v1::GetOplogResponse {
                    result:
                        Some(workerexecutor::v1::get_oplog_response::Result::Success(
                            workerexecutor::v1::GetOplogSuccessResponse {
                                entries,
                                next,
                                first_index_in_chunk,
                                last_index,
                            },
                        )),
                } => {
                    let entries: Vec<PublicOplogEntry> = entries
                        .into_iter()
                        .map(|e| e.try_into())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|err| {
                            WorkerExecutorError::unknown(format!(
                                "Unexpected oplog entries in error: {err}"
                            ))
                        })?;
                    Ok(GetOplogResponse {
                        entries: entries
                            .into_iter()
                            .enumerate()
                            .map(|(idx, entry)| PublicOplogEntryWithIndex {
                                oplog_index: OplogIndex::from_u64(
                                    (first_index_in_chunk) + idx as u64,
                                ),
                                entry,
                            })
                            .collect(),
                        next: next.map(|c| c.into()),
                        first_index_in_chunk,
                        last_index,
                    })
                }
                workerexecutor::v1::GetOplogResponse {
                    result: Some(workerexecutor::v1::get_oplog_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::GetOplogResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await
    }

    async fn search_oplog(
        &self,
        worker_id: &WorkerId,
        cursor: Option<OplogCursor>,
        count: u64,
        query: String,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            "search_oplog",
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                let query_clone = query.clone();
                Box::pin(
                    worker_executor_client.search_oplog(workerexecutor::v1::SearchOplogRequest {
                        worker_id: Some(worker_id.into()),
                        query: query_clone,
                        cursor: cursor.clone().map(|c| c.into()),
                        count,
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into())
                    }),
                )
            },
            |response| match response.into_inner() {
                workerexecutor::v1::SearchOplogResponse {
                    result:
                    Some(golem_api_grpc::proto::golem::workerexecutor::v1::search_oplog_response::Result::Success(
                             workerexecutor::v1::SearchOplogSuccessResponse {
                                 entries,
                                 next,
                                 last_index,
                             },
                         )),
                } => {
                    let entries: Vec<PublicOplogEntryWithIndex> = entries
                        .into_iter()
                        .map(|e| e.try_into())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|err| WorkerExecutorError::unknown(format!("Unexpected oplog entries in error: {err}")))?;
                    let first_index_in_chunk = entries.first().map(|entry| entry.oplog_index).unwrap_or(OplogIndex::INITIAL).into();
                    Ok(GetOplogResponse {
                        entries,
                        next: next.map(|c| c.into()),
                        first_index_in_chunk,
                        last_index,
                    })
                }
                SearchOplogResponse {
                    result: Some(workerexecutor::v1::search_oplog_response::Result::Failure(err)),
                } => Err(err.into()),
                SearchOplogResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
            .await
    }

    async fn get_file_system_node(
        &self,
        worker_id: &WorkerId,
        path: ComponentFilePath,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<ComponentFileSystemNode>> {
        let worker_id = worker_id.clone();
        let path_clone = path.clone();
        self.call_worker_executor(
            worker_id.clone(),
            "get_file_system_node",
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                Box::pin(
                    worker_executor_client.get_file_system_node(workerexecutor::v1::GetFileSystemNodeRequest {
                        worker_id: Some(worker_id.into()),
                        component_owner_account_id: Some(account_id.into()),
                        path: path_clone.to_string(),
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into())
                    }),
                )
            },
            |response| match response.into_inner() {
                workerexecutor::v1::GetFileSystemNodeResponse {
                    result: Some(golem_api_grpc::proto::golem::workerexecutor::v1::get_file_system_node_response::Result::DirSuccess(success)),
                } => {
                    success.nodes
                        .into_iter()
                        .map(|v|
                            v
                                .try_into()
                                .map_err(|_| "Failed to convert node".into())
                        )
                        .collect::<Result<Vec<_>, _>>()
                }
                workerexecutor::v1::GetFileSystemNodeResponse {
                    result: Some(workerexecutor::v1::get_file_system_node_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::GetFileSystemNodeResponse {
                    result: Some(workerexecutor::v1::get_file_system_node_response::Result::NotFound(_)),
                } => Err(WorkerServiceError::FileNotFound(path.clone()).into()),
                workerexecutor::v1::GetFileSystemNodeResponse {
                    result: Some(workerexecutor::v1::get_file_system_node_response::Result::FileSuccess(file_response)),
                } => {
                    let file_node = file_response.file
                        .ok_or(WorkerServiceError::Internal("Missing file data in response".to_string()))?
                        .try_into()
                        .map_err(|_| WorkerServiceError::Internal("Failed to convert file node".to_string()))?;
                    Ok(vec![file_node])
                },
                workerexecutor::v1::GetFileSystemNodeResponse {
                    result: None
                } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
            .await
    }

    async fn get_file_contents(
        &self,
        worker_id: &WorkerId,
        path: ComponentFilePath,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>> {
        let worker_id = worker_id.clone();
        let path_clone = path.clone();
        let stream = self
            .call_worker_executor(
                worker_id.clone(),
                "read_file",
                move |worker_executor_client| {
                    Box::pin(worker_executor_client.get_file_contents(
                        workerexecutor::v1::GetFileContentsRequest {
                            worker_id: Some(worker_id.clone().into()),
                            component_owner_account_id: Some(account_id.into()),
                            file_path: path_clone.to_string(),
                            environment_id: Some(environment_id.into()),
                            auth_ctx: Some(auth_ctx.clone().into()),
                        },
                    ))
                },
                |response| Ok(WorkerStream::new(response.into_inner())),
                WorkerServiceError::InternalCallError,
            )
            .await?;

        let (header, stream) = stream.into_future().await;

        let header = header.ok_or(WorkerServiceError::Internal("Empty stream".to_string()))?;

        match header
            .map_err(|_| WorkerServiceError::Internal("Stream error".to_string()))?
            .result
        {
            Some(workerexecutor::v1::get_file_contents_response::Result::Success(_)) => Err(
                WorkerServiceError::Internal("Protocal violation".to_string()),
            ),
            Some(workerexecutor::v1::get_file_contents_response::Result::Failure(err)) => {
                let converted = WorkerExecutorError::try_from(err).map_err(|err| {
                    WorkerServiceError::Internal(format!("Failed converting errors {err}"))
                })?;
                Err(converted.into())
            }
            Some(workerexecutor::v1::get_file_contents_response::Result::Header(header)) => {
                match header.result {
                    Some(
                        workerexecutor::v1::get_file_contents_response_header::Result::Success(_),
                    ) => Ok(()),
                    Some(
                        workerexecutor::v1::get_file_contents_response_header::Result::NotAFile(_),
                    ) => Err(WorkerServiceError::BadFileType(path)),
                    Some(
                        workerexecutor::v1::get_file_contents_response_header::Result::NotFound(_),
                    ) => Err(WorkerServiceError::FileNotFound(path)),
                    None => Err(WorkerServiceError::Internal("Empty response".to_string())),
                }
            }
            None => Err(WorkerServiceError::Internal("Empty response".to_string())),
        }?;

        let stream = stream
            .map_err(|_| WorkerServiceError::Internal("Stream error".to_string()))
            .map(|item| {
                item.and_then(|response| {
                    response
                        .result
                        .ok_or(WorkerServiceError::Internal("Malformed chunk".to_string()))
                })
            })
            .map_ok(|chunk| match chunk {
                workerexecutor::v1::get_file_contents_response::Result::Success(bytes) => {
                    Ok(Bytes::from(bytes))
                }
                workerexecutor::v1::get_file_contents_response::Result::Failure(err) => {
                    let converted = WorkerExecutorError::try_from(err)
                        .map_err(|err| {
                            WorkerServiceError::Internal(format!("Failed converting errors {err}"))
                        })?
                        .into();
                    Err(converted)
                }
                workerexecutor::v1::get_file_contents_response::Result::Header(_) => Err(
                    WorkerServiceError::Internal("Unexpected header".to_string()),
                ),
            })
            .map(|item| item.and_then(|inner| inner));

        Ok(Box::pin(stream))
    }

    async fn activate_plugin(
        &self,
        worker_id: &WorkerId,
        plugin_priority: PluginPriority,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            "activate_plugin",
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                Box::pin(
                    worker_executor_client.activate_plugin(ActivatePluginRequest {
                        worker_id: Some(worker_id.into()),
                        plugin_priority: plugin_priority.0,
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into())
                    }),
                )
            },
            |response| match response.into_inner() {
                workerexecutor::v1::ActivatePluginResponse {
                    result: Some(workerexecutor::v1::activate_plugin_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::ActivatePluginResponse {
                    result:
                    Some(workerexecutor::v1::activate_plugin_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::ActivatePluginResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
            .await?;

        Ok(())
    }

    async fn deactivate_plugin(
        &self,
        worker_id: &WorkerId,
        plugin_priority: PluginPriority,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            "deactivate_plugin",
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                Box::pin(
                    worker_executor_client.deactivate_plugin(DeactivatePluginRequest {
                        worker_id: Some(worker_id.into()),
                        plugin_priority: plugin_priority.0,
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into())
                    }),
                )
            },
            |response| match response.into_inner() {
                workerexecutor::v1::DeactivatePluginResponse {
                    result: Some(workerexecutor::v1::deactivate_plugin_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::DeactivatePluginResponse {
                    result:
                    Some(workerexecutor::v1::deactivate_plugin_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::DeactivatePluginResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
            .await?;

        Ok(())
    }

    async fn fork_worker(
        &self,
        source_worker_id: &WorkerId,
        target_worker_id: &WorkerId,
        oplog_index_cut_off: OplogIndex,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let source_worker_id = source_worker_id.clone();
        let target_worker_id = target_worker_id.clone();
        self.call_worker_executor(
            source_worker_id.clone(),
            "fork_worker",
            move |worker_executor_client| {
                let source_worker_id = source_worker_id.clone();
                let target_worker_id = target_worker_id.clone();
                Box::pin(worker_executor_client.fork_worker(ForkWorkerRequest {
                    source_worker_id: Some(source_worker_id.into()),
                    target_worker_id: Some(target_worker_id.into()),
                    component_owner_account_id: Some(account_id.into()),
                    oplog_index_cutoff: oplog_index_cut_off.into(),
                    environment_id: Some(environment_id.into()),
                    auth_ctx: Some(auth_ctx.clone().into()),
                }))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::ForkWorkerResponse {
                    result: Some(workerexecutor::v1::fork_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::ForkWorkerResponse {
                    result: Some(workerexecutor::v1::fork_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::ForkWorkerResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;
        Ok(())
    }

    async fn revert_worker(
        &self,
        worker_id: &WorkerId,
        target: RevertWorkerTarget,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            "revert_worker",
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                let target = target.clone();
                Box::pin(worker_executor_client.revert_worker(RevertWorkerRequest {
                    worker_id: Some(worker_id.into()),
                    target: Some(target.into()),
                    environment_id: Some(environment_id.into()),
                    auth_ctx: Some(auth_ctx.clone().into()),
                }))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::RevertWorkerResponse {
                    result: Some(workerexecutor::v1::revert_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::RevertWorkerResponse {
                    result: Some(workerexecutor::v1::revert_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::RevertWorkerResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;
        Ok(())
    }

    async fn cancel_invocation(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool> {
        let worker_id = worker_id.clone();
        let idempotency_key = idempotency_key.clone();
        let canceled = self.call_worker_executor(
            worker_id.clone(),
            "cancel_invocation",
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                let idempotency_key = idempotency_key.clone();
                Box::pin(worker_executor_client.cancel_invocation(CancelInvocationRequest {
                    worker_id: Some(worker_id.into()),
                    idempotency_key: Some(idempotency_key.into()),
                    environment_id: Some(environment_id.into()),
                    auth_ctx: Some(auth_ctx.clone().into()),
                }))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::CancelInvocationResponse {
                    result: Some(workerexecutor::v1::cancel_invocation_response::Result::Success(canceled)),
                } => Ok(canceled),
                workerexecutor::v1::CancelInvocationResponse {
                    result: Some(workerexecutor::v1::cancel_invocation_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::CancelInvocationResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
            .await?;
        Ok(canceled)
    }
}

fn is_filter_with_running_status(filter: &WorkerFilter) -> bool {
    match filter {
        WorkerFilter::Status(f)
            if f.value == WorkerStatus::Running && f.comparator == FilterComparator::Equal =>
        {
            true
        }
        WorkerFilter::And(f) => f.filters.iter().any(is_filter_with_running_status),
        _ => false,
    }
}
