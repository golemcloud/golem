use poem_openapi::Object;
use std::fmt::{Display, Formatter};

use golem_common::SafeDisplay;
use golem_service_base::model::{Component, VersionedComponentId};
use serde::{Deserialize, Serialize};

use crate::gateway_api_definition::http::{HttpApiDefinition, MethodPattern, Route};

use crate::gateway_execution::router::{Router, RouterPattern};
use crate::service::gateway::api_definition_transformer::ApiDefTransformationError;
use crate::service::gateway::api_definition_validator::{
    ApiDefinitionValidatorService, ValidationErrors,
};

// Http Api Definition Validator
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
pub struct RouteValidationError {
    pub method: MethodPattern,
    pub path: String,
    pub component: Option<VersionedComponentId>,
    pub detail: String,
}

impl From<ApiDefTransformationError> for RouteValidationError {
    fn from(value: ApiDefTransformationError) -> Self {
        RouteValidationError {
            method: value.method,
            path: value.path,
            component: None,
            detail: value.detail,
        }
    }
}

impl Display for RouteValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "RouteValidationError: method: {}, path: {}, detail: {}",
            self.method, self.path, self.detail
        )?;

        if let Some(ref component) = self.component {
            write!(f, ", component: {}", component)?;
        }

        Ok(())
    }
}

impl SafeDisplay for RouteValidationError {
    fn to_safe_string(&self) -> String {
        self.to_string()
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
                component: route.binding.get_worker_binding().map(|w| w.component_id),
                detail,
            });
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::gateway_api_definition::http::{MethodPattern, Route};
    use crate::gateway_binding::{GatewayBinding, ResponseMapping};
    use crate::service::gateway::http_api_definition_validator::unique_routes;
    use golem_common::model::ComponentId;
    use golem_common::model::GatewayBindingType;
    use golem_service_base::model::VersionedComponentId;
    use rib::Expr;

    #[test]
    fn test_unique_routes() {
        fn make_route(method: MethodPattern, path: &str) -> Route {
            Route {
                method,
                path: crate::gateway_api_definition::http::AllPathPatterns::parse(path).unwrap(),
                binding: GatewayBinding::Worker(crate::gateway_binding::WorkerBinding {
                    component_id: VersionedComponentId {
                        component_id: ComponentId::new_v4(),
                        version: 1,
                    },
                    worker_name: Some(Expr::identifier("request")),
                    idempotency_key: None,
                    response_mapping: ResponseMapping(Expr::literal("sample")),
                    middleware: None,
                    worker_binding_type: GatewayBindingType::Default,
                }),
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
