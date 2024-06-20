use async_trait::async_trait;
use golem_cloud_client::model::{ProjectGrant, ProjectGrantDataRequest};
use tracing::info;

use crate::cloud::clients::action_cli_to_api;
use crate::cloud::clients::errors::CloudGolemError;
use crate::cloud::model::{ProjectAction, ProjectPolicyId};
use golem_cli::cloud::{AccountId, ProjectId};

#[async_trait]
pub trait ProjectGrantClient {
    async fn create(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        policy_id: ProjectPolicyId,
    ) -> Result<ProjectGrant, CloudGolemError>;
    async fn create_actions(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        actions: Vec<ProjectAction>,
    ) -> Result<ProjectGrant, CloudGolemError>;
}

pub struct ProjectGrantClientLive<C: golem_cloud_client::api::ProjectGrantClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::ProjectGrantClient + Sync + Send> ProjectGrantClient
    for ProjectGrantClientLive<C>
{
    async fn create(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        policy_id: ProjectPolicyId,
    ) -> Result<ProjectGrant, CloudGolemError> {
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
    ) -> Result<ProjectGrant, CloudGolemError> {
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
