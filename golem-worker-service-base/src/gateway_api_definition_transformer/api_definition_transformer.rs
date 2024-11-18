use crate::gateway_api_definition::http::{HttpApiDefinition, MethodPattern};
use std::fmt::{Display, Formatter};

// Any pre-processing required for ApiDefinition
pub trait ApiDefinitionTransformer {
    fn transform(
        &self,
        api_definition: &mut HttpApiDefinition,
    ) -> Result<(), ApiDefTransformationError>;
}

#[derive(Debug)]
pub enum ApiDefTransformationError {
    InvalidRoute {
        method: MethodPattern,
        path: String,
        detail: String,
    },
    Custom(String),
}

impl Display for ApiDefTransformationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiDefTransformationError::InvalidRoute {
                method,
                path,
                detail,
            } => write!(
                f,
                "ApiDefinitionTransformationError: method: {}, path: {}, detail: {}",
                method, path, detail
            )?,
            ApiDefTransformationError::Custom(msg) => {
                write!(f, "ApiDefinitionTransformationError: {}", msg)?
            }
        }

        Ok(())
    }
}

impl ApiDefTransformationError {
    pub fn new(method: MethodPattern, path: String, detail: String) -> Self {
        ApiDefTransformationError::InvalidRoute {
            method,
            path,
            detail,
        }
    }
}
