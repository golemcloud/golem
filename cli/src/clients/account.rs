use async_trait::async_trait;
use golem_client::model::{Account, AccountData, Plan};
use crate::clients::CloudAuthentication;
use crate::model::{AccountId, GolemError};
use tracing::info;

#[async_trait]
pub trait AccountClient {
    async fn get(&self, id: &AccountId, auth: &CloudAuthentication) -> Result<Account, GolemError>;
    async fn get_plan(&self, id: &AccountId, auth: &CloudAuthentication) -> Result<Plan, GolemError>;
    async fn put(&self, id: &AccountId, data: AccountData, auth: &CloudAuthentication) -> Result<Account, GolemError>;
    async fn post(&self, data: AccountData, auth: &CloudAuthentication) -> Result<Account, GolemError>;
    async fn delete(&self, id: &AccountId, auth: &CloudAuthentication) -> Result<(), GolemError>;
}

pub struct AccountClientLive<A: golem_client::account::Account + Send + Sync> {
    pub account: A,
}


#[async_trait]
impl<A: golem_client::account::Account + Send + Sync> AccountClient for AccountClientLive<A> {
    async fn get(&self, id: &AccountId, auth: &CloudAuthentication) -> Result<Account, GolemError> {
        info!("Getting account {id}");
        Ok(self.account.get_account(&id.id, &auth.header()).await?)
    }

    async fn get_plan(&self, id: &AccountId, auth: &CloudAuthentication) -> Result<Plan, GolemError> {
        info!("Getting account plan of {id}.");
        Ok(self.account.get_account_plan(&id.id, &auth.header()).await?)
    }

    async fn put(&self, id: &AccountId, data: AccountData, auth: &CloudAuthentication) -> Result<Account, GolemError> {
        info!("Updating account {id}.");
        Ok(self.account.put_account(&id.id, data, &auth.header()).await?)
    }

    async fn post(&self, data: AccountData, auth: &CloudAuthentication) -> Result<Account, GolemError> {
        info!("Creating account.");
        Ok(self.account.post_account( data, &auth.header()).await?)
    }

    async fn delete(&self, id: &AccountId, auth: &CloudAuthentication) -> Result<(), GolemError> {
        info!("Deleting account {id}.");
        Ok(self.account.delete_account(&id.id, &auth.header()).await?)
    }
}