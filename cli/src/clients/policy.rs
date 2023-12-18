use async_trait::async_trait;
use golem_client::apis::configuration::Configuration;
use golem_client::apis::project_policy_api::{
    v2_project_policies_post, v2_project_policies_project_policy_id_get,
};
use golem_client::models::{ProjectActions, ProjectPolicy, ProjectPolicyData};
use tracing::info;

use crate::clients::action_cli_to_api;
use crate::model::{GolemError, ProjectAction, ProjectPolicyId};

#[async_trait]
pub trait ProjectPolicyClient {
    async fn create(
        &self,
        name: String,
        actions: Vec<ProjectAction>,
    ) -> Result<ProjectPolicy, GolemError>;
    async fn get(&self, policy_id: ProjectPolicyId) -> Result<ProjectPolicy, GolemError>;
}

pub struct ProjectPolicyClientLive {
    pub configuration: Configuration,
}

#[async_trait]
impl ProjectPolicyClient for ProjectPolicyClientLive {
    async fn create(
        &self,
        name: String,
        actions: Vec<ProjectAction>,
    ) -> Result<ProjectPolicy, GolemError> {
        info!("Creation project policy");

        let actions: Vec<golem_client::models::ProjectAction> =
            actions.into_iter().map(action_cli_to_api).collect();
        let data = ProjectPolicyData {
            name,
            project_actions: Box::new(ProjectActions { actions }),
        };

        Ok(v2_project_policies_post(&self.configuration, data).await?)
    }

    async fn get(&self, policy_id: ProjectPolicyId) -> Result<ProjectPolicy, GolemError> {
        info!("Getting project policy");

        Ok(
            v2_project_policies_project_policy_id_get(
                &self.configuration,
                &policy_id.0.to_string(),
            )
            .await?,
        )
    }
}
