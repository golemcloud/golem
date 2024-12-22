use poem::{
    web::{Path, Query, Data},
    Result, handler,
};
use utoipa::openapi::OpenApi;
use crate::gateway_api_definition::http::{
    openapi_export::OpenApiFormat,
    openapi_converter::OpenApiConverter,
};
use poem::web::Json;

pub struct OpenApiHandler {
    converter: OpenApiConverter,
}

impl OpenApiHandler {
    pub fn new() -> Self {
        Self {
            converter: OpenApiConverter::new(),
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
    let content = handler.converter.exporter.export_openapi(&id, &version, openapi, &format);
    Ok(content)
} 