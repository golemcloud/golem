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

mod rib;
mod routes;
mod write;

pub use self::routes::{DeployedRoutesError, DeployedRoutesService};
pub use self::write::DeploymentWriteService;

use super::component::ComponentError;
use super::http_api_definition::HttpApiDefinitionError;
use super::http_api_deployment::HttpApiDeploymentError;
use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::deployment::DeployRepoError;
use crate::services::environment::{EnvironmentError, EnvironmentService};
use ::rib::RibCompilationError;
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::{DeploymentPlan, DeploymentRevision, DeploymentSummary};
use golem_common::model::diff;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::Environment;
use golem_common::model::http_api_definition::HttpApiDefinitionName;
use golem_common::{
    SafeDisplay, error_forwarding,
    model::{deployment::Deployment, environment::EnvironmentId},
};
use golem_service_base::model::auth::EnvironmentAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use std::sync::Arc;
use golem_common::model::agent::RegisteredAgentType;

#[derive(Debug, thiserror::Error)]
pub enum DeploymentError {
    #[error("Parent environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Deployment {0} not found in the environment")]
    DeploymentNotFound(DeploymentRevision),
    #[error("Concurrent deployment attempt")]
    ConcurrentDeployment,
    #[error("Requested deployment would not have any changes compared to current deployment")]
    NoOpDeployment,
    #[error("Provided deployment version {version} already exists in this environment")]
    VersionAlreadyExists { version: String },
    #[error("Deployment validation failed:\n{errors}", errors=format_validation_errors(.0.as_slice()))]
    DeploymentValidationFailed(Vec<DeployValidationError>),
    #[error(
        "Deployment hash mismatch: requested hash: {requested_hash}, actual hash: {actual_hash}"
    )]
    DeploymentHashMismatch {
        requested_hash: diff::Hash,
        actual_hash: diff::Hash,
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
            Self::NoOpDeployment => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    DeploymentError,
    RepoError,
    EnvironmentError,
    DeployRepoError,
    ComponentError,
    HttpApiDefinitionError,
    HttpApiDeploymentError,
);

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum DeployValidationError {
    #[error(
        "Http api definition {missing_http_api_definition} requested by http api deployment {http_api_deployment_domain} is not part of the deployment"
    )]
    HttpApiDeploymentMissingHttpApiDefinition {
        http_api_deployment_domain: Domain,
        missing_http_api_definition: HttpApiDefinitionName,
    },
    #[error("Invalid path pattern: {0}")]
    HttpApiDefinitionInvalidPathPattern(String),
    #[error("Invalid rib expression: {0}")]
    InvalidRibExpr(String),
    #[error("Failed rib compilation: {0}")]
    RibCompilationFailed(RibCompilationError),
    #[error("Invalid http cors binding expression: {0}")]
    InvalidHttpCorsBindingExpr(String),
    #[error("Component {0} not found in deployment")]
    ComponentNotFound(ComponentName),
    #[error("Agent type name {0} is provided by multiple components")]
    AmbiguousAgentTypeName(String),
}

impl SafeDisplay for DeployValidationError {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

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
    ) -> Self {
        Self {
            environment_service,
            deployment_repo,
        }
    }

    pub async fn get_latest_deployment_for_environment(
        &self,
        environment: &Environment,
        auth: &AuthCtx,
    ) -> Result<Option<Deployment>, DeploymentError> {
        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewDeployment,
        )?;

        let deployment: Option<Deployment> = self
            .deployment_repo
            .get_latest_revision(&environment.id.0)
            .await?
            .map(|r| r.into());

        Ok(deployment)
    }

    pub async fn list_deployments(
        &self,
        environment_id: &EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<Deployment>, DeploymentError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    DeploymentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
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

    pub async fn get_deployment(
        &self,
        environment_id: &EnvironmentId,
        deployment_revision: DeploymentRevision,
        auth: &AuthCtx,
    ) -> Result<Deployment, DeploymentError> {
        let (deployment, _) = self
            .get_deployment_and_environment(environment_id, deployment_revision, auth)
            .await?;
        Ok(deployment)
    }

    pub async fn get_deployment_and_environment(
        &self,
        environment_id: &EnvironmentId,
        deployment_revision: DeploymentRevision,
        auth: &AuthCtx,
    ) -> Result<(Deployment, Environment), DeploymentError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    DeploymentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewDeployment,
        )
        .map_err(|_| DeploymentError::DeploymentNotFound(deployment_revision))?;

        let deployment: Deployment = self
            .deployment_repo
            .get_deployed_revision(&environment_id.0, deployment_revision.into())
            .await?
            .ok_or(DeploymentError::DeploymentNotFound(deployment_revision))?
            .into();

        Ok((deployment, environment))
    }

    pub async fn get_current_deployment_plan(
        &self,
        environment_id: &EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<DeploymentPlan, DeploymentError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    DeploymentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewDeploymentPlan,
        )?;

        let staged_revision = self
            .deployment_repo
            .get_next_revision_number(&environment_id.0)
            .await?
            .map(|r| DeploymentRevision(r as u64));

        let summary: DeploymentPlan = self
            .deployment_repo
            .get_staged_identity(&environment_id.0)
            .await?
            .into_plan(staged_revision);

        Ok(summary)
    }

    pub async fn get_deployed_deployment_summary(
        &self,
        environment_id: &EnvironmentId,
        deployment_revision: DeploymentRevision,
        auth: &AuthCtx,
    ) -> Result<DeploymentSummary, DeploymentError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    DeploymentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewDeploymentPlan,
        )?;

        let summary: DeploymentSummary = self
            .deployment_repo
            .get_deployment_identity(&environment_id.0, Some(deployment_revision.into()))
            .await?
            .ok_or(DeploymentError::DeploymentNotFound(deployment_revision))?
            .identity
            .into();

        Ok(summary)
    }

    pub async fn get_deployed_agent_type(
        &self,
        environment_id: &EnvironmentId,
        agent_type_name: String
    ) -> Result<Option<RegisteredAgentType>, DeploymentError> {
        let agent_type = self
            .deployment_repo
            .get_deployed_agent_type(&environment_id.0, &agent_type_name)
            .await?
            .map(|r| r.into());

        Ok(agent_type)
    }

    pub async fn list_deployed_agent_type(
        &self,
        environment_id: &EnvironmentId,
    ) -> Result<Vec<RegisteredAgentType>, DeploymentError> {
        let agent_types = self
            .deployment_repo
            .list_deployed_agent_types(&environment_id.0)
            .await?
            .into_iter()
            .map(|r| r.into())
            .collect();

        Ok(agent_types)
    }

    pub async fn list_deployment_agent_types(
        &self,
        environment_id: &EnvironmentId,
        deployment_revision: DeploymentRevision,
        auth: &AuthCtx
    ) -> Result<Vec<RegisteredAgentType>, DeploymentError> {
        let (_, environment) = self
            .get_deployment_and_environment(environment_id, deployment_revision, auth)
            .await?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewAgentTypes,
        )?;

        let agent_types = self
            .deployment_repo
            .list_deployment_agent_types(&environment_id.0, deployment_revision.into())
            .await?
            .into_iter()
            .map(|r| r.into())
            .collect();

        Ok(agent_types)

    }
}
