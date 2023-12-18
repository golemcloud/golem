use async_trait::async_trait;
use golem_client::apis::configuration::Configuration;
use golem_client::apis::grant_api::{
    v2_accounts_account_id_grants_get, v2_accounts_account_id_grants_role_delete,
    v2_accounts_account_id_grants_role_get, v2_accounts_account_id_grants_role_put,
};
use tracing::info;

use crate::model::{AccountId, GolemError, Role};

#[async_trait]
pub trait GrantClient {
    async fn get_all(&self, account_id: AccountId) -> Result<Vec<Role>, GolemError>;
    async fn get(&self, account_id: AccountId, role: Role) -> Result<Role, GolemError>;
    async fn put(&self, account_id: AccountId, role: Role) -> Result<(), GolemError>;
    async fn delete(&self, account_id: AccountId, role: Role) -> Result<(), GolemError>;
}

pub struct GrantClientLive {
    pub configuration: Configuration,
}

#[async_trait]
impl GrantClient for GrantClientLive {
    async fn get_all(&self, account_id: AccountId) -> Result<Vec<Role>, GolemError> {
        info!("Getting account roles.");

        let roles = v2_accounts_account_id_grants_get(&self.configuration, &account_id.id).await?;

        Ok(roles.into_iter().map(api_to_cli).collect())
    }

    async fn get(&self, account_id: AccountId, role: Role) -> Result<Role, GolemError> {
        info!("Getting account role.");
        let role = cli_to_api(role);

        Ok(api_to_cli(
            v2_accounts_account_id_grants_role_get(&self.configuration, &account_id.id, role)
                .await?,
        ))
    }

    async fn put(&self, account_id: AccountId, role: Role) -> Result<(), GolemError> {
        info!("Adding account role.");
        let role = cli_to_api(role);

        let _ = v2_accounts_account_id_grants_role_put(&self.configuration, &account_id.id, role)
            .await?;

        Ok(())
    }

    async fn delete(&self, account_id: AccountId, role: Role) -> Result<(), GolemError> {
        info!("Deleting account role.");
        let role = cli_to_api(role);

        let _ =
            v2_accounts_account_id_grants_role_delete(&self.configuration, &account_id.id, role)
                .await?;

        Ok(())
    }
}

fn api_to_cli(role: golem_client::models::Role) -> Role {
    match role {
        golem_client::models::Role::Admin {} => Role::Admin,
        golem_client::models::Role::MarketingAdmin {} => Role::MarketingAdmin,
        golem_client::models::Role::ViewProject {} => Role::ViewProject,
        golem_client::models::Role::DeleteProject {} => Role::DeleteProject,
        golem_client::models::Role::CreateProject {} => Role::CreateProject,
        golem_client::models::Role::InstanceServer {} => Role::InstanceServer,
    }
}

fn cli_to_api(role: Role) -> golem_client::models::Role {
    match role {
        Role::Admin {} => golem_client::models::Role::Admin,
        Role::MarketingAdmin {} => golem_client::models::Role::MarketingAdmin,
        Role::ViewProject {} => golem_client::models::Role::ViewProject,
        Role::DeleteProject {} => golem_client::models::Role::DeleteProject,
        Role::CreateProject {} => golem_client::models::Role::CreateProject,
        Role::InstanceServer {} => golem_client::models::Role::InstanceServer,
    }
}
