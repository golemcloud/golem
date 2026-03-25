// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::DeployValidationError;
use super::deployment_context::DeploymentContext;
use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::deployment::{DeployRepoError, DeploymentRevisionCreationRecord};
use crate::services::agent_secret::{AgentSecretError, AgentSecretService};
use crate::services::component::{ComponentError, ComponentService};
use crate::services::deployment::deploy_validation_error::format_validation_errors;
use crate::services::environment::{EnvironmentError, EnvironmentService};
use crate::services::http_api_deployment::{HttpApiDeploymentError, HttpApiDeploymentService};
use crate::services::mcp_deployment::{McpDeploymentError, McpDeploymentService};
use crate::services::registry_change_notifier::RegistryChangeNotifier;
use crate::services::resource_definition::{ResourceDefinitionError, ResourceDefinitionService};
use crate::services::security_scheme::SecuritySchemeService;
use futures::TryFutureExt;
use golem_common::model::agent::DeployedRegisteredAgentType;
use golem_common::model::deployment::{CurrentDeployment, DeploymentRevision, DeploymentRollback};
use golem_common::model::diff;
use golem_common::model::environment::Environment;
use golem_common::model::security_scheme::SecuritySchemeName;
use golem_common::model::{
    deployment::{Deployment, DeploymentCreation},
    environment::EnvironmentId,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::EnvironmentAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use std::collections::HashMap;
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
    McpDeploymentError,
    AgentSecretError,
    ResourceDefinitionError
);

pub struct DeploymentWriteService {
    environment_service: Arc<EnvironmentService>,
    deployment_repo: Arc<dyn DeploymentRepo>,
    component_service: Arc<ComponentService>,
    http_api_deployment_service: Arc<HttpApiDeploymentService>,
    mcp_deployment_service: Arc<McpDeploymentService>,
    agent_secrets_service: Arc<AgentSecretService>,
    registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
    security_scheme_service: Arc<SecuritySchemeService>,
    resource_definition_service: Arc<ResourceDefinitionService>,
}

impl DeploymentWriteService {
    pub fn new(
        environment_service: Arc<EnvironmentService>,
        deployment_repo: Arc<dyn DeploymentRepo>,
        component_service: Arc<ComponentService>,
        http_api_deployment_service: Arc<HttpApiDeploymentService>,
        mcp_deployment_service: Arc<McpDeploymentService>,
        agent_secrets_service: Arc<AgentSecretService>,
        registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
        security_scheme_service: Arc<SecuritySchemeService>,
        resource_definition_service: Arc<ResourceDefinitionService>,
    ) -> DeploymentWriteService {
        Self {
            environment_service,
            deployment_repo,
            component_service,
            http_api_deployment_service,
            mcp_deployment_service,
            agent_secrets_service,
            registry_change_notifier,
            security_scheme_service,
            resource_definition_service,
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

        let (
            components,
            http_api_deployments,
            mcp_deployments,
            agent_secrets_in_environment,
            resource_definitions_in_environment,
        ) = tokio::try_join!(
            self.component_service
                .list_staged_components_for_environment(&environment, auth)
                .map_err(DeploymentWriteError::from),
            self.http_api_deployment_service
                .list_staged_for_environment(&environment, auth)
                .map_err(DeploymentWriteError::from),
            self.mcp_deployment_service
                .list_staged_for_environment(&environment, auth)
                .map_err(DeploymentWriteError::from),
            self.agent_secrets_service
                .list_in_fetched_environment(&environment, auth)
                .map_err(DeploymentWriteError::from),
            self.resource_definition_service
                .list_in_fetched_environment(&environment, auth)
                .map_err(DeploymentWriteError::from),
        )?;

        tracing::info!(
            "Fetched staged deployment data for environment: {environment_id}, components: {}, http api deployments: {}, mcp deployments: {}, agent_secrets: {}, resource_definitions: {}",
            components.len(),
            http_api_deployments.len(),
            mcp_deployments.len(),
            agent_secrets_in_environment.len(),
            resource_definitions_in_environment.len(),
        );

        let account_id = environment.owner_account_id;
        let deployment_context = DeploymentContext::new(
            environment,
            components,
            http_api_deployments,
            mcp_deployments,
        )?;

        {
            let actual_hash = deployment_context.hash();
            if data.expected_deployment_hash != deployment_context.hash() {
                return Err(DeploymentWriteError::DeploymentHashMismatch {
                    requested_hash: data.expected_deployment_hash,
                    actual_hash,
                });
            }
        }

        let mut errors = Vec::new();

        let compiled_routes = deployment_context.compile_http_api_routes(&mut errors);

        let security_schemes_list = self
            .security_scheme_service
            .get_security_schemes_in_environment(environment_id, auth)
            .await
            .unwrap_or_default();

        let security_schemes_map: HashMap<
            SecuritySchemeName,
            golem_service_base::custom_api::SecuritySchemeDetails,
        > = security_schemes_list
            .into_iter()
            .map(|s| {
                let details = golem_service_base::custom_api::SecuritySchemeDetails {
                    id: s.id,
                    name: s.name.clone(),
                    provider_type: s.provider_type,
                    client_id: s.client_id,
                    client_secret: s.client_secret,
                    redirect_url: s.redirect_url,
                    scopes: s.scopes,
                };
                (s.name, details)
            })
            .collect();

        let compiled_mcps = deployment_context.compile_mcp_deployments(
            account_id,
            next_deployment_revision,
            &security_schemes_map,
            &mut errors,
        );

        let (new_agent_secrets, updated_agent_secrets) = deployment_context
            .deployment_agent_secret_creations_and_updates(
                agent_secrets_in_environment,
                data.agent_secret_defaults,
                &mut errors,
            );

        let new_resource_definitions = deployment_context.deployment_resource_definition_creations(
            resource_definitions_in_environment,
            data.quota_resource_defaults,
            &mut errors,
        );

        if !errors.is_empty() {
            return Err(DeploymentWriteError::DeploymentValidationFailed(errors));
        }

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
            deployment_context.mcp_deployments.into_values().collect(),
            compiled_routes,
            compiled_mcps,
            deployment_context
                .registered_agent_types
                .into_values()
                .map(DeployedRegisteredAgentType::from)
                .collect(),
            new_agent_secrets,
            updated_agent_secrets,
            new_resource_definitions,
            auth.account_id(),
        )?;

        let ext_revision = self
            .deployment_repo
            .deploy(record, deployment_context.environment.version_check)
            .await
            .map_err(|err| match err {
                DeployRepoError::AgentSecretConflict { path } => {
                    tracing::warn!(
                        "Failing deployment due to secret conflict for path {}",
                        path.join(".")
                    );
                    DeploymentWriteError::ConcurrentDeployment
                }
                DeployRepoError::ConcurrentModification => {
                    DeploymentWriteError::ConcurrentDeployment
                }
                DeployRepoError::VersionAlreadyExists { version } => {
                    DeploymentWriteError::VersionAlreadyExists { version }
                }
                other => other.into(),
            })?;

        let deployment: CurrentDeployment = ext_revision.try_into()?;

        self.registry_change_notifier.signal_new_events_available();

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

        let revision_record = self
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
            })?;

        let current_deployment: CurrentDeployment = revision_record
            .into_model(target_deployment.version, target_deployment.deployment_hash)?;

        self.registry_change_notifier.signal_new_events_available();

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
