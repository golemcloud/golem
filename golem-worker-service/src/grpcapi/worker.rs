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

use super::error::WorkerTraceErrorKind;
use super::{bad_request_error, validate_protobuf_worker_id};
use crate::service::worker::WorkerService;
use golem_api_grpc::proto::golem::common::Empty;
use golem_api_grpc::proto::golem::worker::v1::worker_service_server::WorkerService as GrpcWorkerService;
use golem_api_grpc::proto::golem::worker::v1::{
    complete_promise_response, fork_worker_response, invoke_and_await_response, invoke_response,
    launch_new_worker_response, resume_worker_response, revert_worker_response,
    update_worker_response, CompletePromiseRequest, CompletePromiseResponse, ForkWorkerRequest,
    ForkWorkerResponse, InvokeAndAwaitRequest, InvokeAndAwaitResponse, InvokeRequest,
    InvokeResponse, LaunchNewWorkerRequest, LaunchNewWorkerResponse,
    LaunchNewWorkerSuccessResponse, ResumeWorkerRequest, ResumeWorkerResponse, RevertWorkerRequest,
    RevertWorkerResponse, UpdateWorkerRequest, UpdateWorkerResponse,
    WorkerError as GrpcWorkerError,
};
use golem_api_grpc::proto::golem::worker::InvokeResultTyped;
use golem_common::grpc::{
    proto_component_id_string, proto_idempotency_key_string,
    proto_invocation_context_parent_worker_id_string, proto_worker_id_string,
};
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::worker::WorkerUpdateMode;
use golem_common::model::WorkerId;
use golem_common::recorded_grpc_api_request;
use golem_service_base::model::auth::AuthCtx;
use std::collections::BTreeMap;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::Instrument;

pub struct WorkerGrpcApi {
    worker_service: Arc<WorkerService>,
}

#[async_trait::async_trait]
impl GrpcWorkerService for WorkerGrpcApi {
    async fn launch_new_worker(
        &self,
        request: Request<LaunchNewWorkerRequest>,
    ) -> Result<Response<LaunchNewWorkerResponse>, Status> {
        let (_, _, request) = request.into_parts();
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
                    component_version: component_version.0,
                }),
            ),
            Err(error) => record.fail(
                launch_new_worker_response::Result::Error(error.clone()),
                &mut WorkerTraceErrorKind(&error),
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
        let (_, _, request) = request.into_parts();
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
                &mut WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(CompletePromiseResponse {
            result: Some(response),
        }))
    }

    async fn invoke_and_await(
        &self,
        request: Request<InvokeAndAwaitRequest>,
    ) -> Result<Response<InvokeAndAwaitResponse>, Status> {
        let (_, _, request) = request.into_parts();
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
                &mut WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(InvokeAndAwaitResponse {
            result: Some(response),
        }))
    }

    async fn invoke(
        &self,
        request: Request<InvokeRequest>,
    ) -> Result<Response<InvokeResponse>, Status> {
        let (_, _, request) = request.into_parts();
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
                &mut WorkerTraceErrorKind(&error),
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
        let (_, _, request) = request.into_parts();
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
                &mut WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(ResumeWorkerResponse {
            result: Some(response),
        }))
    }

    async fn update_worker(
        &self,
        request: Request<UpdateWorkerRequest>,
    ) -> Result<Response<UpdateWorkerResponse>, Status> {
        let (_, _, request) = request.into_parts();
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
                &mut WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(UpdateWorkerResponse {
            result: Some(response),
        }))
    }

    async fn fork_worker(
        &self,
        request: Request<ForkWorkerRequest>,
    ) -> Result<Response<ForkWorkerResponse>, Status> {
        let (_, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "fork_worker",
            source_worker_id = proto_worker_id_string(&request.source_worker_id),
            target_worker_id = proto_worker_id_string(&request.target_worker_id),
        );

        let response = match self
            .fork_worker(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(fork_worker_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                fork_worker_response::Result::Error(error.clone()),
                &mut WorkerTraceErrorKind(&error),
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
        let (_, _, request) = request.into_parts();
        let record = recorded_grpc_api_request!(
            "revert_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        let response = match self
            .revert_worker(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(revert_worker_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                revert_worker_response::Result::Error(error.clone()),
                &mut WorkerTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(RevertWorkerResponse {
            result: Some(response),
        }))
    }
}

impl WorkerGrpcApi {
    pub fn new(worker_service: Arc<WorkerService>) -> Self {
        Self { worker_service }
    }

    async fn launch_new_worker(
        &self,
        request: LaunchNewWorkerRequest,
    ) -> Result<(WorkerId, ComponentRevision), GrpcWorkerError> {
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;

        let component_id: golem_common::model::component::ComponentId = request
            .component_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component id"))?;

        let wasi_config_vars: BTreeMap<String, String> = request
            .wasi_config_vars
            .ok_or_else(|| bad_request_error("no wasi_config_vars field"))?
            .into();

        let worker_id = WorkerId {
            component_id,
            worker_name: request.name,
        };

        let latest_component_revision = self
            .worker_service
            .create(
                &worker_id,
                request.env,
                wasi_config_vars,
                request.ignore_already_existing,
                auth,
            )
            .await?;

        Ok((worker_id, latest_component_revision))
    }

    async fn complete_promise(
        &self,
        request: CompletePromiseRequest,
    ) -> Result<bool, GrpcWorkerError> {
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;
        let worker_id = validate_protobuf_worker_id(request.worker_id)?;

        let parameters = request
            .complete_parameters
            .ok_or_else(|| bad_request_error("Missing complete parameters"))?;

        let result = self
            .worker_service
            .complete_promise(&worker_id, parameters.oplog_idx, parameters.data, auth)
            .await?;

        Ok(result)
    }

    async fn invoke(&self, request: InvokeRequest) -> Result<(), GrpcWorkerError> {
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;
        let worker_id = validate_protobuf_worker_id(request.worker_id)?;

        let params = request
            .invoke_parameters
            .ok_or_else(|| bad_request_error("Missing invoke parameters"))?;

        self.worker_service
            .invoke(
                &worker_id,
                request.idempotency_key.map(|k| k.into()),
                request.function,
                params.params,
                request.context,
                auth,
            )
            .await?;

        Ok(())
    }

    async fn invoke_and_await(
        &self,
        request: InvokeAndAwaitRequest,
    ) -> Result<InvokeResultTyped, GrpcWorkerError> {
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;
        let worker_id = validate_protobuf_worker_id(request.worker_id)?;

        let params = request
            .invoke_parameters
            .ok_or_else(|| bad_request_error("Missing invoke parameters"))?;

        let idempotency_key = request
            .idempotency_key
            .ok_or_else(|| bad_request_error("Missing idempotency key"))?
            .into();

        let result = self
            .worker_service
            .invoke_and_await(
                &worker_id,
                Some(idempotency_key),
                request.function,
                params.params,
                request.context,
                auth,
            )
            .await?;

        Ok(InvokeResultTyped {
            result: result.map(|tav| tav.into()),
        })
    }

    async fn resume_worker(&self, request: ResumeWorkerRequest) -> Result<(), GrpcWorkerError> {
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;
        let worker_id = validate_protobuf_worker_id(request.worker_id)?;

        self.worker_service
            .resume(&worker_id, request.force.unwrap_or(false), auth)
            .await?;

        Ok(())
    }

    async fn update_worker(&self, request: UpdateWorkerRequest) -> Result<(), GrpcWorkerError> {
        let worker_update_mode: WorkerUpdateMode = request.mode().into();
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;
        let worker_id = validate_protobuf_worker_id(request.worker_id)?;
        let target_version = ComponentRevision(request.target_version);

        self.worker_service
            .update(&worker_id, worker_update_mode, target_version, auth)
            .await?;

        Ok(())
    }

    async fn fork_worker(&self, request: ForkWorkerRequest) -> Result<(), GrpcWorkerError> {
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;
        let source_worker_id = validate_protobuf_worker_id(request.source_worker_id)?;
        let target_worker_id = validate_protobuf_worker_id(request.target_worker_id)?;
        let oplog_idx = OplogIndex::from_u64(request.oplog_index_cutoff);

        self.worker_service
            .fork_worker(&source_worker_id, &target_worker_id, oplog_idx, auth)
            .await?;

        Ok(())
    }

    async fn revert_worker(&self, request: RevertWorkerRequest) -> Result<(), GrpcWorkerError> {
        let auth: AuthCtx = request
            .auth_ctx
            .ok_or(bad_request_error("auth_ctx not found"))?
            .try_into()
            .map_err(|e| bad_request_error(format!("failed converting auth_ctx: {e}")))?;

        let worker_id = validate_protobuf_worker_id(request.worker_id)?;

        let target = request
            .target
            .ok_or_else(|| bad_request_error("Missing target"))?
            .try_into()
            .map_err(|err| bad_request_error(format!("Invalid target {err}")))?;

        self.worker_service
            .revert_worker(&worker_id, target, auth)
            .await?;

        Ok(())
    }
}
