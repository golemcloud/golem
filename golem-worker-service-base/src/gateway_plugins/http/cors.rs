use std::collections::HashMap;
use golem_wasm_ast::analysis::analysed_type::{record, str, u64 as tu64};
use golem_wasm_ast::analysis::{AnalysedType, NameTypePair};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::type_annotated_value_from_str;
use rib::{Expr, GetLiteralValue, RibInput};

#[derive(Debug, Clone, PartialEq)]
pub struct CorsPreflight {
    pub allow_origin: String,
    pub allow_methods: String,
    pub allow_headers: String,
    pub expose_headers: Option<String>,
    pub max_age: Option<u64>
}

impl CorsPreflight {
    // probably not a great idea yet.
    pub fn as_type_annotated_value(&self) -> Result<TypeAnnotatedValue, String> {
        type_annotated_value_from_str(&record(
            vec![
                NameTypePair {
                    name: "Access-Control-Allow-Origin".to_string(),
                    typ: str(),
                },
                NameTypePair {
                    name: "Access-Control-Allow-Methods".to_string(),
                    typ: str(),
                },
                NameTypePair {
                    name: "Access-Control-Allow-Headers".to_string(),
                    typ: str(),
                },
                NameTypePair {
                    name: "Access-Control-Expose-Headers".to_string(),
                    typ: str(),
                },
                NameTypePair {
                    name: "Access-Control-Max-Age".to_string(),
                    typ: tu64(),
                }
            ]
        ),format!(r#"
           {{
            Access-Control-Allow-Origin:{}
            Access-Control-Allow-Methods:{}
            Access-Control-Allow-Headers:{}
            Access-Control-Expose-Headers:{}
            Access-Control-Max-Age:{}
           }}

        "#, self.allow_origin, self.allow_methods, self.allow_headers, self.expose_headers, self.max_age).as_str())
    }
    async fn from_cors_preflight_expr(expr: &CorsPreflightExpr) -> Result<CorsPreflight, String> {
        // Compile and evaluate the expression
        let compiled_expr = rib::compile(&expr.0, &[])
            .map_err(|_| "Compilation failed")?;
        let evaluated = rib::interpret_pure(&compiled_expr.byte_code, &RibInput::default())
            .await
            .map_err(|_| "Evaluation failed")?;

        // Ensure the result is a record
        let record = evaluated.get_record().ok_or("Invalid pre-flight CORS response mapping")?;

        // Initialize with default values for CorsPreflight
        let mut cors = CorsPreflight::default();
        let valid_keys = [
            "Access-Control-Allow-Origin",
            "Access-Control-Allow-Methods",
            "Access-Control-Allow-Headers",
            "Access-Control-Expose-Headers",
            "Access-Control-Max-Age",
        ];

        for name_value in record {
            let key = &name_value.name;
            let value = name_value.value
                .as_ref()
                .ok_or("Missing value in type_annotated_value record")?
                .type_annotated_value
                .as_ref()
                .ok_or("Unable to fetch value in type_annotated_value")?
                .get_literal()
                .ok_or("Invalid value for key in CORS preflight response")?
                .as_string();

            if valid_keys.contains(&key.as_str()) {
                match key.as_str() {
                    "Access-Control-Allow-Origin" => cors.set_allow_origin(&value),
                    "Access-Control-Allow-Methods" => cors.set_allow_methods(&value),
                    "Access-Control-Allow-Headers" => cors.set_allow_headers(&value),
                    "Access-Control-Expose-Headers" => cors.set_expose_headers(&value),
                    "Access-Control-Max-Age" => {
                        let max_age = value.parse::<u64>().map_err(|_| "Invalid value for max age")?;
                        cors.set_max_age(max_age);
                    },
                    _ => {}
                }
            } else {
                return Err(format!("Invalid CORS header in response mapping: {}", key));
            }
        }

        Ok(cors)
    }
    fn default() -> CorsPreflight {
        CorsPreflight {
            allow_origin: "*".to_string(),
            allow_methods: "GET, POST, PUT, DELETE, OPTIONS".to_string(),
            allow_headers: "Content-Type, Authorization".to_string(),
            expose_headers: None,
            max_age: None,
        }
    }

    fn set_allow_origin(&mut self, allow_origin: &str) {
        self.allow_origin = allow_origin.to_string();
    }

    fn set_allow_methods(&mut self, allow_methods: &str) {
        self.allow_methods = allow_methods.to_string();
    }

    fn set_allow_headers(&mut self, allow_headers: &str) {
        self.allow_headers = allow_headers.to_string();
    }

    fn set_expose_headers(&mut self, expose_headers: &str) {
        self.expose_headers = Some(expose_headers.to_string());
    }

    fn set_max_age(&mut self, max_age: u64)  {
        self.max_age = Some(max_age);
    }

}

pub struct CorsPreflightExpr(pub Expr);

