use crate::cloud::clients::errors::CloudGolemError;
use crate::cloud::model::Role;
use async_trait::async_trait;
use golem_cli::cloud::AccountId;
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

        let roles = self.client.get(&account_id.id).await?;

        Ok(roles.into_iter().map(api_to_cli).collect())
    }

    async fn get(&self, account_id: &AccountId, role: Role) -> Result<Role, CloudGolemError> {
        info!("Getting account role.");
        let role = cli_to_api(role);

        Ok(api_to_cli(
            self.client.role_get(&account_id.id, &role).await?,
        ))
    }

    async fn put(&self, account_id: &AccountId, role: Role) -> Result<(), CloudGolemError> {
        info!("Adding account role.");
        let role = cli_to_api(role);

        let _ = self.client.role_put(&account_id.id, &role).await?;

        Ok(())
    }

    async fn delete(&self, account_id: &AccountId, role: Role) -> Result<(), CloudGolemError> {
        info!("Deleting account role.");
        let role = cli_to_api(role);

        let _ = self.client.role_delete(&account_id.id, &role).await?;

        Ok(())
    }
}

fn api_to_cli(role: golem_cloud_client::model::Role) -> Role {
    match role {
        golem_cloud_client::model::Role::Admin {} => Role::Admin,
        golem_cloud_client::model::Role::MarketingAdmin {} => Role::MarketingAdmin,
        golem_cloud_client::model::Role::ViewProject {} => Role::ViewProject,
        golem_cloud_client::model::Role::DeleteProject {} => Role::DeleteProject,
        golem_cloud_client::model::Role::CreateProject {} => Role::CreateProject,
        golem_cloud_client::model::Role::InstanceServer {} => Role::InstanceServer,
    }
}

fn cli_to_api(role: Role) -> golem_cloud_client::model::Role {
    match role {
        Role::Admin {} => golem_cloud_client::model::Role::Admin,
        Role::MarketingAdmin {} => golem_cloud_client::model::Role::MarketingAdmin,
        Role::ViewProject {} => golem_cloud_client::model::Role::ViewProject,
        Role::DeleteProject {} => golem_cloud_client::model::Role::DeleteProject,
        Role::CreateProject {} => golem_cloud_client::model::Role::CreateProject,
        Role::InstanceServer {} => golem_cloud_client::model::Role::InstanceServer,
    }
}
