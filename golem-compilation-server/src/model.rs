use std::fmt::Display;

use golem_common::model::TemplateId;
use tokio::sync::{mpsc, oneshot};
use wasmtime::component::Component;

#[derive(Debug, Clone)]
pub struct TemplateWithVersion {
    pub id: TemplateId,
    pub version: i32,
}

impl Display for TemplateWithVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.id, self.version)
    }
}

#[derive(Debug)]
pub struct CompilationRequest {
    pub template: TemplateWithVersion,
    pub result: oneshot::Sender<Result<(), CompilationError>>,
}

pub struct CompiledTemplate {
    pub template: TemplateWithVersion,
    pub component: Component,
    pub result: oneshot::Sender<Result<(), CompilationError>>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum CompilationError {
    #[error("Template not found: {0}")]
    TemplateNotFound(TemplateWithVersion),
    #[error("Failed to compile template: {0}")]
    CompileFailure(String),
    #[error("Failed to download template: {0}")]
    TemplateDownloadFailed(String),
    #[error("Failed to upload template: {0}")]
    TemplateUploadFailed(String),
    #[error("Unexpected error: {0}")]
    Unexpected(String),
}

impl<T> From<mpsc::error::SendError<T>> for CompilationError {
    fn from(_: mpsc::error::SendError<T>) -> Self {
        CompilationError::Unexpected("Failed to send compilation request".to_string())
    }
}
