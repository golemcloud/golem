use async_trait::async_trait;
use golem_gateway_client::apis::configuration::Configuration;
use golem_gateway_client::apis::healthcheck_api::healthcheck_get;
use tracing::info;

use crate::model::GolemError;

#[async_trait]
pub trait HealthcheckClient {
    async fn healthcheck(&self) -> Result<(), GolemError>;
}

pub struct HealthcheckClientLive {
    pub configuration: Configuration,
}

#[async_trait]
impl HealthcheckClient for HealthcheckClientLive {
    async fn healthcheck(&self) -> Result<(), GolemError> {
        info!(
            "Calling healthcheck_get on base url: {}",
            self.configuration.base_path
        );
        healthcheck_get(&self.configuration).await?;
        Ok(())
    }
}
