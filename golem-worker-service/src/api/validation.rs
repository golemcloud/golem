use crate::api::definition::ApiDefinition;
use crate::api::definition::types::*;
use golem_wasm_ast::analysis::AnalysedType;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::definition::types::{Route, HttpMethod};
    
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
}