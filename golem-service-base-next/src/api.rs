use crate::api_tags::ApiTags;
use golem_common_next::golem_version;
use golem_common_next::model::Empty;
use poem_openapi::payload::Json;
use poem_openapi::Object;
use poem_openapi::*;
use std::fmt::Debug;

pub struct HealthcheckApi;

#[derive(Debug, Clone, Object)]
pub struct VersionInfo {
    pub version: String,
}

#[OpenApi(prefix_path = "/", tag = ApiTags::HealthCheck)]
impl HealthcheckApi {
    #[oai(path = "/healthcheck", method = "get", operation_id = "healthcheck")]
    async fn healthcheck(&self) -> Json<Empty> {
        Json(Empty {})
    }

    #[oai(path = "/version", method = "get", operation_id = "version")]
    async fn version(&self) -> Json<VersionInfo> {
        Json(VersionInfo {
            version: golem_version().to_string(),
        })
    }
}
