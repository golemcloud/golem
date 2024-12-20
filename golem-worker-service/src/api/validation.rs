use crate::api::definition::ApiDefinition;
use crate::api::definition::types::*;
use crate::api::openapi::OpenAPIError;
use crate::api::types::OpenAPISpec;
use golem_wasm_ast::analysis::AnalysedType;
use openapiv3::OpenAPI;
use serde_json;

pub fn validate_api_definition(api: &ApiDefinition) -> Result<(), String> {
    for route in &api.routes {
        validate_binding(&route.binding)?;
    }
    Ok(())
}

pub fn validate_binding(binding: &BindingType) -> Result<(), String> {
    match binding {
        BindingType::Default { input_type, output_type, .. } => {
            // Add actual validation for the types
            validate_analysed_type(input_type)?;
            validate_analysed_type(output_type)?;
            Ok(())
        },
        _ => Ok(())
    }
}

fn validate_analysed_type(analysed_type: &AnalysedType) -> Result<(), String> {
    match analysed_type {
        AnalysedType::Str(_) |
        AnalysedType::S32(_) |
        AnalysedType::S64(_) |
        AnalysedType::F32(_) |
        AnalysedType::F64(_) |
        AnalysedType::Bool(_) |
        AnalysedType::U8(_) => Ok(()),
        AnalysedType::List(type_list) => validate_analysed_type(&type_list.inner),
        AnalysedType::Record(record) => {
            for field in record.fields.iter() {
                validate_analysed_type(&field.typ)?;
            }
            Ok(())
        },
        _ => Err(format!("Unsupported type: {:?}", analysed_type))
    }
}

pub fn validate_openapi(spec: &OpenAPISpec) -> Result<(), OpenAPIError> {
    let json_str = serde_json::to_string(spec)
        .map_err(|e| OpenAPIError::ValidationFailed(format!("Failed to serialize OpenAPI spec: {}", e)))?;

    let openapi: OpenAPI = serde_json::from_str(&json_str)
        .map_err(|e| OpenAPIError::ValidationFailed(format!("Failed to parse as OpenAPI: {}", e)))?;

    validate_openapi_structure(&openapi)?;
    Ok(())
}

fn validate_openapi_structure(spec: &OpenAPI) -> Result<(), OpenAPIError> {
    // Check required fields
    if spec.info.title.is_empty() {
        return Err(OpenAPIError::ValidationFailed(
            "OpenAPI spec must have a non-empty title".to_string()
        ));
    }

    if spec.info.version.is_empty() {
        return Err(OpenAPIError::ValidationFailed(
            "OpenAPI spec must have a non-empty version".to_string()
        ));
    }

    // Check paths
    if spec.paths.paths.is_empty() {
        return Err(OpenAPIError::ValidationFailed(
            "OpenAPI spec must have at least one path".to_string()
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::definition::types::{Route, HttpMethod};
    use std::collections::HashMap;
    
    #[test]
    fn test_valid_api_definition() {
        let api = ApiDefinition {
            routes: vec![
                Route {
                    path: "/test".to_string(),
                    method: HttpMethod::Get,
                    binding: BindingType::Default {
                        input_type: "string",
                        output_type: "record",
                        options: None,
                    },
                },
            ],
        };
        assert!(validate_api_definition(&api).is_ok());
    }

    #[test]
    fn test_validate_openapi_valid_v3() {
        let spec = OpenAPISpec {
            openapi: "3.0.0".to_string(),
            info: crate::api::types::Info {
                title: "Test API".to_string(),
                version: "1.0.0".to_string(),
                description: Some("Test Description".to_string()),
            },
            paths: HashMap::new(),
            components: None,
            security: None,
        };
        
        assert!(validate_openapi(&spec).is_ok());
    }

    #[test]
    fn test_validate_openapi_invalid() {
        let spec = OpenAPISpec {
            openapi: "3.0.0".to_string(),
            info: crate::api::types::Info {
                title: "".to_string(), // Invalid empty title
                version: "1.0.0".to_string(),
                description: None,
            },
            paths: HashMap::new(),
            components: None,
            security: None,
        };
        
        assert!(validate_openapi(&spec).is_err());
    }
}