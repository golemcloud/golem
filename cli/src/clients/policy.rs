use async_trait::async_trait;
use golem_client::model::ProjectActions;
use golem_client::model::ProjectPolicy;
use golem_client::model::ProjectPolicyData;
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

pub struct ProjectPolicyClientLive<C: golem_client::api::ProjectPolicyClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::api::ProjectPolicyClient + Sync + Send> ProjectPolicyClient
    for ProjectPolicyClientLive<C>
{
    async fn create(
        &self,
        name: String,
        actions: Vec<ProjectAction>,
    ) -> Result<ProjectPolicy, GolemError> {
        info!("Creation project policy");

        let actions: Vec<golem_client::model::ProjectAction> =
            actions.into_iter().map(action_cli_to_api).collect();
        let data = ProjectPolicyData {
            name,
            project_actions: ProjectActions { actions },
        };

        Ok(self.client.post(&data).await?)
    }

    async fn get(&self, policy_id: ProjectPolicyId) -> Result<ProjectPolicy, GolemError> {
        info!("Getting project policy");

        Ok(self.client.project_policy_id_get(&policy_id.0).await?)
    }
}
