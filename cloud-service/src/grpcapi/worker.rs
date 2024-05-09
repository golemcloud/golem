use std::sync::Arc;

use crate::grpcapi::get_authorisation_token;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::worker::worker_service_server::WorkerService as GrpcWorkerService;
use golem_api_grpc::proto::golem::worker::{
    complete_promise_response, delete_worker_response, get_worker_metadata_response,
    get_workers_metadata_response, interrupt_worker_response, invoke_and_await_response,
    invoke_response, launch_new_worker_response, resume_worker_response, update_worker_response,
    worker_error, worker_execution_error, CompletePromiseRequest, CompletePromiseResponse,
    ConnectWorkerRequest, DeleteWorkerRequest, DeleteWorkerResponse, GetWorkerMetadataRequest,
    GetWorkerMetadataResponse, GetWorkersMetadataRequest, GetWorkersMetadataResponse,
    GetWorkersMetadataSuccessResponse, InterruptWorkerRequest, InterruptWorkerResponse,
    InvokeAndAwaitRequest, InvokeAndAwaitResponse, InvokeRequest, InvokeResponse, InvokeResult,
    LaunchNewWorkerRequest, LaunchNewWorkerResponse, LaunchNewWorkerSuccessResponse,
    ResumeWorkerRequest, ResumeWorkerResponse, UnknownError, UpdateWorkerRequest,
    UpdateWorkerResponse, WorkerError as GrpcWorkerError, WorkerExecutionError, WorkerMetadata,
};
use golem_common::model::{ComponentVersion, WorkerFilter, WorkerId};
use golem_worker_service_base::service::worker::WorkerServiceError;
use tap::TapFallible;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::component::{ComponentError, ComponentService};
use crate::service::worker::{self, ConnectWorkerStream, WorkerService};

pub struct WorkerGrpcApi {
    pub component_service: Arc<dyn ComponentService + Sync + Send>,
    pub worker_service: Arc<dyn WorkerService + Sync + Send>,
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
}

#[async_trait::async_trait]
impl GrpcWorkerService for WorkerGrpcApi {
    async fn launch_new_worker(
        &self,
        request: Request<LaunchNewWorkerRequest>,
    ) -> Result<Response<LaunchNewWorkerResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let response = match self.launch_new_worker(r, m).await {
            Ok(worker_id) => {
                launch_new_worker_response::Result::Success(LaunchNewWorkerSuccessResponse {
                    worker_id: Some(worker_id.0.into()),
                    component_version: worker_id.1,
                })
            }
            Err(error) => launch_new_worker_response::Result::Error(error),
        };

        Ok(Response::new(LaunchNewWorkerResponse {
            result: Some(response),
        }))
    }

    async fn delete_worker(
        &self,
        request: Request<DeleteWorkerRequest>,
    ) -> Result<Response<DeleteWorkerResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let response = match self.delete_worker(r, m).await {
            Ok(()) => delete_worker_response::Result::Success(Empty {}),
            Err(error) => delete_worker_response::Result::Error(error),
        };

        Ok(Response::new(DeleteWorkerResponse {
            result: Some(response),
        }))
    }

    async fn complete_promise(
        &self,
        request: Request<CompletePromiseRequest>,
    ) -> Result<Response<CompletePromiseResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let response = match self.complete_promise(r, m).await {
            Ok(result) => complete_promise_response::Result::Success(result),
            Err(error) => complete_promise_response::Result::Error(error),
        };

        Ok(Response::new(CompletePromiseResponse {
            result: Some(response),
        }))
    }

    async fn update_worker(
        &self,
        request: Request<UpdateWorkerRequest>,
    ) -> Result<Response<UpdateWorkerResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let response = match self.update_worker(r, m).await {
            Ok(()) => update_worker_response::Result::Success(
                golem_api_grpc::proto::golem::common::Empty {},
            ),
            Err(error) => update_worker_response::Result::Error(error),
        };

        Ok(Response::new(UpdateWorkerResponse {
            result: Some(response),
        }))
    }

    async fn get_worker_metadata(
        &self,
        request: Request<GetWorkerMetadataRequest>,
    ) -> Result<Response<GetWorkerMetadataResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let response = match self.get_worker_metadata(r, m).await {
            Ok(metadata) => get_worker_metadata_response::Result::Success(metadata),
            Err(error) => get_worker_metadata_response::Result::Error(error),
        };

        Ok(Response::new(GetWorkerMetadataResponse {
            result: Some(response),
        }))
    }

    async fn get_workers_metadata(
        &self,
        request: Request<GetWorkersMetadataRequest>,
    ) -> Result<Response<GetWorkersMetadataResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let response =
            match self.get_workers_metadata(r, m).await {
                Ok((cursor, workers)) => get_workers_metadata_response::Result::Success(
                    GetWorkersMetadataSuccessResponse { cursor, workers },
                ),
                Err(error) => get_workers_metadata_response::Result::Error(error),
            };

        Ok(Response::new(GetWorkersMetadataResponse {
            result: Some(response),
        }))
    }

    async fn interrupt_worker(
        &self,
        request: Request<InterruptWorkerRequest>,
    ) -> Result<Response<InterruptWorkerResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let response = match self.interrupt_worker(r, m).await {
            Ok(()) => interrupt_worker_response::Result::Success(Empty {}),
            Err(error) => interrupt_worker_response::Result::Error(error),
        };

        Ok(Response::new(InterruptWorkerResponse {
            result: Some(response),
        }))
    }

    async fn invoke_and_await(
        &self,
        request: Request<InvokeAndAwaitRequest>,
    ) -> Result<Response<InvokeAndAwaitResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let response = match self.invoke_and_await(r, m).await {
            Ok(result) => invoke_and_await_response::Result::Success(result),
            Err(error) => invoke_and_await_response::Result::Error(error),
        };

        Ok(Response::new(InvokeAndAwaitResponse {
            result: Some(response),
        }))
    }

    async fn invoke(
        &self,
        request: Request<InvokeRequest>,
    ) -> Result<Response<InvokeResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let reponse = match self.invoke(r, m).await {
            Ok(()) => invoke_response::Result::Success(Empty {}),
            Err(error) => invoke_response::Result::Error(error),
        };

        Ok(Response::new(InvokeResponse {
            result: Some(reponse),
        }))
    }

    async fn resume_worker(
        &self,
        request: Request<ResumeWorkerRequest>,
    ) -> Result<Response<ResumeWorkerResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let response = match self.resume_worker(r, m).await {
            Ok(()) => resume_worker_response::Result::Success(Empty {}),
            Err(error) => resume_worker_response::Result::Error(error),
        };

        Ok(Response::new(ResumeWorkerResponse {
            result: Some(response),
        }))
    }

    type ConnectWorkerStream = crate::service::worker::ConnectWorkerStream;

    async fn connect_worker(
        &self,
        request: Request<ConnectWorkerRequest>,
    ) -> Result<Response<Self::ConnectWorkerStream>, Status> {
        let (m, _, r) = request.into_parts();
        let stream = self.connect_worker(r, m).await;
        match stream {
            Ok(stream) => Ok(Response::new(stream)),
            Err(error) => Err(error_to_status(error)),
        }
    }
}

impl From<AuthServiceError> for GrpcWorkerError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::InvalidToken(error) => {
                worker_error::Error::Unauthorized(ErrorBody { error })
            }
            // TODO: this used to be unauthorized. How do we handle internal server errors?
            AuthServiceError::Unexpected(details) => {
                worker_error::Error::InternalError(WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details,
                    })),
                })
            }
        };
        GrpcWorkerError { error: Some(error) }
    }
}

impl WorkerGrpcApi {
    async fn auth(
        &self,
        metadata: MetadataMap,
    ) -> Result<crate::auth::AccountAuthorisation, GrpcWorkerError> {
        match get_authorisation_token(metadata) {
            Some(t) => self
                .auth_service
                .authorization(&t)
                .await
                .map_err(|e| e.into()),
            None => Err(GrpcWorkerError {
                error: Some(worker_error::Error::Unauthorized(ErrorBody {
                    error: "Missing token".into(),
                })),
            }),
        }
    }

    async fn launch_new_worker(
        &self,
        request: LaunchNewWorkerRequest,
        metadata: MetadataMap,
    ) -> Result<(WorkerId, ComponentVersion), GrpcWorkerError> {
        let auth = self.auth(metadata).await?;
        let component_id: golem_common::model::ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;

        let latest_component = self
            .component_service
            .get_latest_version(&component_id, &auth)
            .await
            .tap_err(|error| {
                tracing::error!("Error getting latest component version: {:?}", error)
            })?
            .ok_or(GrpcWorkerError {
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
                &auth,
            )
            .await?;

        Ok((
            worker.into(),
            latest_component.versioned_component_id.version,
        ))
    }

    async fn delete_worker(
        &self,
        request: DeleteWorkerRequest,
        metadata: MetadataMap,
    ) -> Result<(), GrpcWorkerError> {
        let auth = self.auth(metadata).await?;

        let worker_id = make_crate_worker_id(request.worker_id)?;

        self.worker_service.delete(&worker_id, &auth).await?;

        Ok(())
    }

    async fn complete_promise(
        &self,
        request: CompletePromiseRequest,
        metadata: MetadataMap,
    ) -> Result<bool, GrpcWorkerError> {
        let auth = self.auth(metadata).await?;
        let worker_id = make_crate_worker_id(request.worker_id)?;

        let parameters = request
            .complete_parameters
            .ok_or_else(|| bad_request_error("Missing complete parameters"))?;

        let result = self
            .worker_service
            .complete_promise(&worker_id, parameters.oplog_idx, parameters.data, &auth)
            .await?;

        Ok(result)
    }

    async fn get_worker_metadata(
        &self,
        request: GetWorkerMetadataRequest,
        metadata: MetadataMap,
    ) -> Result<WorkerMetadata, GrpcWorkerError> {
        let auth = self.auth(metadata).await?;
        let worker_id = make_crate_worker_id(request.worker_id)?;

        let metadata = self.worker_service.get_metadata(&worker_id, &auth).await?;

        Ok(metadata.into())
    }

    async fn get_workers_metadata(
        &self,
        request: GetWorkersMetadataRequest,
        metadata: MetadataMap,
    ) -> Result<(Option<u64>, Vec<WorkerMetadata>), GrpcWorkerError> {
        let auth = self.auth(metadata).await?;
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
                request.cursor,
                request.count,
                request.precise,
                &auth,
            )
            .await?;

        let result: Vec<WorkerMetadata> = workers.into_iter().map(|worker| worker.into()).collect();

        Ok((new_cursor, result))
    }

    async fn interrupt_worker(
        &self,
        request: InterruptWorkerRequest,
        metadata: MetadataMap,
    ) -> Result<(), GrpcWorkerError> {
        let auth = self.auth(metadata).await?;
        let worker_id = make_crate_worker_id(request.worker_id)?;

        self.worker_service
            .interrupt(&worker_id, request.recover_immediately, &auth)
            .await?;

        Ok(())
    }

    async fn invoke(
        &self,
        request: InvokeRequest,
        metadata: MetadataMap,
    ) -> Result<(), GrpcWorkerError> {
        let auth = self.auth(metadata).await?;
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
                &auth,
            )
            .await?;

        Ok(())
    }

    async fn invoke_and_await(
        &self,
        request: InvokeAndAwaitRequest,
        metadata: MetadataMap,
    ) -> Result<InvokeResult, GrpcWorkerError> {
        let auth = self.auth(metadata).await?;
        let worker_id = make_crate_worker_id(request.worker_id)?;

        let params = request
            .invoke_parameters
            .ok_or(bad_request_error("Missing invoke parameters"))?;

        let calling_convention: golem_common::model::CallingConvention = request
            .calling_convention
            .try_into()
            .map_err(bad_request_error)?;

        let result = self
            .worker_service
            .invoke_and_await_function_proto(
                &worker_id,
                request.idempotency_key,
                request.function,
                params.params,
                &calling_convention,
                &auth,
            )
            .await?;

        Ok(result)
    }

    async fn resume_worker(
        &self,
        request: ResumeWorkerRequest,
        metadata: MetadataMap,
    ) -> Result<(), GrpcWorkerError> {
        let auth = self.auth(metadata).await?;
        let worker_id = make_crate_worker_id(request.worker_id)?;

        self.worker_service.resume(&worker_id, &auth).await?;

        Ok(())
    }

    async fn connect_worker(
        &self,
        request: ConnectWorkerRequest,
        metadata: MetadataMap,
    ) -> Result<ConnectWorkerStream, GrpcWorkerError> {
        let auth = self.auth(metadata).await?;
        let worker_id = make_crate_worker_id(request.worker_id)?;
        let stream = self.worker_service.connect(&worker_id, &auth).await?;

        Ok(stream)
    }

    async fn update_worker(
        &self,
        request: UpdateWorkerRequest,
        metadata: MetadataMap,
    ) -> Result<(), GrpcWorkerError> {
        let worker_id = make_crate_worker_id(request.worker_id.clone())?;

        let auth = self.auth(metadata).await?;

        self.worker_service
            .update(&worker_id, request.mode(), request.target_version, &auth)
            .await?;

        Ok(())
    }
}

impl From<crate::service::worker::WorkerError> for GrpcWorkerError {
    fn from(error: crate::service::worker::WorkerError) -> Self {
        GrpcWorkerError {
            error: Some(error.into()),
        }
    }
}

impl From<crate::service::worker::WorkerError> for worker_error::Error {
    fn from(value: crate::service::worker::WorkerError) -> Self {
        match value {
            worker::WorkerError::Base(error) => match error {
                WorkerServiceError::Component(error) => error.into(),
                WorkerServiceError::TypeChecker(error) => {
                    worker_error::Error::BadRequest(ErrorsBody {
                        errors: vec![error],
                    })
                }
                WorkerServiceError::VersionedComponentIdNotFound(_)
                | WorkerServiceError::ComponentNotFound(_)
                | WorkerServiceError::AccountIdNotFound(_)
                | WorkerServiceError::WorkerNotFound(_) => {
                    worker_error::Error::NotFound(ErrorBody {
                        error: error.to_string(),
                    })
                }
                WorkerServiceError::Golem(golem) => {
                    worker_error::Error::InternalError(golem.into())
                }
                WorkerServiceError::Internal(error) => {
                    worker_error::Error::InternalError(WorkerExecutionError {
                        error: Some(worker_execution_error::Error::Unknown(UnknownError {
                            details: error.to_string(),
                        })),
                    })
                }
            },
            worker::WorkerError::Forbidden(error) => {
                worker_error::Error::LimitExceeded(ErrorBody { error })
            }
            worker::WorkerError::Unauthorized(error) => {
                worker_error::Error::Unauthorized(ErrorBody { error })
            }
            worker::WorkerError::ProjectNotFound(_) => worker_error::Error::NotFound(ErrorBody {
                error: value.to_string(),
            }),
        }
    }
}

impl From<ComponentError> for GrpcWorkerError {
    fn from(error: ComponentError) -> Self {
        GrpcWorkerError {
            error: Some(error.into()),
        }
    }
}

impl From<ComponentError> for worker_error::Error {
    fn from(value: ComponentError) -> Self {
        match value {
            ComponentError::Internal(error) => {
                worker_error::Error::InternalError(WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: error.to_string(),
                    })),
                })
            }
            ComponentError::Unauthorized(error) => {
                worker_error::Error::Unauthorized(ErrorBody { error })
            }
            ComponentError::LimitExceeded(error) => {
                worker_error::Error::LimitExceeded(ErrorBody { error })
            }
            ComponentError::AlreadyExists(_) => worker_error::Error::BadRequest(ErrorsBody {
                errors: vec![value.to_string()],
            }),
            ComponentError::UnknownComponentId(_)
            | ComponentError::UnknownVersionedComponentId(_)
            | ComponentError::UnknownProjectId(_) => worker_error::Error::NotFound(ErrorBody {
                error: value.to_string(),
            }),
            ComponentError::ComponentProcessing(error) => {
                worker_error::Error::BadRequest(ErrorsBody {
                    errors: vec![error.to_string()],
                })
            }
        }
    }
}

fn make_worker_id(
    component_id: golem_common::model::ComponentId,
    worker_name: String,
) -> std::result::Result<golem_service_base::model::WorkerId, GrpcWorkerError> {
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
            tonic::Status::invalid_argument(format!("Bad Request: {:?}", errors))
        }
        Some(worker_error::Error::Unauthorized(ErrorBody { error })) => {
            tonic::Status::unauthenticated(error)
        }
        Some(worker_error::Error::LimitExceeded(ErrorBody { error })) => {
            tonic::Status::resource_exhausted(error)
        }
        Some(worker_error::Error::NotFound(ErrorBody { error })) => tonic::Status::not_found(error),
        Some(worker_error::Error::AlreadyExists(ErrorBody { error })) => {
            tonic::Status::already_exists(error)
        }
        Some(worker_error::Error::InternalError(WorkerExecutionError { error: None })) => {
            tonic::Status::unknown("Unknown error")
        }

        Some(worker_error::Error::InternalError(WorkerExecutionError {
            error: Some(golem_error),
        })) => {
            let message = match golem_error {
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
                    "Component Parse Failed: Component ID = {:?}, Version: {}, Reason: {}",
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
            };
            tonic::Status::internal(message)
        }
        None => tonic::Status::unknown("Unknown error"),
    }
}
