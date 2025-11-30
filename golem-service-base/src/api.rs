use crate::api_tags::ApiTags;
use golem_common::golem_version;
use golem_common::model::{Empty, VersionInfo};
use poem_openapi::payload::Json;
use poem_openapi::*;

pub struct HealthcheckApi;

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
