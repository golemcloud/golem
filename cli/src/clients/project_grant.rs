use async_trait::async_trait;
use golem_client::model::ProjectGrant;
use golem_client::model::ProjectGrantDataRequest;
use tracing::info;

use crate::clients::action_cli_to_api;
use crate::model::{AccountId, GolemError, ProjectAction, ProjectId, ProjectPolicyId};

#[async_trait]
pub trait ProjectGrantClient {
    async fn create(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        policy_id: ProjectPolicyId,
    ) -> Result<ProjectGrant, GolemError>;
    async fn create_actions(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        actions: Vec<ProjectAction>,
    ) -> Result<ProjectGrant, GolemError>;
}

pub struct ProjectGrantClientLive<C: golem_client::api::ProjectGrantClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::api::ProjectGrantClient + Sync + Send> ProjectGrantClient
    for ProjectGrantClientLive<C>
{
    async fn create(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        policy_id: ProjectPolicyId,
    ) -> Result<ProjectGrant, GolemError> {
        info!("Creating project grant for policy {policy_id}.");

        let data = ProjectGrantDataRequest {
            grantee_account_id: account_id.id,
            project_policy_id: Some(policy_id.0),
            project_actions: Vec::new(),
            project_policy_name: None,
        };

        Ok(self.client.post(&project_id.0, &data).await?)
    }

    async fn create_actions(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        actions: Vec<ProjectAction>,
    ) -> Result<ProjectGrant, GolemError> {
        info!("Creating project grant for actions.");

        let data = ProjectGrantDataRequest {
            grantee_account_id: account_id.id,
            project_policy_id: None,
            project_policy_name: None,
            project_actions: actions.into_iter().map(action_cli_to_api).collect(),
        };

        Ok(self.client.post(&project_id.0, &data).await?)
    }
}
