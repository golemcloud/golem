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

use crate::model::component::Component;
use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::deployment::{DeployRepoError, DeploymentRevisionCreationRecord};
use crate::services::component::{ComponentError, ComponentService};
use crate::services::environment::{EnvironmentError, EnvironmentService};
use crate::services::http_api_definition::{HttpApiDefinitionError, HttpApiDefinitionService};
use crate::services::http_api_deployment::{HttpApiDeploymentError, HttpApiDeploymentService};
use futures::TryFutureExt;
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::agent::{
    AgentMethod, AgentType, AgentTypeName, CorsOptions, HttpMountDetails, RegisteredAgentType, RegisteredAgentTypeImplementer
};
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::{CurrentDeployment, DeploymentRevision, DeploymentRollback};
use golem_common::model::diff::{self, HashOf, Hashable};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::Environment;
use golem_common::model::http_api_definition::{
    HttpApiDefinition, HttpApiDefinitionName, RouteMethod,
};
use golem_common::model::http_api_deployment::HttpApiDeployment;
use golem_common::model::{
    deployment::{Deployment, DeploymentCreation},
    environment::EnvironmentId,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::EnvironmentAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use indoc::formatdoc;
use rib::RibCompilationError;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use crate::model::api_definition::{CompiledRouteWithContext, CompiledRouteWithDynamicReferences, CompiledRouteWithoutSecurity};
use golem_service_base::custom_api::{CompiledRoute, RouteBehaviour};

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
    HttpApiDefinitionError,
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
    #[error("Invalid rib expression: {0}")]
    InvalidRibExpr(String),
    #[error(fmt = format_rib_compilation_failed)]
    RibCompilationFailed {
        definition: HttpApiDefinitionName,
        method: RouteMethod,
        path: String,
        field: String,
        error: RibCompilationError,
    },
    #[error("Invalid http cors binding expression: {0}")]
    InvalidHttpCorsBindingExpr(String),
    #[error("Component {0} not found in deployment")]
    ComponentNotFound(ComponentName),
    #[error("Agent type name {0} is provided by multiple components")]
    AmbiguousAgentTypeName(AgentTypeName),
}

fn indent_multiline_string(text: &str, indent: usize) -> String {
    let prefix = " ".repeat(indent);
    let mut out = String::new();

    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }

        if i == 0 {
            out.push_str(line);
        } else {
            out.push_str(&prefix);
            out.push_str(line);
        }
    }

    out
}

fn format_rib_compilation_failed(
    definition: &HttpApiDefinitionName,
    method: &RouteMethod,
    path: &String,
    field: &String,
    error: &RibCompilationError,
    f: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    write!(
        f,
        "{}",
        formatdoc!(
            r#"
                Failed rib compilation:
                    definition: {definition}
                    method: {method}
                    path: {path}
                    field: {field}
                    error:
                        {}
            "#,
            indent_multiline_string(&error.to_string(), 8)
        )
    )?;

    Ok(())
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
    http_api_definition_service: Arc<HttpApiDefinitionService>,
    http_api_deployment_service: Arc<HttpApiDeploymentService>,
}

impl DeploymentWriteService {
    pub fn new(
        environment_service: Arc<EnvironmentService>,
        deployment_repo: Arc<dyn DeploymentRepo>,
        component_service: Arc<ComponentService>,
        http_api_definition_service: Arc<HttpApiDefinitionService>,
        http_api_deployment_service: Arc<HttpApiDeploymentService>,
    ) -> DeploymentWriteService {
        Self {
            environment_service,
            deployment_repo,
            component_service,
            http_api_definition_service,
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

        let (components, http_api_definitions, http_api_deployments) = tokio::try_join!(
            self.component_service
                .list_staged_components_for_environment(&environment, auth)
                .map_err(DeploymentWriteError::from),
            self.http_api_definition_service
                .list_staged_for_environment(&environment, auth)
                .map_err(DeploymentWriteError::from),
            self.http_api_deployment_service
                .list_staged_for_environment(&environment, auth)
                .map_err(DeploymentWriteError::from),
        )?;

        let deployment_context =
            DeploymentContext::new(components, http_api_definitions, http_api_deployments);

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

        deployment_context.validate_http_api_deployments(&registered_agent_types)?;


        // Fixme: code-first routes
        // let (domain_http_api_definitions, compiled_http_api_routes, deployment_context) = {
        //     let deployment_context = Arc::new(deployment_context);
        //     let deployment_context_clone = deployment_context.clone();
        //     let (domain_http_api_definitions, compiled_http_api_routes) =
        //         run_cpu_bound_work(move || deployment_context_clone.compiled_http_api_routes())
        //             .await?;
        //     let deployment_context = Arc::try_unwrap(deployment_context)
        //         .expect("should only have one reference to deployment context");
        //     (
        //         domain_http_api_definitions,
        //         compiled_http_api_routes,
        //         deployment_context,
        //     )
        // };

        let record = DeploymentRevisionCreationRecord::from_model(
            environment_id,
            next_deployment_revision,
            data.version,
            data.expected_deployment_hash,
            deployment_context.components.into_values().collect(),
            Vec::new(),
            deployment_context
                .http_api_deployments
                .into_values()
                .collect(),
            HashSet::new(),
            Vec::new(),
            registered_agent_types.into_values().into_iter().collect(),
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
    http_api_definitions: BTreeMap<HttpApiDefinitionName, HttpApiDefinition>,
    http_api_deployments: BTreeMap<Domain, HttpApiDeployment>,
}

impl DeploymentContext {
    fn new(
        components: Vec<Component>,
        http_api_definitions: Vec<HttpApiDefinition>,
        http_api_deployments: Vec<HttpApiDeployment>,
    ) -> Self {
        Self {
            components: components
                .into_iter()
                .map(|c| (c.component_name.clone(), c))
                .collect(),
            http_api_definitions: http_api_definitions
                .into_iter()
                .map(|had| (had.name.clone(), had))
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
            http_api_definitions: self
                .http_api_definitions
                .iter()
                .map(|(k, v)| (k.0.clone(), HashOf::from_hash(v.hash)))
                .collect(),
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

                if !agent_types.insert(agent_type_name, registered_agent_type).is_some() {
                    return Err(DeploymentWriteError::DeploymentValidationFailed(vec![
                        DeployValidationError::AmbiguousAgentTypeName(agent_type.type_name.clone()),
                    ]));
                };
            }
        }
        Ok(agent_types)
    }

    fn validate_http_api_deployments(&self, registered_agent_types: &HashMap<AgentTypeName, RegisteredAgentType>) -> Result<(), DeploymentWriteError> {
        let mut errors = Vec::new();

        for deployment in self.http_api_deployments.values() {
            for agent_type in &deployment.agent_types {
                if !registered_agent_types.contains_key(agent_type) {
                    errors.push(
                        DeployValidationError::HttpApiDeploymentMissingAgentType {
                            http_api_deployment_domain: deployment.domain.clone(),
                            missing_agent_type: agent_type.clone(),
                        },
                    );
                }
            }
        }

        if !errors.is_empty() {
            return Err(DeploymentWriteError::DeploymentValidationFailed(errors));
        };

        Ok(())
    }

    #[allow(clippy::type_complexity)]
    fn compiled_http_api_routes(
        &self,
        registered_agent_types: &HashMap<AgentTypeName, RegisteredAgentType>
    ) -> Result<
        Vec<CompiledRouteWithDynamicReferences>,
        DeploymentWriteError,
    > {
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
                    let mut compiled_agent_routes = ok_or_continue!(self.compile_agent_methods_http_routes(&registered_agent_type.agent_type, &registered_agent_type.implemented_by, &http_mount, &registered_agent_type.agent_type.methods), errors);
                    compiled_routes.append(&mut compiled_agent_routes);
                };
            }
        }

        if !errors.is_empty() {
            return Err(DeploymentWriteError::DeploymentValidationFailed(errors));
        };

        Ok(compiled_routes)
    }

    fn compile_agent_methods_http_routes(
        &self,
        agent: &AgentType,
        implementer: &RegisteredAgentTypeImplementer,
        http_mount: &HttpMountDetails,
        methods: &Vec<AgentMethod>
    ) -> Result<Vec<CompiledRouteWithDynamicReferences>, DeployValidationError> {
        let mut result = Vec::new();

        for method in methods {
            for http_endpoint in method.http_endpoint {
                let cors = if !http_endpoint.cors_options.allowed_patterns.is_empty() {
                    http_endpoint.cors_options.clone()
                } else {
                    http_mount.cors_options.clone()
                };

                let mut header_vars = http_mount.header_vars.clone();
                header_vars.extend(http_endpoint.header_vars);

                let mut query_vars = http_mount.query_vars.clone();
                query_vars.extend(http_endpoint.query_vars);

                let compiled = CompiledRouteWithDynamicReferences {
                    method: http_endpoint.http_method.clone(),
                    path: http_mount.path_prefix.clone().into_iter().flat_map(|p| p.concat).chain(http_endpoint.path_suffix.clone().into_iter().flat_map(|p| p.concat)).collect(),
                    header_vars,
                    query_vars,
                    behaviour: RouteBehaviour::CallAgent {
                        component_id: implementer.component_id.clone(),
                        component_revision: implementer.component_revision.clone(),
                        agent_type: agent.type_name.clone(),
                        method_name: method.name.clone(),
                        input_schema: method.input_schema.clone(),
                        output_schema: method.output_schema.clone()
                    },
                    // TODO:
                    security_scheme: None,
                    cors
                };

                result.push(compiled);
            }
        }

        Ok(result)
    }

    // #[allow(clippy::type_complexity)]
    // fn compiled_http_api_routes(
    //     &self,
    // ) -> Result<
    //     (
    //         HashSet<(Domain, HttpApiDefinitionId)>,
    //         Vec<CompiledRouteWithContext>,
    //     ),
    //     DeploymentWriteError,
    // > {
    //     let mut domain_http_api_definitions = HashSet::new();
    //     let mut compiled_routes = Vec::new();
    //     let mut errors = Vec::new();

    //     for deployment in self.http_api_deployments.values() {
    //         for definition_name in &deployment.api_definitions {
    //             let definition = ok_or_continue!(
    //                 self.http_api_definitions.get(definition_name).ok_or(
    //                     DeployValidationError::HttpApiDeploymentMissingHttpApiDefinition {
    //                         http_api_deployment_domain: deployment.domain.clone(),
    //                         missing_http_api_definition: definition_name.clone()
    //                     }
    //                 ),
    //                 errors
    //             );

    //             if !definition.routes.is_empty() {
    //                 domain_http_api_definitions.insert((deployment.domain.clone(), definition.id));
    //             };

    //             for route in &definition.routes {
    //                 let path_pattern = ok_or_continue!(
    //                     AllPathPatterns::parse(&route.path).map_err(|_| {
    //                         DeployValidationError::HttpApiDefinitionInvalidPathPattern(
    //                             route.path.clone(),
    //                         )
    //                     }),
    //                     errors
    //                 );

    //                 let binding = ok_or_continue!(
    //                     self.compile_gateway_binding(
    //                         definition.id,
    //                         &definition.name,
    //                         &definition.version,
    //                         route.method,
    //                         &route.path,
    //                         &route.binding
    //                     ),
    //                     errors
    //                 );

    //                 let compiled_route = CompiledRouteWithContext {
    //                     http_api_definition_id: definition.id,
    //                     security_scheme: route.security.clone(),
    //                     route: CompiledRouteWithoutSecurity {
    //                         method: route.method,
    //                         path: path_pattern,
    //                         binding,
    //                     },
    //                 };

    //                 compiled_routes.push(compiled_route);
    //             }
    //         }
    //     }

    //     if !errors.is_empty() {
    //         return Err(DeploymentWriteError::DeploymentValidationFailed(errors));
    //     };

    //     Ok((domain_http_api_definitions, compiled_routes))
    // }

    // fn compile_gateway_binding(
    //     &self,
    //     http_api_definition_id: HttpApiDefinitionId,
    //     http_api_definition_name: &HttpApiDefinitionName,
    //     http_api_definition_version: &HttpApiDefinitionVersion,
    //     method: RouteMethod,
    //     path: &str,
    //     binding: &GatewayBinding,
    // ) -> Result<GatewayBindingCompiled, DeployValidationError> {
    //     match binding {
    //         GatewayBinding::Worker(inner) => {
    //             self.compile_worker_binding(http_api_definition_name, method, path, inner)
    //         }
    //         GatewayBinding::FileServer(inner) => {
    //             self.compile_file_server_binding(http_api_definition_name, method, path, inner)
    //         }
    //         GatewayBinding::HttpHandler(inner) => {
    //             self.compile_http_handler_binding(http_api_definition_name, method, path, inner)
    //         }
    //         GatewayBinding::CorsPreflight(inner) => self.compile_cors_preflight_binding(inner),
    //         GatewayBinding::SwaggerUi(_) => Ok(GatewayBindingCompiled::SwaggerUi(Box::new(
    //             SwaggerUiBindingCompiled {
    //                 http_api_definition_id,
    //                 http_api_definition_name: http_api_definition_name.clone(),
    //                 http_api_definition_version: http_api_definition_version.clone(),
    //             },
    //         ))),
    //     }
    // }

    // fn compile_worker_binding(
    //     &self,
    //     definition: &HttpApiDefinitionName,
    //     method: RouteMethod,
    //     path: &str,
    //     binding: &WorkerGatewayBinding,
    // ) -> Result<GatewayBindingCompiled, DeployValidationError> {
    //     let component = self.components.get(&binding.component_name).ok_or(
    //         DeployValidationError::ComponentNotFound(binding.component_name.clone()),
    //     )?;

    //     let idempotency_key_compiled = if let Some(expr) = &binding.idempotency_key {
    //         let rib_expr = rib::from_string(expr).map_err(DeployValidationError::InvalidRibExpr)?;
    //         Some(
    //             IdempotencyKeyCompiled::from_expr(&rib_expr).map_err(|error| {
    //                 DeployValidationError::RibCompilationFailed {
    //                     definition: definition.clone(),
    //                     method,
    //                     path: path.to_string(),
    //                     field: "idempotency_key".to_string(),
    //                     error,
    //                 }
    //             })?,
    //         )
    //     } else {
    //         None
    //     };

    //     let invocation_context_compiled = if let Some(expr) = &binding.invocation_context {
    //         let rib_expr = rib::from_string(expr).map_err(DeployValidationError::InvalidRibExpr)?;
    //         Some(
    //             InvocationContextCompiled::from_expr(&rib_expr).map_err(|error| {
    //                 DeployValidationError::RibCompilationFailed {
    //                     definition: definition.clone(),
    //                     method,
    //                     path: path.to_string(),
    //                     field: "invocation_context".to_string(),
    //                     error,
    //                 }
    //             })?,
    //         )
    //     } else {
    //         None
    //     };

    //     let response_compiled = {
    //         let component_dependency_key = ComponentDependencyKey {
    //             component_id: component.id.0,
    //             component_revision: component.revision.into(),
    //             component_name: component.component_name.0.clone(),
    //             root_package_name: component.metadata.root_package_name().clone(),
    //             root_package_version: component.metadata.root_package_version().clone(),
    //         };

    //         let component_dependency = ComponentDependencyWithAgentInfo::new(
    //             component_dependency_key,
    //             component.metadata.clone(),
    //         );

    //         let component_dependencies = vec![component_dependency];

    //         let rib_expr = rib::from_string(&binding.response)
    //             .map_err(DeployValidationError::InvalidRibExpr)?;
    //         ResponseMappingCompiled::from_expr(&rib_expr, &component_dependencies).map_err(
    //             |error| DeployValidationError::RibCompilationFailed {
    //                 definition: definition.clone(),
    //                 method,
    //                 path: path.to_string(),
    //                 field: "response".to_string(),
    //                 error,
    //             },
    //         )?
    //     };

    //     let binding = WorkerBindingCompiled {
    //         component_id: component.id,
    //         component_name: component.component_name.clone(),
    //         component_revision: component.revision,
    //         idempotency_key_compiled,
    //         invocation_context_compiled,
    //         response_compiled,
    //     };

    //     Ok(GatewayBindingCompiled::Worker(Box::new(binding)))
    // }

    // fn compile_file_server_binding(
    //     &self,
    //     definition: &HttpApiDefinitionName,
    //     method: RouteMethod,
    //     path: &str,
    //     binding: &FileServerBinding,
    // ) -> Result<GatewayBindingCompiled, DeployValidationError> {
    //     let component = self.components.get(&binding.component_name).ok_or(
    //         DeployValidationError::ComponentNotFound(binding.component_name.clone()),
    //     )?;

    //     let response_compiled = {
    //         let component_dependency_key = ComponentDependencyKey {
    //             component_id: component.id.0,
    //             component_revision: component.revision.into(),
    //             component_name: component.component_name.0.clone(),
    //             root_package_name: component.metadata.root_package_name().clone(),
    //             root_package_version: component.metadata.root_package_version().clone(),
    //         };

    //         let component_dependency = ComponentDependencyWithAgentInfo::new(
    //             component_dependency_key,
    //             component.metadata.clone(),
    //         );

    //         let rib_expr = rib::from_string(&binding.response)
    //             .map_err(DeployValidationError::InvalidRibExpr)?;
    //         ResponseMappingCompiled::from_expr(&rib_expr, &[component_dependency]).map_err(
    //             |error| DeployValidationError::RibCompilationFailed {
    //                 definition: definition.clone(),
    //                 method,
    //                 path: path.to_string(),
    //                 field: "response".to_string(),
    //                 error,
    //             },
    //         )?
    //     };

    //     let worker_name_compiled = {
    //         let rib_expr = rib::from_string(&binding.worker_name)
    //             .map_err(DeployValidationError::InvalidRibExpr)?;
    //         WorkerNameCompiled::from_expr(&rib_expr).map_err(|error| {
    //             DeployValidationError::RibCompilationFailed {
    //                 definition: definition.clone(),
    //                 method,
    //                 path: path.to_string(),
    //                 field: "worker_name".to_string(),
    //                 error,
    //             }
    //         })?
    //     };

    //     let binding = FileServerBindingCompiled {
    //         component_id: component.id,
    //         component_name: component.component_name.clone(),
    //         component_revision: component.revision,
    //         worker_name_compiled,
    //         response_compiled,
    //     };

    //     Ok(GatewayBindingCompiled::FileServer(Box::new(binding)))
    // }

    // fn compile_http_handler_binding(
    //     &self,
    //     definition: &HttpApiDefinitionName,
    //     method: RouteMethod,
    //     path: &str,
    //     binding: &HttpHandlerBinding,
    // ) -> Result<GatewayBindingCompiled, DeployValidationError> {
    //     let component = self.components.get(&binding.component_name).ok_or(
    //         DeployValidationError::ComponentNotFound(binding.component_name.clone()),
    //     )?;

    //     let worker_name_compiled = {
    //         let rib_expr = rib::from_string(&binding.worker_name)
    //             .map_err(DeployValidationError::InvalidRibExpr)?;
    //         WorkerNameCompiled::from_expr(&rib_expr).map_err(|error| {
    //             DeployValidationError::RibCompilationFailed {
    //                 definition: definition.clone(),
    //                 method,
    //                 path: path.to_string(),
    //                 field: "worker_name".to_string(),
    //                 error,
    //             }
    //         })?
    //     };

    //     let idempotency_key_compiled = if let Some(expr) = &binding.idempotency_key {
    //         let rib_expr = rib::from_string(expr).map_err(DeployValidationError::InvalidRibExpr)?;
    //         Some(
    //             IdempotencyKeyCompiled::from_expr(&rib_expr).map_err(|error| {
    //                 DeployValidationError::RibCompilationFailed {
    //                     definition: definition.clone(),
    //                     method,
    //                     path: path.to_string(),
    //                     field: "idempotency_key".to_string(),
    //                     error,
    //                 }
    //             })?,
    //         )
    //     } else {
    //         None
    //     };

    //     let invocation_context_compiled = if let Some(expr) = &binding.invocation_context {
    //         let rib_expr = rib::from_string(expr).map_err(DeployValidationError::InvalidRibExpr)?;
    //         Some(
    //             InvocationContextCompiled::from_expr(&rib_expr).map_err(|error| {
    //                 DeployValidationError::RibCompilationFailed {
    //                     definition: definition.clone(),
    //                     method,
    //                     path: path.to_string(),
    //                     field: "invocation_context".to_string(),
    //                     error,
    //                 }
    //             })?,
    //         )
    //     } else {
    //         None
    //     };

    //     let binding = HttpHandlerBindingCompiled {
    //         component_id: component.id,
    //         component_name: component.component_name.clone(),
    //         component_revision: component.revision,
    //         worker_name_compiled,
    //         idempotency_key_compiled,
    //         invocation_context_compiled,
    //     };

    //     Ok(GatewayBindingCompiled::HttpHandler(Box::new(binding)))
    // }

    // fn compile_cors_preflight_binding(
    //     &self,
    //     binding: &CorsPreflightBinding,
    // ) -> Result<GatewayBindingCompiled, DeployValidationError> {
    //     match &binding.response {
    //         Some(expr) => {
    //             let expr = rib::from_string(expr).map_err(DeployValidationError::InvalidRibExpr)?;
    //             let cors_preflight_expr = CorsPreflightExpr(expr);
    //             let http_cors = cors_preflight_expr
    //                 .into_http_cors()
    //                 .map_err(DeployValidationError::InvalidHttpCorsBindingExpr)?;
    //             let binding = HttpCorsBindingCompiled { http_cors };
    //             Ok(GatewayBindingCompiled::HttpCorsPreflight(Box::new(binding)))
    //         }
    //         None => {
    //             let http_cors = HttpCors::default();
    //             let binding = HttpCorsBindingCompiled { http_cors };
    //             Ok(GatewayBindingCompiled::HttpCorsPreflight(Box::new(binding)))
    //         }
    //     }
    // }
}
