use poem_openapi::payload::Json;
use poem_openapi::*;

use crate::api::ApiTags;
use crate::model::{HealthcheckResponse, VersionInfo};
use crate::VERSION;

pub struct HealthcheckApi;

#[OpenApi(prefix_path = "/", tag = ApiTags::HealthCheck)]
impl HealthcheckApi {
    #[oai(path = "/healthcheck", method = "get", operation_id = "healthcheck")]
    async fn healthcheck(&self) -> Json<HealthcheckResponse> {
        Json(HealthcheckResponse {})
    }

    #[oai(path = "/version", method = "get", operation_id = "version")]
    async fn version(&self) -> Json<VersionInfo> {
        Json(VersionInfo {
            version: VERSION.to_string(),
        })
    }
}
