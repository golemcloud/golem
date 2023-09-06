use crate::clients::CloudAuthentication;
use crate::model::Role;
use crate::model::{AccountId, GolemError};
use async_trait::async_trait;
use tracing::info;

#[async_trait]
pub trait GrantClient {
    async fn get_all(
        &self,
        account_id: AccountId,
        auth: &CloudAuthentication,
    ) -> Result<Vec<Role>, GolemError>;
    async fn get(
        &self,
        account_id: AccountId,
        role: Role,
        auth: &CloudAuthentication,
    ) -> Result<Role, GolemError>;
    async fn put(
        &self,
        account_id: AccountId,
        role: Role,
        auth: &CloudAuthentication,
    ) -> Result<(), GolemError>;
    async fn delete(
        &self,
        account_id: AccountId,
        role: Role,
        auth: &CloudAuthentication,
    ) -> Result<(), GolemError>;
}

pub struct GrantClientLive<C: golem_client::grant::Grant + Send + Sync> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::grant::Grant + Send + Sync> GrantClient for GrantClientLive<C> {
    async fn get_all(
        &self,
        account_id: AccountId,
        auth: &CloudAuthentication,
    ) -> Result<Vec<Role>, GolemError> {
        info!("Getting account roles.");

        let roles = self
            .client
            .get_grants(&account_id.id, &auth.header())
            .await?;

        Ok(roles.into_iter().map(api_to_cli).collect())
    }

    async fn get(
        &self,
        account_id: AccountId,
        role: Role,
        auth: &CloudAuthentication,
    ) -> Result<Role, GolemError> {
        info!("Getting account role.");
        let role_str = format!("{role}");

        Ok(api_to_cli(
            self.client
                .get_grant(&account_id.id, &role_str, &auth.header())
                .await?,
        ))
    }

    async fn put(
        &self,
        account_id: AccountId,
        role: Role,
        auth: &CloudAuthentication,
    ) -> Result<(), GolemError> {
        info!("Adding account role.");
        let role_str = format!("{role}");

        let _ = self
            .client
            .put_grant(&account_id.id, &role_str, &auth.header())
            .await?;

        Ok(())
    }

    async fn delete(
        &self,
        account_id: AccountId,
        role: Role,
        auth: &CloudAuthentication,
    ) -> Result<(), GolemError> {
        info!("Deleting account role.");
        let role_str = format!("{role}");

        let _ = self
            .client
            .delete_grant(&account_id.id, &role_str, &auth.header())
            .await?;

        Ok(())
    }
}

fn api_to_cli(role: golem_client::model::Role) -> Role {
    match role {
        golem_client::model::Role::Admin {} => Role::Admin,
        golem_client::model::Role::WhitelistAdmin {} => Role::WhitelistAdmin,
        golem_client::model::Role::MarketingAdmin {} => Role::MarketingAdmin,
        golem_client::model::Role::ViewProject {} => Role::ViewProject,
        golem_client::model::Role::DeleteProject {} => Role::DeleteProject,
        golem_client::model::Role::CreateProject {} => Role::CreateProject,
        golem_client::model::Role::InstanceServer {} => Role::InstanceServer,
    }
}
