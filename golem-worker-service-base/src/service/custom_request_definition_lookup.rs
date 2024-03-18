use crate::api_definition::{ApiDefinition, ApiDefinitionId, Version};
use crate::api_definition_repo::ApiDefinitionRepo;
use crate::auth::CommonNamespace;
use crate::http_request::InputHttpRequest;
use crate::oas_worker_bridge::{GOLEM_API_DEFINITION_ID_EXTENSION, GOLEM_API_DEFINITION_VERSION};
use crate::service::register_definition::ApiDefinitionKey;
use async_trait::async_trait;
use http::HeaderMap;
use std::fmt::Display;
use std::sync::Arc;
use tracing::error;

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
