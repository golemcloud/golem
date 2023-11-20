use async_trait::async_trait;

use crate::clients::gateway::healthcheck::HealthcheckClient;
use crate::model::{GolemError, GolemResult};

#[async_trait]
pub trait HealthcheckHandler {
    async fn handle(&self) -> Result<GolemResult, GolemError>;
}

pub struct HealthcheckHandlerLive<H: HealthcheckClient + Sync + Send> {
    pub healthcheck: H,
}

#[async_trait]
impl<H: HealthcheckClient + Sync + Send> HealthcheckHandler for HealthcheckHandlerLive<H> {
    async fn handle(&self) -> Result<GolemResult, GolemError> {
        self.healthcheck.healthcheck().await?;

        Ok(GolemResult::Str("Online".to_string()))
    }
}
