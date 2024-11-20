use crate::gateway_api_definition::http::RouteRequest;
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use crate::gateway_security::SecuritySchemeReference;

#[derive(Debug, Clone, PartialEq)]
pub struct HttpApiDefinitionRequest {
    pub id: ApiDefinitionId,
    pub security_schemes: Vec<SecuritySchemeReference>, // This is needed at global level only for request (user facing http api definition)
    pub version: ApiVersion,
    pub routes: Vec<RouteRequest>,
    pub draft: bool,
}
