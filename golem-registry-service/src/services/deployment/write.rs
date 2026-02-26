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

use super::deployment_context::DeploymentContext;
use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::deployment::{DeployRepoError, DeploymentRevisionCreationRecord};
use crate::services::component::{ComponentError, ComponentService};
use crate::services::deployment::route_compilation::render_http_method;
use crate::services::environment::{EnvironmentError, EnvironmentService};
use crate::services::http_api_deployment::{HttpApiDeploymentError, HttpApiDeploymentService};
use crate::services::mcp_deployment::{McpDeploymentError, McpDeploymentService};
use futures::TryFutureExt;
use golem_common::model::agent::{AgentTypeName, DeployedRegisteredAgentType, HttpMethod};
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::{CurrentDeployment, DeploymentRevision, DeploymentRollback};
use golem_common::model::diff;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::Environment;
use golem_common::model::{
    deployment::{Deployment, DeploymentCreation},
    environment::EnvironmentId,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::custom_api::PathSegment;
use golem_service_base::model::auth::EnvironmentAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum DeploymentWriteError {
    #[error("Parent environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Deployment {0} not found in the environment")]
    DeploymentNotFound(DeploymentRevision),
    #[error("Environment has not yet been deployed")]
    EnvironmentNotYetDeployed,
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

impl SafeDisplay for DeploymentWriteError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::DeploymentNotFound(_) => self.to_string(),
            Self::EnvironmentNotYetDeployed => self.to_string(),
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
    DeploymentWriteError,
    RepoError,
    EnvironmentError,
    DeployRepoError,
    ComponentError,
    HttpApiDeploymentError,
    McpDeploymentError
);

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum DeployValidationError {
    #[error(
        "Agent type {missing_agent_type} requested by http api deployment {http_api_deployment_domain} is not part of the deployment"
    )]
    HttpApiDeploymentMissingAgentType {
        http_api_deployment_domain: Domain,
        missing_agent_type: AgentTypeName,
    },
    #[error(
        "Agent type {missing_agent_type} requested by mcp deployment {mcp_deployment_domain} is not part of the deployment"
    )]
    McpDeploymentMissingAgentType {
        mcp_deployment_domain: Domain,
        missing_agent_type: AgentTypeName,
    },
    #[error("Invalid path pattern: {0}")]
    HttpApiDefinitionInvalidPathPattern(String),
    #[error("Invalid http cors binding expression: {0}")]
    InvalidHttpCorsBindingExpr(String),
    #[error("Component {0} not found in deployment")]
    ComponentNotFound(ComponentName),
    #[error("Agent type name {0} is provided by multiple components")]
    AmbiguousAgentTypeName(AgentTypeName),
    #[error("No security scheme configured for agent {0} but agent has methods that require auth")]
    NoSecuritySchemeConfigured(AgentTypeName),
    #[error(
        "Method {agent_method} of agent {agent_type} used by http api at {method} {domain}/{path} is invalid: {error}"
    )]
    HttpApiDeploymentAgentMethodInvalid {
        domain: Domain,
        method: String,
        path: String,
        agent_type: AgentTypeName,
        agent_method: String,
        error: String,
    },
    #[error(
        "Method constructor of agent {agent_type} mounted by by http api at {domain}/{path} is invalid: {error}"
    )]
    HttpApiDeploymentAgentConstructorInvalid {
        domain: Domain,
        path: String,
        agent_type: AgentTypeName,
        error: String,
    },
    #[error(
        "Agent type {agent_type} is deployed to multiple domains. An agent type can only be deployed to one domain at a time"
    )]
    HttpApiDeploymentMultipleDeploymentsForAgentType { agent_type: AgentTypeName },
    #[error("Agent type {agent_type} is deployed to a domain but does not have http mount details")]
    HttpApiDeploymentAgentTypeMissingHttpMount { agent_type: AgentTypeName },
    #[error(
        "Agent type {agent_type} uses forbidden patterns in its webhook. Variable and catchall segments are not allowed in webhook urls"
    )]
    HttpApiDeploymentInvalidAgentWebhookSegmentType { agent_type: AgentTypeName },
    #[error(
        "Agent type {agent_type} has an invalid final webhook url {url}. (Protocol is a placeholder)"
    )]
    HttpApiDeploymentInvalidWebhookUrl {
        agent_type: AgentTypeName,
        url: String,
    },
    #[error("Overriding security scheme is only allowed if the environment level option is set")]
    SecurityOverrideDisabled,
    #[error("Http api for domain {domain} has multiple routes for pattern {rendered_method} {rendered_path}", rendered_method = render_http_method(method), rendered_path = itertools::join(path.iter().map(|p| p.to_string()), "/"))]
    RouteIsAmbiguous {
        domain: Domain,
        method: HttpMethod,
        path: Vec<PathSegment>,
    },
    #[error("Invalid http method: {method:?}")]
    InvalidHttpMethod { method: HttpMethod },
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

pub struct DeploymentWriteService {
    environment_service: Arc<EnvironmentService>,
    deployment_repo: Arc<dyn DeploymentRepo>,
    component_service: Arc<ComponentService>,
    http_api_deployment_service: Arc<HttpApiDeploymentService>,
    mcp_deployment_service: Arc<McpDeploymentService>,
}

impl DeploymentWriteService {
    pub fn new(
        environment_service: Arc<EnvironmentService>,
        deployment_repo: Arc<dyn DeploymentRepo>,
        component_service: Arc<ComponentService>,
        http_api_deployment_service: Arc<HttpApiDeploymentService>,
        mcp_deployment_service: Arc<McpDeploymentService>,
    ) -> DeploymentWriteService {
        Self {
            environment_service,
            deployment_repo,
            component_service,
            http_api_deployment_service,
            mcp_deployment_service,
        }
    }

    pub async fn create_deployment(
        &self,
        environment_id: EnvironmentId,
        data: DeploymentCreation,
        auth: &AuthCtx,
    ) -> Result<CurrentDeployment, DeploymentWriteError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    DeploymentWriteError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::DeployEnvironment,
        )?;

        if data.current_revision
            != environment
                .current_deployment
                .as_ref()
                .map(|cd| cd.revision)
        {
            return Err(DeploymentWriteError::ConcurrentDeployment);
        };

        if let Some(current_deployment_hash) = environment
            .current_deployment
            .as_ref()
            .map(|ld| ld.deployment_hash)
            && data.expected_deployment_hash == current_deployment_hash
        {
            return Err(DeploymentWriteError::NoOpDeployment);
        }

        let latest_deployment = self
            .get_latest_deployment_for_environment(&environment, auth)
            .await?;

        let next_deployment_revision = latest_deployment
            .as_ref()
            .map(|ld| ld.revision.next())
            .transpose()?
            .unwrap_or(DeploymentRevision::INITIAL);

        tracing::info!("Creating deployment for environment: {environment_id}");

        let (components, http_api_deployments, mcp_deployments) = tokio::try_join!(
            self.component_service
                .list_staged_components_for_environment(&environment, auth)
                .map_err(DeploymentWriteError::from),
            self.http_api_deployment_service
                .list_staged_for_environment(&environment, auth)
                .map_err(DeploymentWriteError::from),
            self.mcp_deployment_service
                .list_staged_for_environment(&environment, auth)
                .map_err(DeploymentWriteError::from),
        )?;

        tracing::info!(
            "Fetched staged deployment data for environment: {environment_id}, components: {}, http api deployments: {}, mcp deployments: {}",
            components.len(),
            http_api_deployments.len(),
            mcp_deployments.len()
        );

        let account_id = environment.owner_account_id;
        let deployment_context = DeploymentContext::new(
            environment,
            components,
            http_api_deployments,
            mcp_deployments.clone(),
        );

        {
            let actual_hash = deployment_context.hash();
            if data.expected_deployment_hash != deployment_context.hash() {
                return Err(DeploymentWriteError::DeploymentHashMismatch {
                    requested_hash: data.expected_deployment_hash,
                    actual_hash,
                });
            }
        }

        let registered_agent_types = deployment_context.extract_registered_agent_types()?;
        let compiled_routes =
            deployment_context.compile_http_api_routes(&registered_agent_types)?;

        let compiled_mcps = deployment_context.compile_mcp_deployments(
            &registered_agent_types,
            account_id,
            next_deployment_revision,
        )?;

        let record = DeploymentRevisionCreationRecord::from_model(
            environment_id,
            next_deployment_revision,
            data.version,
            data.expected_deployment_hash,
            deployment_context.components.into_values().collect(),
            deployment_context
                .http_api_deployments
                .into_values()
                .collect(),
            mcp_deployments,
            compiled_routes,
            compiled_mcps,
            registered_agent_types
                .into_values()
                .map(DeployedRegisteredAgentType::from)
                .collect(),
        );

        let deployment: CurrentDeployment = self
            .deployment_repo
            .deploy(
                auth.account_id().0,
                record,
                deployment_context.environment.version_check,
            )
            .await
            .map_err(|err| match err {
                DeployRepoError::ConcurrentModification => {
                    DeploymentWriteError::ConcurrentDeployment
                }
                DeployRepoError::VersionAlreadyExists { version } => {
                    DeploymentWriteError::VersionAlreadyExists { version }
                }
                other => other.into(),
            })?
            .try_into()?;

        Ok(deployment)
    }

    pub async fn rollback_environment(
        &self,
        environment_id: EnvironmentId,
        payload: DeploymentRollback,
        auth: &AuthCtx,
    ) -> Result<CurrentDeployment, DeploymentWriteError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    DeploymentWriteError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::DeployEnvironment,
        )?;

        let current_deployment = environment
            .current_deployment
            .ok_or(DeploymentWriteError::EnvironmentNotYetDeployed)?;

        if payload.current_revision != current_deployment.revision {
            return Err(DeploymentWriteError::ConcurrentDeployment);
        }

        if current_deployment.deployment_revision == payload.deployment_revision {
            // environment is already at target version, nothing to do
            return Err(DeploymentWriteError::NoOpDeployment);
        }

        let target_deployment: Deployment = self
            .deployment_repo
            .get_deployment_revision(environment_id.0, payload.deployment_revision.into())
            .await?
            .ok_or(DeploymentWriteError::DeploymentNotFound(
                payload.deployment_revision,
            ))?
            .try_into()?;

        let current_deployment: CurrentDeployment = self
            .deployment_repo
            .set_current_deployment(
                auth.account_id().0,
                environment_id.0,
                payload.deployment_revision.into(),
            )
            .await
            .map_err(|e| match e {
                DeployRepoError::ConcurrentModification => {
                    DeploymentWriteError::ConcurrentDeployment
                }
                other => other.into(),
            })?
            .into_model(target_deployment.version, target_deployment.deployment_hash)?;

        Ok(current_deployment)
    }

    async fn get_latest_deployment_for_environment(
        &self,
        environment: &Environment,
        auth: &AuthCtx,
    ) -> Result<Option<Deployment>, DeploymentWriteError> {
        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewDeployment,
        )?;

        let deployment: Option<Deployment> = self
            .deployment_repo
            .get_latest_revision(environment.id.0)
            .await?
            .map(|r| r.try_into())
            .transpose()?;

        Ok(deployment)
    }
}
