use anyhow::Result;

#[cfg(test)]
mod openapi_export_integration_tests {
    use super::*;
    use golem_worker_service_base::gateway_api_definition::http::openapi_export::{OpenApiExporter, OpenApiFormat};
    use poem_openapi::{
        Object, OpenApi, ApiResponse,
        payload::Json,
    };

    // Define test API types
    #[derive(Object)]
    struct ComplexRequest {
        id: u32,
        name: String,
        flags: Vec<bool>,
    }

    #[derive(ApiResponse)]
    enum ComplexResponse {
        #[oai(status = 200)]
        Success(Json<ComplexRequest>),
    }

    #[derive(Clone)]
    struct ComplexApi;

    #[OpenApi]
    impl ComplexApi {
        /// Create a complex entity
        #[oai(path = "/api/v1/complex", method = "post")]
        async fn create_complex_entity(&self, payload: Json<ComplexRequest>) -> ComplexResponse {
            ComplexResponse::Success(payload)
        }
    }

    #[test]
    fn test_complex_api_export() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let exporter = OpenApiExporter;

            // Test JSON export
            let json_format = OpenApiFormat { json: true };
            let exported_json = exporter.export_openapi(ComplexApi, &json_format);

            // Validate JSON structure
            let json_value: serde_json::Value = serde_json::from_str(&exported_json)?;
            assert!(json_value["paths"]["/api/v1/complex"]["post"].is_object());
            assert!(json_value["components"]["schemas"]["ComplexRequest"].is_object());

            // Test YAML export
            let yaml_format = OpenApiFormat { json: false };
            let exported_yaml = exporter.export_openapi(ComplexApi, &yaml_format);

            // Basic YAML validation
            assert!(exported_yaml.contains("/api/v1/complex:"));
            assert!(exported_yaml.contains("ComplexRequest:"));

            Ok(())
        })
    }
}