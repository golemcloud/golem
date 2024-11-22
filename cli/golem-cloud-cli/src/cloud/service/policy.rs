use crate::cloud::clients::policy::ProjectPolicyClient;
use crate::cloud::model::text::project::{ProjectPolicyAddView, ProjectPolicyGetView};
use crate::cloud::model::ProjectPolicyId;
use async_trait::async_trait;
use golem_cli::model::{GolemError, GolemResult};
use golem_cloud_client::model::ProjectAction;

#[async_trait]
pub trait ProjectPolicyService {
    async fn add(
        &self,
        project_policy_name: String,
        project_actions: Vec<ProjectAction>,
    ) -> Result<GolemResult, GolemError>;
    async fn get(&self, project_policy_id: ProjectPolicyId) -> Result<GolemResult, GolemError>;
}

pub struct ProjectPolicyServiceLive {
    pub client: Box<dyn ProjectPolicyClient + Send + Sync>,
}

#[async_trait]
impl ProjectPolicyService for ProjectPolicyServiceLive {
    async fn add(
        &self,
        project_policy_name: String,
        project_actions: Vec<ProjectAction>,
    ) -> Result<GolemResult, GolemError> {
        let policy = self
            .client
            .create(project_policy_name, project_actions)
            .await?;

        Ok(GolemResult::Ok(Box::new(ProjectPolicyAddView(policy))))
    }

    async fn get(&self, project_policy_id: ProjectPolicyId) -> Result<GolemResult, GolemError> {
        let policy = self.client.get(project_policy_id).await?;

        Ok(GolemResult::Ok(Box::new(ProjectPolicyGetView(policy))))
    }
}
