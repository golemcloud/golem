use poem_openapi::{OpenApi, payload::Json, param::Path};
use serde_json::Value;

use super::export_templates::{get_storage_api_template, get_complex_api_template, get_available_templates};

#[derive(Clone)]
pub struct ExportApi;

#[OpenApi]
impl ExportApi {
    /// List available API templates
    #[oai(path = "/templates", method = "get")]
    async fn list_templates(&self) -> Json<Value> {
        let templates = get_available_templates();
        Json(serde_json::json!({
            "templates": templates.iter().map(|(id, description)| {
                serde_json::json!({
                    "id": id,
                    "description": description
                })
            }).collect::<Vec<_>>()
        }))
    }

    /// Get a specific API template
    #[oai(path = "/templates/:template_id", method = "get")]
    async fn get_template(&self, template_id: Path<String>) -> Json<Value> {
        let spec = match template_id.as_str() {
            "storage" => get_storage_api_template(),
            "complex" => get_complex_api_template(),
            _ => serde_json::json!({
                "error": "Template not found",
                "available_templates": get_available_templates()
                    .iter()
                    .map(|(id, _)| id)
                    .collect::<Vec<_>>()
            })
        };
        Json(spec)
    }
} 