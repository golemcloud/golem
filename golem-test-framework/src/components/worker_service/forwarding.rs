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

use anyhow::anyhow;
use std::sync::Arc;

use crate::components::component_service::ComponentService;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::common::{Empty, ResourceLimits};
use golem_api_grpc::proto::golem::worker::v1::worker_service_client::WorkerServiceClient;
use golem_api_grpc::proto::golem::worker::v1::{
    ConnectWorkerRequest, DeleteWorkerRequest, DeleteWorkerResponse, GetWorkerMetadataRequest,
    GetWorkerMetadataResponse, InterruptWorkerRequest, InterruptWorkerResponse,
    InvokeAndAwaitJsonRequest, InvokeAndAwaitJsonResponse, InvokeAndAwaitRequest,
    InvokeAndAwaitResponse, InvokeJsonRequest, InvokeRequest, InvokeResponse,
    LaunchNewWorkerRequest, LaunchNewWorkerResponse, LaunchNewWorkerSuccessResponse,
    ResumeWorkerRequest, ResumeWorkerResponse, UpdateWorkerRequest, UpdateWorkerResponse,
    WorkerError,
};
use golem_api_grpc::proto::golem::worker::{InvokeResult, LogEvent, WorkerId};
use golem_api_grpc::proto::golem::workerexecutor::v1::CreateWorkerRequest;
use golem_api_grpc::proto::golem::{worker, workerexecutor};
use golem_common::model::AccountId;
use tonic::transport::Channel;
use tonic::Streaming;

use crate::components::worker_executor::WorkerExecutor;
use crate::components::worker_service::WorkerService;

pub struct ForwardingWorkerService {
    worker_executor: Arc<dyn WorkerExecutor + Send + Sync + 'static>,
    component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
}

impl ForwardingWorkerService {
    pub fn new(
        worker_executor: Arc<dyn WorkerExecutor + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
    ) -> Self {
        Self {
            worker_executor,
            component_service,
        }
    }
}

#[async_trait]
impl WorkerService for ForwardingWorkerService {
    async fn client(&self) -> crate::Result<WorkerServiceClient<Channel>> {
        Err(anyhow!(
            "There is no worker-service, cannot create gRPC client"
        ))
    }

    async fn create_worker(
        &self,
        request: LaunchNewWorkerRequest,
    ) -> crate::Result<LaunchNewWorkerResponse> {
        let component_id = request
            .component_id
            .as_ref()
            .ok_or(anyhow!("Requires component ID"))?
            .clone()
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
        let response = self
            .worker_executor
            .client()
            .await?
            .create_worker(CreateWorkerRequest {
                worker_id: Some(worker_id.clone()),
                component_version: latest_component_version,
                args: request.args,
                env: request.env,
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
            .await?
            .into_inner();

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
        let result = self
            .worker_executor
            .client()
            .await?
            .delete_worker(workerexecutor::v1::DeleteWorkerRequest {
                worker_id: request.worker_id,
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
            })
            .await?
            .into_inner();

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
        let result = self
            .worker_executor
            .client()
            .await?
            .get_worker_metadata(workerexecutor::v1::GetWorkerMetadataRequest {
                worker_id: Some(request.worker_id.ok_or(anyhow!("Worker ID is required"))?),
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
            })
            .await?
            .into_inner();

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

    async fn invoke(&self, request: InvokeRequest) -> crate::Result<InvokeResponse> {
        let result = self
            .worker_executor
            .client()
            .await?
            .invoke_worker(workerexecutor::v1::InvokeWorkerRequest {
                worker_id: request.worker_id,
                idempotency_key: request.idempotency_key,
                name: request.function,
                input: request
                    .invoke_parameters
                    .map(|p| p.params.clone())
                    .unwrap_or_default(),
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
                context: request.context,
            })
            .await?
            .into_inner();

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

    async fn invoke_and_await(
        &self,
        request: InvokeAndAwaitRequest,
    ) -> crate::Result<InvokeAndAwaitResponse> {
        let result = self
            .worker_executor
            .client()
            .await?
            .invoke_and_await_worker(workerexecutor::v1::InvokeAndAwaitWorkerRequest {
                worker_id: request.worker_id,
                idempotency_key: request.idempotency_key,
                name: request.function,
                input: request
                    .invoke_parameters
                    .map(|p| p.params.clone())
                    .unwrap_or_default(),
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
                context: request.context,
            })
            .await?
            .into_inner();

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

    async fn invoke_json(&self, _request: InvokeJsonRequest) -> crate::Result<InvokeResponse> {
        panic!("invoke_json can only be used through worker service");
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
    ) -> crate::Result<Streaming<LogEvent>> {
        Ok(self
            .worker_executor
            .client()
            .await?
            .connect_worker(workerexecutor::v1::ConnectWorkerRequest {
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
            })
            .await?
            .into_inner())
    }

    async fn resume_worker(
        &self,
        request: ResumeWorkerRequest,
    ) -> crate::Result<ResumeWorkerResponse> {
        let result = self
            .worker_executor
            .client()
            .await?
            .resume_worker(workerexecutor::v1::ResumeWorkerRequest {
                worker_id: request.worker_id,
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
            })
            .await?
            .into_inner();

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
        let result = self
            .worker_executor
            .client()
            .await?
            .interrupt_worker(workerexecutor::v1::InterruptWorkerRequest {
                worker_id: request.worker_id,
                recover_immediately: request.recover_immediately,
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
            })
            .await?
            .into_inner();

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
        let result = self
            .worker_executor
            .client()
            .await?
            .update_worker(workerexecutor::v1::UpdateWorkerRequest {
                worker_id: request.worker_id,
                target_version: request.target_version,
                mode: request.mode,
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
            })
            .await?
            .into_inner();

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

    fn kill(&self) {}
}
