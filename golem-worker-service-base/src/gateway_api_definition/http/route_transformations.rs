use crate::gateway_api_definition::http::Route;

// For those resources with cors preflight enabled,
// update the middlewares of all the endpoints accessing that resource with cors middleware
pub fn update_routes_with_cors_middleware(routes: Vec<Route>) -> Result<Vec<Route>, String> {
    let mut updated_routes = routes.clone();

    for route in &routes {
        // If Route has a preflight binding,
        // enforce OPTIONS method
        // and apply CORS middleware to all the other routes with the same path / resource
        if let Some(cors) = route.cors_preflight_binding() {
            internal::enforce_options_method(&route.path, &route.method)?;
            internal::apply_cors_middleware_to_routes(&mut updated_routes, &route.path, cors)?;
        }
    }

    Ok(updated_routes)
}

mod internal {
    use crate::gateway_api_definition::http::{AllPathPatterns, MethodPattern, Route};
    use crate::gateway_middleware::{CorsPreflight, HttpMiddleware, Middleware};

    pub(crate) fn enforce_options_method(
        path: &AllPathPatterns,
        method: &MethodPattern,
    ) -> Result<(), String> {
        if *method != MethodPattern::Options {
            Err(format!(
                "Invalid binding for resource '{}' with method '{}'. CORS binding is only supported for the OPTIONS method.",
                path, method
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn apply_cors_middleware_to_routes(
        routes: &mut [Route],
        target_path: &AllPathPatterns,
        cors: CorsPreflight,
    ) -> Result<(), String> {
        for route in routes.iter_mut() {
            if route.path == *target_path && route.method != MethodPattern::Options {
                add_cors_middleware_to_route(route, cors.clone())?;
            }
        }
        Ok(())
    }

    fn add_cors_middleware_to_route(route: &mut Route, cors: CorsPreflight) -> Result<(), String> {
        if let Some(worker_binding) = route.binding.get_worker_binding_mut() {
            if worker_binding.get_cors_middleware().is_some() {
                Err(format!(
                    "Conflicting CORS middleware configuration for path '{}' at method '{}'. \
                CORS preflight is already configured for the same path.",
                    route.path, route.method
                ))
            } else {
                worker_binding.add_middleware(Middleware::Http(HttpMiddleware::Cors(cors)));
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

    use crate::gateway_api_definition::http::route_transformations::update_routes_with_cors_middleware;
    use crate::gateway_api_definition::http::{AllPathPatterns, MethodPattern, Route};
    use crate::gateway_binding::{GatewayBinding, ResponseMapping, StaticBinding, WorkerBinding};
    use crate::gateway_middleware::{CorsPreflight, HttpMiddleware, Middleware, Middlewares};
    use golem_common::model::ComponentId;
    use golem_service_base::model::VersionedComponentId;
    use rib::Expr;

    fn get_cors_preflight_route() -> Route {
        Route {
            method: MethodPattern::Options,
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Static(StaticBinding::from_http_middleware(
                HttpMiddleware::cors(cors()),
            )),
        }
    }

    fn get_invalid_cors_preflight_route() -> Route {
        Route {
            method: MethodPattern::Get, // Should be OPTIONS
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Static(StaticBinding::from_http_middleware(
                HttpMiddleware::cors(cors()),
            )),
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
            response: ResponseMapping(Expr::literal("")),
            middleware: None,
        };

        Route {
            method: MethodPattern::Get,
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Worker(worker_binding.clone()),
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
            response: ResponseMapping(Expr::literal("")),
            middleware: Some(Middlewares(vec![Middleware::Http(HttpMiddleware::Cors(
                cors(),
            ))])),
        };

        Route {
            method: MethodPattern::Get,
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Worker(worker_binding.clone()),
        }
    }

    fn cors() -> CorsPreflight {
        CorsPreflight {
            allow_origin: "*".to_string(),
            allow_methods: "GET, POST".to_string(),
            allow_headers: "Content-Type".to_string(),
            expose_headers: Some("X-Custom-Header".to_string()),
            max_age: Some(86400),
        }
    }

    #[test]
    fn test_update_routes_with_cors_middleware_applies_cors_to_worker_routes() {
        let cors_preflight_route = get_cors_preflight_route();

        let route_with_worker_binding = get_route_with_worker_binding();

        let routes = vec![
            cors_preflight_route.clone(),
            route_with_worker_binding.clone(),
        ];

        let result = update_routes_with_cors_middleware(routes);

        assert!(result.is_ok());

        let updated_routes = result.unwrap();

        let updated_route_with_worker_binding = updated_routes
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

        let result = update_routes_with_cors_middleware(routes);

        assert!(result.is_err());
        assert_eq!(result.err().unwrap(), "Invalid binding for resource '/test' with method 'Get'. CORS binding is only supported for the OPTIONS method.");
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

        let result = update_routes_with_cors_middleware(routes);

        assert!(result.is_err());
    }
}
