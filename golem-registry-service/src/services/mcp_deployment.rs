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

use super::environment::{EnvironmentError, EnvironmentService};
use crate::repo::mcp_deployment::McpDeploymentRepo;
use crate::repo::model::mcp_deployment::{McpDeploymentRepoError, McpDeploymentRevisionRecord};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_common::model::mcp_deployment::{
    McpDeployment, McpDeploymentCreation, McpDeploymentId, McpDeploymentUpdate,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError, EnvironmentAction};
use golem_service_base::repo::RepoError;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum McpDeploymentError {
    #[error("Environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("MCP deployment for id {0} not found")]
    McpDeploymentNotFound(McpDeploymentId),
    #[error("MCP deployment for domain {0} not found")]
    McpDeploymentByDomainNotFound(Domain),
    #[error("MCP deployment for domain {0} already exists in this environment")]
    McpDeploymentForDomainAlreadyExists(Domain),
    #[error("Concurrent update attempt")]
    ConcurrentUpdate,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for McpDeploymentError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::McpDeploymentNotFound(_) => self.to_string(),
            Self::McpDeploymentByDomainNotFound(_) => self.to_string(),
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::McpDeploymentForDomainAlreadyExists(_) => self.to_string(),
            Self::ConcurrentUpdate => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    McpDeploymentError,
    McpDeploymentRepoError,
    RepoError,
    EnvironmentError,
);

pub struct McpDeploymentService {
    mcp_deployment_repo: Arc<dyn McpDeploymentRepo>,
    environment_service: Arc<EnvironmentService>,
}

impl McpDeploymentService {
    pub fn new(
        mcp_deployment_repo: Arc<dyn McpDeploymentRepo>,
        environment_service: Arc<EnvironmentService>,
    ) -> Self {
        Self {
            mcp_deployment_repo,
            environment_service,
        }
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        data: McpDeploymentCreation,
        auth: &AuthCtx,
    ) -> Result<McpDeployment, McpDeploymentError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(id) => {
                    McpDeploymentError::ParentEnvironmentNotFound(id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::CreateHttpApiDeployment,
        )?;

        let id = McpDeploymentId::new();
        let record =
            McpDeploymentRevisionRecord::creation(id, data.domain.clone(), auth.account_id());

        let stored_mcp_deployment: McpDeployment = self
            .mcp_deployment_repo
            .create(environment_id.0, &data.domain.0, record)
            .await
            .map_err(|err| match err {
                McpDeploymentRepoError::ConcurrentModification => {
                    McpDeploymentError::ConcurrentUpdate
                }
                McpDeploymentRepoError::McpDeploymentViolatesUniqueness => {
                    McpDeploymentError::McpDeploymentForDomainAlreadyExists(data.domain)
                }
                other => other.into(),
            })?
            .try_into()?;

        Ok(stored_mcp_deployment)
    }

    pub async fn update(
        &self,
        mcp_deployment_id: McpDeploymentId,
        update: McpDeploymentUpdate,
        auth: &AuthCtx,
    ) -> Result<McpDeployment, McpDeploymentError> {
        let mut mcp_deployment: McpDeployment = self
            .mcp_deployment_repo
            .get_staged_by_id(mcp_deployment_id.0)
            .await?
            .ok_or(McpDeploymentError::McpDeploymentNotFound(mcp_deployment_id))?
            .try_into()?;

        let environment = self
            .environment_service
            .get(mcp_deployment.environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    McpDeploymentError::McpDeploymentNotFound(mcp_deployment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDeployment,
        )
        .map_err(|_| McpDeploymentError::McpDeploymentNotFound(mcp_deployment_id))?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::UpdateHttpApiDeployment,
        )?;

        if update.current_revision != mcp_deployment.revision {
            Err(McpDeploymentError::ConcurrentUpdate)?
        };

        mcp_deployment.revision = mcp_deployment.revision.next()?;
        if let Some(domain) = update.domain {
            mcp_deployment.domain = domain;
        };

        let record = McpDeploymentRevisionRecord::from_model(mcp_deployment);

        let stored_mcp_deployment: McpDeployment = self
            .mcp_deployment_repo
            .update(record)
            .await
            .map_err(|err| match err {
                McpDeploymentRepoError::ConcurrentModification => {
                    McpDeploymentError::ConcurrentUpdate
                }
                other => other.into(),
            })?
            .try_into()?;

        Ok(stored_mcp_deployment)
    }

    pub async fn delete(
        &self,
        mcp_deployment_id: McpDeploymentId,
        current_revision: golem_common::model::mcp_deployment::McpDeploymentRevision,
        auth: &AuthCtx,
    ) -> Result<(), McpDeploymentError> {
        let mcp_deployment: McpDeployment = self
            .mcp_deployment_repo
            .get_staged_by_id(mcp_deployment_id.0)
            .await?
            .ok_or(McpDeploymentError::McpDeploymentNotFound(mcp_deployment_id))?
            .try_into()?;

        let environment = self
            .environment_service
            .get(mcp_deployment.environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    McpDeploymentError::McpDeploymentNotFound(mcp_deployment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDeployment,
        )
        .map_err(|_| McpDeploymentError::McpDeploymentNotFound(mcp_deployment_id))?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::DeleteHttpApiDeployment,
        )?;

        if current_revision != mcp_deployment.revision {
            Err(McpDeploymentError::ConcurrentUpdate)?
        };

        self.mcp_deployment_repo
            .delete(
                auth.account_id().0,
                mcp_deployment_id.0,
                current_revision.next()?.into(),
            )
            .await
            .map_err(|err| match err {
                McpDeploymentRepoError::ConcurrentModification => {
                    McpDeploymentError::ConcurrentUpdate
                }
                other => other.into(),
            })?;

        Ok(())
    }

    pub async fn get_staged(
        &self,
        mcp_deployment_id: McpDeploymentId,
        auth: &AuthCtx,
    ) -> Result<McpDeployment, McpDeploymentError> {
        let mcp_deployment: McpDeployment = self
            .mcp_deployment_repo
            .get_staged_by_id(mcp_deployment_id.0)
            .await?
            .ok_or(McpDeploymentError::McpDeploymentNotFound(mcp_deployment_id))?
            .try_into()?;

        let environment = self
            .environment_service
            .get(mcp_deployment.environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    McpDeploymentError::McpDeploymentNotFound(mcp_deployment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDeployment,
        )
        .map_err(|_| McpDeploymentError::McpDeploymentNotFound(mcp_deployment_id))?;

        Ok(mcp_deployment)
    }

    pub async fn list_staged(
        &self,
        environment_id: EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<McpDeployment>, McpDeploymentError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    McpDeploymentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        self.list_staged_for_environment(&environment, auth).await
    }

    pub async fn list_staged_for_environment(
        &self,
        environment: &Environment,
        auth: &AuthCtx,
    ) -> Result<Vec<McpDeployment>, McpDeploymentError> {
        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDeployment,
        )?;

        let mcp_deployments: Vec<McpDeployment> = self
            .mcp_deployment_repo
            .list_staged(environment.id.0)
            .await?
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<Vec<_>, _>>()?;

        Ok(mcp_deployments)
    }

    pub async fn get_staged_by_domain(
        &self,
        environment_id: EnvironmentId,
        domain: &Domain,
        auth: &AuthCtx,
    ) -> Result<McpDeployment, McpDeploymentError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    McpDeploymentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDeployment,
        )
        .map_err(|_| McpDeploymentError::McpDeploymentByDomainNotFound(domain.clone()))?;

        let mcp_deployment: McpDeployment = self
            .mcp_deployment_repo
            .get_staged_by_domain(environment_id.0, &domain.0)
            .await?
            .ok_or(McpDeploymentError::McpDeploymentByDomainNotFound(
                domain.clone(),
            ))?
            .try_into()?;

        Ok(mcp_deployment)
    }

    pub async fn get_revision(
        &self,
        mcp_deployment_id: McpDeploymentId,
        revision: golem_common::model::mcp_deployment::McpDeploymentRevision,
        auth: &AuthCtx,
    ) -> Result<McpDeployment, McpDeploymentError> {
        let mcp_deployment: McpDeployment = self
            .mcp_deployment_repo
            .get_staged_by_id(mcp_deployment_id.0)
            .await?
            .ok_or(McpDeploymentError::McpDeploymentNotFound(mcp_deployment_id))?
            .try_into()?;

        if mcp_deployment.revision != revision {
            return Err(McpDeploymentError::McpDeploymentNotFound(mcp_deployment_id));
        }

        let environment = self
            .environment_service
            .get(mcp_deployment.environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    McpDeploymentError::McpDeploymentNotFound(mcp_deployment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDeployment,
        )
        .map_err(|_| McpDeploymentError::McpDeploymentNotFound(mcp_deployment_id))?;

        Ok(mcp_deployment)
    }

    pub async fn get_in_deployment_by_domain(
        &self,
        environment_id: EnvironmentId,
        _deployment_revision: golem_common::model::deployment::DeploymentRevision,
        domain: &Domain,
        auth: &AuthCtx,
    ) -> Result<McpDeployment, McpDeploymentError> {
        // For now, MCP deployments don't have deployment-scoped versions
        // Just return the staged version by domain
        self.get_staged_by_domain(environment_id, domain, auth)
            .await
    }

    pub async fn list_in_deployment(
        &self,
        environment_id: EnvironmentId,
        _deployment_revision: golem_common::model::deployment::DeploymentRevision,
        auth: &AuthCtx,
    ) -> Result<Vec<McpDeployment>, McpDeploymentError> {
        // For now, MCP deployments don't have deployment-scoped versions
        // Just return all staged deployments
        self.list_staged(environment_id, auth).await
    }
}
