// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::deployment::{DeployRepoError, DeployValidationError};
use crate::repo::model::hash::SqlBlake3Hash;
use crate::services::environment::{EnvironmentError, EnvironmentService};
use golem_common::model::deployment::{DeploymentPlan, DeploymentRevision};
use golem_common::{
    SafeDisplay, error_forwarding,
    model::{
        deployment::{Deployment, DeploymentCreation},
        environment::EnvironmentId,
    },
};
use golem_service_base::model::auth::EnvironmentAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum DeploymentError {
    #[error("Parent environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Deployment {0} not found in the environment")]
    DeploymentNotFound(DeploymentRevision),
    #[error("Concurrent deployment attempt")]
    ConcurrentDeployment,
    #[error("Provided deployment version {version} already exists in this environment")]
    VersionAlreadyExists { version: String },
    #[error("Deployment validation failed:\n{errors}", errors=format_validation_errors(.0.as_slice()))]
    DeploymentValidationFailed(Vec<DeployValidationError>),
    #[error(
        "Deployment hash mismatch: requested hash: {requested_hash:?}, actual hash: {actual_hash:?}"
    )]
    DeploymentHashMismatch {
        requested_hash: SqlBlake3Hash,
        actual_hash: SqlBlake3Hash,
    },
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for DeploymentError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::DeploymentNotFound(_) => self.to_string(),
            Self::DeploymentHashMismatch { .. } => self.to_string(),
            Self::DeploymentValidationFailed(_) => self.to_string(),
            Self::ConcurrentDeployment => self.to_string(),
            Self::VersionAlreadyExists { .. } => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    DeploymentError,
    RepoError,
    EnvironmentError,
    DeployRepoError
);

fn format_validation_errors(errors: &[DeployValidationError]) -> String {
    errors
        .iter()
        .map(|err| format!("{err}"))
        .collect::<Vec<_>>()
        .join(",\n")
}

pub struct DeploymentService {
    environment_service: Arc<EnvironmentService>,
    deployment_repo: Arc<dyn DeploymentRepo>,
}

impl DeploymentService {
    pub fn new(
        environment_service: Arc<EnvironmentService>,
        deployment_repo: Arc<dyn DeploymentRepo>,
    ) -> DeploymentService {
        Self {
            environment_service,
            deployment_repo,
        }
    }

    pub async fn create_deployment(
        &self,
        environment_id: &EnvironmentId,
        new_deployment: DeploymentCreation,
        auth: &AuthCtx,
    ) -> Result<Deployment, DeploymentError> {
        let environment = self
            .environment_service
            .get(environment_id, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    DeploymentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_shares,
            EnvironmentAction::DeployEnvironment,
        )?;

        // Validation of the deployment is done as part of the repo.
        let deployment: Deployment = self
            .deployment_repo
            .deploy(
                &auth.account_id().0,
                &environment_id.0,
                new_deployment
                    .current_deployment_revision
                    .map(|cdr| cdr.into()),
                new_deployment.version,
                new_deployment.expected_deployment_hash.into_blake3().into(),
            )
            .await
            .map_err(|err| match err {
                DeployRepoError::ConcurrentModification => DeploymentError::ConcurrentDeployment,
                DeployRepoError::VersionAlreadyExists { version } => {
                    DeploymentError::VersionAlreadyExists { version }
                }
                DeployRepoError::ValidationErrors(validation_errors) => {
                    DeploymentError::DeploymentValidationFailed(validation_errors)
                }
                DeployRepoError::DeploymentHashMismatch {
                    requested_hash,
                    actual_hash,
                } => DeploymentError::DeploymentHashMismatch {
                    requested_hash,
                    actual_hash,
                },
                other => other.into(),
            })?
            .into();

        Ok(deployment)
    }

    pub async fn list_deployments(
        &self,
        environment_id: &EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<Deployment>, DeploymentError> {
        let environment = self
            .environment_service
            .get(environment_id, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    DeploymentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_shares,
            EnvironmentAction::ViewDeployment,
        )?;

        let deployments = self
            .deployment_repo
            .list_deployment_revisions(&environment_id.0)
            .await?
            .into_iter()
            .map(Deployment::from)
            .collect();

        Ok(deployments)
    }

    pub async fn get_current_deployment_plan(
        &self,
        environment_id: &EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<DeploymentPlan, DeploymentError> {
        let environment = self
            .environment_service
            .get(environment_id, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    DeploymentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_shares,
            EnvironmentAction::ViewDeploymentPlan,
        )?;

        let summary: DeploymentPlan = self
            .deployment_repo
            .get_staged_identity(&environment_id.0)
            .await?
            .into();

        Ok(summary)
    }

    pub async fn get_deployed_deployment_summary(
        &self,
        environment_id: &EnvironmentId,
        deployment_revision: DeploymentRevision,
        auth: &AuthCtx,
    ) -> Result<DeploymentPlan, DeploymentError> {
        let environment = self
            .environment_service
            .get(environment_id, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    DeploymentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_shares,
            EnvironmentAction::ViewDeploymentPlan,
        )?;

        let summary: DeploymentPlan = self
            .deployment_repo
            .get_deployment_identity(&environment_id.0, Some(deployment_revision.into()))
            .await?
            .ok_or(DeploymentError::DeploymentNotFound(deployment_revision))?
            .identity
            .into();

        Ok(summary)
    }
}
