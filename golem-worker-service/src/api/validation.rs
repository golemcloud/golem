use crate::api::definition::ApiDefinition;
use crate::api::definition::types::BindingType;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_ast::analysis::{TypeStr, TypeS32, TypeS64, TypeF32, TypeF64, TypeBool};

pub fn validate_api_definition(api: &ApiDefinition) -> Result<(), String> {
    for route in &api.routes {
        match &route.binding {
            BindingType::Default { input_type, output_type, .. } => {
                // Convert string types to AnalysedType
                let input_analysed = string_to_analysed_type(input_type)?;
                let output_analysed = string_to_analysed_type(output_type)?;
                
                validate_wit_type(&input_analysed)?;
                validate_wit_type(&output_analysed)?;
                validate_wit_binding_types(&input_analysed, &output_analysed)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn string_to_analysed_type(type_str: &str) -> Result<AnalysedType, String> {
    match type_str {
        "string" => Ok(AnalysedType::Str(TypeStr)),
        "i32" => Ok(AnalysedType::S32(TypeS32)),
        "i64" => Ok(AnalysedType::S64(TypeS64)),
        "f32" => Ok(AnalysedType::F32(TypeF32)),
        "f64" => Ok(AnalysedType::F64(TypeF64)),
        "bool" => Ok(AnalysedType::Bool(TypeBool)),
        _ => Err(format!("Unsupported type: {}", type_str))
    }
}

fn validate_wit_type(wit_type: &AnalysedType) -> Result<(), String> {
    match wit_type {
        AnalysedType::Str(_) |
        AnalysedType::S32(_) |
        AnalysedType::S64(_) |
        AnalysedType::F32(_) |
        AnalysedType::F64(_) |
        AnalysedType::Bool(_) => Ok(()),
        _ => Err(format!("Unsupported WIT type: {:?}", wit_type))
    }
}

fn validate_wit_binding_types(
    _input_type: &AnalysedType,
    _output_type: &AnalysedType,
) -> Result<(), String> {
    Ok(())
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