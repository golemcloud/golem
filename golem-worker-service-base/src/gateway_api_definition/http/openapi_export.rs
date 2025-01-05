use poem_openapi::{
    OpenApi,
    OpenApiService,
    registry::{MetaSchema, MetaSchemaRef},
    Object,
    ExternalDocumentObject,
    ServerObject,
    ContactObject,
    LicenseObject,
};
use serde::{Serialize, Deserialize};
use serde_json::Value as JsonValue;
use super::rib_converter::RibConverter;
use golem_wasm_rpc::ValueAndType;

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
pub struct OpenApiFormat {
    /// Whether to output JSON (true) or YAML (false)
    pub json: bool,
}

impl Default for OpenApiFormat {
    fn default() -> Self {
        Self { json: true }
    }
}

#[derive(Debug, Clone)]
pub struct OpenApiExporter;

impl OpenApiExporter {
    #[inline]
    pub fn export_openapi<T: OpenApi + Clone>(
        &self,
        api: T,
        format: &OpenApiFormat,
    ) -> String {
        // Get metadata from the API implementation
        let meta = T::meta();
        let api_meta = meta.first().expect("API must have metadata");
        let title = api_meta.paths.first()
            .and_then(|p| p.operations.first())
            .and_then(|op| op.tags.first())
            .map(|t| (*t).to_string())
            .unwrap_or_else(|| "API".to_string());
        
        let description = api_meta.paths.first()
            .and_then(|p| p.operations.first())
            .and_then(|op| op.description.as_deref())
            .unwrap_or("API Service");
        
        let service = OpenApiService::new(api, title, env!("CARGO_PKG_VERSION"))
            .description(description)
            .server(ServerObject::new("http://localhost:3000"))
            .server(ServerObject::new("https://localhost:3000"))
            .contact(ContactObject::new()
                .name("Golem Team")
                .email("team@golem.cloud")
                .url("https://golem.cloud"))
            .license(LicenseObject::new("MIT")
                .url("https://opensource.org/licenses/MIT"))
            .external_document(ExternalDocumentObject::new("https://github.com/bytecodealliance/wit-bindgen"))
            .external_document(ExternalDocumentObject::new("https://swagger.io/specification/"));
        
        if format.json {
            service.spec()
        } else {
            service.spec_yaml()
        }
    }

    /// Generate OpenAPI schema for a JSON value
    pub fn generate_schema(&self, value: &JsonValue) -> MetaSchema {
        match value {
            JsonValue::Null => {
                let mut schema = MetaSchema::new("null");
                schema.ty = "null";
                schema
            },
            JsonValue::Bool(_) => {
                let mut schema = MetaSchema::new("boolean");
                schema.ty = "boolean";
                schema
            },
            JsonValue::Number(n) => {
                let mut schema = MetaSchema::new(if n.is_i64() || n.is_u64() { "integer" } else { "number" });
                schema.ty = if n.is_i64() || n.is_u64() { "integer" } else { "number" };
                if n.is_i64() {
                    schema.format = Some("int64");
                } else if n.is_u64() {
                    schema.format = Some("uint64");
                } else {
                    schema.format = Some("double");
                }
                schema
            },
            JsonValue::String(_) => {
                let mut schema = MetaSchema::new("string");
                schema.ty = "string";
                schema
            },
            JsonValue::Array(arr) => {
                let mut schema = MetaSchema::new("array");
                schema.ty = "array";
                if let Some(first) = arr.first() {
                    let item_schema = self.generate_schema(first);
                    let item_ref = MetaSchemaRef::Inline(Box::new(item_schema));
                    schema.items = Some(Box::new(item_ref));
                }
                schema
            },
            JsonValue::Object(map) => {
                let mut schema = MetaSchema::new("object");
                schema.ty = "object";
                let mut properties = Vec::new();
                
                for (key, value) in map.iter() {
                    let prop_schema = self.generate_schema(value);
                    let prop_ref = MetaSchemaRef::Inline(Box::new(prop_schema));
                    let static_key: &'static str = Box::leak(key.clone().into_boxed_str());
                    properties.push((static_key, prop_ref));
                }
                schema.properties = properties;
                schema
            },
        }
    }

    /// Generate OpenAPI schema for a WIT-formatted value
    pub fn generate_schema_from_wit(&self, value: &ValueAndType) -> Result<MetaSchema, String> {
        let mut converter = RibConverter::new_openapi();
        let unwrapped_json = converter.convert_value(value)?;
        Ok(self.generate_schema(&unwrapped_json))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_schema_generation() {
        let exporter = OpenApiExporter;
        let value = json!({
            "string": "test",
            "number": 42,
            "boolean": true,
            "array": ["item1", "item2"],
            "object": {
                "nested": "value"
            }
        });

        let schema = exporter.generate_schema(&value);
        let schema_json = serde_json::to_string_pretty(&schema).unwrap();
        assert!(schema_json.contains("string"));
        assert!(schema_json.contains("integer"));
        assert!(schema_json.contains("boolean"));
        assert!(schema_json.contains("array"));
        assert!(schema_json.contains("object"));
    }

    #[test]
    fn test_openapi_format_default() {
        let format = OpenApiFormat::default();
        assert!(format.json);
    }
}
