use async_trait::async_trait;

use crate::model::GolemError;

#[async_trait]
pub trait HealthcheckClient {
    async fn healthcheck(&self) -> Result<(), GolemError>;
}

pub struct HealthcheckClientLive<C: golem_gateway_client::api::HealthcheckClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_gateway_client::api::HealthcheckClient + Sync + Send> HealthcheckClient
    for HealthcheckClientLive<C>
{
    async fn healthcheck(&self) -> Result<(), GolemError> {
        self.client.healthcheck_get().await?;
        Ok(())
    }
}
