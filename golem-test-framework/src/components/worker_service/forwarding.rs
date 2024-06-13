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

use std::sync::Arc;

use async_trait::async_trait;
use tonic::Streaming;

use crate::components::component_service::ComponentService;
use golem_api_grpc::proto::golem::common::{Empty, ResourceLimits};
use golem_api_grpc::proto::golem::worker::{
    ConnectWorkerRequest, DeleteWorkerRequest, DeleteWorkerResponse, GetWorkerMetadataRequest,
    GetWorkerMetadataResponse, InterruptWorkerRequest, InterruptWorkerResponse,
    InvokeAndAwaitRequest, InvokeAndAwaitResponse, InvokeRequest, InvokeResponse, InvokeResult,
    LaunchNewWorkerRequest, LaunchNewWorkerResponse, LaunchNewWorkerSuccessResponse, LogEvent,
    ResumeWorkerRequest, ResumeWorkerResponse, UpdateWorkerRequest, UpdateWorkerResponse,
    WorkerError, WorkerId,
};
use golem_api_grpc::proto::golem::workerexecutor::CreateWorkerRequest;
use golem_api_grpc::proto::golem::{worker, workerexecutor};
use golem_common::model::AccountId;

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
    async fn create_worker(&self, request: LaunchNewWorkerRequest) -> LaunchNewWorkerResponse {
        let component_id = request
            .component_id
            .as_ref()
            .expect("Requires component ID")
            .clone()
            .try_into()
            .expect("Requires valid component ID");
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
            .await
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
            .await
            .expect("Failed to call golem-worker-executor")
            .into_inner();

        match response.result {
            None => {
                panic!("No response from golem-worker-executor create-worker call");
            }
            Some(workerexecutor::create_worker_response::Result::Success(_)) => {
                LaunchNewWorkerResponse {
                    result: Some(worker::launch_new_worker_response::Result::Success(
                        LaunchNewWorkerSuccessResponse {
                            worker_id: Some(worker_id),
                            component_version: latest_component_version,
                        },
                    )),
                }
            }
            Some(workerexecutor::create_worker_response::Result::Failure(error)) => {
                LaunchNewWorkerResponse {
                    result: Some(worker::launch_new_worker_response::Result::Error(
                        WorkerError {
                            error: Some(worker::worker_error::Error::InternalError(error)),
                        },
                    )),
                }
            }
        }
    }

    async fn delete_worker(&self, request: DeleteWorkerRequest) -> DeleteWorkerResponse {
        let result = self
            .worker_executor
            .client()
            .await
            .delete_worker(workerexecutor::DeleteWorkerRequest {
                worker_id: request.worker_id,
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
            })
            .await
            .expect("Failed to call golem-worker-executor")
            .into_inner();

        match result.result {
            None => {
                panic!("No response from golem-worker-executor delete-worker call");
            }
            Some(workerexecutor::delete_worker_response::Result::Success(_)) => {
                DeleteWorkerResponse {
                    result: Some(worker::delete_worker_response::Result::Success(Empty {})),
                }
            }
            Some(workerexecutor::delete_worker_response::Result::Failure(error)) => {
                DeleteWorkerResponse {
                    result: Some(worker::delete_worker_response::Result::Error(WorkerError {
                        error: Some(worker::worker_error::Error::InternalError(error)),
                    })),
                }
            }
        }
    }

    async fn get_worker_metadata(
        &self,
        request: GetWorkerMetadataRequest,
    ) -> GetWorkerMetadataResponse {
        let result = self
            .worker_executor
            .client()
            .await
            .get_worker_metadata(workerexecutor::GetWorkerMetadataRequest {
                worker_id: Some(request.worker_id.expect("Worker ID is required")),
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
            })
            .await
            .expect("Failed to call golem-worker-executor")
            .into_inner();

        match result.result {
            None => panic!("No response from golem-worker-executor get-worker-metadata call"),
            Some(workerexecutor::get_worker_metadata_response::Result::Success(metadata)) => {
                GetWorkerMetadataResponse {
                    result: Some(worker::get_worker_metadata_response::Result::Success(
                        metadata,
                    )),
                }
            }
            Some(workerexecutor::get_worker_metadata_response::Result::Failure(error)) => {
                GetWorkerMetadataResponse {
                    result: Some(worker::get_worker_metadata_response::Result::Error(
                        WorkerError {
                            error: Some(worker::worker_error::Error::InternalError(error)),
                        },
                    )),
                }
            }
        }
    }

    async fn invoke(&self, request: InvokeRequest) -> InvokeResponse {
        let result = self
            .worker_executor
            .client()
            .await
            .invoke_worker(workerexecutor::InvokeWorkerRequest {
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
            .await
            .expect("Failed to call golem-worker-executor")
            .into_inner();

        match result.result {
            None => panic!("No response from golem-worker-executor invoke call"),
            Some(workerexecutor::invoke_worker_response::Result::Success(empty)) => {
                InvokeResponse {
                    result: Some(worker::invoke_response::Result::Success(empty)),
                }
            }
            Some(workerexecutor::invoke_worker_response::Result::Failure(error)) => {
                InvokeResponse {
                    result: Some(worker::invoke_response::Result::Error(WorkerError {
                        error: Some(worker::worker_error::Error::InternalError(error)),
                    })),
                }
            }
        }
    }

    async fn invoke_and_await(&self, request: InvokeAndAwaitRequest) -> InvokeAndAwaitResponse {
        let result = self
            .worker_executor
            .client()
            .await
            .invoke_and_await_worker(workerexecutor::InvokeAndAwaitWorkerRequest {
                worker_id: request.worker_id,
                idempotency_key: request.idempotency_key,
                name: request.function,
                calling_convention: request.calling_convention,
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
            .await
            .expect("Failed to call golem-worker-executor")
            .into_inner();

        match result.result {
            None => panic!("No response from golem-worker-executor invoke call"),
            Some(workerexecutor::invoke_and_await_worker_response::Result::Success(result)) => {
                InvokeAndAwaitResponse {
                    result: Some(worker::invoke_and_await_response::Result::Success(
                        InvokeResult {
                            result: result.output,
                        },
                    )),
                }
            }
            Some(workerexecutor::invoke_and_await_worker_response::Result::Failure(error)) => {
                InvokeAndAwaitResponse {
                    result: Some(worker::invoke_and_await_response::Result::Error(
                        WorkerError {
                            error: Some(worker::worker_error::Error::InternalError(error)),
                        },
                    )),
                }
            }
        }
    }

    async fn connect_worker(&self, request: ConnectWorkerRequest) -> Streaming<LogEvent> {
        self.worker_executor
            .client()
            .await
            .connect_worker(workerexecutor::ConnectWorkerRequest {
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
            .await
            .expect("Failed to call golem-worker-executor")
            .into_inner()
    }

    async fn resume_worker(&self, request: ResumeWorkerRequest) -> ResumeWorkerResponse {
        let result = self
            .worker_executor
            .client()
            .await
            .resume_worker(workerexecutor::ResumeWorkerRequest {
                worker_id: request.worker_id,
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
            })
            .await
            .expect("Failed to call golem-worker-executor")
            .into_inner();

        match result.result {
            None => {
                panic!("No response from golem-worker-executor delete-worker call");
            }
            Some(workerexecutor::resume_worker_response::Result::Success(_)) => {
                ResumeWorkerResponse {
                    result: Some(worker::resume_worker_response::Result::Success(Empty {})),
                }
            }
            Some(workerexecutor::resume_worker_response::Result::Failure(error)) => {
                ResumeWorkerResponse {
                    result: Some(worker::resume_worker_response::Result::Error(WorkerError {
                        error: Some(worker::worker_error::Error::InternalError(error)),
                    })),
                }
            }
        }
    }

    async fn interrupt_worker(&self, request: InterruptWorkerRequest) -> InterruptWorkerResponse {
        let result = self
            .worker_executor
            .client()
            .await
            .interrupt_worker(workerexecutor::InterruptWorkerRequest {
                worker_id: request.worker_id,
                recover_immediately: request.recover_immediately,
                account_id: Some(
                    AccountId {
                        value: "test-account".to_string(),
                    }
                    .into(),
                ),
            })
            .await
            .expect("Failed to call golem-worker-executor")
            .into_inner();

        match result.result {
            None => {
                panic!("No response from golem-worker-executor delete-worker call");
            }
            Some(workerexecutor::interrupt_worker_response::Result::Success(_)) => {
                InterruptWorkerResponse {
                    result: Some(worker::interrupt_worker_response::Result::Success(Empty {})),
                }
            }
            Some(workerexecutor::interrupt_worker_response::Result::Failure(error)) => {
                InterruptWorkerResponse {
                    result: Some(worker::interrupt_worker_response::Result::Error(
                        WorkerError {
                            error: Some(worker::worker_error::Error::InternalError(error)),
                        },
                    )),
                }
            }
        }
    }

    async fn update_worker(&self, request: UpdateWorkerRequest) -> UpdateWorkerResponse {
        let result = self
            .worker_executor
            .client()
            .await
            .update_worker(workerexecutor::UpdateWorkerRequest {
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
            .await
            .expect("Failed to call golem-worker-executor")
            .into_inner();

        match result.result {
            None => {
                panic!("No response from golem-worker-executor delete-worker call");
            }
            Some(workerexecutor::update_worker_response::Result::Success(_)) => {
                UpdateWorkerResponse {
                    result: Some(worker::update_worker_response::Result::Success(Empty {})),
                }
            }
            Some(workerexecutor::update_worker_response::Result::Failure(error)) => {
                UpdateWorkerResponse {
                    result: Some(worker::update_worker_response::Result::Error(WorkerError {
                        error: Some(worker::worker_error::Error::InternalError(error)),
                    })),
                }
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
