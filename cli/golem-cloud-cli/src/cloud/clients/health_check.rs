use async_trait::async_trait;
use golem_cli::clients::health_check::HealthCheckClient;
use golem_client::model::VersionInfo;
use tracing::debug;

use crate::cloud::clients::errors::CloudGolemError;
use golem_cli::model::GolemError;

#[derive(Clone)]
pub struct HealthCheckClientLive<C: golem_cloud_client::api::HealthCheckClient + Sync + Send> {
    pub client: C,
}

fn to_oss_version_info(v: golem_cloud_client::model::VersionInfo) -> VersionInfo {
    VersionInfo { version: v.version }
}

#[async_trait]
impl<C: golem_cloud_client::api::HealthCheckClient + Sync + Send> HealthCheckClient
    for HealthCheckClientLive<C>
{
    async fn version(&self) -> Result<VersionInfo, GolemError> {
        debug!("Getting server version");

        Ok(to_oss_version_info(
            self.client.version().await.map_err(CloudGolemError::from)?,
        ))
    }
}
