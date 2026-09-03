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
use super::authorize_environment_permission;
use super::deployment_context::DeploymentContext;
use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::deployment::{DeployRepoError, DeploymentRevisionCreationRecord};
use crate::services::agent_secret::{AgentSecretError, AgentSecretService};
use crate::services::component::{ComponentError, ComponentService};
use crate::services::deployment::deploy_validation_error::format_validation_errors;
use crate::services::environment::{EnvironmentError, EnvironmentService};
use crate::services::environment_tool_grant::{
    EnvironmentToolGrantError, EnvironmentToolGrantService,
};
use crate::services::http_api_deployment::{HttpApiDeploymentError, HttpApiDeploymentService};
use crate::services::mcp_deployment::{McpDeploymentError, McpDeploymentService};
use crate::services::registry_change_notifier::{
    RegistryChangeNotifier, RequiresNotificationSignalExt,
};
use crate::services::resource_definition::{ResourceDefinitionError, ResourceDefinitionService};
use crate::services::retry_policy::{RetryPolicyError, RetryPolicyService};
use crate::services::security_scheme::SecuritySchemeService;
use crate::services::tool_release::{ToolReleaseError, ToolReleaseService};
use futures::TryFutureExt;
use golem_common::model::agent::{DeployedRegisteredAgentType, InitialAgentFileUpload};
use golem_common::model::card::EnvironmentVerb;
use golem_common::model::deployment::{CurrentDeployment, DeploymentRevision, DeploymentRollback};
use golem_common::model::diff;
use golem_common::model::environment::Environment;
use golem_common::model::security_scheme::SecuritySchemeName;
use golem_common::model::{
    deployment::{Deployment, DeploymentCreation},
    environment::EnvironmentId,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::replayable_stream::ReplayableStream;
use golem_service_base::repo::RepoError;
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::NamedTempFile;

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
    ResourceDefinitionError,
    RetryPolicyError,
    EnvironmentToolGrantError,
    ToolReleaseError
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
    retry_policy_service: Arc<RetryPolicyService>,
    environment_tool_grant_service: Arc<EnvironmentToolGrantService>,
    tool_release_service: Arc<ToolReleaseService>,
    initial_agent_files_service: Arc<InitialAgentFilesService>,
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
        retry_policy_service: Arc<RetryPolicyService>,
        environment_tool_grant_service: Arc<EnvironmentToolGrantService>,
        tool_release_service: Arc<ToolReleaseService>,
        initial_agent_files_service: Arc<InitialAgentFilesService>,
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
            retry_policy_service,
            environment_tool_grant_service,
            tool_release_service,
            initial_agent_files_service,
        }
    }

    pub async fn upload_initial_agent_file(
        &self,
        environment_id: EnvironmentId,
        data: Arc<NamedTempFile>,
        auth: &AuthCtx,
    ) -> Result<InitialAgentFileUpload, DeploymentWriteError> {
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

        let size = data.length().await?;
        let stream = data.map_item(|item| item.map(|bytes| bytes.to_vec()));
        store_initial_agent_file_stream(
            self.initial_agent_files_service.as_ref(),
            &environment,
            stream,
            size,
            auth,
        )
        .await
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

        authorize_environment_permission(auth, &environment, EnvironmentVerb::Deploy)?;

        if data.current_revision
            != environment
                .current_deployment
                .as_ref()
                .map(|cd| cd.revision)
        {
            return Err(DeploymentWriteError::ConcurrentDeployment);
        };

        let deployment_hash_unchanged = environment
            .current_deployment
            .as_ref()
            .map(|ld| ld.deployment_hash)
            .is_some_and(|current_deployment_hash| {
                data.expected_deployment_hash == current_deployment_hash
            });

        let latest_deployment = self
            .get_latest_deployment_for_environment(&environment, &AuthCtx::System)
            .await?;

        let compatibility_check_enabled = environment.compatibility_check;

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
            retry_policies_in_environment,
        ) = tokio::try_join!(
            self.component_service
                .list_staged_components_for_environment(&environment, &AuthCtx::System)
                .map_err(DeploymentWriteError::from),
            self.http_api_deployment_service
                .list_staged_for_environment(&environment, &AuthCtx::System)
                .map_err(DeploymentWriteError::from),
            self.mcp_deployment_service
                .list_staged_for_environment(&environment, &AuthCtx::System)
                .map_err(DeploymentWriteError::from),
            self.agent_secrets_service
                .list_in_fetched_environment(&environment, &AuthCtx::System)
                .map_err(DeploymentWriteError::from),
            self.resource_definition_service
                .list_in_fetched_environment(&environment, &AuthCtx::System)
                .map_err(DeploymentWriteError::from),
            self.retry_policy_service
                .list_in_fetched_environment(&environment, &AuthCtx::System)
                .map_err(DeploymentWriteError::from),
        )?;

        tracing::info!(
            "Fetched staged deployment data for environment: {environment_id}, components: {}, http api deployments: {}, mcp deployments: {}, agent_secrets: {}, resource_definitions: {}, retry_policies: {}",
            components.len(),
            http_api_deployments.len(),
            mcp_deployments.len(),
            agent_secrets_in_environment.len(),
            resource_definitions_in_environment.len(),
            retry_policies_in_environment.len(),
        );

        let account_id = environment.owner_account_id;
        let remote_tool_references = data
            .remote_tools
            .iter()
            .map(|deployment| deployment.release.clone())
            .collect::<Vec<_>>();
        let resolved_remote_tools = self
            .environment_tool_grant_service
            .resolve_active_references_partial(&environment, &remote_tool_references, auth)
            .await?;
        let remote_tools = data
            .remote_tools
            .iter()
            .cloned()
            .zip(resolved_remote_tools)
            .collect::<Vec<_>>();
        let deployment_context = DeploymentContext::new(
            environment,
            components,
            http_api_deployments,
            mcp_deployments,
        )?;

        let mut errors = Vec::new();
        let mut warnings: Vec<super::DeployValidationWarning> = Vec::new();

        if data.replace_incompatible_agent_secrets && compatibility_check_enabled {
            errors.push(DeployValidationError::ResetOverrideRequiresCompatibilityCheckDisabled);
        }

        let compiled_routes =
            deployment_context.compile_http_api_routes(&mut errors, &mut warnings);

        let security_schemes_list = self
            .security_scheme_service
            .get_security_schemes_in_environment(environment_id, &AuthCtx::System)
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

        let mut compiled_tools = deployment_context.compile_tools_with_remote(
            next_deployment_revision,
            &remote_tools,
            &mut errors,
            &mut warnings,
        );

        let registered_tools_by_name = compiled_tools
            .registered_tools
            .iter()
            .filter_map(|tool| {
                tool.definition
                    .name()
                    .and_then(|name| golem_common::model::tool::ToolName::try_from(name).ok())
                    .map(|name| (name, tool.clone()))
            })
            .collect();
        let mut tool_releases = self.tool_release_service.prepare_publications(
            &deployment_context.environment,
            &registered_tools_by_name,
            &data.publish_tools,
            auth,
        )?;
        let publications_need_change = self
            .tool_release_service
            .publications_need_change(&mut tool_releases)
            .await?;
        let published_release_ids = tool_releases
            .iter()
            .map(|release| {
                (
                    release.tool_name.as_str(),
                    golem_common::model::tool_release::ToolReleaseId(release.tool_release_id),
                )
            })
            .collect::<HashMap<_, _>>();
        for tool in &mut compiled_tools.registered_tools {
            if let Some(release_id) = tool
                .definition
                .name()
                .and_then(|name| published_release_ids.get(name))
            {
                tool.release_id = Some(*release_id);
            }
        }
        for binding in &mut compiled_tools.agent_tool_bindings {
            if let Some(release_id) = published_release_ids.get(binding.tool_name.as_str()) {
                binding.release_id = Some(*release_id);
            }
        }

        let (new_agent_secrets, updated_agent_secrets, replaced_agent_secrets) = deployment_context
            .deployment_agent_secret_creations_and_updates(
                agent_secrets_in_environment,
                data.agent_secret_defaults,
                data.replace_incompatible_agent_secrets,
                &mut errors,
            );

        let new_resource_definitions = deployment_context.deployment_resource_definition_creations(
            resource_definitions_in_environment,
            data.quota_resource_defaults,
            &mut errors,
        );

        let new_retry_policies = deployment_context.deployment_retry_policy_creations(
            retry_policies_in_environment,
            data.retry_policy_defaults,
            auth.actor_account_id(),
            &mut errors,
        )?;

        if !errors.is_empty() {
            return Err(DeploymentWriteError::DeploymentValidationFailed(errors));
        }

        let actual_hash = deployment_context
            .hash_with_tools(&compiled_tools, &data.publish_tools)
            .map_err(anyhow::Error::new)?;
        if data.expected_deployment_hash != actual_hash {
            return Err(DeploymentWriteError::DeploymentHashMismatch {
                requested_hash: data.expected_deployment_hash,
                actual_hash,
            });
        }

        if deployment_hash_unchanged
            && new_agent_secrets.is_empty()
            && updated_agent_secrets.is_empty()
            && replaced_agent_secrets.is_empty()
            && new_resource_definitions.is_empty()
            && new_retry_policies.is_empty()
            && !publications_need_change
        {
            return Err(DeploymentWriteError::NoOpDeployment);
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
            compiled_tools.registered_tools,
            compiled_tools.agent_tool_bindings,
            tool_releases,
            new_agent_secrets,
            updated_agent_secrets,
            replaced_agent_secrets,
            new_resource_definitions,
            new_retry_policies,
            auth.actor_account_id(),
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
                DeployRepoError::RetryPolicyConflict { name } => {
                    tracing::warn!(
                        "Failing deployment due to retry policy conflict for name {}",
                        name
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
            })?
            .signal_new_events_available(&self.registry_change_notifier);

        let mut deployment: CurrentDeployment = ext_revision.try_into()?;
        deployment.validation_warnings = warnings;

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

        authorize_environment_permission(auth, &environment, EnvironmentVerb::Deploy)?;

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
                auth.actor_account_id().0,
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
            .signal_new_events_available(&self.registry_change_notifier);

        let current_deployment: CurrentDeployment = revision_record
            .into_model(target_deployment.version, target_deployment.deployment_hash)?;

        Ok(current_deployment)
    }

    async fn get_latest_deployment_for_environment(
        &self,
        environment: &Environment,
        auth: &AuthCtx,
    ) -> Result<Option<Deployment>, DeploymentWriteError> {
        authorize_environment_permission(auth, environment, EnvironmentVerb::ViewDeployment)?;

        let deployment: Option<Deployment> = self
            .deployment_repo
            .get_latest_revision(environment.id.0)
            .await?
            .map(|r| r.try_into())
            .transpose()?;

        Ok(deployment)
    }
}

#[cfg(test)]
async fn store_initial_agent_file(
    initial_agent_files_service: &InitialAgentFilesService,
    environment: &Environment,
    data: Vec<u8>,
    auth: &AuthCtx,
) -> Result<InitialAgentFileUpload, DeploymentWriteError> {
    let size = data.len() as u64;
    let stream = data
        .map_item(|item| item.map_err(anyhow::Error::from))
        .map_error(anyhow::Error::from);
    store_initial_agent_file_stream(initial_agent_files_service, environment, stream, size, auth)
        .await
}

async fn store_initial_agent_file_stream(
    initial_agent_files_service: &InitialAgentFilesService,
    environment: &Environment,
    stream: impl ReplayableStream<Item = Result<Vec<u8>, anyhow::Error>, Error = anyhow::Error>,
    size: u64,
    auth: &AuthCtx,
) -> Result<InitialAgentFileUpload, DeploymentWriteError> {
    authorize_environment_permission(auth, environment, EnvironmentVerb::Deploy)?;

    let content_hash = initial_agent_files_service
        .put_if_not_exists(environment.id, stream)
        .await?;

    Ok(InitialAgentFileUpload { content_hash, size })
}

#[cfg(test)]
mod initial_agent_file_tests {
    use super::*;
    use futures::TryStreamExt;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::card::owner::EnvironmentOwnerPattern;
    use golem_common::model::card::{
        ClassPermissionTarget, EffectiveSurface, EnvironmentResourcePattern, GrantSurface,
        PermissionTarget,
    };
    use golem_common::model::environment::{EnvironmentName, EnvironmentRevision};
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use test_r::test;

    fn environment(id: EnvironmentId, name: &str) -> Environment {
        Environment {
            id,
            revision: EnvironmentRevision::INITIAL,
            application_id: ApplicationId::new(),
            application_name: ApplicationName::try_from("app").unwrap(),
            name: EnvironmentName::try_from(name).unwrap(),
            diff_model_version: 0,
            compatibility_check: false,
            version_check: false,
            security_overrides: false,
            owner_account_id: AccountId::new(),
            owner_account_email: AccountEmail::new("owner@example.com"),
            current_deployment: None,
        }
    }

    fn auth_for(environment: &Environment, verb: EnvironmentVerb) -> AuthCtx {
        AuthCtx::agent_with_effective_surface(
            environment.owner_account_id,
            environment.owner_account_email.clone(),
            EffectiveSurface {
                source_card_ids: Vec::new(),
                lower: vec![GrantSurface {
                    positive: vec![PermissionTarget::Environment(ClassPermissionTarget {
                        verb: Some(verb),
                        owner: EnvironmentOwnerPattern::Environment {
                            account: environment.owner_account_email.clone(),
                            application: environment.application_name.clone(),
                            environment: environment.name.clone(),
                        },
                        resource: EnvironmentResourcePattern::Any,
                    })],
                    negative: Vec::new(),
                }],
                upper: Vec::new(),
            },
        )
    }

    #[test]
    async fn upload_requires_deploy_permission_and_stores_by_environment() {
        let files = InitialAgentFilesService::new(Arc::new(InMemoryBlobStorage::new()));
        let allowed = environment(EnvironmentId::new(), "allowed");
        let other = environment(EnvironmentId::new(), "other");
        let content = b"remote tool bridge".to_vec();

        assert!(
            store_initial_agent_file(
                &files,
                &allowed,
                content.clone(),
                &auth_for(&allowed, EnvironmentVerb::View),
            )
            .await
            .is_err()
        );
        assert!(
            store_initial_agent_file(
                &files,
                &other,
                content.clone(),
                &auth_for(&allowed, EnvironmentVerb::Deploy),
            )
            .await
            .is_err()
        );

        let uploaded = store_initial_agent_file(
            &files,
            &allowed,
            content.clone(),
            &auth_for(&allowed, EnvironmentVerb::Deploy),
        )
        .await
        .unwrap();

        assert_eq!(uploaded.size, content.len() as u64);
        assert_eq!(
            uploaded.content_hash,
            golem_common::model::agent::AgentFileContentHash(diff::Hash::new(blake3::hash(
                &content
            )))
        );
        assert!(
            files
                .exists(allowed.id, uploaded.content_hash)
                .await
                .unwrap()
        );
        assert!(!files.exists(other.id, uploaded.content_hash).await.unwrap());
        let stored = files
            .get(allowed.id, uploaded.content_hash)
            .await
            .unwrap()
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(stored.concat(), content);
    }
}
