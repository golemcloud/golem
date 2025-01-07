// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::gateway_api_definition::http::{AllPathPatterns, MethodPattern, Route};
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use crate::gateway_binding::GatewayBinding;
use crate::gateway_middleware::HttpCors;
use crate::gateway_security::SecuritySchemeReference;

// HttpApiDefinitionRequest corresponds to the user facing http api definition.
// It has security at the global level, which is following OpenAPI style of defining security at the root level.
// Along with this, it has RouteRequest instead of Route.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpApiDefinitionRequest {
    pub id: ApiDefinitionId,
    pub security: Option<Vec<SecuritySchemeReference>>, // This is needed at global level only for request (user facing http api definition)
    pub version: ApiVersion,
    pub routes: Vec<RouteRequest>,
    pub draft: bool,
}

// In a RouteRequest, security is defined at the outer level
// to keep it consistent with the openAPI style of defining security at the root level.
// Also this security has minimal information (and avoid details such as client-id, secret etc).
// When `RouteRequest` is converted to `Route`, this security is pushed as middleware in the binding
// along with fetching more details about the security scheme
#[derive(Debug, Clone, PartialEq)]
pub struct RouteRequest {
    pub method: MethodPattern,
    pub path: AllPathPatterns,
    pub binding: GatewayBinding,
    pub cors: Option<HttpCors>,
    pub security: Option<SecuritySchemeReference>,
}

impl From<Route> for RouteRequest {
    fn from(value: Route) -> Self {
        let security_middleware = value
            .middlewares
            .clone()
            .and_then(|x| x.get_http_authentication_middleware());

        let cors_middleware = value.middlewares.and_then(|x| x.get_cors_middleware());

        RouteRequest {
            method: value.method,
            path: value.path,
            binding: value.binding,
            security: security_middleware
                .map(|x| SecuritySchemeReference::from(x.security_scheme_with_metadata)),
            cors: cors_middleware,
        }
    }
}
