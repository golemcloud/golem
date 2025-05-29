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

use crate::components::component_service::ComponentService;
use crate::components::worker_executor::WorkerExecutor;
use crate::components::worker_service::{
    ApiDefinitionServiceClient, ApiDeploymentServiceClient, ApiSecurityServiceClient,
    WorkerLogEventStream, WorkerService, WorkerServiceClient,
};
use crate::config::GolemClientProtocol;
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;
use golem_api_grpc::proto::golem::common::{Empty, ResourceLimits};
use golem_api_grpc::proto::golem::worker::v1::{
    revert_worker_response, CancelInvocationRequest, CancelInvocationResponse,
    ConnectWorkerRequest, DeleteWorkerRequest, DeleteWorkerResponse, ForkWorkerRequest,
    ForkWorkerResponse, GetFileContentsRequest, GetOplogRequest, GetOplogResponse,
    GetOplogSuccessResponse, GetWorkerMetadataRequest, GetWorkerMetadataResponse,
    InterruptWorkerRequest, InterruptWorkerResponse, InvokeAndAwaitJsonRequest,
    InvokeAndAwaitJsonResponse, InvokeAndAwaitResponse, InvokeAndAwaitTypedResponse,
    InvokeJsonRequest, InvokeResponse, LaunchNewWorkerRequest, LaunchNewWorkerResponse,
    LaunchNewWorkerSuccessResponse, ListDirectoryRequest, ListDirectoryResponse,
    ListDirectorySuccessResponse, ResumeWorkerRequest, ResumeWorkerResponse, RevertWorkerRequest,
    RevertWorkerResponse, SearchOplogRequest, SearchOplogResponse, SearchOplogSuccessResponse,
    UpdateWorkerRequest, UpdateWorkerResponse, WorkerError,
};
use golem_api_grpc::proto::golem::worker::{
    IdempotencyKey, InvocationContext, InvokeResult, InvokeResultTyped, LogEvent, TargetWorkerId,
    WorkerId,
};
use golem_api_grpc::proto::golem::workerexecutor::v1::CreateWorkerRequest;
use golem_api_grpc::proto::golem::{worker, workerexecutor};
use golem_common::model::AccountId;
use golem_wasm_rpc::ValueAndType;
use std::sync::Arc;
use tonic::Streaming;

use super::WorkerServiceInternal;

pub struct ForwardingWorkerService {
    worker_executor: Arc<dyn WorkerExecutor + Send + Sync + 'static>,
    component_service: Arc<dyn ComponentService>,
}

impl ForwardingWorkerService {
    pub fn new(
        worker_executor: Arc<dyn WorkerExecutor + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService>,
    ) -> Self {
        Self {
            worker_executor,
            component_service,
        }
    }

    fn should_retry<R>(retry_count: &mut usize, result: &Result<R, tonic::Status>) -> bool {
        if let Err(status) = result {
            if *retry_count > 0 && status.code() == tonic::Code::Unavailable {
                *retry_count -= 1;
                return true;
            }
        }
        false
    }

    const RETRY_COUNT: usize = 5;
}

impl WorkerServiceInternal for ForwardingWorkerService {
    fn client_protocol(&self) -> GolemClientProtocol {
        panic!("There is no worker-service, cannot get client protocol")
    }

    fn worker_client(&self) -> WorkerServiceClient {
        panic!("There is no worker-service, cannot create worker client")
    }

    fn api_definition_client(&self) -> ApiDefinitionServiceClient {
        panic!("There is no worker-service, cannot create api-definition client")
    }

    fn api_deployment_client(&self) -> ApiDeploymentServiceClient {
        panic!("There is no worker-service, cannot create api-deployment client")
    }

    fn api_security_client(&self) -> ApiSecurityServiceClient {
        panic!("There is no worker-service, cannot create api-security client")
    }

    fn component_service(&self) -> &Arc<dyn ComponentService> {
        &self.component_service
    }
}

#[async_trait]
impl WorkerService for ForwardingWorkerService {
    async fn create_worker(
        &self,
        request: LaunchNewWorkerRequest,
    ) -> crate::Result<LaunchNewWorkerResponse> {
        let component_id = (*request
            .component_id
            .as_ref()
            .ok_or(anyhow!("Requires component ID"))?)
        .try_into()
        .map_err(|err: String| anyhow!(err))?;
        let worker_id = WorkerId {
            component_id: request.component_id,
            name: request.name,
        };
        let latest_component_version = self
            .component_service
            .get_latest_version(&component_id)
            .await;

        let mut retry_count = Self::RETRY_COUNT;
        let response = loop {
            let result = self
                .worker_executor
                .client()
                .await?
                .create_worker(CreateWorkerRequest {
                    worker_id: Some(worker_id.clone()),
                    component_version: latest_component_version,
                    args: request.args.clone(),
                    env: request.env.clone(),
                    account_id: Some(
                        AccountId {
                            value: "test-account".to_string(),
                        }
                        .into(),
                    ),
                    account_limits: Some(ResourceLimits {
                        available_fuel: i64::MAX,
                        max_memory_per_worker: i64::MAX,
                    }),
                })
                .await;

            if Self::should_retry(&mut retry_count, &result) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            } else {
                break result;
            }
        };
        let response = response?.into_inner();

        match response.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor create-worker call"
            )),
            Some(workerexecutor::v1::create_worker_response::Result::Success(_)) => {
                Ok(LaunchNewWorkerResponse {
                    result: Some(worker::v1::launch_new_worker_response::Result::Success(
                        LaunchNewWorkerSuccessResponse {
                            worker_id: Some(worker_id),
                            component_version: latest_component_version,
                        },
                    )),
                })
            }
            Some(workerexecutor::v1::create_worker_response::Result::Failure(error)) => {
                Ok(LaunchNewWorkerResponse {
                    result: Some(worker::v1::launch_new_worker_response::Result::Error(
                        WorkerError {
                            error: Some(worker::v1::worker_error::Error::InternalError(error)),
                        },
                    )),
                })
            }
        }
    }

    async fn delete_worker(
        &self,
        request: DeleteWorkerRequest,
    ) -> crate::Result<DeleteWorkerResponse> {
        let mut retry_count = Self::RETRY_COUNT;
        let result = loop {
            let result = self
                .worker_executor
                .client()
                .await?
                .delete_worker(workerexecutor::v1::DeleteWorkerRequest {
                    worker_id: request.worker_id.clone(),
                    account_id: Some(
                        AccountId {
                            value: "test-account".to_string(),
                        }
                        .into(),
                    ),
                })
                .await;

            if Self::should_retry(&mut retry_count, &result) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            } else {
                break result;
            }
        };
        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor delete-worker call"
            )),
            Some(workerexecutor::v1::delete_worker_response::Result::Success(_)) => {
                Ok(DeleteWorkerResponse {
                    result: Some(worker::v1::delete_worker_response::Result::Success(
                        Empty {},
                    )),
                })
            }
            Some(workerexecutor::v1::delete_worker_response::Result::Failure(error)) => {
                Ok(DeleteWorkerResponse {
                    result: Some(worker::v1::delete_worker_response::Result::Error(
                        WorkerError {
                            error: Some(worker::v1::worker_error::Error::InternalError(error)),
                        },
                    )),
                })
            }
        }
    }

    async fn get_worker_metadata(
        &self,
        request: GetWorkerMetadataRequest,
    ) -> crate::Result<GetWorkerMetadataResponse> {
        let mut retry_count = Self::RETRY_COUNT;
        let result = loop {
            let result = self
                .worker_executor
                .client()
                .await?
                .get_worker_metadata(workerexecutor::v1::GetWorkerMetadataRequest {
                    worker_id: Some(
                        request
                            .worker_id
                            .clone()
                            .ok_or(anyhow!("Worker ID is required"))?,
                    ),
                    account_id: Some(
                        AccountId {
                            value: "test-account".to_string(),
                        }
                        .into(),
                    ),
                })
                .await;

            if Self::should_retry(&mut retry_count, &result) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            } else {
                break result;
            }
        };
        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor get-worker-metadata call"
            )),
            Some(workerexecutor::v1::get_worker_metadata_response::Result::Success(metadata)) => {
                Ok(GetWorkerMetadataResponse {
                    result: Some(worker::v1::get_worker_metadata_response::Result::Success(
                        metadata,
                    )),
                })
            }
            Some(workerexecutor::v1::get_worker_metadata_response::Result::Failure(error)) => {
                Ok(GetWorkerMetadataResponse {
                    result: Some(worker::v1::get_worker_metadata_response::Result::Error(
                        WorkerError {
                            error: Some(worker::v1::worker_error::Error::InternalError(error)),
                        },
                    )),
                })
            }
        }
    }

    async fn invoke(
        &self,
        worker_id: TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        invoke_parameters: Vec<ValueAndType>,
        context: Option<InvocationContext>,
    ) -> crate::Result<InvokeResponse> {
        let mut retry_count = Self::RETRY_COUNT;
        let result = loop {
            let result = self
                .worker_executor
                .client()
                .await?
                .invoke_worker(workerexecutor::v1::InvokeWorkerRequest {
                    worker_id: Some(worker_id.clone()),
                    idempotency_key: idempotency_key.clone(),
                    name: function.clone(),
                    input: invoke_parameters
                        .clone()
                        .into_iter()
                        .map(|param| param.value.into())
                        .collect(),
                    account_id: Some(
                        AccountId {
                            value: "test-account".to_string(),
                        }
                        .into(),
                    ),
                    account_limits: Some(ResourceLimits {
                        available_fuel: i64::MAX,
                        max_memory_per_worker: i64::MAX,
                    }),
                    context: context.clone(),
                })
                .await;

            if Self::should_retry(&mut retry_count, &result) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            } else {
                break result;
            }
        };
        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor invoke call"
            )),
            Some(workerexecutor::v1::invoke_worker_response::Result::Success(empty)) => {
                Ok(InvokeResponse {
                    result: Some(worker::v1::invoke_response::Result::Success(empty)),
                })
            }
            Some(workerexecutor::v1::invoke_worker_response::Result::Failure(error)) => {
                Ok(InvokeResponse {
                    result: Some(worker::v1::invoke_response::Result::Error(WorkerError {
                        error: Some(worker::v1::worker_error::Error::InternalError(error)),
                    })),
                })
            }
        }
    }

    async fn invoke_json(&self, _request: InvokeJsonRequest) -> crate::Result<InvokeResponse> {
        panic!("invoke_json can only be used through worker service");
    }

    async fn invoke_and_await(
        &self,
        worker_id: TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        invoke_parameters: Vec<ValueAndType>,
        context: Option<InvocationContext>,
    ) -> crate::Result<InvokeAndAwaitResponse> {
        let mut retry_count = Self::RETRY_COUNT;
        let result = loop {
            let result = self
                .worker_executor
                .client()
                .await?
                .invoke_and_await_worker(workerexecutor::v1::InvokeAndAwaitWorkerRequest {
                    worker_id: Some(worker_id.clone()),
                    idempotency_key: idempotency_key.clone(),
                    name: function.clone(),
                    input: invoke_parameters
                        .clone()
                        .into_iter()
                        .map(|param| param.value.into())
                        .collect(),
                    account_id: Some(
                        AccountId {
                            value: "test-account".to_string(),
                        }
                        .into(),
                    ),
                    account_limits: Some(ResourceLimits {
                        available_fuel: i64::MAX,
                        max_memory_per_worker: i64::MAX,
                    }),
                    context: context.clone(),
                })
                .await;

            if Self::should_retry(&mut retry_count, &result) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            } else {
                break result;
            }
        };
        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor invoke call"
            )),
            Some(workerexecutor::v1::invoke_and_await_worker_response::Result::Success(result)) => {
                Ok(InvokeAndAwaitResponse {
                    result: Some(worker::v1::invoke_and_await_response::Result::Success(
                        InvokeResult {
                            result: result.output,
                        },
                    )),
                })
            }
            Some(workerexecutor::v1::invoke_and_await_worker_response::Result::Failure(error)) => {
                Ok(InvokeAndAwaitResponse {
                    result: Some(worker::v1::invoke_and_await_response::Result::Error(
                        WorkerError {
                            error: Some(worker::v1::worker_error::Error::InternalError(error)),
                        },
                    )),
                })
            }
        }
    }

    async fn invoke_and_await_typed(
        &self,
        worker_id: TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        invoke_parameters: Vec<ValueAndType>,
        context: Option<InvocationContext>,
    ) -> crate::Result<InvokeAndAwaitTypedResponse> {
        let mut retry_count = Self::RETRY_COUNT;
        let result = loop {
            let result = self
                .worker_executor
                .client()
                .await?
                .invoke_and_await_worker_typed(workerexecutor::v1::InvokeAndAwaitWorkerRequest {
                    worker_id: Some(worker_id.clone()),
                    idempotency_key: idempotency_key.clone(),
                    name: function.clone(),
                    input: invoke_parameters
                        .clone()
                        .into_iter()
                        .map(|param| param.value.into())
                        .collect(),
                    account_id: Some(
                        AccountId {
                            value: "test-account".to_string(),
                        }
                        .into(),
                    ),
                    account_limits: Some(ResourceLimits {
                        available_fuel: i64::MAX,
                        max_memory_per_worker: i64::MAX,
                    }),
                    context: context.clone(),
                })
                .await;

            if Self::should_retry(&mut retry_count, &result) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            } else {
                break result;
            }
        };
        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor invoke call"
            )),
            Some(workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Success(
                result,
            )) => Ok(InvokeAndAwaitTypedResponse {
                result: Some(
                    worker::v1::invoke_and_await_typed_response::Result::Success(
                        InvokeResultTyped {
                            result: result.output,
                        },
                    ),
                ),
            }),
            Some(workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Failure(
                error,
            )) => Ok(InvokeAndAwaitTypedResponse {
                result: Some(worker::v1::invoke_and_await_typed_response::Result::Error(
                    WorkerError {
                        error: Some(worker::v1::worker_error::Error::InternalError(error)),
                    },
                )),
            }),
        }
    }

    async fn invoke_and_await_json(
        &self,
        _request: InvokeAndAwaitJsonRequest,
    ) -> crate::Result<InvokeAndAwaitJsonResponse> {
        panic!("invoke_and_await_json can only be used through worker service");
    }

    async fn connect_worker(
        &self,
        request: ConnectWorkerRequest,
    ) -> crate::Result<Box<dyn WorkerLogEventStream>> {
        let mut retry_count = Self::RETRY_COUNT;
        let result = loop {
            let result = self
                .worker_executor
                .client()
                .await?
                .connect_worker(workerexecutor::v1::ConnectWorkerRequest {
                    worker_id: request.worker_id.clone(),
                    account_id: Some(
                        AccountId {
                            value: "test-account".to_string(),
                        }
                        .into(),
                    ),
                    account_limits: Some(ResourceLimits {
                        available_fuel: i64::MAX,
                        max_memory_per_worker: i64::MAX,
                    }),
                })
                .await;

            if Self::should_retry(&mut retry_count, &result) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            } else {
                break result;
            }
        };
        let result = result?.into_inner();

        Ok(Box::new(
            GrpcForwardingWorkerLogEventStream::new(result).await?,
        ))
    }

    async fn resume_worker(
        &self,
        request: ResumeWorkerRequest,
    ) -> crate::Result<ResumeWorkerResponse> {
        let mut retry_count = Self::RETRY_COUNT;
        let result = loop {
            let result = self
                .worker_executor
                .client()
                .await?
                .resume_worker(workerexecutor::v1::ResumeWorkerRequest {
                    worker_id: request.worker_id.clone(),
                    account_id: Some(
                        AccountId {
                            value: "test-account".to_string(),
                        }
                        .into(),
                    ),
                    force: request.force,
                })
                .await;

            if Self::should_retry(&mut retry_count, &result) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            } else {
                break result;
            }
        };
        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor delete-worker call"
            )),
            Some(workerexecutor::v1::resume_worker_response::Result::Success(_)) => {
                Ok(ResumeWorkerResponse {
                    result: Some(worker::v1::resume_worker_response::Result::Success(
                        Empty {},
                    )),
                })
            }
            Some(workerexecutor::v1::resume_worker_response::Result::Failure(error)) => {
                Ok(ResumeWorkerResponse {
                    result: Some(worker::v1::resume_worker_response::Result::Error(
                        WorkerError {
                            error: Some(worker::v1::worker_error::Error::InternalError(error)),
                        },
                    )),
                })
            }
        }
    }

    async fn interrupt_worker(
        &self,
        request: InterruptWorkerRequest,
    ) -> crate::Result<InterruptWorkerResponse> {
        let mut retry_count = Self::RETRY_COUNT;
        let result = loop {
            let result = self
                .worker_executor
                .client()
                .await?
                .interrupt_worker(workerexecutor::v1::InterruptWorkerRequest {
                    worker_id: request.worker_id.clone(),
                    recover_immediately: request.recover_immediately,
                    account_id: Some(
                        AccountId {
                            value: "test-account".to_string(),
                        }
                        .into(),
                    ),
                })
                .await;

            if Self::should_retry(&mut retry_count, &result) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            } else {
                break result;
            }
        };
        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor delete-worker call"
            )),
            Some(workerexecutor::v1::interrupt_worker_response::Result::Success(_)) => {
                Ok(InterruptWorkerResponse {
                    result: Some(worker::v1::interrupt_worker_response::Result::Success(
                        Empty {},
                    )),
                })
            }
            Some(workerexecutor::v1::interrupt_worker_response::Result::Failure(error)) => {
                Ok(InterruptWorkerResponse {
                    result: Some(worker::v1::interrupt_worker_response::Result::Error(
                        WorkerError {
                            error: Some(worker::v1::worker_error::Error::InternalError(error)),
                        },
                    )),
                })
            }
        }
    }

    async fn update_worker(
        &self,
        request: UpdateWorkerRequest,
    ) -> crate::Result<UpdateWorkerResponse> {
        let mut retry_count = Self::RETRY_COUNT;
        let result = loop {
            let result = self
                .worker_executor
                .client()
                .await?
                .update_worker(workerexecutor::v1::UpdateWorkerRequest {
                    worker_id: request.worker_id.clone(),
                    target_version: request.target_version,
                    mode: request.mode,
                    account_id: Some(
                        AccountId {
                            value: "test-account".to_string(),
                        }
                        .into(),
                    ),
                })
                .await;

            if Self::should_retry(&mut retry_count, &result) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            } else {
                break result;
            }
        };
        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor delete-worker call"
            )),
            Some(workerexecutor::v1::update_worker_response::Result::Success(_)) => {
                Ok(UpdateWorkerResponse {
                    result: Some(worker::v1::update_worker_response::Result::Success(
                        Empty {},
                    )),
                })
            }
            Some(workerexecutor::v1::update_worker_response::Result::Failure(error)) => {
                Ok(UpdateWorkerResponse {
                    result: Some(worker::v1::update_worker_response::Result::Error(
                        WorkerError {
                            error: Some(worker::v1::worker_error::Error::InternalError(error)),
                        },
                    )),
                })
            }
        }
    }

    async fn get_oplog(&self, request: GetOplogRequest) -> crate::Result<GetOplogResponse> {
        let mut retry_count = Self::RETRY_COUNT;
        let result = loop {
            let result = self
                .worker_executor
                .client()
                .await?
                .get_oplog(workerexecutor::v1::GetOplogRequest {
                    worker_id: request.worker_id.clone(),
                    account_id: Some(
                        AccountId {
                            value: "test-account".to_string(),
                        }
                        .into(),
                    ),
                    from_oplog_index: request.from_oplog_index,
                    cursor: request.cursor,
                    count: request.count,
                })
                .await;

            if Self::should_retry(&mut retry_count, &result) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            } else {
                break result;
            }
        };
        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor get-oplog call"
            )),
            Some(workerexecutor::v1::get_oplog_response::Result::Success(oplog)) => {
                Ok(GetOplogResponse {
                    result: Some(worker::v1::get_oplog_response::Result::Success(
                        GetOplogSuccessResponse {
                            entries: oplog.entries,
                            next: oplog.next,
                            first_index_in_chunk: oplog.first_index_in_chunk,
                            last_index: oplog.last_index,
                        },
                    )),
                })
            }
            Some(workerexecutor::v1::get_oplog_response::Result::Failure(error)) => {
                Ok(GetOplogResponse {
                    result: Some(worker::v1::get_oplog_response::Result::Error(WorkerError {
                        error: Some(worker::v1::worker_error::Error::InternalError(error)),
                    })),
                })
            }
        }
    }

    async fn search_oplog(
        &self,
        request: SearchOplogRequest,
    ) -> crate::Result<SearchOplogResponse> {
        let result = self
            .worker_executor
            .client()
            .await?
            .search_oplog(workerexecutor::v1::SearchOplogRequest {
                worker_id: request.worker_id,
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
                query: request.query,
                cursor: request.cursor,
                count: request.count,
            })
            .await?
            .into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor search-oplog call"
            )),
            Some(workerexecutor::v1::search_oplog_response::Result::Success(oplog)) => {
                Ok(SearchOplogResponse {
                    result: Some(worker::v1::search_oplog_response::Result::Success(
                        SearchOplogSuccessResponse {
                            entries: oplog.entries,
                            next: oplog.next,
                            last_index: oplog.last_index,
                        },
                    )),
                })
            }
            Some(workerexecutor::v1::search_oplog_response::Result::Failure(error)) => {
                Ok(SearchOplogResponse {
                    result: Some(worker::v1::search_oplog_response::Result::Error(
                        WorkerError {
                            error: Some(worker::v1::worker_error::Error::InternalError(error)),
                        },
                    )),
                })
            }
        }
    }

    async fn list_directory(
        &self,
        request: ListDirectoryRequest,
    ) -> crate::Result<ListDirectoryResponse> {
        let result = self
            .worker_executor
            .client()
            .await?
            .list_directory(workerexecutor::v1::ListDirectoryRequest {
                worker_id: request.worker_id,
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
                account_limits: Some(ResourceLimits {
                    available_fuel: i64::MAX,
                    max_memory_per_worker: i64::MAX,
                }),
                path: request.path,
            })
            .await?
            .into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor list-directory call"
            )),
            Some(workerexecutor::v1::list_directory_response::Result::Success(data)) => {
                Ok(ListDirectoryResponse {
                    result: Some(worker::v1::list_directory_response::Result::Success(
                        ListDirectorySuccessResponse { nodes: data.nodes },
                    )),
                })
            }
            Some(workerexecutor::v1::list_directory_response::Result::Failure(error)) => {
                Ok(ListDirectoryResponse {
                    result: Some(worker::v1::list_directory_response::Result::Error(
                        WorkerError {
                            error: Some(worker::v1::worker_error::Error::InternalError(error)),
                        },
                    )),
                })
            }
            Some(_) => Err(anyhow!(
                "Unsupported response from golem-worker-executor list-directory call"
            )),
        }
    }

    async fn get_file_contents(&self, request: GetFileContentsRequest) -> crate::Result<Bytes> {
        let mut stream = self
            .worker_executor
            .client()
            .await?
            .get_file_contents(workerexecutor::v1::GetFileContentsRequest {
                worker_id: request.worker_id,
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
                account_limits: Some(ResourceLimits {
                    available_fuel: i64::MAX,
                    max_memory_per_worker: i64::MAX,
                }),
                file_path: request.file_path,
            })
            .await?
            .into_inner();

        let mut bytes = Vec::new();
        while let Some(chunk) = stream.message().await? {
            match chunk.result {
                Some(workerexecutor::v1::get_file_contents_response::Result::Success(data)) => {
                    bytes.extend_from_slice(&data);
                }
                Some(workerexecutor::v1::get_file_contents_response::Result::Header(header)) => {
                    match header.result {
                        Some(
                            workerexecutor::v1::get_file_contents_response_header::Result::Success(
                                _,
                            ),
                        ) => {}
                        _ => {
                            return Err(anyhow!("Unexpected header from get_file_contents"));
                        }
                    }
                }
                Some(workerexecutor::v1::get_file_contents_response::Result::Failure(err)) => {
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
        let mut retry_count = Self::RETRY_COUNT;
        let result = loop {
            let result = self
                .worker_executor
                .client()
                .await?
                .fork_worker(workerexecutor::v1::ForkWorkerRequest {
                    source_worker_id: fork_worker_request.source_worker_id.clone(),
                    target_worker_id: fork_worker_request.target_worker_id.clone(),
                    account_id: Some(
                        AccountId {
                            value: "test-account".to_string(),
                        }
                        .into(),
                    ),
                    oplog_index_cutoff: fork_worker_request.oplog_index_cutoff,
                })
                .await;

            if Self::should_retry(&mut retry_count, &result) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            } else {
                break result;
            }
        };
        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor fork-worker call"
            )),
            Some(workerexecutor::v1::fork_worker_response::Result::Success(_)) => {
                Ok(ForkWorkerResponse {
                    result: Some(worker::v1::fork_worker_response::Result::Success(Empty {})),
                })
            }
            Some(workerexecutor::v1::fork_worker_response::Result::Failure(error)) => {
                Ok(ForkWorkerResponse {
                    result: Some(worker::v1::fork_worker_response::Result::Error(
                        WorkerError {
                            error: Some(worker::v1::worker_error::Error::InternalError(error)),
                        },
                    )),
                })
            }
        }
    }

    async fn revert_worker(
        &self,
        request: RevertWorkerRequest,
    ) -> crate::Result<RevertWorkerResponse> {
        let result = self
            .worker_executor
            .client()
            .await?
            .revert_worker(workerexecutor::v1::RevertWorkerRequest {
                worker_id: request.worker_id.clone(),
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
                target: request.target,
            })
            .await;

        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor revert-worker call"
            )),
            Some(workerexecutor::v1::revert_worker_response::Result::Success(empty)) => {
                Ok(RevertWorkerResponse {
                    result: Some(revert_worker_response::Result::Success(empty)),
                })
            }
            Some(workerexecutor::v1::revert_worker_response::Result::Failure(error)) => {
                Ok(RevertWorkerResponse {
                    result: Some(revert_worker_response::Result::Error(WorkerError {
                        error: Some(worker::v1::worker_error::Error::InternalError(error)),
                    })),
                })
            }
        }
    }

    async fn cancel_invocation(
        &self,
        request: CancelInvocationRequest,
    ) -> crate::Result<CancelInvocationResponse> {
        let result = self
            .worker_executor
            .client()
            .await?
            .cancel_invocation(workerexecutor::v1::CancelInvocationRequest {
                worker_id: request.worker_id.clone(),
                idempotency_key: request.idempotency_key.clone(),
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
            })
            .await;

        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor cancel-invocation call"
            )),
            Some(workerexecutor::v1::cancel_invocation_response::Result::Success(canceled)) => {
                Ok(CancelInvocationResponse {
                    result: Some(worker::v1::cancel_invocation_response::Result::Success(
                        canceled,
                    )),
                })
            }
            Some(workerexecutor::v1::cancel_invocation_response::Result::Failure(error)) => {
                Ok(CancelInvocationResponse {
                    result: Some(worker::v1::cancel_invocation_response::Result::Error(
                        WorkerError {
                            error: Some(worker::v1::worker_error::Error::InternalError(error)),
                        },
                    )),
                })
            }
        }
    }

    fn private_host(&self) -> String {
        panic!("No real golem-worker-service, forwarding requests to worker-executor");
    }

    fn private_http_port(&self) -> u16 {
        panic!("No real golem-worker-service, forwarding requests to worker-executor");
    }

    fn private_grpc_port(&self) -> u16 {
        panic!("No real golem-worker-service, forwarding requests to worker-executor");
    }

    fn private_custom_request_port(&self) -> u16 {
        panic!("No real golem-worker-service, forwarding requests to worker-executor");
    }

    async fn kill(&self) {}
}

pub struct GrpcForwardingWorkerLogEventStream {
    streaming: Streaming<LogEvent>,
}

impl GrpcForwardingWorkerLogEventStream {
    async fn new(streaming: Streaming<LogEvent>) -> crate::Result<Self> {
        Ok(Self { streaming })
    }
}

#[async_trait]
impl WorkerLogEventStream for GrpcForwardingWorkerLogEventStream {
    async fn message(&mut self) -> crate::Result<Option<LogEvent>> {
        Ok(self.streaming.message().await?)
    }
}
