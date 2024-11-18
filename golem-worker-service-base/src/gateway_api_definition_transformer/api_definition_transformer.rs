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
pub struct ApiDefTransformationError {
    pub method: MethodPattern,
    pub path: String,
    pub detail: String,
}

impl Display for ApiDefTransformationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "RouteValidationError: method: {}, path: {}, detail: {}",
            self.method, self.path, self.detail
        )?;

        Ok(())
    }
}

impl ApiDefTransformationError {
    pub fn new(method: MethodPattern, path: String, detail: String) -> Self {
        ApiDefTransformationError {
            method,
            path,
            detail,
        }
    }
}
