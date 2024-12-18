use thiserror::Error;

#[derive(Error, Debug)]
pub enum OpenAPIError {
    #[error("OpenAPI validation failed: {0}")]
    ValidationFailed(String),
    #[error("Invalid API definition: {0}")]
    InvalidDefinition(String),
    #[error("Cache error: {0}")]
    CacheError(String),
}