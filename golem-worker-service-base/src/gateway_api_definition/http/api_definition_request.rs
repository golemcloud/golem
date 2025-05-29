// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::gateway_api_definition::http::{AllPathPatterns, MethodPattern, Route};
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use crate::gateway_binding::GatewayBinding;
use crate::gateway_security::{SecuritySchemeIdentifier, SecuritySchemeReference};
use golem_api_grpc::proto::golem::apidefinition as grpc_apidefinition;

// HttpApiDefinitionRequest corresponds to the user facing http api definition.
// It has security at the global level, which is following OpenAPI style of defining security at the root level.
// Along with this, it has RouteRequest instead of Route.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpApiDefinitionRequest {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub routes: Vec<RouteRequest>,
    pub draft: bool,
}

impl TryFrom<grpc_apidefinition::v1::ApiDefinitionRequest> for HttpApiDefinitionRequest {
    type Error = String;

    fn try_from(value: grpc_apidefinition::v1::ApiDefinitionRequest) -> Result<Self, Self::Error> {
        let mut global_securities = vec![];
        let mut route_requests = vec![];

        match value.definition.ok_or("definition is missing")? {
            grpc_apidefinition::v1::api_definition_request::Definition::Http(http) => {
                for route in http.routes {
                    let route_request =
                        crate::gateway_api_definition::http::RouteRequest::try_from(route)?;
                    if let Some(security) = &route_request.security {
                        global_securities.push(security.clone());
                    }

                    route_requests.push(route_request);
                }
            }
        };

        let id = value.id.ok_or("Api Definition ID is missing")?;

        let result = Self {
            id: ApiDefinitionId(id.value),
            version: ApiVersion(value.version),
            routes: route_requests,
            draft: value.draft,
        };

        Ok(result)
    }
}

// In a RouteRequest, security is defined at the outer level
// Also this security has minimal information (and avoid details such as client-id, secret etc).
// When `RouteRequest` is converted to `Route`, this security is pushed as middleware in the binding
// along with fetching more details about the security scheme
#[derive(Debug, Clone, PartialEq)]
pub struct RouteRequest {
    pub method: MethodPattern,
    pub path: AllPathPatterns,
    pub binding: GatewayBinding,
    pub security: Option<SecuritySchemeReference>,
}

impl From<Route> for RouteRequest {
    fn from(value: Route) -> Self {
        let security_middleware = value
            .middlewares
            .clone()
            .and_then(|x| x.get_http_authentication_middleware());

        RouteRequest {
            method: value.method,
            path: value.path,
            binding: value.binding,
            security: security_middleware
                .map(|x| SecuritySchemeReference::from(x.security_scheme_with_metadata)),
        }
    }
}

impl TryFrom<grpc_apidefinition::HttpRoute> for RouteRequest {
    type Error = String;

    fn try_from(value: grpc_apidefinition::HttpRoute) -> Result<Self, Self::Error> {
        let path = AllPathPatterns::parse(value.path.as_str()).map_err(|e| e.to_string())?;
        let binding = value.binding.ok_or("binding is missing")?;
        let method: MethodPattern = value.method.try_into()?;

        let gateway_binding = GatewayBinding::try_from(binding)?;
        let security = value.middleware.clone().and_then(|x| x.http_authentication);

        let security = security.and_then(|x| {
            x.security_scheme.map(|x| SecuritySchemeReference {
                security_scheme_identifier: SecuritySchemeIdentifier::new(x.scheme_identifier),
            })
        });

        let result = Self {
            method,
            path,
            binding: gateway_binding,
            security,
        };

        Ok(result)
    }
}
