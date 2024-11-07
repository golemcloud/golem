use std::collections::HashMap;
use rib::{Expr, GetLiteralValue, RibInput};

pub struct CorsPreflight {
    pub allow_origin: String,
    pub allow_methods: String,
    pub allow_headers: String,
    pub expose_headers: Option<String>,
    pub max_age: Option<u64>
}

impl CorsPreflight {
    async fn from_cors_preflight_expr(expr: &CorsPreflightExpr) -> Result<CorsPreflight, String> {
        // Compile and evaluate the expression
        let expr = rib::compile(&expr.0, &vec![])
            .map_err(|_| "Compilation failed")?;
        let evaluated = rib::interpret_pure(&expr.byte_code, &RibInput::default())
            .await
            .map_err(|_| "Evaluation failed")?;

        // Ensure the result is a record and map fields to a hash map
        let record = evaluated.get_record().ok_or("Invalid pre-flight CORS response mapping")?;
        let mut fields: HashMap<String, String> = HashMap::new();

        // Expected field names
        let valid_keys = [
            "Access-Control-Allow-Origin",
            "Access-Control-Allow-Methods",
            "Access-Control-Allow-Headers",
            "Access-Control-Expose-Headers",
            "Access-Control-Max-Age",
        ];

        let mut cors = CorsPreflight::default();

        for name_value in record {
            let key = name_value.name.clone();
            let value = name_value.value.ok_or("Internal error. Unable to fetch value in the type_annotated_value record")?;

            // Validate key names and insert valid entries into the map
            if valid_keys.contains(&key.as_str()) {
                let value = value.type_annotated_value.ok_or("Internal error. Unable to fetch value in type_annotated_value")?;
                let literal = value.get_literal().ok_or("Invalud value for key {} in cors preflight response")?.as_string();

                fields.insert(key, literal);
            } else {
                return Err("Invalid CORS header in response mapping".to_string());
            }
        }

        fields.get("Access-Control-Allow-Origin").map(|x| cors.set_allow_origin(x.as_str()));
        fields.get("Access-Control-Allow-Methods").map(|x| cors.set_allow_methods(x.as_str()));
        fields.get("Access-Control-Allow-Headers").map(|x| cors.set_allow_methods(x.as_str()));
        fields.get("Access-Control-Expose-Headers").map(|x| cors.set_expose_headers(x.as_str()));
        fields.get("Access-Control-Max-Age").map(|x| x.parse::<u64>().map_err(|err| "Invalid value for max age").map(|x| cors.set_max_age(x))).transpose()?;

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

