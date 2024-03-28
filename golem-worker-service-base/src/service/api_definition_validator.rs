use std::collections::HashMap;

use crate::api_definition::{ApiDefinition, MethodPattern, PathPattern, Route};
use async_trait::async_trait;
use golem_common::model::TemplateId;
use golem_service_base::model::{
    Export, ExportFunction, ExportInstance, Template, TemplateMetadata,
};
use serde::{Deserialize, Serialize};

pub trait ApiDefinitionValidatorService {
    fn validate(&self, api: &ApiDefinition, templates: &[Template]) -> Result<(), ValidationError>;
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
    pub fn from_route(route: Route, detail: String) -> Self {
        Self {
            method: route.method,
            path: route.path.to_string(),
            template: route.binding.template,
            detail,
        }
    }
}

#[derive(Clone)]
pub struct ApiDefinitionValidatorDefault {}
pub struct ValidationResult {
    pub result: HashMap<(TemplateId, String)>
}

struct FunctionDetails {
    template_id: TemplateId,
    function: ExportFunction,
}

impl ApiDefinitionValidatorService for ApiDefinitionValidatorDefault {
    fn validate(&self, api: &ApiDefinition, templates: &[Template]) -> Result<Vec<FunctionDetails>, ValidationError> {
        let templates: HashMap<&TemplateId, &TemplateMetadata> = templates
            .iter()
            .map(|template| {
                (
                    &template.versioned_template_id.template_id,
                    &template.metadata,
                )
            })
            .collect();


            let mut success_results = Vec::new();
            let mut route_errors = Vec::new();

            for route in &api.routes {
                match validate_route(route.clone(), &templates) {
                    Ok(result) => success_results.push(result),
                    Err(err) => route_errors.push(err),
                }
            }

            let unique_route_errors = unique_routes(api.routes.as_slice());

            let mut errors = route_errors;
            errors.extend(unique_route_errors);

        if errors.is_empty() {
            Ok(success_results)
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
    templates: &HashMap<&TemplateId, &TemplateMetadata>,
) -> Result<FunctionDetails, RouteValidationError> {
    let template_id = route.binding.template.clone();
    // We can unwrap here because we've already validated that all templates are present.
    let template = templates.get(&template_id).unwrap();

    let function_name = route.binding.function_name.clone();

    let function = find_function(function_name.as_str(), template).ok_or_else(|| {
        RouteValidationError::from_route(route, format!("Invalid function name: {function_name}"))
    })?;

    let function_details = FunctionDetails {
        template_id: template_id.clone(),
        function: function.clone(),
    };

    Ok(function_details)
}

fn find_function(name: &str, template: &TemplateMetadata) -> Option<ExportFunction> {
    template.exports.iter().find_map(|exp| match exp {
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
    fn validate(
        &self,
        _api: &ApiDefinition,
        _templates: &[Template],
    ) -> Result<(), ValidationError> {
        Ok(())
    }
}
