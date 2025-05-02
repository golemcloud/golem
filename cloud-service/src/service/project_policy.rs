use std::sync::Arc;

use crate::model::ProjectPolicy;
use crate::repo::project_policy::{ProjectPolicyRecord, ProjectPolicyRepo};
use async_trait::async_trait;
use cloud_common::model::ProjectPolicyId;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use tracing::info;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ProjectPolicyError {
    #[error("Internal repository error: {0}")]
    InternalRepoError(#[from] RepoError),
}

impl SafeDisplay for ProjectPolicyError {
    fn to_safe_string(&self) -> String {
        match self {
            ProjectPolicyError::InternalRepoError(inner) => inner.to_safe_string(),
        }
    }
}

#[async_trait]
pub trait ProjectPolicyService {
    async fn create(&self, project_policy: &ProjectPolicy) -> Result<(), ProjectPolicyError>;

    async fn get_all(
        &self,
        project_policy_ids: Vec<ProjectPolicyId>,
    ) -> Result<Vec<ProjectPolicy>, ProjectPolicyError>;

    async fn get(
        &self,
        project_policy_id: &ProjectPolicyId,
    ) -> Result<Option<ProjectPolicy>, ProjectPolicyError>;

    async fn delete(&self, project_policy_id: &ProjectPolicyId) -> Result<(), ProjectPolicyError>;
}

pub struct ProjectPolicyServiceDefault {
    project_policy_repo: Arc<dyn ProjectPolicyRepo + Sync + Send>,
}

impl ProjectPolicyServiceDefault {
    pub fn new(project_policy_repo: Arc<dyn ProjectPolicyRepo + Sync + Send>) -> Self {
        ProjectPolicyServiceDefault {
            project_policy_repo,
        }
    }
}

// TODO: Project policies have no owner, so also no way to check who owns them / has permission to access them.
// Currently policies can be created and deleted by any user.
#[async_trait]
impl ProjectPolicyService for ProjectPolicyServiceDefault {
    async fn create(&self, project_policy: &ProjectPolicy) -> Result<(), ProjectPolicyError> {
        info!("Create project policy {}", project_policy.id);
        let project_policy: ProjectPolicyRecord = project_policy.clone().into();
        self.project_policy_repo.create(&project_policy).await?;
        Ok(())
    }

    async fn get_all(
        &self,
        project_policy_ids: Vec<ProjectPolicyId>,
    ) -> Result<Vec<ProjectPolicy>, ProjectPolicyError> {
        let ids: Vec<Uuid> = project_policy_ids.iter().map(|p| p.0).collect();
        info!(
            "Getting project policies for project {}",
            project_policy_ids
                .iter()
                .map(|p| p.0.clone().to_string())
                .collect::<Vec<String>>()
                .join(",")
        );
        let result = self.project_policy_repo.get_all(ids).await?;
        Ok(result.iter().map(|p| p.clone().into()).collect())
    }

    async fn get(
        &self,
        project_policy_id: &ProjectPolicyId,
    ) -> Result<Option<ProjectPolicy>, ProjectPolicyError> {
        info!("Getting project policy {}", project_policy_id);
        let result = self.project_policy_repo.get(&project_policy_id.0).await?;
        Ok(result.map(|p| p.into()))
    }

    async fn delete(&self, project_policy_id: &ProjectPolicyId) -> Result<(), ProjectPolicyError> {
        info!("Deleting project policy {}", project_policy_id);
        self.project_policy_repo
            .delete(&project_policy_id.0)
            .await?;
        Ok(())
    }
}
