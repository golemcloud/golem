use poem_openapi::Object;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use golem_common::model::ComponentId;
use golem_service_base::model::{
    Component, ComponentMetadata, Export, ExportFunction, ExportInstance,
};

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
            component: route.binding.component,
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
        components: &[Component],
    ) -> Result<(), ValidationErrors<RouteValidationError>> {
        let components: HashMap<&ComponentId, &ComponentMetadata> = components
            .iter()
            .map(|component| {
                (
                    &component.versioned_component_id.component_id,
                    &component.metadata,
                )
            })
            .collect();

        let errors = {
            let route_validation = api
                .routes
                .iter()
                .cloned()
                .flat_map(|route| validate_route(route, &components).err())
                .collect::<Vec<_>>();

            let unique_route_errors = unique_routes(api.routes.as_slice());

            let mut errors = route_validation;
            errors.extend(unique_route_errors);

            errors
        };

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
                component: route.binding.component.clone(),
                detail,
            });
        }
    }

    errors
}

fn validate_route(
    route: Route,
    components: &HashMap<&ComponentId, &ComponentMetadata>,
) -> Result<(), RouteValidationError> {
    let component_id = route.binding.component.clone();
    // We can unwrap here because we've already validated that all components are present.
    let component = components.get(&component_id).unwrap();

    let function_name = route.binding.function_name.clone();

    // TODO: Validate function params.
    let _function = find_function(function_name.as_str(), component).ok_or_else(|| {
        RouteValidationError::from_route(route, format!("Invalid function name: {function_name}"))
    })?;

    Ok(())
}

fn find_function(name: &str, component: &ComponentMetadata) -> Option<ExportFunction> {
    component.exports.iter().find_map(|exp| match exp {
        Export::Instance(ExportInstance {
            name: instance_name,
            functions,
        }) => functions.iter().find_map(|f| {
            let full_name = format!("{}/{}", instance_name, f.name);
            if full_name == name {
                Some(f.clone())
            } else {
                None
            }
        }),
        Export::Function(f) => {
            if f.name == name {
                Some(f.clone())
            } else {
                None
            }
        }
    })
}

#[test]
fn test_unique_routes() {
    fn make_route(method: MethodPattern, path: &str) -> Route {
        Route {
            method,
            path: crate::api_definition::http::AllPathPatterns::parse(path).unwrap(),
            binding: crate::worker_binding::GolemWorkerBinding {
                component: ComponentId::new_v4(),
                worker_id: crate::expression::Expr::Request(),
                function_name: "test".into(),
                function_params: vec![],
                response: None,
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
