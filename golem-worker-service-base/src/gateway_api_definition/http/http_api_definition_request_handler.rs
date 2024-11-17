use crate::gateway_api_definition::http::{HttpApiDefinition, HttpApiDefinitionRequest};
use crate::service::gateway::security_scheme::SecuritySchemeService;
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
trait HttpApiDefinitionRequestHandler {
    async fn get_http_api_definition(&self, request: HttpApiDefinitionRequest)
        -> HttpApiDefinition;
}

pub struct DefaultHttpApiDefinitionRequestHandler<Namespace> {
    pub security_scheme_service: Arc<dyn SecuritySchemeService<Namespace> + Send + Sync>,
}

#[async_trait]
impl<Namespace> HttpApiDefinitionRequestHandler
    for DefaultHttpApiDefinitionRequestHandler<Namespace>
{
    async fn get_http_api_definition(
        &self,
        request: HttpApiDefinitionRequest,
    ) -> HttpApiDefinition {
    }
}
