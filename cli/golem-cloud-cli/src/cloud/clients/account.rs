use crate::cloud::clients::errors::CloudGolemError;
use async_trait::async_trait;
use golem_cli::cloud::AccountId;
use golem_cloud_client::model::{Account, AccountData, Plan};
use tracing::info;

#[async_trait]
pub trait AccountClient {
    async fn get(&self, id: &AccountId) -> Result<Account, CloudGolemError>;
    async fn get_plan(&self, id: &AccountId) -> Result<Plan, CloudGolemError>;
    async fn put(&self, id: &AccountId, data: AccountData) -> Result<Account, CloudGolemError>;
    async fn post(&self, data: AccountData) -> Result<Account, CloudGolemError>;
    async fn delete(&self, id: &AccountId) -> Result<(), CloudGolemError>;
}

pub struct AccountClientLive<C: golem_cloud_client::api::AccountClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::AccountClient + Sync + Send> AccountClient
    for AccountClientLive<C>
{
    async fn get(&self, id: &AccountId) -> Result<Account, CloudGolemError> {
        info!("Getting account {id}");
        Ok(self.client.account_id_get(&id.id).await?)
    }

    async fn get_plan(&self, id: &AccountId) -> Result<Plan, CloudGolemError> {
        info!("Getting account plan of {id}.");
        Ok(self.client.account_id_plan_get(&id.id).await?)
    }

    async fn put(&self, id: &AccountId, data: AccountData) -> Result<Account, CloudGolemError> {
        info!("Updating account {id}.");
        Ok(self.client.account_id_put(&id.id, &data).await?)
    }

    async fn post(&self, data: AccountData) -> Result<Account, CloudGolemError> {
        info!("Creating account.");
        Ok(self.client.post(&data).await?)
    }

    async fn delete(&self, id: &AccountId) -> Result<(), CloudGolemError> {
        info!("Deleting account {id}.");
        let _ = self.client.account_id_delete(&id.id).await?;
        Ok(())
    }
}
