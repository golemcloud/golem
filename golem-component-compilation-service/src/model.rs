use std::fmt::Display;

use golem_common::model::ComponentId;
use tokio::sync::mpsc;
use wasmtime::component::Component;

#[derive(Debug, Clone)]
pub struct ComponentWithVersion {
    pub id: ComponentId,
    pub version: u64,
}

impl Display for ComponentWithVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.id, self.version)
    }
}

#[derive(Debug)]
pub struct CompilationRequest {
    pub component: ComponentWithVersion,
}

pub struct CompiledComponent {
    pub component_and_version: ComponentWithVersion,
    pub component: Component,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum CompilationError {
    #[error("Component not found: {0}")]
    ComponentNotFound(ComponentWithVersion),
    #[error("Failed to compile component: {0}")]
    CompileFailure(String),
    #[error("Failed to download component: {0}")]
    ComponentDownloadFailed(String),
    #[error("Failed to upload component: {0}")]
    ComponentUploadFailed(String),
    #[error("Unexpected error: {0}")]
    Unexpected(String),
}

impl<T> From<mpsc::error::SendError<T>> for CompilationError {
    fn from(_: mpsc::error::SendError<T>) -> Self {
        CompilationError::Unexpected("Failed to send compilation request".to_string())
    }
}
