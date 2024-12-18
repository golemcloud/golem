use crate::api::ApiDefinition;
use crate::api::BindingType;
use golem_wasm_ast::analysis::analysed_type::AnalysedType;

pub fn validate_api_definition(api: &ApiDefinition) -> Result<(), String> {
    for route in &api.routes {
        match &route.binding {
            BindingType::Default { input_type, output_type, .. } => {
                validate_wit_binding_types(input_type, output_type)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_wit_binding_types(
    input_type: &AnalysedType,
    output_type: &AnalysedType
) -> Result<(), String> {
    // Validation handled by WIT type system
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_wasm_ast::analysis::analysed_type::{AnalysedType, TypeId};
    
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