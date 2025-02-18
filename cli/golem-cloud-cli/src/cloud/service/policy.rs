use crate::cloud::clients::policy::ProjectPolicyClient;
use crate::cloud::model::text::project::{ProjectPolicyAddView, ProjectPolicyGetView};
use crate::cloud::model::{ProjectAction, ProjectPolicyId};
use async_trait::async_trait;
use golem_cli::model::{GolemError, GolemResult};

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
            .create(
                project_policy_name,
                project_actions.into_iter().map(|a| a.into()).collect(),
            )
            .await?;

        Ok(GolemResult::Ok(Box::new(ProjectPolicyAddView(policy))))
    }

    async fn get(&self, project_policy_id: ProjectPolicyId) -> Result<GolemResult, GolemError> {
        let policy = self.client.get(project_policy_id).await?;

        Ok(GolemResult::Ok(Box::new(ProjectPolicyGetView(policy))))
    }
}
