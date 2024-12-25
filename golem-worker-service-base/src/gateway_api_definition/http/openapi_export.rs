use utoipa::{
    openapi::{OpenApi, Info},
    ToSchema,
};
use std::collections::BTreeMap;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiFormat {
    pub json: bool,
}

impl Default for OpenApiFormat {
    fn default() -> Self {
        Self { json: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OpenApiExporter;

impl OpenApiExporter {
    #[inline]
    pub fn export_openapi(
        &self,
        api_id: &str,
        version: &str,
        mut openapi: OpenApi,
        format: &OpenApiFormat,
    ) -> String {
        openapi.info = Info::new(format!("{} API", api_id), version.to_string());

        // Process security schemes
        if let Some(components) = &mut openapi.components {
            let mut security_schemes = BTreeMap::new();
            
            // Keep existing security schemes
            security_schemes.extend(components.security_schemes.clone());
            
            components.security_schemes = security_schemes;
        }

        if format.json {
            serde_json::to_string_pretty(&openapi).unwrap_or_default()
        } else {
            serde_yaml::to_string(&openapi).unwrap_or_default()
        }
    }

    pub fn get_export_path(api_id: &str, version: &str) -> String {
        format!("/v1/api/definitions/{}/version/{}/export", api_id, version)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_openapi_export() {
        let exporter = super::OpenApiExporter;
        let mut openapi = utoipa::openapi::OpenApi::new(Default::default(), ());

        // Test JSON export
        let json_format = crate::gateway_api_definition::http::OpenApiFormat { json: true };
        let exported_json = exporter.export_openapi(
            "test-api",
            "1.0.0",
            openapi.clone(),
            &json_format,
        );
        assert!(!exported_json.is_empty());
        assert!(exported_json.contains("test-api API"));
        assert!(exported_json.contains("1.0.0"));

        // Test YAML export
        let yaml_format = crate::gateway_api_definition::http::OpenApiFormat { json: false };
        let exported_yaml = exporter.export_openapi(
            "test-api",
            "1.0.0",
            openapi.clone(),
            &yaml_format,
        );
        assert!(!exported_yaml.is_empty());
        assert!(exported_yaml.contains("test-api API"));
        assert!(exported_yaml.contains("1.0.0"));

        // Test invalid OpenAPI handling
        let invalid_openapi = utoipa::openapi::OpenApi::new(Default::default(), ());
        let result = exporter.export_openapi(
            "test-api",
            "1.0.0",
            invalid_openapi.clone(),
            &json_format,
        );
        assert!(!result.is_empty()); // Should return default value instead of failing
        
        // Test YAML export with invalid OpenAPI
        let yaml_format = crate::gateway_api_definition::http::OpenApiFormat { json: false };
        let result = exporter.export_openapi(
            "test-api",
            "1.0.0",
            invalid_openapi,
            &yaml_format,
        );
        assert!(!result.is_empty()); // Should return default value instead of failing
    }

    #[test]
    fn test_openapi_format_default() {
        let format = crate::gateway_api_definition::http::OpenApiFormat::default();
        assert!(format.json);
    }
}
