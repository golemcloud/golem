use async_trait::async_trait;
use std::fmt::Display;
use crate::http::http_api_definition::HttpApiDefinition;
use crate::http::http_request::InputHttpRequest;

#[async_trait]
pub trait HttpRequestDefinitionLookup {
    async fn get(
        &self,
        input_http_request: &InputHttpRequest<'_>,
    ) -> Result<HttpApiDefinition, ApiDefinitionLookupError>;
}

pub struct ApiDefinitionLookupError(pub String);

impl Display for ApiDefinitionLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ApiDefinitionLookupError: {}", self.0)
    }
}
