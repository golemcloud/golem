use std::sync::Arc;

use crate::service::CompilationService;

use async_trait::async_trait;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::template;
use golem_api_grpc::proto::golem::templatecompilation::template_compilation_service_server::TemplateCompilationService as GrpcCompilationServer;
use golem_api_grpc::proto::golem::templatecompilation::{
    template_compilation_error, template_compilation_response, TemplateCompilationError,
    TemplateCompilationRequest, TemplateCompilationResponse,
};
use golem_common::model::TemplateId;
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
        request: Request<TemplateCompilationRequest>,
    ) -> Result<tonic::Response<TemplateCompilationResponse>, Status> {
        let response = match self.enqueue_compilation_impl(request.into_inner()).await {
            Ok(_) => template_compilation_response::Result::Success(Empty {}),
            Err(e) => template_compilation_response::Result::Failure(e),
        };

        Ok(Response::new(TemplateCompilationResponse {
            result: Some(response),
        }))
    }
}

impl CompileGrpcService {
    async fn enqueue_compilation_impl(
        &self,
        request: TemplateCompilationRequest,
    ) -> Result<(), TemplateCompilationError> {
        let template_id = make_template_id(request.template_id)?;
        let template_version = request.template_version;
        self.service
            .enqueue_compilation(template_id, template_version)
            .await?;
        Ok(())
    }
}

impl From<crate::model::CompilationError> for TemplateCompilationError {
    fn from(value: crate::model::CompilationError) -> Self {
        let body = ErrorBody {
            error: value.to_string(),
        };

        let error = match value {
            crate::model::CompilationError::TemplateNotFound(_) => {
                template_compilation_error::Error::NotFound(body)
            }
            crate::model::CompilationError::CompileFailure(_)
            | crate::model::CompilationError::TemplateDownloadFailed(_)
            | crate::model::CompilationError::TemplateUploadFailed(_)
            | crate::model::CompilationError::Unexpected(_) => {
                template_compilation_error::Error::InternalError(body)
            }
        };

        TemplateCompilationError { error: Some(error) }
    }
}

fn make_template_id(
    id: Option<template::TemplateId>,
) -> Result<TemplateId, TemplateCompilationError> {
    let id = id.ok_or_else(|| bad_request_error("Missing template id"))?;
    let id: TemplateId = id
        .try_into()
        .map_err(|_| bad_request_error("Invalid template id"))?;
    Ok(id)
}

fn bad_request_error(error: impl Into<String>) -> TemplateCompilationError {
    TemplateCompilationError {
        error: Some(template_compilation_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.into()],
        })),
    }
}
