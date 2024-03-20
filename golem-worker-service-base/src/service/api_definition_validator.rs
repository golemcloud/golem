use std::{collections::HashMap, sync::Arc};

use crate::{
    api_definition::{ApiDefinition, MethodPattern, PathPattern, Route},
    service::template_service::TemplateService,
};
use async_trait::async_trait;
use futures::future::join_all;
use golem_common::model::TemplateId;
use golem_service_base::model::{Export, ExportFunction, ExportInstance, Template};
use serde::{Deserialize, Serialize};

#[async_trait]
pub trait ApiDefinitionValidatorService {
    async fn validate(&self, api: &ApiDefinition) -> Result<(), ValidationError>;
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, thiserror::Error)]
// TODO: Fix this display impl.
#[error("Validation error: {errors:?}")]
pub struct ValidationError {
    pub errors: Vec<RouteValidationError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteValidationError {
    pub method: MethodPattern,
    pub path: String,
    pub template: TemplateId,
    pub detail: String,
}

impl RouteValidationError {
    fn from_route(route: Route, detail: String) -> Self {
        Self {
            method: route.method,
            path: route.path.to_string(),
            template: route.binding.template,
            detail,
        }
    }
}

#[derive(Clone)]
pub struct ApiDefinitionValidatorDefault {
    template_service: Arc<dyn TemplateService + Send + Sync>,
}

impl ApiDefinitionValidatorDefault {
    pub fn new(template_service: Arc<dyn TemplateService + Send + Sync>) -> Self {
        Self { template_service }
    }
}

#[async_trait]
impl ApiDefinitionValidatorService for ApiDefinitionValidatorDefault {
    async fn validate(&self, api: &ApiDefinition) -> Result<(), ValidationError> {
        let get_templates = api
            .routes
            .iter()
            .cloned()
            .map(|route| (route.binding.template.clone(), route))
            .collect::<HashMap<_, _>>()
            .into_values()
            .map(|route| async move {
                let id = &route.binding.template;
                self.template_service.get_latest(id).await.map_err(|e| {
                    tracing::error!("Error getting latest template: {:?}", e);
                    // TODO: Better error message.
                    RouteValidationError::from_route(route, "Error getting latest template".into())
                })
            })
            .collect::<Vec<_>>();

        let templates: HashMap<TemplateId, Template> = {
            let results = join_all(get_templates).await;
            let (successes, errors) = results
                .into_iter()
                .partition::<Vec<_>, _>(|result| result.is_ok());
            // Ensure that all templates were retrieved.
            if !errors.is_empty() {
                let errors = errors.into_iter().map(|r| r.unwrap_err()).collect();
                return Err(ValidationError { errors });
            }

            successes
                .into_iter()
                .map(|r| r.unwrap())
                .map(|template| (template.versioned_template_id.template_id.clone(), template))
                .collect()
        };

        let errors = {
            let route_validation = api
                .routes
                .iter()
                .cloned()
                .flat_map(|route| validate_route(route, &templates).err())
                .collect::<Vec<_>>();

            let unique_route_errors = unique_routes(api.routes.as_slice());

            let mut errors = route_validation;
            errors.extend(unique_route_errors);

            errors
        };

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError { errors })
        }
    }
}

fn unique_routes(routes: &[Route]) -> Vec<RouteValidationError> {
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub struct RouteKey<'a> {
        pub method: &'a MethodPattern,
        pub path: &'a PathPattern,
    }

    let mut seen = std::collections::HashSet::new();

    routes
        .iter()
        .flat_map(|route| {
            let route_key = RouteKey {
                method: &route.method,
                path: &route.path,
            };
            if seen.contains(&route_key) {
                Some(RouteValidationError {
                    method: route_key.method.clone(),
                    path: route_key.path.to_string(),
                    template: route.binding.template.clone(),
                    detail: "Duplicate route".to_string(),
                })
            } else {
                seen.insert(route_key);
                None
            }
        })
        .collect()
}

fn validate_route(
    route: Route,
    templates: &HashMap<TemplateId, Template>,
) -> Result<(), RouteValidationError> {
    let template_id = route.binding.template.clone();
    // We can unwrap here because we've already validated that all templates are present.
    let template = templates.get(&template_id).unwrap();

    let function_name = route.binding.function_name.clone();

    // TODO: Validate function params.
    let _function = find_function(function_name.as_str(), template).ok_or_else(|| {
        RouteValidationError::from_route(route, format!("Invalid function name: {function_name}"))
    })?;

    Ok(())
}

fn find_function(name: &str, template: &Template) -> Option<ExportFunction> {
    template.metadata.exports.iter().find_map(|exp| match exp {
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

#[derive(Copy, Clone)]
pub struct ApiDefinitionValidatorNoop {}

#[async_trait]
impl ApiDefinitionValidatorService for ApiDefinitionValidatorNoop {
    async fn validate(&self, _api: &ApiDefinition) -> Result<(), ValidationError> {
        Ok(())
    }
}
