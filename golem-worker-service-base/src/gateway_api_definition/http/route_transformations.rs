use crate::gateway_api_definition::http::{MethodPattern, Route};
use crate::gateway_binding::{GatewayBinding, StaticBinding};
use crate::gateway_middleware::{HttpMiddleware, Middleware};

// For those resources, with CORS enabled through OPTIONS method,
// update the middlewares of all the endpoints accessing that resource
pub fn  update_routes_with_cors_middleware(routes: Vec<Route>) -> Result<Vec<Route>, String> {
    let mut updated_routes = routes.clone();

    for route in routes.iter() {
        // Check if the route binding contains CORS middleware
        if let Some(HttpMiddleware::Cors(preflight)) = &route.binding.get_http_cors() {
            // Attempt to retrieve CORS middleware
            let path = &route.path;
            let method = &route.method;

            if method != &MethodPattern::Options {
                return Err(format!("Invalid route for {}. CORS binding is only supported for OPTIONS method", path));
            } else {
                updated_routes.iter_mut().for_each(|r| {
                    if &r.path == path && method != &MethodPattern::Options {
                        if let GatewayBinding::Worker(worker_binding) = &mut r.binding {
                            worker_binding.add_middleware(Middleware::Http(HttpMiddleware::Cors(preflight.clone())));
                        };
                    }
                });
            }
        }
    }

    Ok(updated_routes)
}

#[cfg(test)]
mod tests {
    use test_r::test;
    
    use golem_common::model::ComponentId;
    use golem_service_base::model::VersionedComponentId;
    use rib::Expr;
    use crate::gateway_api_definition::http::AllPathPatterns;
    use crate::gateway_binding::{ResponseMapping, WorkerBinding};
    use crate::gateway_middleware::{CorsPreflight, Middlewares};
    use super::*;

    #[test]
    fn test_update_routes_with_cors_middleware_applies_cors_to_worker_routes() {
        // Set up CORS preflight route with OPTIONS method
        let cors_preflight = CorsPreflight {
            allow_origin: "*".to_string(),
            allow_methods: "GET, POST".to_string(),
            allow_headers: "Content-Type".to_string(),
            expose_headers: Some("X-Custom-Header".to_string()),
            max_age: Some(86400),
        };

        let cors_route = Route {
            method: MethodPattern::Options,
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Static(StaticBinding::from_http_middleware(HttpMiddleware::cors(cors_preflight.clone()))),
        };

        // Worker route for the same path but with a different method (e.g., GET)
        let mut worker_binding = WorkerBinding {
            component_id: VersionedComponentId {
                component_id: ComponentId::new_v4(),
                version: 1,
            },
            worker_name: None,
            idempotency_key: None,
            response: ResponseMapping(Expr::literal("")),
            middleware: None,
        };

        let worker_route = Route {
            method: MethodPattern::Get,
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Worker(worker_binding.clone()),
        };

        let routes = vec![cors_route.clone(), worker_route.clone()];

        // Call the function
        let result = update_routes_with_cors_middleware(routes);

        // Assert that the function was successful
        assert!(result.is_ok());

        let updated_routes = result.unwrap();

        // Ensure CORS middleware is added to the worker route
        let updated_worker_route = updated_routes.iter().find(|r| r.method == MethodPattern::Get && r.path == worker_route.path).unwrap();
        if let GatewayBinding::Worker(updated_worker_binding) = &updated_worker_route.binding {
            assert!(updated_worker_binding.middleware.is_some());
            assert!(updated_worker_binding.middleware.as_ref().unwrap().0.contains(&Middleware::Http(HttpMiddleware::Cors(cors_preflight))));
        }
    }

    #[test]
    fn test_update_routes_with_cors_middleware_fails_for_non_options_cors_route() {
        // Set up an invalid CORS route (non-OPTIONS method)
        let cors_preflight = CorsPreflight {
            allow_origin: "*".to_string(),
            allow_methods: "GET, POST".to_string(),
            allow_headers: "Content-Type".to_string(),
            expose_headers: Some("X-Custom-Header".to_string()),
            max_age: Some(86400),
        };

        let invalid_cors_route = Route {
            method: MethodPattern::Get, // Should be OPTIONS
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Static(StaticBinding::from_http_middleware(HttpMiddleware::cors(cors_preflight.clone()))),
        };

        let worker_route = Route {
            method: MethodPattern::Get,
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Worker(WorkerBinding {
                component_id: VersionedComponentId {
                    component_id: ComponentId::new_v4(),
                    version: 1,
                },
                worker_name: None,
                idempotency_key: None,
                response: ResponseMapping(Expr::literal("")),
                middleware: None,
            }),
        };

        let routes = vec![invalid_cors_route, worker_route];

        // Call the function
        let result = update_routes_with_cors_middleware(routes);

        // Assert that an error is returned
        assert!(result.is_err());
        assert_eq!(result.err().unwrap(), "Invalid route for /test. CORS binding is only supported for OPTIONS method");
    }

    #[test]
    fn test_update_routes_with_cors_middleware_does_not_duplicate_cors() {
        // Set up CORS preflight route with OPTIONS method
        let cors_preflight = CorsPreflight {
            allow_origin: "*".to_string(),
            allow_methods: "GET, POST".to_string(),
            allow_headers: "Content-Type".to_string(),
            expose_headers: Some("X-Custom-Header".to_string()),
            max_age: Some(86400),
        };

        let cors_route = Route {
            method: MethodPattern::Options,
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Static(StaticBinding::from_http_middleware(HttpMiddleware::cors(cors_preflight.clone()))),
        };

        // Worker route for the same path with pre-existing CORS middleware
        let mut worker_binding = WorkerBinding {
            component_id: VersionedComponentId {
                component_id: ComponentId::new_v4(),
                version: 1,
            },
            worker_name: None,
            idempotency_key: None,
            response: ResponseMapping(Expr::literal("")),
            middleware: Some(Middlewares(vec![Middleware::Http(HttpMiddleware::Cors(cors_preflight.clone()))])),
        };

        let worker_route = Route {
            method: MethodPattern::Get,
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Worker(worker_binding.clone()),
        };

        let routes = vec![cors_route.clone(), worker_route.clone()];

        let result = update_routes_with_cors_middleware(routes);

        // Assert that the function was successful
        assert!(result.is_ok());

        let updated_routes = result.unwrap();

        // Ensure that no duplicate CORS middleware was added to the worker route
        let updated_worker_route = updated_routes.iter().find(|r| r.method == MethodPattern::Get && r.path == worker_route.path).unwrap();
        if let GatewayBinding::Worker(updated_worker_binding) = &updated_worker_route.binding {
            assert!(updated_worker_binding.middleware.is_some());
            assert_eq!(updated_worker_binding.middleware.as_ref().unwrap().0.len(), 1);
            assert_eq!(updated_worker_binding.middleware.as_ref().unwrap().0[0], Middleware::Http(HttpMiddleware::Cors(cors_preflight)));
        }
    }
}
