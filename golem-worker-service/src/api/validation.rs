use crate::api::definition::ApiDefinition;
use crate::api::definition::types::BindingType;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_ast::component::PrimitiveValueType;

pub fn validate_api_definition(api: &ApiDefinition) -> Result<(), String> {
    for route in &api.routes {
        match &route.binding {
            BindingType::Default { input_type, output_type, .. } => {
                let input_analysed = match input_type.as_str() {
                    "string" => AnalysedType::from(&PrimitiveValueType::Str),  // Changed from String
                    "i32" => AnalysedType::from(&PrimitiveValueType::S32),     // Changed from I32
                    "i64" => AnalysedType::from(&PrimitiveValueType::S64),
                    "f32" => AnalysedType::from(&PrimitiveValueType::F32),
                    "f64" => AnalysedType::from(&PrimitiveValueType::F64),
                    "bool" => AnalysedType::from(&PrimitiveValueType::Bool),
                    _ => return Err(format!("Unsupported input type: {}", input_type)),
                };
                
                let output_analysed = match output_type.as_str() {
                    "string" => AnalysedType::from(&PrimitiveValueType::Str),  // Changed from String
                    "i32" => AnalysedType::from(&PrimitiveValueType::S32),     // Changed from I32
                    "i64" => AnalysedType::from(&PrimitiveValueType::S64),
                    "f32" => AnalysedType::from(&PrimitiveValueType::F32),
                    "f64" => AnalysedType::from(&PrimitiveValueType::F64),
                    "bool" => AnalysedType::from(&PrimitiveValueType::Bool),
                    _ => return Err(format!("Unsupported output type: {}", output_type)),
                };
                
                validate_wit_binding_types(&input_analysed, &output_analysed)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_wit_binding_types(
    _input_type: &AnalysedType,
    _output_type: &AnalysedType,
) -> Result<(), String> {
    // Add your validation logic here
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
                        input_type: AnalysedType::String,
                        output_type: AnalysedType::Record(vec![
                            ("field".to_string(), AnalysedType::I32)
                        ]),
                        options: None,
                    },
                },
            ],
        };
        assert!(validate_api_definition(&api).is_ok());
    }
}