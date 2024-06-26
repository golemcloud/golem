use poem_openapi::Object;

use serde::{Deserialize, Serialize};

use golem_common::model::ComponentId;
use golem_service_base::model::Component;

use crate::api_definition::http::{HttpApiDefinition, MethodPattern, Route};

use crate::http::router::{Router, RouterPattern};
use crate::service::api_definition_validator::{ApiDefinitionValidatorService, ValidationErrors};

// Http Api Definition Validator
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
pub struct RouteValidationError {
    pub method: MethodPattern,
    pub path: String,
    pub component: ComponentId,
    pub detail: String,
}

impl RouteValidationError {
    pub fn from_route(route: Route, detail: String) -> Self {
        Self {
            method: route.method,
            path: route.path.to_string(),
            component: route.binding.component_id,
            detail,
        }
    }
}

#[derive(Clone)]
pub struct HttpApiDefinitionValidator {}

impl ApiDefinitionValidatorService<HttpApiDefinition, RouteValidationError>
    for HttpApiDefinitionValidator
{
    fn validate(
        &self,
        api: &HttpApiDefinition,
        _components: &[Component],
    ) -> Result<(), ValidationErrors<RouteValidationError>> {
        let errors = unique_routes(api.routes.as_slice());

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationErrors { errors })
        }
    }
}

fn unique_routes(routes: &[Route]) -> Vec<RouteValidationError> {
    let mut router = Router::<&Route>::new();

    let mut errors = vec![];

    for route in routes {
        let method: hyper::Method = route.method.clone().into();
        let path: Vec<RouterPattern> = route
            .path
            .path_patterns
            .clone()
            .into_iter()
            .map(|p| p.into())
            .collect();

        if !router.add_route(method.clone(), path.clone(), route) {
            let current_route = router.get_route(&method, &path).unwrap();

            let detail = format!("Duplicate route with path: {}", current_route.path);

            errors.push(RouteValidationError {
                method: route.method.clone(),
                path: route.path.to_string(),
                component: route.binding.component_id.clone(),
                detail,
            });
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use crate::api_definition::http::{MethodPattern, Route};
    use crate::service::http::http_api_definition_validator::unique_routes;
    use crate::worker_binding::ResponseMapping;
    use golem_common::model::ComponentId;
    use rib::Expr;

    #[test]
    fn test_unique_routes() {
        fn make_route(method: MethodPattern, path: &str) -> Route {
            Route {
                method,
                path: crate::api_definition::http::AllPathPatterns::parse(path).unwrap(),
                binding: crate::worker_binding::GolemWorkerBinding {
                    component_id: ComponentId::new_v4(),
                    worker_name: Expr::Identifier("request".to_string()),
                    idempotency_key: None,
                    response: ResponseMapping(Expr::Literal("sample".to_string())),
                },
            }
        }

        let paths = &[
            "/users/{id}/posts/{post_id}",
            "/users/{id}/posts/{post_id}/comments/{comment_id}",
            "/users/{id}/posts/{post_id}/comments/{comment_id}/replies/{reply_id}",
        ];

        let get_paths: Vec<Route> = paths
            .iter()
            .map(|s| make_route(MethodPattern::Get, s))
            .collect();

        let post_paths: Vec<Route> = paths
            .iter()
            .map(|s| make_route(MethodPattern::Post, s))
            .collect();

        let all_paths = [&get_paths[..], &post_paths[..]].concat();

        let errors = unique_routes(&all_paths);
        assert!(errors.is_empty());

        let conflict_route = make_route(MethodPattern::Get, "/users/{a}/posts/{b}");

        let with_conflict = [&get_paths[..], &[conflict_route]].concat();

        let errors = unique_routes(&with_conflict);
        assert!(errors.len() == 1);
        assert!(errors[0].detail.contains(paths[0]), "Received: {errors:?}");
    }
}
