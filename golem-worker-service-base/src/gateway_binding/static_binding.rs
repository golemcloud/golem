use crate::gateway_middleware::{Cors, SecuritySchemeWithProviderMetadata};
use crate::gateway_security::{
    GolemIdentityProviderMetadata, IdentityProvider, OpenIdClient, SecurityScheme,
};
use openidconnect::{AuthorizationCode, ClientId, ClientSecret};
use std::sync::Arc;

// Static bindings must NOT contain Rib, in either pre-compiled or raw form,
// as it may introduce unnecessary latency
// in serving the requests when not needed.
// Example of a static binding is a pre-flight request which can be handled by CorsPreflight
// Example: browser requests for preflights need only what's contained in a pre-flight CORS middleware and
// don't need to pass through to the backend.
#[derive(Debug, Clone, PartialEq)]
pub enum StaticBinding {
    HttpCorsPreflight(Cors),
    HttpAuthCallBack(SecuritySchemeWithProviderMetadata),
}

impl StaticBinding {
    pub fn from_http_cors(cors: Cors) -> Self {
        StaticBinding::HttpCorsPreflight(cors)
    }

    pub fn get_cors_preflight(&self) -> Option<Cors> {
        match self {
            StaticBinding::HttpCorsPreflight(preflight) => Some(preflight.clone()),
            _ => None,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::StaticBinding> for StaticBinding {
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::StaticBinding,
    ) -> Result<Self, String> {
        match value.static_binding {
            Some(golem_api_grpc::proto::golem::apidefinition::static_binding::StaticBinding::HttpCorsPreflight(cors_preflight)) => {
                Ok(StaticBinding::HttpCorsPreflight(cors_preflight.try_into()?))

            }
            _ => Err("Unknown static binding type".to_string()),
        }
    }
}

impl From<StaticBinding> for golem_api_grpc::proto::golem::apidefinition::StaticBinding {
    fn from(value: StaticBinding) -> Self {
        match value {
            StaticBinding::HttpCorsPreflight(cors) => {
                golem_api_grpc::proto::golem::apidefinition::StaticBinding {
                    static_binding: Some(golem_api_grpc::proto::golem::apidefinition::static_binding::StaticBinding::HttpCorsPreflight(
                        golem_api_grpc::proto::golem::apidefinition::CorsPreflight::from(cors)
                    )),
                }
            }
            StaticBinding::HttpAuthCallBack(_) => {
                golem_api_grpc::proto::golem::apidefinition::StaticBinding {
                    static_binding: Some(golem_api_grpc::proto::golem::apidefinition::static_binding::StaticBinding::AuthCallback(
                        golem_api_grpc::proto::golem::apidefinition::AuthCallBack{}
                    )),
                }
            }
        }
    }
}
