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

use crate::gateway_api_definition::http::HttpApiDefinition;
use crate::gateway_api_definition_transformer::ApiDefTransformationError;

pub fn cors_transform(
    api_definition: &mut HttpApiDefinition,
) -> Result<(), ApiDefTransformationError> {
    internal::update_routes_with_cors_middleware(&mut api_definition.routes)
}
mod internal {
    use crate::gateway_api_definition::http::{AllPathPatterns, MethodPattern, Route};
    use crate::gateway_api_definition_transformer::ApiDefTransformationError;
    use crate::gateway_middleware::{HttpCors, HttpMiddleware, HttpMiddlewares};
    use poem_openapi::types::Type;

    pub(crate) fn update_routes_with_cors_middleware(
        routes: &mut [Route],
    ) -> Result<(), ApiDefTransformationError> {
        // Collect paths that need CORS middleware to apply
        let mut paths_to_apply_cors = Vec::new();

        for route in routes.iter_mut() {
            // If Route has a preflight binding,
            // enforce OPTIONS method
            // and mark this reource for CORS middleware application
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
        cors: HttpCors,
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
        cors: HttpCors,
    ) -> Result<(), ApiDefTransformationError> {
        let middlewares = &mut route.middlewares;

        match middlewares {
            Some(middlewares) => {
                if middlewares.get_cors_middleware().is_empty() {
                    middlewares.add_cors(cors);
                }
            }
            None => *middlewares = Some(HttpMiddlewares(vec![HttpMiddleware::cors(cors)])),
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::gateway_api_definition::http::{
        AllPathPatterns, HttpApiDefinition, MethodPattern, Route,
    };
    use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
    use crate::gateway_api_definition_transformer::cors_transformer::cors_transform;
    use crate::gateway_api_definition_transformer::ApiDefTransformationError;
    use crate::gateway_binding::{GatewayBinding, ResponseMapping, StaticBinding, WorkerBinding};
    use crate::gateway_middleware::{HttpCors, HttpMiddleware, HttpMiddlewares};
    use golem_common::model::component::VersionedComponentId;
    use golem_common::model::ComponentId;
    use rib::Expr;
    use test_r::test;

    fn get_cors_preflight_route() -> Route {
        Route {
            method: MethodPattern::Options,
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::static_binding(StaticBinding::from_http_cors(cors())),
            middlewares: None,
        }
    }

    fn get_invalid_cors_preflight_route() -> Route {
        Route {
            method: MethodPattern::Get, // Should be OPTIONS
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::static_binding(StaticBinding::from_http_cors(cors())),
            middlewares: None,
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
            invocation_context: None,
        };

        Route {
            method: MethodPattern::Get,
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Default(worker_binding.clone()),
            middlewares: None,
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
            invocation_context: None,
        };

        Route {
            method: MethodPattern::Get,
            path: AllPathPatterns::parse("/test").unwrap(),
            binding: GatewayBinding::Default(worker_binding.clone()),
            middlewares: Some(HttpMiddlewares(vec![HttpMiddleware::Cors(cors())])),
        }
    }

    fn cors() -> HttpCors {
        HttpCors::from_parameters(
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

        cors_transform(&mut api_definition).unwrap();

        let updated_route_with_worker_binding = api_definition
            .routes
            .iter()
            .find(|r| r.method == MethodPattern::Get && r.path == route_with_worker_binding.path)
            .unwrap();

        let new_worker_binding_middlewares = updated_route_with_worker_binding
            .middlewares
            .clone()
            .unwrap();

        assert!(new_worker_binding_middlewares
            .0
            .contains(&HttpMiddleware::cors(cors())));
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

        let result = cors_transform(&mut api_definition).map_err(|x| match x {
            ApiDefTransformationError::InvalidRoute { detail, .. } => detail,
            ApiDefTransformationError::Custom(custom) => custom.to_string(),
        });

        assert_eq!(result.err(), Some("Invalid binding for resource '/test' with method 'GET'. CORS binding is only supported for the OPTIONS method.".to_string()));
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

        let expected = api_definition.clone();

        let _ = cors_transform(&mut api_definition);

        assert_eq!(api_definition, expected);
    }
}
