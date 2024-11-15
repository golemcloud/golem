use crate::gateway_api_definition::http::{HttpApiDefinition, MethodPattern};

// Any pre-processing required for ApiDefinition
pub trait ApiDefinitionTransformer {
    fn transform(&mut self) -> Result<(), ApiDefTransformationError>;
}

#[derive(Debug)]
pub struct ApiDefTransformationError {
    pub method: MethodPattern,
    pub path: String,
    pub detail: String,
}

impl ApiDefTransformationError {
    pub fn new(method: MethodPattern, path: String, detail: String) -> Self {
        ApiDefTransformationError {
            method,
            path,
            detail,
        }
    }
}

impl ApiDefinitionTransformer for HttpApiDefinition {
    fn transform(&mut self) -> Result<(), ApiDefTransformationError> {
        internal::update_routes_with_cors_middleware(&mut self.routes)
    }
}

mod internal {
    use crate::gateway_api_definition::http::{AllPathPatterns, MethodPattern, Route};
    use crate::gateway_middleware::{Cors, HttpMiddleware, Middleware};
    use crate::service::gateway::api_definition_transformer::ApiDefTransformationError;

    pub(crate) fn update_routes_with_cors_middleware(
        routes: &mut [Route],
    ) -> Result<(), ApiDefTransformationError> {
        // Collect paths that need CORS middleware to apply
        let mut paths_to_apply_cors = Vec::new();

        for route in routes.iter_mut() {
            // If Route has a preflight binding,
            // enforce OPTIONS method
            // and mark this route for CORS middleware application
            if let Some(cors) = route.cors_preflight_binding() {
                enforce_options_method(&route.path, &route.method)?;
                paths_to_apply_cors.push((route.path.clone(), cors)); // collect the paths and their CORS data
            }
        }

        for (path, cors) in paths_to_apply_cors {
            apply_cors_middleware_to_routes(routes, &path, cors)?;
        }

        Ok(())
    }

    fn enforce_options_method(
        path: &AllPathPatterns,
        method: &MethodPattern,
    ) -> Result<(), ApiDefTransformationError> {
        if *method != MethodPattern::Options {
            let error = ApiDefTransformationError::new(
                method.clone(),
                path.to_string(),
                format!(
                    "Invalid binding for resource '{}' with method '{}'. CORS binding is only supported for the OPTIONS method.",
                    path, method
                )
            );
            Err(error)
        } else {
            Ok(())
        }
    }

    fn apply_cors_middleware_to_routes(
        routes: &mut [Route],
        target_path: &AllPathPatterns,
        cors: Cors,
    ) -> Result<(), ApiDefTransformationError> {
        for route in routes.iter_mut() {
            if route.path == *target_path && route.method != MethodPattern::Options {
                add_cors_middleware_to_route(route, cors.clone())?;
            }
        }
        Ok(())
    }

    fn add_cors_middleware_to_route(
        route: &mut Route,
        cors: Cors,
    ) -> Result<(), ApiDefTransformationError> {
        if let Some(worker_binding) = route.binding.get_worker_binding_mut() {
            if worker_binding.get_cors_middleware().is_some() {
                let error = ApiDefTransformationError::new(
                    route.method.clone(),
                    route.path.to_string(),
                    format!(
                        "Conflicting CORS middleware configuration for path '{}' at method '{}'. \
                    CORS preflight is already configured for the same path.",
                        route.path, route.method
                    ),
                );
                Err(error)
            } else {
                worker_binding
                    .add_middleware(Middleware::Http(HttpMiddleware::AddCorsHeaders(cors)));
                Ok(())
            }
        } else {
            Ok(())
        }
    }
}
#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::gateway_api_definition::http::{
        AllPathPatterns, HttpApiDefinition, MethodPattern, Route,
    };
    use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
    use crate::gateway_binding::{GatewayBinding, ResponseMapping, StaticBinding, WorkerBinding};
    use crate::gateway_middleware::{Cors, HttpMiddleware, Middleware, Middlewares};
    use crate::service::gateway::api_definition_transformer::ApiDefinitionTransformer;
    use golem_common::model::ComponentId;
    use golem_service_base::model::VersionedComponentId;
    use rib::Expr;

    fn get_cors_preflight_route() -> Route {
        Route {
            method: MethodPattern::Options,
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Static(StaticBinding::from_http_cors(cors())),
        }
    }

    fn get_invalid_cors_preflight_route() -> Route {
        Route {
            method: MethodPattern::Get, // Should be OPTIONS
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Static(StaticBinding::from_http_cors(cors())),
        }
    }

    fn get_route_with_worker_binding() -> Route {
        let worker_binding = WorkerBinding {
            component_id: VersionedComponentId {
                component_id: ComponentId::new_v4(),
                version: 1,
            },
            worker_name: None,
            idempotency_key: None,
            response_mapping: ResponseMapping(Expr::literal("")),
            middleware: None,
        };

        Route {
            method: MethodPattern::Get,
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Default(worker_binding.clone()),
        }
    }

    fn get_route_with_worker_binding_and_cors_middleware() -> Route {
        let worker_binding = WorkerBinding {
            component_id: VersionedComponentId {
                component_id: ComponentId::new_v4(),
                version: 1,
            },
            worker_name: None,
            idempotency_key: None,
            response_mapping: ResponseMapping(Expr::literal("")),
            middleware: Some(Middlewares(vec![Middleware::Http(
                HttpMiddleware::AddCorsHeaders(cors()),
            )])),
        };

        Route {
            method: MethodPattern::Get,
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Default(worker_binding.clone()),
        }
    }

    fn cors() -> Cors {
        Cors::from_parameters(
            Some("*".to_string()),
            Some("GET, POST".to_string()),
            Some("Content-Type".to_string()),
            Some("X-Custom-Header".to_string()),
            Some(true),
            Some(86400),
        )
        .unwrap()
    }

    #[test]
    fn test_update_routes_with_cors_middleware_applies_cors_to_worker_routes() {
        let cors_preflight_route = get_cors_preflight_route();

        let route_with_worker_binding = get_route_with_worker_binding();

        let routes = vec![
            cors_preflight_route.clone(),
            route_with_worker_binding.clone(),
        ];

        let mut api_definition = HttpApiDefinition {
            id: ApiDefinitionId("test".to_string()),
            routes,
            version: ApiVersion::new("v1"),
            draft: false,
            created_at: chrono::Utc::now(),
        };

        api_definition.transform().unwrap();

        let updated_route_with_worker_binding = api_definition
            .routes
            .iter()
            .find(|r| r.method == MethodPattern::Get && r.path == route_with_worker_binding.path)
            .unwrap();

        let new_worker_binding_middlewares = updated_route_with_worker_binding
            .binding
            .get_worker_binding()
            .unwrap()
            .middleware
            .unwrap();

        assert!(new_worker_binding_middlewares
            .0
            .contains(&Middleware::cors(&cors())));
    }

    #[test]
    fn test_update_routes_with_cors_middleware_fails_for_non_options_cors_route() {
        let invalid_cors_preflight_route = get_invalid_cors_preflight_route();

        let route_with_worker_binding = get_route_with_worker_binding();

        let routes = vec![invalid_cors_preflight_route, route_with_worker_binding];

        let mut api_definition = HttpApiDefinition {
            id: ApiDefinitionId("test".to_string()),
            routes,
            version: ApiVersion::new("v1"),
            draft: false,
            created_at: chrono::Utc::now(),
        };

        let result = api_definition.transform().map_err(|x| x.detail);

        assert_eq!(result.err(), Some("Invalid binding for resource '/test' with method 'Get'. CORS binding is only supported for the OPTIONS method.".to_string()));
    }

    #[test]
    fn test_update_routes_with_cors_middleware_does_not_duplicate_cors() {
        // Set up CORS preflight route with OPTIONS method
        let cors_preflight_route = get_cors_preflight_route();

        // Worker route for the same path with pre-existing CORS middleware
        let route_with_worker_binding_with_cors_middleware =
            get_route_with_worker_binding_and_cors_middleware();

        let routes = vec![
            cors_preflight_route.clone(),
            route_with_worker_binding_with_cors_middleware.clone(),
        ];

        let mut api_definition = HttpApiDefinition {
            id: ApiDefinitionId("test".to_string()),
            routes,
            version: ApiVersion::new("v1"),
            draft: false,
            created_at: chrono::Utc::now(),
        };

        let result = api_definition.transform();

        assert!(result.is_err());
    }
}
