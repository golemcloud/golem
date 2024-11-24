// Copyright 2024 Golem Cloud
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

use crate::gateway_api_definition::http::HttpApiDefinition;
use crate::gateway_api_definition_transformer::{
    ApiDefTransformationError, ApiDefinitionTransformer,
};
use std::collections::HashMap;

// Auth transformer ensures that for all security schemes
// configured in different parts of the ApiDefinition, there
// exist a auth call back endpoint. We are not letting the users
// define this to have a reasonable DX.
pub struct AuthTransformer;

impl ApiDefinitionTransformer for AuthTransformer {
    fn transform(
        &self,
        api_definition: &mut HttpApiDefinition,
    ) -> Result<(), ApiDefTransformationError> {
        let mut distinct_auth_middlewares = HashMap::new();

        for i in api_definition.routes.iter() {
            let binding = &i.binding;
            let auth_middleware = binding.get_authenticate_request_middleware();

            if let Some(auth_middleware) = auth_middleware {
                distinct_auth_middlewares.insert(
                    auth_middleware
                        .security_scheme
                        .security_scheme
                        .scheme_identifier(),
                    auth_middleware.security_scheme,
                );
            }
        }

        let auth_call_back_routes = internal::get_auth_call_back_routes(distinct_auth_middlewares)
            .map_err(ApiDefTransformationError::Custom)?;

        let routes = &mut api_definition.routes;

        // Add if doesn't exist
        for r in auth_call_back_routes.iter() {
            if !routes
                .iter()
                .any(|x| (x.path == r.path) && (x.method == r.method))
            {
                routes.push(r.clone())
            }
        }

        Ok(())
    }
}

mod internal {
    use crate::gateway_api_definition::http::{AllPathPatterns, MethodPattern, Route};
    use crate::gateway_binding::{GatewayBinding, StaticBinding};
    use crate::gateway_middleware::HttpRequestAuthentication;
    use crate::gateway_security::{SecuritySchemeIdentifier, SecuritySchemeWithProviderMetadata};
    use std::collections::HashMap;

    pub(crate) fn get_auth_call_back_routes(
        security_schemes: HashMap<SecuritySchemeIdentifier, SecuritySchemeWithProviderMetadata>,
    ) -> Result<Vec<Route>, String> {
        let mut routes = vec![];

        for (_, scheme) in security_schemes {
            // In a security scheme, the auth-call-back (aka redirect_url) is full URL
            // and not just the relative path
            let redirect_url = scheme.security_scheme.redirect_url();
            let path = redirect_url.url().path();
            let path = AllPathPatterns::parse(path)?;
            let method = MethodPattern::Get;
            let binding = GatewayBinding::static_binding(StaticBinding::http_auth_call_back(
                HttpRequestAuthentication {
                    security_scheme: scheme,
                },
            ));

            let route = Route {
                path,
                method,
                binding,
            };

            routes.push(route)
        }

        Ok(routes)
    }
}
