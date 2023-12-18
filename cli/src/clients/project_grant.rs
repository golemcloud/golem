use async_trait::async_trait;
use golem_client::apis::configuration::Configuration;
use golem_client::apis::project_grant_api::v2_projects_project_id_grants_post;
use golem_client::models::{ProjectGrant, ProjectGrantDataRequest};
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

pub struct ProjectGrantClientLive {
    pub configuration: Configuration,
}

#[async_trait]
impl ProjectGrantClient for ProjectGrantClientLive {
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

        Ok(
            v2_projects_project_id_grants_post(
                &self.configuration,
                &project_id.0.to_string(),
                data,
            )
            .await?,
        )
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
        Ok(
            v2_projects_project_id_grants_post(
                &self.configuration,
                &project_id.0.to_string(),
                data,
            )
            .await?,
        )
    }
}
