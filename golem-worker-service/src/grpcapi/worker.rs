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

use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use tap::TapFallible;
use tonic::{Request, Response, Status};
use tracing::Instrument;

use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::worker::v1::worker_service_server::WorkerService as GrpcWorkerService;
use golem_api_grpc::proto::golem::worker::v1::{
    complete_promise_response, delete_worker_response, get_worker_metadata_response,
    get_workers_metadata_response, interrupt_worker_response, invoke_and_await_json_response,
    invoke_and_await_response, invoke_response, launch_new_worker_response, resume_worker_response,
    update_worker_response, worker_error, worker_execution_error, CompletePromiseRequest,
    CompletePromiseResponse, ConnectWorkerRequest, DeleteWorkerRequest, DeleteWorkerResponse,
    GetWorkerMetadataRequest, GetWorkerMetadataResponse, GetWorkersMetadataRequest,
    GetWorkersMetadataResponse, GetWorkersMetadataSuccessResponse, InterruptWorkerRequest,
    InterruptWorkerResponse, InvokeAndAwaitJsonRequest, InvokeAndAwaitJsonResponse,
    InvokeAndAwaitRequest, InvokeAndAwaitResponse, InvokeJsonRequest, InvokeRequest,
    InvokeResponse, LaunchNewWorkerRequest, LaunchNewWorkerResponse,
    LaunchNewWorkerSuccessResponse, ResumeWorkerRequest, ResumeWorkerResponse, UnknownError,
    UpdateWorkerRequest, UpdateWorkerResponse, WorkerError as GrpcWorkerError,
    WorkerExecutionError,
};
use golem_api_grpc::proto::golem::worker::{InvokeResult, WorkerMetadata};
use golem_common::grpc::{
    proto_component_id_string, proto_idempotency_key_string,
    proto_invocation_context_parent_worker_id_string, proto_worker_id_string,
};
use golem_common::model::{ComponentVersion, ScanCursor, WorkerFilter, WorkerId};
use golem_common::recorded_grpc_api_request;
use golem_service_base::auth::EmptyAuthCtx;
use golem_worker_service_base::api::WorkerTraceErrorKind;
use golem_worker_service_base::service::worker::ConnectWorkerStream;

use crate::empty_worker_metadata;
use crate::service::component::ComponentService;
use crate::service::worker::WorkerService;

pub struct WorkerGrpcApi {
    component_service: ComponentService,
    worker_service: WorkerService,
}

impl WorkerGrpcApi {
    pub fn new(component_service: ComponentService, worker_service: WorkerService) -> Self {
        Self {
            component_service,
            worker_service,
        }
    }
}

#[async_trait::async_trait]
impl GrpcWorkerService for WorkerGrpcApi {
    async fn launch_new_worker(
        &self,
        request: Request<LaunchNewWorkerRequest>,
    ) -> Result<Response<LaunchNewWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "launch_new_worker",
            component_id = proto_component_id_string(&request.component_id),
            name = request.name
        );

        let response = match self
            .launch_new_worker(request)
            .instrument(record.span.clone())
            .await
        {
            Ok((worker_id, component_version)) => record.succeed(
                launch_new_worker_response::Result::Success(LaunchNewWorkerSuccessResponse {
                    worker_id: Some(worker_id.into()),
                    component_version,
                }),
            ),
            Err(error) => record.fail(
                launch_new_worker_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(LaunchNewWorkerResponse {
            result: Some(response),
        }))
    }

    async fn complete_promise(
        &self,
        request: Request<CompletePromiseRequest>,
    ) -> Result<Response<CompletePromiseResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "complete_promise",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        let response = match self
            .complete_promise(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(result) => record.succeed(complete_promise_response::Result::Success(result)),
            Err(error) => record.fail(
                complete_promise_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(CompletePromiseResponse {
            result: Some(response),
        }))
    }

    async fn delete_worker(
        &self,
        request: Request<DeleteWorkerRequest>,
    ) -> Result<Response<DeleteWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "delete_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        let response = match self
            .delete_worker(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(()) => record.succeed(delete_worker_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                delete_worker_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(DeleteWorkerResponse {
            result: Some(response),
        }))
    }

    async fn get_worker_metadata(
        &self,
        request: Request<GetWorkerMetadataRequest>,
    ) -> Result<Response<GetWorkerMetadataResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_worker_metadata",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        let response = match self
            .get_worker_metadata(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(metadata) => record.succeed(get_worker_metadata_response::Result::Success(metadata)),
            Err(error) => record.fail(
                get_worker_metadata_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetWorkerMetadataResponse {
            result: Some(response),
        }))
    }

    async fn interrupt_worker(
        &self,
        request: Request<InterruptWorkerRequest>,
    ) -> Result<Response<InterruptWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "interrupt_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
            recover_immedietaly = request.recover_immediately,
        );

        let response = match self
            .interrupt_worker(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(()) => record.succeed(interrupt_worker_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                interrupt_worker_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(InterruptWorkerResponse {
            result: Some(response),
        }))
    }

    async fn invoke_and_await(
        &self,
        request: Request<InvokeAndAwaitRequest>,
    ) -> Result<Response<InvokeAndAwaitResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "invoke_and_await",
            worker_id = proto_worker_id_string(&request.worker_id),
            idempotency_key = proto_idempotency_key_string(&request.idempotency_key),
            function = request.function,
            context_parent_worker_id =
                proto_invocation_context_parent_worker_id_string(&request.context)
        );

        let response = match self
            .invoke_and_await(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(result) => record.succeed(invoke_and_await_response::Result::Success(result)),
            Err(error) => record.fail(
                invoke_and_await_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(InvokeAndAwaitResponse {
            result: Some(response),
        }))
    }

    async fn invoke_and_await_json(
        &self,
        request: Request<InvokeAndAwaitJsonRequest>,
    ) -> Result<Response<InvokeAndAwaitJsonResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "invoke_and_await_json",
            worker_id = proto_worker_id_string(&request.worker_id),
            idempotency_key = proto_idempotency_key_string(&request.idempotency_key),
            function = request.function,
            context_parent_worker_id =
                proto_invocation_context_parent_worker_id_string(&request.context)
        );

        let response = match self
            .invoke_and_await_json(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(result) => record.succeed(invoke_and_await_json_response::Result::Success(result)),
            Err(error) => record.fail(
                invoke_and_await_json_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(InvokeAndAwaitJsonResponse {
            result: Some(response),
        }))
    }

    async fn invoke(
        &self,
        request: Request<InvokeRequest>,
    ) -> Result<Response<InvokeResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "invoke",
            worker_id = proto_worker_id_string(&request.worker_id),
            idempotency_key = proto_idempotency_key_string(&request.idempotency_key),
            function = request.function,
            context_parent_worker_id =
                proto_invocation_context_parent_worker_id_string(&request.context)
        );

        let response = match self.invoke(request).instrument(record.span.clone()).await {
            Ok(()) => record.succeed(invoke_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                invoke_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(InvokeResponse {
            result: Some(response),
        }))
    }

    async fn invoke_json(
        &self,
        request: Request<InvokeJsonRequest>,
    ) -> Result<Response<InvokeResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "invoke_json",
            worker_id = proto_worker_id_string(&request.worker_id),
            idempotency_key = proto_idempotency_key_string(&request.idempotency_key),
            function = request.function,
            context_parent_worker_id =
                proto_invocation_context_parent_worker_id_string(&request.context)
        );

        let response = match self
            .invoke_json(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(()) => record.succeed(invoke_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                invoke_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(InvokeResponse {
            result: Some(response),
        }))
    }

    async fn resume_worker(
        &self,
        request: Request<ResumeWorkerRequest>,
    ) -> Result<Response<ResumeWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "resume_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        let response = match self
            .resume_worker(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(()) => record.succeed(resume_worker_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                resume_worker_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(ResumeWorkerResponse {
            result: Some(response),
        }))
    }

    type ConnectWorkerStream = golem_worker_service_base::service::worker::ConnectWorkerStream;

    async fn connect_worker(
        &self,
        request: Request<ConnectWorkerRequest>,
    ) -> Result<Response<Self::ConnectWorkerStream>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "connect_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        let stream = self
            .connect_worker(request)
            .instrument(record.span.clone())
            .await;
        match stream {
            Ok(stream) => Ok(Response::new(stream)),
            Err(error) => Err(error_to_status(error)),
        }
    }

    async fn get_workers_metadata(
        &self,
        request: Request<GetWorkersMetadataRequest>,
    ) -> Result<Response<GetWorkersMetadataResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_workers_metadata",
            component_id = proto_component_id_string(&request.component_id),
        );

        let response = match self
            .get_workers_metadata(request)
            .instrument(record.span.clone())
            .await
        {
            Ok((cursor, workers)) => record.succeed(
                get_workers_metadata_response::Result::Success(GetWorkersMetadataSuccessResponse {
                    workers,
                    cursor: cursor.map(|c| c.into()),
                }),
            ),
            Err(error) => record.fail(
                get_workers_metadata_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetWorkersMetadataResponse {
            result: Some(response),
        }))
    }

    async fn update_worker(
        &self,
        request: Request<UpdateWorkerRequest>,
    ) -> Result<Response<UpdateWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "update_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        let response = match self
            .update_worker(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(()) => record.succeed(update_worker_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                update_worker_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(UpdateWorkerResponse {
            result: Some(response),
        }))
    }
}

impl WorkerGrpcApi {
    async fn launch_new_worker(
        &self,
        request: LaunchNewWorkerRequest,
    ) -> Result<(WorkerId, ComponentVersion), GrpcWorkerError> {
        let component_id: golem_common::model::ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;

        let latest_component = self
            .component_service
            .get_latest(&component_id, &EmptyAuthCtx::default())
            .await
            .tap_err(|error| {
                tracing::error!(error = error.to_string(), "Error getting latest component")
            })
            .map_err(|_| GrpcWorkerError {
                error: Some(worker_error::Error::NotFound(ErrorBody {
                    error: format!("Component not found: {}", &component_id),
                })),
            })?;

        let worker_id = make_worker_id(component_id, request.name)?;

        let worker = self
            .worker_service
            .create(
                &worker_id,
                latest_component.versioned_component_id.version,
                request.args,
                request.env,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok((
            worker.into(),
            latest_component.versioned_component_id.version,
        ))
    }

    async fn delete_worker(&self, request: DeleteWorkerRequest) -> Result<(), GrpcWorkerError> {
        let worker_id = make_crate_worker_id(request.worker_id)?;

        self.worker_service
            .delete(
                &worker_id,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(())
    }

    async fn complete_promise(
        &self,
        request: CompletePromiseRequest,
    ) -> Result<bool, GrpcWorkerError> {
        let worker_id = make_crate_worker_id(request.worker_id)?;

        let parameters = request
            .complete_parameters
            .ok_or_else(|| bad_request_error("Missing complete parameters"))?;

        let result = self
            .worker_service
            .complete_promise(
                &worker_id,
                parameters.oplog_idx,
                parameters.data,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(result)
    }

    async fn get_worker_metadata(
        &self,
        request: GetWorkerMetadataRequest,
    ) -> Result<WorkerMetadata, GrpcWorkerError> {
        let worker_id = make_crate_worker_id(request.worker_id)?;

        let metadata = self
            .worker_service
            .get_metadata(
                &worker_id,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(metadata.into())
    }

    async fn get_workers_metadata(
        &self,
        request: GetWorkersMetadataRequest,
    ) -> Result<(Option<ScanCursor>, Vec<WorkerMetadata>), GrpcWorkerError> {
        let component_id: golem_common::model::ComponentId = request
            .component_id
            .ok_or_else(|| bad_request_error("Missing component id"))?
            .try_into()
            .map_err(|_| bad_request_error("Invalid component id"))?;

        let filter: Option<WorkerFilter> =
            match request.filter {
                Some(f) => Some(f.try_into().map_err(|error| {
                    bad_request_error(format!("Invalid worker filter: {error}"))
                })?),
                _ => None,
            };

        let (new_cursor, workers) = self
            .worker_service
            .find_metadata(
                &component_id,
                filter,
                request.cursor.map(|c| c.into()).unwrap_or_default(),
                request.count,
                request.precise,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        let result: Vec<WorkerMetadata> = workers.into_iter().map(|worker| worker.into()).collect();

        Ok((new_cursor, result))
    }

    async fn interrupt_worker(
        &self,
        request: InterruptWorkerRequest,
    ) -> Result<(), GrpcWorkerError> {
        let worker_id = make_crate_worker_id(request.worker_id)?;

        self.worker_service
            .interrupt(
                &worker_id,
                request.recover_immediately,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(())
    }

    async fn invoke(&self, request: InvokeRequest) -> Result<(), GrpcWorkerError> {
        let worker_id = make_crate_worker_id(request.worker_id)?;

        let params = request
            .invoke_parameters
            .ok_or_else(|| bad_request_error("Missing invoke parameters"))?;

        self.worker_service
            .invoke_function_proto(
                &worker_id,
                request.idempotency_key,
                request.function,
                params.params,
                request.context,
                empty_worker_metadata(),
            )
            .await?;

        Ok(())
    }

    async fn invoke_json(&self, request: InvokeJsonRequest) -> Result<(), GrpcWorkerError> {
        let worker_id = make_crate_worker_id(request.worker_id)?;

        let params = parse_json_invoke_parameters(&request.invoke_parameters)?;

        let idempotency_key = request
            .idempotency_key
            .ok_or_else(|| bad_request_error("Missing idempotency key"))?
            .into();

        self.worker_service
            .invoke_function_json(
                &worker_id,
                Some(idempotency_key),
                request.function,
                params,
                request.context,
                empty_worker_metadata(),
            )
            .await?;

        Ok(())
    }

    async fn invoke_and_await(
        &self,
        request: InvokeAndAwaitRequest,
    ) -> Result<InvokeResult, GrpcWorkerError> {
        let worker_id = make_crate_worker_id(request.worker_id)?;

        let params = request
            .invoke_parameters
            .ok_or(bad_request_error("Missing invoke parameters"))?;

        let result = self
            .worker_service
            .invoke_and_await_function_proto(
                &worker_id,
                request.idempotency_key,
                request.function,
                params.params,
                request.context,
                empty_worker_metadata(),
            )
            .await?;

        Ok(result)
    }

    async fn invoke_and_await_json(
        &self,
        request: InvokeAndAwaitJsonRequest,
    ) -> Result<String, GrpcWorkerError> {
        let worker_id = make_crate_worker_id(request.worker_id)?;
        let params = parse_json_invoke_parameters(&request.invoke_parameters)?;

        let idempotency_key = request
            .idempotency_key
            .ok_or_else(|| bad_request_error("Missing idempotency key"))?
            .into();

        let result = self
            .worker_service
            .invoke_and_await_function_json(
                &worker_id,
                Some(idempotency_key),
                request.function,
                params,
                request.context,
                empty_worker_metadata(),
            )
            .await?;

        Ok(serde_json::to_value(result)
            .map_err(|err| GrpcWorkerError {
                error: Some(worker_error::Error::InternalError(WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: format!("Failed to serialize response: {err:?}"),
                    })),
                })),
            })?
            .to_string())
    }

    async fn resume_worker(&self, request: ResumeWorkerRequest) -> Result<(), GrpcWorkerError> {
        let worker_id = make_crate_worker_id(request.worker_id)?;

        self.worker_service
            .resume(
                &worker_id,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(())
    }

    async fn connect_worker(
        &self,
        request: ConnectWorkerRequest,
    ) -> Result<ConnectWorkerStream, GrpcWorkerError> {
        let worker_id = make_crate_worker_id(request.worker_id)?;
        let stream = self
            .worker_service
            .connect(
                &worker_id,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(stream)
    }

    async fn update_worker(&self, request: UpdateWorkerRequest) -> Result<(), GrpcWorkerError> {
        let worker_id = make_crate_worker_id(request.worker_id.clone())?;

        self.worker_service
            .update(
                &worker_id,
                request.mode(),
                request.target_version,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(())
    }
}

fn make_worker_id(
    component_id: golem_common::model::ComponentId,
    worker_name: String,
) -> Result<golem_service_base::model::WorkerId, GrpcWorkerError> {
    golem_service_base::model::WorkerId::new(component_id, worker_name)
        .map_err(|error| bad_request_error(format!("Invalid worker name: {error}")))
}

fn make_crate_worker_id(
    worker_id: Option<golem_api_grpc::proto::golem::worker::WorkerId>,
) -> Result<golem_service_base::model::WorkerId, GrpcWorkerError> {
    let result: golem_service_base::model::WorkerId = worker_id
        .ok_or_else(|| bad_request_error("Missing worker id"))?
        .try_into()
        .map_err(|e| bad_request_error(format!("Invalid worker name: {e}")))?;

    Ok(result)
}

fn bad_request_error<T>(error: T) -> GrpcWorkerError
where
    T: Into<String>,
{
    GrpcWorkerError {
        error: Some(worker_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.into()],
        })),
    }
}

fn error_to_status(error: GrpcWorkerError) -> Status {
    match error.error {
        Some(worker_error::Error::BadRequest(ErrorsBody { errors })) => {
            Status::invalid_argument(format!("Bad Request: {:?}", errors))
        }
        Some(worker_error::Error::Unauthorized(ErrorBody { error })) => {
            Status::unauthenticated(error)
        }
        Some(worker_error::Error::LimitExceeded(ErrorBody { error })) => {
            Status::resource_exhausted(error)
        }
        Some(worker_error::Error::NotFound(ErrorBody { error })) => Status::not_found(error),
        Some(worker_error::Error::AlreadyExists(ErrorBody { error })) => {
            Status::already_exists(error)
        }
        Some(worker_error::Error::InternalError(WorkerExecutionError { error: None })) => {
            Status::unknown("Unknown error")
        }

        Some(worker_error::Error::InternalError(WorkerExecutionError {
            error: Some(worker_execution_error),
        })) => {
            let message = match worker_execution_error {
                worker_execution_error::Error::InvalidRequest(err) => {
                    format!("Invalid Request: {}", err.details)
                }
                worker_execution_error::Error::WorkerAlreadyExists(err) => {
                    format!("Worker Already Exists: Worker ID = {:?}", err.worker_id)
                }
                worker_execution_error::Error::WorkerCreationFailed(err) => format!(
                    "Worker Creation Failed: Worker ID = {:?}, Details: {}",
                    err.worker_id, err.details
                ),
                worker_execution_error::Error::FailedToResumeWorker(err) => {
                    format!("Failed To Resume Worker: Worker ID = {:?}", err.worker_id)
                }
                worker_execution_error::Error::ComponentDownloadFailed(err) => format!(
                    "Component Download Failed: Component ID = {:?}, Version: {}, Reason: {}",
                    err.component_id, err.component_version, err.reason
                ),
                worker_execution_error::Error::ComponentParseFailed(err) => format!(
                    "Component Parsing Failed: Component ID = {:?}, Version: {}, Reason: {}",
                    err.component_id, err.component_version, err.reason
                ),
                worker_execution_error::Error::GetLatestVersionOfComponentFailed(err) => format!(
                    "Get Latest Version Of Component Failed: Component ID = {:?}, Reason: {}",
                    err.component_id, err.reason
                ),
                worker_execution_error::Error::PromiseNotFound(err) => {
                    format!("Promise Not Found: Promise ID = {:?}", err.promise_id)
                }
                worker_execution_error::Error::PromiseDropped(err) => {
                    format!("Promise Dropped: Promise ID = {:?}", err.promise_id)
                }
                worker_execution_error::Error::PromiseAlreadyCompleted(err) => format!(
                    "Promise Already Completed: Promise ID = {:?}",
                    err.promise_id
                ),
                worker_execution_error::Error::Interrupted(err) => format!(
                    "Interrupted: Recover Immediately = {}",
                    err.recover_immediately
                ),
                worker_execution_error::Error::ParamTypeMismatch(_) => {
                    "Parameter Type Mismatch".to_string()
                }
                worker_execution_error::Error::NoValueInMessage(_) => {
                    "No Value In Message".to_string()
                }
                worker_execution_error::Error::ValueMismatch(err) => {
                    format!("Value Mismatch: {}", err.details)
                }
                worker_execution_error::Error::UnexpectedOplogEntry(err) => format!(
                    "Unexpected Oplog Entry: Expected = {}, Got = {}",
                    err.expected, err.got
                ),
                worker_execution_error::Error::RuntimeError(err) => {
                    format!("Runtime Error: {}", err.details)
                }
                worker_execution_error::Error::InvalidShardId(err) => format!(
                    "Invalid Shard ID: Shard ID = {:?}, Shard IDs: {:?}",
                    err.shard_id, err.shard_ids
                ),
                worker_execution_error::Error::PreviousInvocationFailed(_) => {
                    "Previous Invocation Failed".to_string()
                }
                worker_execution_error::Error::PreviousInvocationExited(_) => {
                    "Previous Invocation Exited".to_string()
                }
                worker_execution_error::Error::Unknown(err) => {
                    format!("Unknown Error: {}", err.details)
                }
                worker_execution_error::Error::InvalidAccount(_) => "Invalid Account".to_string(),
                worker_execution_error::Error::WorkerNotFound(err) => {
                    format!("Worker Not Found: Worker ID = {:?}", err.worker_id)
                }
                worker_execution_error::Error::ShardingNotReady(_) => {
                    "Sharding Not Ready".to_string()
                }
            };
            Status::internal(message)
        }
        None => Status::unknown("Unknown error"),
    }
}

fn parse_json_invoke_parameters(
    parameters: &[String],
) -> Result<Vec<TypeAnnotatedValue>, GrpcWorkerError> {
    parameters
        .iter()
        .map(|param| serde_json::from_str(param))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| bad_request_error(format!("Failed to parse JSON parameters: {err:?}")))
}
