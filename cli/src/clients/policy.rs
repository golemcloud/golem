use crate::clients::{action_cli_to_api, CloudAuthentication};
use crate::model::{GolemError, ProjectAction, ProjectPolicyId};
use async_trait::async_trait;
use golem_client::model::{ProjectActions, ProjectPolicy, ProjectPolicyData};
use std::collections::HashSet;
use tracing::info;

#[async_trait]
pub trait ProjectPolicyClient {
    async fn create(
        &self,
        name: String,
        actions: Vec<ProjectAction>,
        auth: &CloudAuthentication,
    ) -> Result<ProjectPolicy, GolemError>;
    async fn get(
        &self,
        policy_id: ProjectPolicyId,
        auth: &CloudAuthentication,
    ) -> Result<ProjectPolicy, GolemError>;
}

pub struct ProjectPolicyClientLive<C: golem_client::project_policy::ProjectPolicy + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::project_policy::ProjectPolicy + Sync + Send> ProjectPolicyClient
    for ProjectPolicyClientLive<C>
{
    async fn create(
        &self,
        name: String,
        actions: Vec<ProjectAction>,
        auth: &CloudAuthentication,
    ) -> Result<ProjectPolicy, GolemError> {
        info!("Creation project policy");

        let actions: HashSet<golem_client::model::ProjectAction> =
            actions.into_iter().map(action_cli_to_api).collect();
        let data = ProjectPolicyData {
            name,
            project_actions: ProjectActions { actions },
        };

        Ok(self
            .client
            .post_project_policy(data, &auth.header())
            .await?)
    }

    async fn get(
        &self,
        policy_id: ProjectPolicyId,
        auth: &CloudAuthentication,
    ) -> Result<ProjectPolicy, GolemError> {
        info!("Getting project policy");

        Ok(self
            .client
            .get_project_policies(&policy_id.0.to_string(), &auth.header())
            .await?)
    }
}
