use async_trait::async_trait;
use golem_cloud_client::model::{ProjectAction, ProjectActions, ProjectPolicy, ProjectPolicyData};
use tracing::info;

use crate::cloud::clients::errors::CloudGolemError;
use crate::cloud::model::ProjectPolicyId;

#[async_trait]
pub trait ProjectPolicyClient {
    async fn create(
        &self,
        name: String,
        actions: Vec<ProjectAction>,
    ) -> Result<ProjectPolicy, CloudGolemError>;
    async fn get(&self, policy_id: ProjectPolicyId) -> Result<ProjectPolicy, CloudGolemError>;
}

pub struct ProjectPolicyClientLive<C: golem_cloud_client::api::ProjectPolicyClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::ProjectPolicyClient + Sync + Send> ProjectPolicyClient
    for ProjectPolicyClientLive<C>
{
    async fn create(
        &self,
        name: String,
        actions: Vec<ProjectAction>,
    ) -> Result<ProjectPolicy, CloudGolemError> {
        info!("Creation project policy");

        let data = ProjectPolicyData {
            name,
            project_actions: ProjectActions { actions },
        };

        Ok(self.client.create_project_policy(&data).await?)
    }

    async fn get(&self, policy_id: ProjectPolicyId) -> Result<ProjectPolicy, CloudGolemError> {
        info!("Getting project policy");

        Ok(self.client.get_project_policies(&policy_id.0).await?)
    }
}
