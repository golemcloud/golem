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