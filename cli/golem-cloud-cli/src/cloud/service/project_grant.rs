use crate::cloud::clients::project_grant::ProjectGrantClient;
use crate::cloud::model::text::ProjectGrantView;
use crate::cloud::model::{ProjectAction, ProjectPolicyId, ProjectRef};
use crate::cloud::service::project::ProjectService;
use async_trait::async_trait;
use golem_cli::cloud::AccountId;
use golem_cli::model::{GolemError, GolemResult};
use std::sync::Arc;

#[async_trait]
pub trait ProjectGrantService {
    async fn grant(
        &self,
        project_ref: ProjectRef,
        recipient_account_id: AccountId,
        project_policy_id: Option<ProjectPolicyId>,
        project_actions: Option<Vec<ProjectAction>>,
    ) -> Result<GolemResult, GolemError>;
}

pub struct ProjectGrantServiceLive {
    pub client: Box<dyn ProjectGrantClient + Send + Sync>,
    pub projects: Arc<dyn ProjectService + Send + Sync>,
}

#[async_trait]
impl ProjectGrantService for ProjectGrantServiceLive {
    async fn grant(
        &self,
        project_ref: ProjectRef,
        recipient_account_id: AccountId,
        project_policy_id: Option<ProjectPolicyId>,
        project_actions: Option<Vec<ProjectAction>>,
    ) -> Result<GolemResult, GolemError> {
        let project_urn = self.projects.resolve_urn_or_default(project_ref).await?;
        match project_policy_id {
            None => {
                let actions = project_actions.unwrap();

                let grant = self
                    .client
                    .create_actions(project_urn, recipient_account_id, actions)
                    .await?;

                Ok(GolemResult::Ok(Box::new(ProjectGrantView(grant))))
            }
            Some(policy_id) => {
                let grant = self
                    .client
                    .create(project_urn, recipient_account_id, policy_id)
                    .await?;

                Ok(GolemResult::Ok(Box::new(ProjectGrantView(grant))))
            }
        }
    }
}
