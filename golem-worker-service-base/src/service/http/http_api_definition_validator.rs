use std::collections::HashMap;

use golem_common::model::TemplateId;
use golem_service_base::model::{
    Export, ExportFunction, ExportInstance, Template, TemplateMetadata,
};
use serde::{Deserialize, Serialize};
use crate::http::http_api_definition::{HttpApiDefinition, MethodPattern, PathPattern, Route};
use crate::service::api_definition_validator::{ApiDefinitionValidatorService, ValidationError};


// Http Api Definition Validator
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
pub struct HttpApiDefinitionValidator {}

impl ApiDefinitionValidatorService<HttpApiDefinition, RouteValidationError> for HttpApiDefinitionValidator {
    fn validate(&self, api: &HttpApiDefinition, templates: &[Template]) -> Result<(), ValidationError<RouteValidationError>> {
        let templates: HashMap<&TemplateId, &TemplateMetadata> = templates
            .iter()
            .map(|template| {
                (
                    &template.versioned_template_id.template_id,
                    &template.metadata,
                )
            })
            .collect();

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
    templates: &HashMap<&TemplateId, &TemplateMetadata>,
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
