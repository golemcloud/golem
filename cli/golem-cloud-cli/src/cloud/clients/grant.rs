use crate::cloud::clients::errors::CloudGolemError;
use async_trait::async_trait;
use golem_cli::cloud::AccountId;
use golem_cloud_client::model::Role;
use tracing::info;

#[async_trait]
pub trait GrantClient {
    async fn get_all(&self, account_id: &AccountId) -> Result<Vec<Role>, CloudGolemError>;
    async fn get(&self, account_id: &AccountId, role: Role) -> Result<Role, CloudGolemError>;
    async fn put(&self, account_id: &AccountId, role: Role) -> Result<(), CloudGolemError>;
    async fn delete(&self, account_id: &AccountId, role: Role) -> Result<(), CloudGolemError>;
}

pub struct GrantClientLive<C: golem_cloud_client::api::GrantClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::GrantClient + Sync + Send> GrantClient for GrantClientLive<C> {
    async fn get_all(&self, account_id: &AccountId) -> Result<Vec<Role>, CloudGolemError> {
        info!("Getting account roles.");

        let roles = self.client.get_account_grants(&account_id.id).await?;

        Ok(roles)
    }

    async fn get(&self, account_id: &AccountId, role: Role) -> Result<Role, CloudGolemError> {
        info!("Getting account role.");

        Ok(self.client.get_account_grant(&account_id.id, &role).await?)
    }

    async fn put(&self, account_id: &AccountId, role: Role) -> Result<(), CloudGolemError> {
        info!("Adding account role.");

        let _ = self
            .client
            .create_account_grant(&account_id.id, &role)
            .await?;

        Ok(())
    }

    async fn delete(&self, account_id: &AccountId, role: Role) -> Result<(), CloudGolemError> {
        info!("Deleting account role.");

        let _ = self
            .client
            .delete_account_grant(&account_id.id, &role)
            .await?;

        Ok(())
    }
}
