use std::fmt::Display;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::ProjectPolicyId;
use tracing::info;
use uuid::Uuid;

use crate::model::ProjectPolicy;
use crate::repo::project_policy::{ProjectPolicyRecord, ProjectPolicyRepo};
use crate::repo::RepoError;

#[derive(Debug, Clone)]
pub enum ProjectPolicyError {
    Internal(String),
}

impl ProjectPolicyError {
    pub fn internal<T: Display>(error: T) -> Self {
        ProjectPolicyError::Internal(error.to_string())
    }
}

impl From<RepoError> for ProjectPolicyError {
    fn from(error: RepoError) -> Self {
        ProjectPolicyError::internal(error)
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

#[async_trait]
impl ProjectPolicyService for ProjectPolicyServiceDefault {
    async fn create(&self, project_policy: &ProjectPolicy) -> Result<(), ProjectPolicyError> {
        info!("Create project policy {}", project_policy.id);
        let project_policy: ProjectPolicyRecord = project_policy.clone().into();
        self.project_policy_repo
            .create(&project_policy)
            .await
            .map_err(ProjectPolicyError::internal)
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
            .await
            .map_err(ProjectPolicyError::internal)
    }
}

#[derive(Default)]
pub struct ProjectPolicyServiceNoOp {}

#[async_trait]
impl ProjectPolicyService for ProjectPolicyServiceNoOp {
    async fn create(&self, _project_policy: &ProjectPolicy) -> Result<(), ProjectPolicyError> {
        Ok(())
    }

    async fn get_all(
        &self,
        _project_policy_ids: Vec<ProjectPolicyId>,
    ) -> Result<Vec<ProjectPolicy>, ProjectPolicyError> {
        Ok(vec![])
    }

    async fn get(
        &self,
        _project_policy_id: &ProjectPolicyId,
    ) -> Result<Option<ProjectPolicy>, ProjectPolicyError> {
        Ok(None)
    }

    async fn delete(&self, _project_policy_id: &ProjectPolicyId) -> Result<(), ProjectPolicyError> {
        Ok(())
    }
}
