use async_trait::async_trait;
use golem_client::apis::account_api::{
    v2_accounts_account_id_delete, v2_accounts_account_id_get, v2_accounts_account_id_plan_get,
    v2_accounts_account_id_put, v2_accounts_post,
};
use golem_client::apis::configuration::Configuration;
use golem_client::models::{Account, AccountData, Plan};
use tracing::info;

use crate::model::{AccountId, GolemError};

#[async_trait]
pub trait AccountClient {
    async fn get(&self, id: &AccountId) -> Result<Account, GolemError>;
    async fn get_plan(&self, id: &AccountId) -> Result<Plan, GolemError>;
    async fn put(&self, id: &AccountId, data: AccountData) -> Result<Account, GolemError>;
    async fn post(&self, data: AccountData) -> Result<Account, GolemError>;
    async fn delete(&self, id: &AccountId) -> Result<(), GolemError>;
}

pub struct AccountClientLive {
    pub configuration: Configuration,
}

#[async_trait]
impl AccountClient for AccountClientLive {
    async fn get(&self, id: &AccountId) -> Result<Account, GolemError> {
        info!("Getting account {id}");
        Ok(v2_accounts_account_id_get(&self.configuration, &id.id).await?)
    }

    async fn get_plan(&self, id: &AccountId) -> Result<Plan, GolemError> {
        info!("Getting account plan of {id}.");
        Ok(v2_accounts_account_id_plan_get(&self.configuration, &id.id).await?)
    }

    async fn put(&self, id: &AccountId, data: AccountData) -> Result<Account, GolemError> {
        info!("Updating account {id}.");
        Ok(v2_accounts_account_id_put(&self.configuration, &id.id, data).await?)
    }

    async fn post(&self, data: AccountData) -> Result<Account, GolemError> {
        info!("Creating account.");
        Ok(v2_accounts_post(&self.configuration, data).await?)
    }

    async fn delete(&self, id: &AccountId) -> Result<(), GolemError> {
        info!("Deleting account {id}.");
        let _ = v2_accounts_account_id_delete(&self.configuration, &id.id).await?;
        Ok(())
    }
}
