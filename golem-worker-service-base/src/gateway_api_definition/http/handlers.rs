use poem::{
    web::{Path, Query, Data},
    Result, handler,
};
use poem_openapi::OpenApi;
use crate::gateway_api_definition::http::{
    openapi_export::{OpenApiFormat, OpenApiExporter},
};
use poem::web::Json;

pub struct OpenApiHandler {
    exporter: OpenApiExporter,
}

impl OpenApiHandler {
    pub fn new() -> Self {
        Self {
            exporter: OpenApiExporter,
        }
    }
}

#[handler]
pub async fn export_openapi(
    Data(handler): Data<&OpenApiHandler>,
    Path((id, version)): Path<(String, String)>,
    Query(format): Query<OpenApiFormat>,
    Json(openapi): Json<OpenApi>,
) -> Result<String> {
    let content = handler.exporter.export_openapi(&id, &version, openapi, &format);
    Ok(content)
} 