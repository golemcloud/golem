use std::sync::Arc;

use crate::service::CompilationService;

use async_trait::async_trait;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::compilationserver::compilation_server_server::CompilationServer as GrpcCompilationServer;
use golem_api_grpc::proto::golem::compilationserver::{
    compilation_error, compilation_response, CompilationError, CompilationRequest,
    CompilationResponse,
};
use golem_api_grpc::proto::golem::template;
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
        request: Request<CompilationRequest>,
    ) -> Result<tonic::Response<CompilationResponse>, Status> {
        let response = match self.enqueue_compilation_impl(request.into_inner()).await {
            Ok(_) => compilation_response::Result::Success(Empty {}),
            Err(e) => compilation_response::Result::Failure(e),
        };

        Ok(Response::new(CompilationResponse {
            result: Some(response),
        }))
    }
}

impl CompileGrpcService {
    async fn enqueue_compilation_impl(
        &self,
        request: CompilationRequest,
    ) -> Result<(), CompilationError> {
        let template_id = make_template_id(request.template_id)?;
        let template_version = request.template_version;
        self.service
            .enqueue_compilation(template_id, template_version)
            .await?;
        Ok(())
    }
}

impl From<crate::model::CompilationError> for CompilationError {
    fn from(value: crate::model::CompilationError) -> Self {
        let body = ErrorBody {
            error: value.to_string(),
        };

        let error = match value {
            crate::model::CompilationError::TemplateNotFound(_) => {
                compilation_error::Error::NotFound(body)
            }
            crate::model::CompilationError::CompileFailure(_)
            | crate::model::CompilationError::TemplateDownloadFailed(_)
            | crate::model::CompilationError::TemplateUploadFailed(_)
            | crate::model::CompilationError::Unexpected(_) => {
                compilation_error::Error::InternalError(body)
            }
        };

        CompilationError { error: Some(error) }
    }
}

fn make_template_id(id: Option<template::TemplateId>) -> Result<TemplateId, CompilationError> {
    let id = id.ok_or_else(|| bad_request_error("Missing template id"))?;
    let id: TemplateId = id
        .try_into()
        .map_err(|_| bad_request_error("Invalid template id"))?;
    Ok(id)
}

fn bad_request_error(error: impl Into<String>) -> CompilationError {
    CompilationError {
        error: Some(compilation_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.into()],
        })),
    }
}
