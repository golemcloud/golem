use crate::service::auth::AuthService;
use crate::service::worker::{self, ConnectWorkerStream, WorkerService};
use cloud_common::auth::{CloudAuthCtx, CloudNamespace};
use cloud_common::clients::auth::get_authorisation_token;
use cloud_common::model::ProjectAction;
use futures::Stream;
use futures::StreamExt;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::worker::v1::worker_service_server::WorkerService as GrpcWorkerService;
use golem_api_grpc::proto::golem::worker::v1::{
    activate_plugin_response, cancel_invocation_response, complete_promise_response,
    deactivate_plugin_response, delete_worker_response, fork_worker_response, get_oplog_response,
    get_worker_metadata_response, get_workers_metadata_response, interrupt_worker_response,
    invoke_and_await_json_response, invoke_and_await_response, invoke_and_await_typed_response,
    invoke_response, launch_new_worker_response, list_directory_response, resume_worker_response,
    revert_worker_response, search_oplog_response, update_worker_response, worker_error,
    worker_execution_error, ActivatePluginRequest, ActivatePluginResponse, CancelInvocationRequest,
    CancelInvocationResponse, CompletePromiseRequest, CompletePromiseResponse,
    ConnectWorkerRequest, DeactivatePluginRequest, DeactivatePluginResponse, DeleteWorkerRequest,
    DeleteWorkerResponse, ForkWorkerRequest, ForkWorkerResponse, GetFileContentsRequest,
    GetFileContentsResponse, GetOplogRequest, GetOplogResponse, GetOplogSuccessResponse,
    GetWorkerMetadataRequest, GetWorkerMetadataResponse, GetWorkersMetadataRequest,
    GetWorkersMetadataResponse, GetWorkersMetadataSuccessResponse, InterruptWorkerRequest,
    InterruptWorkerResponse, InvokeAndAwaitJsonRequest, InvokeAndAwaitJsonResponse,
    InvokeAndAwaitRequest, InvokeAndAwaitResponse, InvokeAndAwaitTypedResponse, InvokeJsonRequest,
    InvokeRequest, InvokeResponse, LaunchNewWorkerRequest, LaunchNewWorkerResponse,
    LaunchNewWorkerSuccessResponse, ListDirectoryRequest, ListDirectoryResponse,
    ResumeWorkerRequest, ResumeWorkerResponse, RevertWorkerRequest, RevertWorkerResponse,
    SearchOplogRequest, SearchOplogResponse, SearchOplogSuccessResponse, UnknownError,
    UpdateWorkerRequest, UpdateWorkerResponse, WorkerError as GrpcWorkerError,
    WorkerExecutionError,
};
use golem_api_grpc::proto::golem::worker::{InvokeResult, InvokeResultTyped, WorkerMetadata};
use golem_common::grpc::{
    proto_component_id_string, proto_idempotency_key_string,
    proto_invocation_context_parent_worker_id_string, proto_plugin_installation_id_string,
    proto_target_worker_id_string, proto_worker_id_string,
};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{ComponentVersion, ScanCursor, WorkerFilter, WorkerId};
use golem_common::recorded_grpc_api_request;
use golem_common::SafeDisplay;
use golem_worker_service_base::api::WorkerTraceErrorKind;
use golem_worker_service_base::grpcapi::{
    bad_request_error, bad_request_errors, error_to_status, parse_json_invoke_parameters,
    validate_component_file_path, validate_protobuf_plugin_installation_id,
    validate_protobuf_target_worker_id, validate_protobuf_worker_id, validated_worker_id,
};
use golem_worker_service_base::service::component::ComponentService;
use golem_worker_service_base::service::worker::{InvocationParameters, WorkerServiceError};
use std::pin::Pin;
use std::sync::Arc;
use tap::TapFallible;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};
use tracing::Instrument;

pub struct WorkerGrpcApi {
    component_service: Arc<dyn ComponentService<CloudNamespace, CloudAuthCtx>>,
    worker_service: Arc<dyn WorkerService + Send + Sync>,
    auth_service: Arc<dyn AuthService + Send + Sync>,
}

#[async_trait::async_trait]
impl GrpcWorkerService for WorkerGrpcApi {
    async fn launch_new_worker(
        &self,
        request: Request<LaunchNewWorkerRequest>,
    ) -> Result<Response<LaunchNewWorkerResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "launch_new_worker",
            component_id = proto_component_id_string(&r.component_id),
            name = r.name
        );

        let response = match self
            .launch_new_worker(r, m)
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
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "complete_promise",
            worker_id = proto_worker_id_string(&r.worker_id),
        );

        let response = match self
            .complete_promise(r, m)
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
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "delete_worker",
            worker_id = proto_worker_id_string(&r.worker_id),
        );

        let response = match self
            .delete_worker(r, m)
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
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "get_worker_metadata",
            worker_id = proto_worker_id_string(&r.worker_id),
        );

        let response = match self
            .get_worker_metadata(r, m)
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
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "interrupt_worker",
            worker_id = proto_worker_id_string(&r.worker_id),
            recover_immedietaly = r.recover_immediately,
        );

        let response = match self
            .interrupt_worker(r, m)
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
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "invoke_and_await",
            worker_id = proto_target_worker_id_string(&r.worker_id),
            idempotency_key = proto_idempotency_key_string(&r.idempotency_key),
            function = r.function,
            context_parent_worker_id = proto_invocation_context_parent_worker_id_string(&r.context)
        );

        let response = match self
            .invoke_and_await(r, m)
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
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "invoke_and_await_json",
            worker_id = proto_target_worker_id_string(&r.worker_id),
            idempotency_key = proto_idempotency_key_string(&r.idempotency_key),
            function = r.function,
            context_parent_worker_id = proto_invocation_context_parent_worker_id_string(&r.context)
        );

        let response = match self
            .invoke_and_await_json(r, m)
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

    async fn invoke_and_await_typed(
        &self,
        request: Request<InvokeAndAwaitRequest>,
    ) -> Result<Response<InvokeAndAwaitTypedResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "invoke_and_await_typed",
            worker_id = proto_target_worker_id_string(&r.worker_id),
            idempotency_key = proto_idempotency_key_string(&r.idempotency_key),
            function = r.function,
            context_parent_worker_id = proto_invocation_context_parent_worker_id_string(&r.context)
        );

        let response = match self
            .invoke_and_await_typed(r, m)
            .instrument(record.span.clone())
            .await
        {
            Ok(result) => record.succeed(invoke_and_await_typed_response::Result::Success(result)),
            Err(error) => record.fail(
                invoke_and_await_typed_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(InvokeAndAwaitTypedResponse {
            result: Some(response),
        }))
    }

    async fn invoke(
        &self,
        request: Request<InvokeRequest>,
    ) -> Result<Response<InvokeResponse>, Status> {
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "invoke",
            worker_id = proto_target_worker_id_string(&r.worker_id),
            idempotency_key = proto_idempotency_key_string(&r.idempotency_key),
            function = r.function,
            context_parent_worker_id = proto_invocation_context_parent_worker_id_string(&r.context)
        );

        let response = match self.invoke(r, m).instrument(record.span.clone()).await {
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
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "invoke_json",
            worker_id = proto_target_worker_id_string(&r.worker_id),
            idempotency_key = proto_idempotency_key_string(&r.idempotency_key),
            function = r.function,
            context_parent_worker_id = proto_invocation_context_parent_worker_id_string(&r.context)
        );

        let response = match self.invoke_json(r, m).instrument(record.span.clone()).await {
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
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "resume_worker",
            worker_id = proto_worker_id_string(&r.worker_id),
        );

        let response = match self
            .resume_worker(r, m)
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

    type ConnectWorkerStream = crate::service::worker::ConnectWorkerStream;

    async fn connect_worker(
        &self,
        request: Request<ConnectWorkerRequest>,
    ) -> Result<Response<Self::ConnectWorkerStream>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "connect_worker",
            worker_id = proto_worker_id_string(&r.worker_id),
        );

        let stream = self
            .connect_worker(r, m)
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
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "get_workers_metadata",
            component_id = proto_component_id_string(&r.component_id),
        );

        let response = match self
            .get_workers_metadata(r, m)
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
        let (m, _, r) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "update_worker",
            worker_id = proto_worker_id_string(&r.worker_id),
        );

        let response = match self
            .update_worker(r, m)
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

    async fn get_oplog(
        &self,
        request: Request<GetOplogRequest>,
    ) -> Result<Response<GetOplogResponse>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "get_oplog",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        let response = match self
            .get_oplog(request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(response) => record.succeed(get_oplog_response::Result::Success(response)),
            Err(error) => record.fail(
                get_oplog_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetOplogResponse {
            result: Some(response),
        }))
    }

    async fn search_oplog(
        &self,
        request: Request<SearchOplogRequest>,
    ) -> Result<Response<SearchOplogResponse>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "search_oplog",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        let response = match self
            .search_oplog(request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(response) => record.succeed(search_oplog_response::Result::Success(response)),
            Err(error) => record.fail(
                search_oplog_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(SearchOplogResponse {
            result: Some(response),
        }))
    }

    async fn list_directory(
        &self,
        request: Request<ListDirectoryRequest>,
    ) -> Result<Response<ListDirectoryResponse>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "get_file_contents",
            worker_id = proto_target_worker_id_string(&request.worker_id),
        );

        let response = match self
            .list_directory(request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(response) => record.succeed(list_directory_response::Result::Success(response)),
            Err(error) => record.fail(
                list_directory_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(
            golem_api_grpc::proto::golem::worker::v1::ListDirectoryResponse {
                result: Some(response),
            },
        ))
    }

    type GetFileContentsStream =
        Pin<Box<dyn Stream<Item = Result<GetFileContentsResponse, Status>> + Send + 'static>>;

    async fn get_file_contents(
        &self,
        request: Request<GetFileContentsRequest>,
    ) -> Result<Response<Self::GetFileContentsStream>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "get_file_contents",
            worker_id = proto_target_worker_id_string(&request.worker_id),
        );

        let stream = self
            .get_file_contents(request, metadata)
            .instrument(record.span.clone())
            .await;

        let stream = match stream {
            Ok(stream) => record.succeed(stream),
            Err(error) => {
                let res = golem_api_grpc::proto::golem::worker::v1::GetFileContentsResponse {
                    result: Some(
                        golem_api_grpc::proto::golem::worker::v1::get_file_contents_response::Result::Error(error.clone())
                    )
                };
                let err_stream: Self::GetFileContentsStream =
                    Box::pin(tokio_stream::iter(vec![Ok(res)]));
                record.fail(err_stream, &WorkerTraceErrorKind(&error))
            }
        };
        Ok(Response::new(stream))
    }

    async fn activate_plugin(
        &self,
        request: Request<ActivatePluginRequest>,
    ) -> Result<Response<ActivatePluginResponse>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "activate_plugin",
            worker_id = proto_worker_id_string(&request.worker_id),
            plugin_installation_id = proto_plugin_installation_id_string(&request.installation_id),
        );

        let response = match self
            .activate_plugin(request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(()) => record.succeed(activate_plugin_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                activate_plugin_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(ActivatePluginResponse {
            result: Some(response),
        }))
    }

    async fn deactivate_plugin(
        &self,
        request: Request<DeactivatePluginRequest>,
    ) -> Result<Response<DeactivatePluginResponse>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "deactivate_plugin",
            worker_id = proto_worker_id_string(&request.worker_id),
            plugin_installation_id = proto_plugin_installation_id_string(&request.installation_id),
        );

        let response = match self
            .deactivate_plugin(request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(()) => record.succeed(deactivate_plugin_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                deactivate_plugin_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(DeactivatePluginResponse {
            result: Some(response),
        }))
    }

    async fn fork_worker(
        &self,
        request: Request<ForkWorkerRequest>,
    ) -> Result<Response<ForkWorkerResponse>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "fork_worker",
            source_worker_id = proto_worker_id_string(&request.source_worker_id),
            target_worker_id = proto_worker_id_string(&request.target_worker_id),
        );

        let response = match self
            .fork_worker(request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(fork_worker_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                fork_worker_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(ForkWorkerResponse {
            result: Some(response),
        }))
    }

    async fn revert_worker(
        &self,
        request: Request<RevertWorkerRequest>,
    ) -> Result<Response<RevertWorkerResponse>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "revert_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        let response = match self
            .revert_worker(request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(revert_worker_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                revert_worker_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(RevertWorkerResponse {
            result: Some(response),
        }))
    }

    async fn cancel_invocation(
        &self,
        request: Request<CancelInvocationRequest>,
    ) -> Result<Response<CancelInvocationResponse>, Status> {
        let (metadata, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "revert_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        let response = match self
            .cancel_invocation(request, metadata)
            .instrument(record.span.clone())
            .await
        {
            Ok(result) => record.succeed(cancel_invocation_response::Result::Success(result)),
            Err(error) => record.fail(
                cancel_invocation_response::Result::Error(error.clone()),
                &WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(CancelInvocationResponse {
            result: Some(response),
        }))
    }
}

impl WorkerGrpcApi {
    pub fn new(
        component_service: Arc<dyn ComponentService<CloudNamespace, CloudAuthCtx>>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        auth_service: Arc<dyn AuthService + Send + Sync>,
    ) -> Self {
        Self {
            component_service,
            worker_service,
            auth_service,
        }
    }

    fn auth(&self, metadata: MetadataMap) -> Result<CloudAuthCtx, GrpcWorkerError> {
        match get_authorisation_token(metadata) {
            Some(t) => Ok(CloudAuthCtx::new(t)),
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
        let auth = self.auth(metadata)?;
        let component_id: golem_common::model::ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;

        let latest_component = self
            .component_service
            .get_latest(&component_id, &auth)
            .await
            .tap_err(|error| tracing::error!("Error getting latest component: {:?}", error))
            .map_err(|_| GrpcWorkerError {
                error: Some(worker_error::Error::NotFound(ErrorBody {
                    error: format!("Component not found: {}", &component_id),
                })),
            })?;

        let worker_id = validated_worker_id(component_id, request.name)?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::CreateWorker, &auth)
            .await?;
        let worker = self
            .worker_service
            .create(
                &worker_id,
                latest_component.versioned_component_id.version,
                request.args,
                request.env,
                namespace,
            )
            .await?;

        Ok((worker, latest_component.versioned_component_id.version))
    }

    async fn delete_worker(
        &self,
        request: DeleteWorkerRequest,
        metadata: MetadataMap,
    ) -> Result<(), GrpcWorkerError> {
        let auth = self.auth(metadata)?;

        let worker_id = validate_protobuf_worker_id(request.worker_id)?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::DeleteWorker, &auth)
            .await?;
        self.worker_service.delete(&worker_id, namespace).await?;

        Ok(())
    }

    async fn complete_promise(
        &self,
        request: CompletePromiseRequest,
        metadata: MetadataMap,
    ) -> Result<bool, GrpcWorkerError> {
        let auth = self.auth(metadata)?;
        let worker_id = validate_protobuf_worker_id(request.worker_id)?;

        let parameters = request
            .complete_parameters
            .ok_or_else(|| bad_request_error("Missing complete parameters"))?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;
        let result = self
            .worker_service
            .complete_promise(&worker_id, parameters.oplog_idx, parameters.data, namespace)
            .await?;

        Ok(result)
    }

    async fn get_worker_metadata(
        &self,
        request: GetWorkerMetadataRequest,
        metadata: MetadataMap,
    ) -> Result<WorkerMetadata, GrpcWorkerError> {
        let auth = self.auth(metadata)?;
        let worker_id = validate_protobuf_worker_id(request.worker_id)?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::ViewWorker, &auth)
            .await?;
        let metadata = self
            .worker_service
            .get_metadata(&worker_id, namespace)
            .await?;

        Ok(metadata.into())
    }

    async fn get_workers_metadata(
        &self,
        request: GetWorkersMetadataRequest,
        metadata: MetadataMap,
    ) -> Result<(Option<ScanCursor>, Vec<WorkerMetadata>), GrpcWorkerError> {
        let auth = self.auth(metadata)?;
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

        let namespace = self
            .auth_service
            .is_authorized_by_component(&component_id, ProjectAction::ViewWorker, &auth)
            .await?;
        let (new_cursor, workers) = self
            .worker_service
            .find_metadata(
                &component_id,
                filter,
                request.cursor.map(|c| c.into()).unwrap_or_default(),
                request.count,
                request.precise,
                namespace,
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
        let auth = self.auth(metadata)?;
        let worker_id = validate_protobuf_worker_id(request.worker_id)?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;
        self.worker_service
            .interrupt(&worker_id, request.recover_immediately, namespace)
            .await?;

        Ok(())
    }

    async fn invoke(
        &self,
        request: InvokeRequest,
        metadata: MetadataMap,
    ) -> Result<(), GrpcWorkerError> {
        let auth = self.auth(metadata)?;
        let worker_id = validate_protobuf_target_worker_id(request.worker_id)?;

        let params = request
            .invoke_parameters
            .ok_or_else(|| bad_request_error("Missing invoke parameters"))?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;
        self.worker_service
            .invoke(
                &worker_id,
                request.idempotency_key.map(|k| k.into()),
                request.function,
                params.params,
                request.context,
                namespace,
            )
            .await?;

        Ok(())
    }

    async fn invoke_json(
        &self,
        request: InvokeJsonRequest,
        metadata: MetadataMap,
    ) -> Result<(), GrpcWorkerError> {
        let auth = self.auth(metadata)?;
        let worker_id = validate_protobuf_target_worker_id(request.worker_id)?;

        let params = parse_json_invoke_parameters(&request.invoke_parameters)?;
        let params = InvocationParameters::from_optionally_type_annotated_value_jsons(params)
            .map_err(bad_request_errors)?;

        let idempotency_key = request
            .idempotency_key
            .ok_or_else(|| bad_request_error("Missing idempotency key"))?
            .into();

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;

        match params {
            InvocationParameters::TypedProtoVals(params) => {
                self.worker_service
                    .validate_and_invoke(
                        &worker_id,
                        Some(idempotency_key),
                        request.function,
                        params,
                        request.context,
                        namespace,
                    )
                    .await?
            }
            InvocationParameters::RawJsonStrings(jsons) => {
                self.worker_service
                    .invoke_json(
                        &worker_id,
                        Some(idempotency_key),
                        request.function,
                        jsons,
                        request.context,
                        namespace,
                    )
                    .await?
            }
        }

        Ok(())
    }

    async fn invoke_and_await(
        &self,
        request: InvokeAndAwaitRequest,
        metadata: MetadataMap,
    ) -> Result<InvokeResult, GrpcWorkerError> {
        let auth = self.auth(metadata)?;
        let worker_id = validate_protobuf_target_worker_id(request.worker_id)?;

        let params = request
            .invoke_parameters
            .ok_or(bad_request_error("Missing invoke parameters"))?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;
        let result = self
            .worker_service
            .invoke_and_await(
                &worker_id,
                request.idempotency_key.map(|k| k.into()),
                request.function,
                params.params,
                request.context,
                namespace,
            )
            .await?;

        Ok(result)
    }

    async fn invoke_and_await_json(
        &self,
        request: InvokeAndAwaitJsonRequest,
        metadata: MetadataMap,
    ) -> Result<String, GrpcWorkerError> {
        let auth = self.auth(metadata)?;
        let worker_id = validate_protobuf_target_worker_id(request.worker_id)?;
        let params = parse_json_invoke_parameters(&request.invoke_parameters)?;
        let params = InvocationParameters::from_optionally_type_annotated_value_jsons(params)
            .map_err(bad_request_errors)?;

        let idempotency_key = request
            .idempotency_key
            .ok_or_else(|| bad_request_error("Missing idempotency key"))?
            .into();

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;

        let result = match params {
            InvocationParameters::TypedProtoVals(params) => {
                self.worker_service
                    .validate_and_invoke_and_await_typed(
                        &worker_id,
                        Some(idempotency_key),
                        request.function,
                        params,
                        request.context,
                        namespace,
                    )
                    .await?
            }
            InvocationParameters::RawJsonStrings(jsons) => {
                self.worker_service
                    .invoke_and_await_json(
                        &worker_id,
                        Some(idempotency_key),
                        request.function,
                        jsons,
                        request.context,
                        namespace,
                    )
                    .await?
            }
        };

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

    async fn invoke_and_await_typed(
        &self,
        request: InvokeAndAwaitRequest,
        metadata: MetadataMap,
    ) -> Result<InvokeResultTyped, GrpcWorkerError> {
        let auth = self.auth(metadata)?;
        let worker_id = validate_protobuf_target_worker_id(request.worker_id)?;
        let params = request
            .invoke_parameters
            .ok_or(bad_request_error("Missing invoke parameters"))?;

        let idempotency_key = request
            .idempotency_key
            .ok_or_else(|| bad_request_error("Missing idempotency key"))?
            .into();

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;
        let result = self
            .worker_service
            .invoke_and_await_typed(
                &worker_id,
                Some(idempotency_key),
                request.function,
                params.params,
                request.context,
                namespace,
            )
            .await?;

        Ok(InvokeResultTyped {
            result: Some(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                type_annotated_value: Some(result),
            }),
        })
    }

    async fn resume_worker(
        &self,
        request: ResumeWorkerRequest,
        metadata: MetadataMap,
    ) -> Result<(), GrpcWorkerError> {
        let auth = self.auth(metadata)?;
        let worker_id = validate_protobuf_worker_id(request.worker_id)?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;
        self.worker_service
            .resume(&worker_id, namespace, request.force.unwrap_or(false))
            .await?;

        Ok(())
    }

    async fn connect_worker(
        &self,
        request: ConnectWorkerRequest,
        metadata: MetadataMap,
    ) -> Result<ConnectWorkerStream, GrpcWorkerError> {
        let auth = self.auth(metadata)?;
        let worker_id = validate_protobuf_worker_id(request.worker_id)?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::ViewWorker, &auth)
            .await?;
        let stream = self.worker_service.connect(&worker_id, namespace).await?;

        Ok(stream)
    }

    async fn update_worker(
        &self,
        request: UpdateWorkerRequest,
        metadata: MetadataMap,
    ) -> Result<(), GrpcWorkerError> {
        let worker_id = validate_protobuf_worker_id(request.worker_id.clone())?;

        let auth = self.auth(metadata)?;
        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;
        self.worker_service
            .update(
                &worker_id,
                request.mode(),
                request.target_version,
                namespace,
            )
            .await?;

        Ok(())
    }

    async fn get_oplog(
        &self,
        request: GetOplogRequest,
        metadata: MetadataMap,
    ) -> Result<GetOplogSuccessResponse, GrpcWorkerError> {
        let auth = self.auth(metadata)?;
        let worker_id = validate_protobuf_worker_id(request.worker_id)?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::ViewWorker, &auth)
            .await?;
        let result = self
            .worker_service
            .get_oplog(
                &worker_id,
                OplogIndex::from_u64(request.from_oplog_index),
                request.cursor.map(|cursor| cursor.into()),
                request.count,
                namespace,
            )
            .await?;

        Ok(GetOplogSuccessResponse {
            entries: result
                .entries
                .into_iter()
                .map(|e| {
                    let entry: Result<
                        golem_api_grpc::proto::golem::worker::OplogEntryWithIndex,
                        String,
                    > = e.try_into();
                    entry.map(|e| e.entry.unwrap())
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| GrpcWorkerError {
                    error: Some(worker_error::Error::InternalError(WorkerExecutionError {
                        error: Some(worker_execution_error::Error::Unknown(UnknownError {
                            details: format!("Failed to convert oplog entry: {err:?}"),
                        })),
                    })),
                })?,
            next: result.next.map(|c| c.into()),
            first_index_in_chunk: result.first_index_in_chunk,
            last_index: result.last_index,
        })
    }

    async fn search_oplog(
        &self,
        request: SearchOplogRequest,
        metadata: MetadataMap,
    ) -> Result<SearchOplogSuccessResponse, GrpcWorkerError> {
        let auth = self.auth(metadata)?;
        let worker_id = validate_protobuf_worker_id(request.worker_id)?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::ViewWorker, &auth)
            .await?;
        let result = self
            .worker_service
            .search_oplog(
                &worker_id,
                request.cursor.map(|cursor| cursor.into()),
                request.count,
                request.query,
                namespace,
            )
            .await?;

        Ok(SearchOplogSuccessResponse {
            entries: result
                .entries
                .into_iter()
                .map(|e| {
                    let entry: Result<
                        golem_api_grpc::proto::golem::worker::OplogEntryWithIndex,
                        String,
                    > = e.try_into();
                    entry
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| GrpcWorkerError {
                    error: Some(worker_error::Error::InternalError(WorkerExecutionError {
                        error: Some(worker_execution_error::Error::Unknown(UnknownError {
                            details: format!("Failed to convert oplog entry: {err:?}"),
                        })),
                    })),
                })?,
            next: result.next.map(|c| c.into()),
            last_index: result.last_index,
        })
    }

    async fn list_directory(
        &self,
        request: ListDirectoryRequest,
        metadata: MetadataMap,
    ) -> Result<
        golem_api_grpc::proto::golem::worker::v1::ListDirectorySuccessResponse,
        GrpcWorkerError,
    > {
        let auth = self.auth(metadata)?;
        let worker_id = validate_protobuf_target_worker_id(request.worker_id)?;
        let file_path = validate_component_file_path(request.path)?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::ViewWorker, &auth)
            .await?;
        let result = self
            .worker_service
            .list_directory(&worker_id, file_path, namespace)
            .await?;

        Ok(
            golem_api_grpc::proto::golem::worker::v1::ListDirectorySuccessResponse {
                nodes: result.into_iter().map(|e| e.into()).collect(),
            },
        )
    }

    async fn get_file_contents(
        &self,
        request: GetFileContentsRequest,
        metadata: MetadataMap,
    ) -> Result<<Self as GrpcWorkerService>::GetFileContentsStream, GrpcWorkerError> {
        let auth = self.auth(metadata)?;
        let worker_id = validate_protobuf_target_worker_id(request.worker_id)?;
        let file_path = validate_component_file_path(request.file_path)?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::ViewWorker, &auth)
            .await?;
        let stream = self
            .worker_service
            .get_file_contents(
                &worker_id,
                file_path,
                namespace,
            )
            .await?
            .map(|item|
                match item {
                    Ok(data) =>
                        Ok(golem_api_grpc::proto::golem::worker::v1::GetFileContentsResponse {
                            result: Some(golem_api_grpc::proto::golem::worker::v1::get_file_contents_response::Result::Success(data.into())),
                        }),
                    Err(error) =>
                        Ok(golem_api_grpc::proto::golem::worker::v1::GetFileContentsResponse {
                            result: Some(golem_api_grpc::proto::golem::worker::v1::get_file_contents_response::Result::Error(error.into())),
                        })
                }
            )
            ;

        Ok(Box::pin(stream))
    }

    async fn activate_plugin(
        &self,
        request: ActivatePluginRequest,
        metadata: MetadataMap,
    ) -> Result<(), GrpcWorkerError> {
        let worker_id = validate_protobuf_worker_id(request.worker_id)?;
        let plugin_installation_id =
            validate_protobuf_plugin_installation_id(request.installation_id)?;

        let auth = self.auth(metadata)?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::CreateWorker, &auth)
            .await?;

        self.worker_service
            .activate_plugin(&worker_id, &plugin_installation_id, namespace)
            .await?;

        Ok(())
    }

    async fn deactivate_plugin(
        &self,
        request: DeactivatePluginRequest,
        metadata: MetadataMap,
    ) -> Result<(), GrpcWorkerError> {
        let worker_id = validate_protobuf_worker_id(request.worker_id)?;
        let plugin_installation_id =
            validate_protobuf_plugin_installation_id(request.installation_id)?;

        let auth = self.auth(metadata)?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::CreateWorker, &auth)
            .await?;

        self.worker_service
            .deactivate_plugin(&worker_id, &plugin_installation_id, namespace)
            .await?;

        Ok(())
    }

    async fn fork_worker(
        &self,
        request: ForkWorkerRequest,
        metadata: MetadataMap,
    ) -> Result<(), GrpcWorkerError> {
        let auth = self.auth(metadata)?;

        let source_worker_id = validate_protobuf_worker_id(request.source_worker_id)?;
        let target_worker_id = validate_protobuf_worker_id(request.target_worker_id)?;
        let oplog_idx = OplogIndex::from_u64(request.oplog_index_cutoff);

        let namespace = self
            .auth_service
            .is_authorized_by_component(
                &source_worker_id.component_id,
                ProjectAction::UpdateWorker,
                &auth,
            )
            .await?;

        self.worker_service
            .fork_worker(&source_worker_id, &target_worker_id, oplog_idx, namespace)
            .await?;

        Ok(())
    }

    async fn revert_worker(
        &self,
        request: RevertWorkerRequest,
        metadata: MetadataMap,
    ) -> Result<(), GrpcWorkerError> {
        let auth = self.auth(metadata)?;

        let worker_id = validate_protobuf_worker_id(request.worker_id)?;
        let target = request
            .target
            .ok_or_else(|| bad_request_error("Missing target"))?
            .try_into()
            .map_err(|err| bad_request_error(format!("Invalid target {}", err)))?;

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;

        self.worker_service
            .revert_worker(&worker_id, target, namespace)
            .await?;

        Ok(())
    }

    async fn cancel_invocation(
        &self,
        request: CancelInvocationRequest,
        metadata: MetadataMap,
    ) -> Result<bool, GrpcWorkerError> {
        let auth = self.auth(metadata)?;

        let worker_id = validate_protobuf_worker_id(request.worker_id)?;
        let idempotency_key = request
            .idempotency_key
            .ok_or_else(|| bad_request_error("Missing idempotency key"))?
            .into();

        let namespace = self
            .auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;

        let result = self
            .worker_service
            .cancel_invocation(&worker_id, &idempotency_key, namespace)
            .await?;

        Ok(result)
    }
}

impl From<worker::WorkerError> for GrpcWorkerError {
    fn from(error: worker::WorkerError) -> Self {
        GrpcWorkerError {
            error: Some(error.into()),
        }
    }
}

impl From<worker::WorkerError> for worker_error::Error {
    fn from(value: worker::WorkerError) -> Self {
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
                        error: error.to_safe_string(),
                    })
                }
                WorkerServiceError::Golem(golem) => {
                    worker_error::Error::InternalError(golem.into())
                }
                WorkerServiceError::Internal(error) => {
                    worker_error::Error::InternalError(WorkerExecutionError {
                        error: Some(worker_execution_error::Error::Unknown(UnknownError {
                            details: error,
                        })),
                    })
                }
                WorkerServiceError::InternalCallError(_) => {
                    worker_error::Error::InternalError(WorkerExecutionError {
                        error: Some(worker_execution_error::Error::Unknown(UnknownError {
                            details: error.to_safe_string(),
                        })),
                    })
                }
                WorkerServiceError::FileNotFound(_) => {
                    worker_error::Error::BadRequest(ErrorsBody {
                        errors: vec![error.to_safe_string()],
                    })
                }
                WorkerServiceError::BadFileType(_) => worker_error::Error::BadRequest(ErrorsBody {
                    errors: vec![error.to_safe_string()],
                }),
            },
            worker::WorkerError::Forbidden(error) => {
                worker_error::Error::LimitExceeded(ErrorBody { error })
            }
            worker::WorkerError::Unauthorized(error) => {
                worker_error::Error::Unauthorized(ErrorBody { error })
            }
            worker::WorkerError::InternalAuthServiceError(error) => {
                worker_error::Error::InternalError(WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: error.to_safe_string(),
                    })),
                })
            }
            worker::WorkerError::ProjectNotFound(_) => worker_error::Error::NotFound(ErrorBody {
                error: value.to_safe_string(),
            }),
        }
    }
}
