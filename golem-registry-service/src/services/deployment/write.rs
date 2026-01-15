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

use crate::model::api_definition::UnboundCompiledRoute;
use crate::model::component::Component;
use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::deployment::{DeployRepoError, DeploymentRevisionCreationRecord};
use crate::services::component::{ComponentError, ComponentService};
use crate::services::environment::{EnvironmentError, EnvironmentService};
use crate::services::http_api_deployment::{HttpApiDeploymentError, HttpApiDeploymentService};
use futures::TryFutureExt;
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::agent::{
    AgentMethod, AgentType, AgentTypeName, HttpMountDetails, RegisteredAgentType,
    RegisteredAgentTypeImplementer,
};
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::{CurrentDeployment, DeploymentRevision, DeploymentRollback};
use golem_common::model::diff::{self, HashOf, Hashable};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::Environment;
use golem_common::model::http_api_deployment::HttpApiDeployment;
use golem_common::model::{
    deployment::{Deployment, DeploymentCreation},
    environment::EnvironmentId,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::custom_api::RouteBehaviour;
use golem_service_base::model::auth::EnvironmentAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

macro_rules! ok_or_continue {
    ($expr:expr, $errors:ident) => {{
        match ($expr) {
            Ok(v) => v,
            Err(e) => {
                $errors.push(e);
                continue;
            }
        }
    }};
}

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
    HttpApiDeploymentError
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
    #[error("Invalid path pattern: {0}")]
    HttpApiDefinitionInvalidPathPattern(String),
    #[error("Invalid http cors binding expression: {0}")]
    InvalidHttpCorsBindingExpr(String),
    #[error("Component {0} not found in deployment")]
    ComponentNotFound(ComponentName),
    #[error("Agent type name {0} is provided by multiple components")]
    AmbiguousAgentTypeName(AgentTypeName),
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
}

impl DeploymentWriteService {
    pub fn new(
        environment_service: Arc<EnvironmentService>,
        deployment_repo: Arc<dyn DeploymentRepo>,
        component_service: Arc<ComponentService>,
        http_api_deployment_service: Arc<HttpApiDeploymentService>,
    ) -> DeploymentWriteService {
        Self {
            environment_service,
            deployment_repo,
            component_service,
            http_api_deployment_service,
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

        let (components, http_api_deployments) = tokio::try_join!(
            self.component_service
                .list_staged_components_for_environment(&environment, auth)
                .map_err(DeploymentWriteError::from),
            self.http_api_deployment_service
                .list_staged_for_environment(&environment, auth)
                .map_err(DeploymentWriteError::from),
        )?;

        let deployment_context = DeploymentContext::new(components, http_api_deployments);

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
            compiled_routes,
            registered_agent_types.into_values().collect(),
        );

        let deployment: CurrentDeployment = self
            .deployment_repo
            .deploy(auth.account_id().0, record, environment.version_check)
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

#[derive(Debug)]
struct DeploymentContext {
    components: BTreeMap<ComponentName, Component>,
    http_api_deployments: BTreeMap<Domain, HttpApiDeployment>,
}

impl DeploymentContext {
    fn new(components: Vec<Component>, http_api_deployments: Vec<HttpApiDeployment>) -> Self {
        Self {
            components: components
                .into_iter()
                .map(|c| (c.component_name.clone(), c))
                .collect(),
            http_api_deployments: http_api_deployments
                .into_iter()
                .map(|had| (had.domain.clone(), had))
                .collect(),
        }
    }

    fn hash(&self) -> diff::Hash {
        let diffable = diff::Deployment {
            components: self
                .components
                .iter()
                .map(|(k, v)| (k.0.clone(), HashOf::from_hash(v.hash)))
                .collect(),
            // Fixme: code-first routes
            http_api_definitions: BTreeMap::new(),
            http_api_deployments: self
                .http_api_deployments
                .iter()
                .map(|(k, v)| (k.0.clone(), HashOf::from_hash(v.hash)))
                .collect(),
        };
        diffable.hash()
    }

    fn extract_registered_agent_types(
        &self,
    ) -> Result<HashMap<AgentTypeName, RegisteredAgentType>, DeploymentWriteError> {
        let mut agent_types = HashMap::new();

        for component in self.components.values() {
            for agent_type in component.metadata.agent_types() {
                let agent_type_name = agent_type.type_name.to_wit_naming();
                let registered_agent_type = RegisteredAgentType {
                    agent_type: agent_type.clone(),
                    implemented_by: RegisteredAgentTypeImplementer {
                        component_id: component.id,
                        component_revision: component.revision,
                    },
                };

                if agent_types
                    .insert(agent_type_name, registered_agent_type)
                    .is_some()
                {
                    return Err(DeploymentWriteError::DeploymentValidationFailed(vec![
                        DeployValidationError::AmbiguousAgentTypeName(agent_type.type_name.clone()),
                    ]));
                };
            }
        }
        Ok(agent_types)
    }

    #[allow(clippy::type_complexity)]
    fn compile_http_api_routes(
        &self,
        registered_agent_types: &HashMap<AgentTypeName, RegisteredAgentType>,
    ) -> Result<Vec<UnboundCompiledRoute>, DeploymentWriteError> {
        let mut current_route_id = 0i32;
        let mut compiled_routes = Vec::new();
        let mut errors = Vec::new();

        for deployment in self.http_api_deployments.values() {
            for agent_type in &deployment.agent_types {
                let registered_agent_type = ok_or_continue!(
                    registered_agent_types.get(agent_type).ok_or(
                        DeployValidationError::HttpApiDeploymentMissingAgentType {
                            http_api_deployment_domain: deployment.domain.clone(),
                            missing_agent_type: agent_type.clone(),
                        }
                    ),
                    errors
                );

                if let Some(http_mount) = &registered_agent_type.agent_type.http_mount {
                    let mut compiled_agent_routes = ok_or_continue!(
                        self.compile_agent_methods_http_routes(
                            &mut current_route_id,
                            deployment,
                            &registered_agent_type.agent_type,
                            &registered_agent_type.implemented_by,
                            http_mount,
                            &registered_agent_type.agent_type.methods
                        ),
                        errors
                    );
                    compiled_routes.append(&mut compiled_agent_routes);
                };
            }
        }

        // Fixme: code-first routes
        // * SwaggerUi and WebHook routes
        // * Validation of final router

        if !errors.is_empty() {
            return Err(DeploymentWriteError::DeploymentValidationFailed(errors));
        };

        Ok(compiled_routes)
    }

    fn compile_agent_methods_http_routes(
        &self,
        current_route_id: &mut i32,
        deployment: &HttpApiDeployment,
        agent: &AgentType,
        implementer: &RegisteredAgentTypeImplementer,
        http_mount: &HttpMountDetails,
        methods: &[AgentMethod],
    ) -> Result<Vec<UnboundCompiledRoute>, DeployValidationError> {
        let mut result = Vec::new();

        for method in methods {
            for http_endpoint in &method.http_endpoint {
                let cors = if !http_endpoint.cors_options.allowed_patterns.is_empty() {
                    http_endpoint.cors_options.clone()
                } else {
                    http_mount.cors_options.clone()
                };

                let mut header_vars = http_mount.header_vars.clone();
                header_vars.extend(http_endpoint.header_vars.iter().cloned());

                let mut query_vars = http_mount.query_vars.clone();
                query_vars.extend(http_endpoint.query_vars.iter().cloned());

                let route_id = *current_route_id;
                *current_route_id += 1;

                let compiled = UnboundCompiledRoute {
                    route_id,
                    domain: deployment.domain.clone(),
                    method: http_endpoint.http_method.clone(),
                    path: http_mount
                        .path_prefix
                        .iter()
                        .cloned()
                        .chain(http_endpoint.path_suffix.iter().cloned())
                        .collect(),
                    header_vars,
                    query_vars,
                    behaviour: RouteBehaviour::CallAgent {
                        component_id: implementer.component_id,
                        component_revision: implementer.component_revision,
                        agent_type: agent.type_name.clone(),
                        method_name: method.name.clone(),
                        input_schema: method.input_schema.clone(),
                        output_schema: method.output_schema.clone(),
                    },
                    // Fixme: code-first routes
                    security_scheme: None,
                    cors,
                };

                result.push(compiled);
            }
        }

        Ok(result)
    }
}
