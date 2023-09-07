use async_trait::async_trait;
use golem_client::model::{
    ProjectGrant, ProjectGrantDataRequest, ProjectGrantDataWithProjectActions,
};
use tracing::info;

use crate::clients::{action_cli_to_api, CloudAuthentication};
use crate::model::{AccountId, GolemError, ProjectAction, ProjectId, ProjectPolicyId};

#[async_trait]
pub trait ProjectGrantClient {
    async fn create(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        policy_id: ProjectPolicyId,
        auth: &CloudAuthentication,
    ) -> Result<ProjectGrant, GolemError>;
    async fn create_actions(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        actions: Vec<ProjectAction>,
        auth: &CloudAuthentication,
    ) -> Result<ProjectGrant, GolemError>;
}

pub struct ProjectGrantClientLive<C: golem_client::project_grant::ProjectGrant + Send + Sync> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::project_grant::ProjectGrant + Send + Sync> ProjectGrantClient
    for ProjectGrantClientLive<C>
{
    async fn create(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        policy_id: ProjectPolicyId,
        auth: &CloudAuthentication,
    ) -> Result<ProjectGrant, GolemError> {
        info!("Creating project grant for policy {policy_id}.");

        let data = ProjectGrantDataRequest {
            grantee_account_id: account_id.id,
            project_policy_id: policy_id.0,
        };
        Ok(self
            .client
            .post_project_grant(&project_id.0.to_string(), data, &auth.header())
            .await?)
    }

    async fn create_actions(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        actions: Vec<ProjectAction>,
        auth: &CloudAuthentication,
    ) -> Result<ProjectGrant, GolemError> {
        info!("Creating project grant for actions.");

        let data = ProjectGrantDataWithProjectActions {
            grantee_account_id: account_id.id,
            project_policy_name: None,
            project_actions: actions.into_iter().map(action_cli_to_api).collect(),
        };
        Ok(self
            .client
            .post_project_grant_with_actions(&project_id.0.to_string(), data, &auth.header())
            .await?)
    }
}
