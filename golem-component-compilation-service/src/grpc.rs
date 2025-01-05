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

use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use tracing::Instrument;

use crate::service::CompilationService;

use async_trait::async_trait;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::component;
use golem_api_grpc::proto::golem::componentcompilation::v1::component_compilation_service_server::ComponentCompilationService as GrpcCompilationServer;
use golem_api_grpc::proto::golem::componentcompilation::v1::{
    component_compilation_error, component_compilation_response, ComponentCompilationError,
    ComponentCompilationRequest, ComponentCompilationResponse,
};
use golem_common::grpc::proto_component_id_string;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::ComponentId;
use golem_common::recorded_grpc_api_request;
use tonic::{Request, Response, Status};

#[derive(Clone)]
pub struct CompileGrpcService {
    service: Arc<dyn CompilationService + Send + Sync>,
}

impl CompileGrpcService {
    pub fn new(service: Arc<dyn CompilationService + Send + Sync>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl GrpcCompilationServer for CompileGrpcService {
    async fn enqueue_compilation(
        &self,
        request: Request<ComponentCompilationRequest>,
    ) -> Result<tonic::Response<ComponentCompilationResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "enqueue_compilation",
            component_id = proto_component_id_string(&request.component_id),
        );

        let response = match self
            .enqueue_compilation_impl(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(()) => record.succeed(component_compilation_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                component_compilation_response::Result::Failure(error.clone()),
                &ComponentCompilationTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(ComponentCompilationResponse {
            result: Some(response),
        }))
    }
}

impl CompileGrpcService {
    async fn enqueue_compilation_impl(
        &self,
        request: ComponentCompilationRequest,
    ) -> Result<(), ComponentCompilationError> {
        let component_id = make_component_id(request.component_id)?;
        let component_version = request.component_version;
        self.service
            .enqueue_compilation(component_id, component_version)
            .await?;
        Ok(())
    }
}

impl From<crate::model::CompilationError> for ComponentCompilationError {
    fn from(value: crate::model::CompilationError) -> Self {
        let body = ErrorBody {
            error: value.to_string(),
        };

        let error = match value {
            crate::model::CompilationError::ComponentNotFound(_) => {
                component_compilation_error::Error::NotFound(body)
            }
            crate::model::CompilationError::CompileFailure(_)
            | crate::model::CompilationError::ComponentDownloadFailed(_)
            | crate::model::CompilationError::ComponentUploadFailed(_)
            | crate::model::CompilationError::Unexpected(_) => {
                component_compilation_error::Error::InternalError(body)
            }
        };

        ComponentCompilationError { error: Some(error) }
    }
}

fn make_component_id(
    id: Option<component::ComponentId>,
) -> Result<ComponentId, ComponentCompilationError> {
    let id = id.ok_or_else(|| bad_request_error("Missing component id"))?;
    let id: ComponentId = id
        .try_into()
        .map_err(|_| bad_request_error("Invalid component id"))?;
    Ok(id)
}

fn bad_request_error(error: impl Into<String>) -> ComponentCompilationError {
    ComponentCompilationError {
        error: Some(component_compilation_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.into()],
        })),
    }
}

struct ComponentCompilationTraceErrorKind<'a>(&'a ComponentCompilationError);

impl Debug for ComponentCompilationTraceErrorKind<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl TraceErrorKind for ComponentCompilationTraceErrorKind<'_> {
    fn trace_error_kind(&self) -> &'static str {
        match &self.0.error {
            None => "None",
            Some(error) => match error {
                component_compilation_error::Error::BadRequest(_) => "BadRequest",
                component_compilation_error::Error::NotFound(_) => "NotFound",
                component_compilation_error::Error::InternalError(_) => "InternalError",
            },
        }
    }
}
