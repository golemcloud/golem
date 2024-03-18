use crate::api_definition::ApiDefinition;
use crate::http_request::InputHttpRequest;
use async_trait::async_trait;
use std::fmt::Display;

#[async_trait]
pub trait CustomRequestDefinitionLookup {
    async fn get(
        &self,
        input_http_request: &InputHttpRequest<'_>,
    ) -> Result<ApiDefinition, ApiDefinitionLookupError>;
}

pub struct ApiDefinitionLookupError(String);

impl Display for ApiDefinitionLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ApiDefinitionLookupError: {}", self.0)
    }
}
