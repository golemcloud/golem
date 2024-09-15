use golem_common::golem_version;
use golem_service_base::api_tags::ApiTags;
use poem_openapi::payload::Json;
use poem_openapi::*;

pub struct HealthcheckApi;

const VERSION: &str = golem_version!();

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct HealthcheckResponse {}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct VersionInfo {
    pub version: String,
}

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
