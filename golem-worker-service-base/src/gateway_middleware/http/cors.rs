use poem_openapi::Object;
use rib::{Expr, GetLiteralValue, RibInput};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CorsPreflight {
    pub allow_origin: String,
    pub allow_methods: String,
    pub allow_headers: String,
    pub expose_headers: Option<String>,
    pub max_age: Option<u64>,
}

impl CorsPreflight {
    pub fn from_parameters(
        allow_origin: Option<String>,
        allow_methods: Option<String>,
        allow_headers: Option<String>,
        expose_headers: Option<String>,
        max_age: Option<u64>,
    ) -> CorsPreflight {
        let mut cors_preflight = CorsPreflight::default();

        if let Some(allow_origin) = allow_origin {
            cors_preflight.set_allow_origin(allow_origin.as_str())
        }

        if let Some(allow_methods) = allow_methods {
            cors_preflight.set_allow_methods(allow_methods.as_str())
        }

        if let Some(allow_headers) = allow_headers {
            cors_preflight.set_allow_headers(allow_headers.as_str())
        }
        if let Some(expose_headers) = expose_headers {
            cors_preflight.set_expose_headers(expose_headers.as_str())
        }

        if let Some(max_age) = max_age {
            cors_preflight.set_max_age(max_age)
        }

        cors_preflight
    }

    // https://github.com/afsalthaj/golem-timeline/issues/70
    async fn from_cors_preflight_expr(expr: &CorsPreflightExpr) -> Result<CorsPreflight, String> {
        // Compile and evaluate the expression
        let compiled_expr = rib::compile(&expr.0, &vec![]).map_err(|_| "Compilation failed")?;
        let evaluated = rib::interpret_pure(&compiled_expr.byte_code, &RibInput::default())
            .await
            .map_err(|_| "Evaluation failed")?;

        // Ensure the result is a record
        let record = evaluated
            .get_record()
            .ok_or("Invalid pre-flight CORS response mapping")?;

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
            let value = name_value
                .value
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
                        let max_age = value
                            .parse::<u64>()
                            .map_err(|_| "Invalid value for max age")?;
                        cors.set_max_age(max_age);
                    }
                    _ => {}
                }
            } else {
                return Err(format!("Invalid CORS header in response mapping: {}", key));
            }
        }

        Ok(cors)
    }
    pub fn default() -> CorsPreflight {
        CorsPreflight {
            allow_origin: "*".to_string(),
            allow_methods: "GET, POST, PUT, DELETE, OPTIONS".to_string(),
            allow_headers: "Content-Type, Authorization".to_string(),
            expose_headers: None,
            max_age: None,
        }
    }

    pub fn set_allow_origin(&mut self, allow_origin: &str) {
        self.allow_origin = allow_origin.to_string();
    }

    pub fn set_allow_methods(&mut self, allow_methods: &str) {
        self.allow_methods = allow_methods.to_string();
    }

    pub fn set_allow_headers(&mut self, allow_headers: &str) {
        self.allow_headers = allow_headers.to_string();
    }

    pub fn set_expose_headers(&mut self, expose_headers: &str) {
        self.expose_headers = Some(expose_headers.to_string());
    }

    pub fn set_max_age(&mut self, max_age: u64) {
        self.max_age = Some(max_age);
    }
}

pub struct CorsPreflightExpr(pub Expr);
