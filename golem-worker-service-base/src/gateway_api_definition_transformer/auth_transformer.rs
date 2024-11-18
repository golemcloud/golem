use crate::gateway_api_definition::http::HttpApiDefinition;
use crate::gateway_api_definition_transformer::{
    ApiDefTransformationError, ApiDefinitionTransformer,
};
use std::collections::HashMap;

// Auth transformer ensures that for all security schemes
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
                .iter().any(|x| (x.path == r.path) && (x.method == r.method))
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
            let redirect_url = scheme.security_scheme.redirect_url().to_string();
            let path = AllPathPatterns::parse(redirect_url.as_str())?;
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
