use crate::api::ApiDefinition;
use crate::api::BindingType;

pub fn validate_api_definition(api: &ApiDefinition) -> Result<(), String> {
    for route in &api.routes {
        match &route.binding {
            BindingType::Default { input_type, output_type, .. } => {
                validate_wit_type(input_type)?;
                validate_wit_type(output_type)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_wit_type(wit_type: &str) -> Result<(), String> {
    match wit_type {
        "string" | "i32" | "i64" | "f32" | "f64" | "bool" | "binary" => Ok(()),
        t if t.starts_with("list<") => {
            let inner_type = &t[5..t.len()-1];
            validate_wit_type(inner_type)
        },
        t if t.starts_with("record{") => {
            if let Some(fields_str) = t.strip_prefix("record{").and_then(|s| s.strip_suffix("}")) {
                for field in fields_str.split(',').map(str::trim) {
                    if let Some((_, type_str)) = field.split_once(':') {
                        validate_wit_type(type_str.trim())?;
                    }
                }
            }
            Ok(())
        },
        t => {
            if t.contains("{") || t.contains("<") || t.contains(">") {
                Err("Invalid character found in type".to_string())
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_valid_types() {
        let simple_types = vec!["string", "i32", "i64", "f32", "f64", "bool", "binary"];
        for t in simple_types {
            assert!(validate_wit_type(t).is_ok());
        }
    }

    #[test]
    fn test_valid_list() {
        assert!(validate_wit_type("list<string>").is_ok());
        assert!(validate_wit_type("list<i32>").is_ok());
        assert!(validate_wit_type("list<list<string>>").is_ok());
    }

    #[test]
    fn test_valid_record() {
        assert!(validate_wit_type("record{field1:string,field2:i32}").is_ok());
        assert!(validate_wit_type("record{nested:record{field:string}}").is_ok());
    }

    #[test]
    fn test_invalid_types() {
        assert!(validate_wit_type("invalid<type>").is_err());
        assert!(validate_wit_type("record{invalid}").is_err());
        assert!(validate_wit_type("list<>").is_err());
    }

    #[test]
    fn test_valid_api_definition() {
        let api = ApiDefinition {
            routes: vec![
                Route {
                    path: "/test".to_string(),
                    method: HttpMethod::Get,
                    binding: BindingType::Default {
                        input_type: "string".to_string(),
                        output_type: "record{field:i32}".to_string(),
                        options: None,
                    },
                },
            ],
        };
        assert!(validate_api_definition(&api).is_ok());
    }
}