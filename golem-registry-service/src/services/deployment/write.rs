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

use super::rib::CorsPreflightExpr;
use super::{DeploymentError, DeploymentService};
use crate::model::api_definition::{
    CompiledRoute, CompiledRouteWithContext, GatewayBindingCompiled,
};
use crate::model::component::Component;
use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::deployment::{
    DeployRepoError, DeployValidationError, DeploymentRevisionCreationRecord,
};
use crate::services::component::ComponentService;
use crate::services::environment::{EnvironmentError, EnvironmentService};
use crate::services::http_api_definition::HttpApiDefinitionService;
use crate::services::http_api_deployment::HttpApiDeploymentService;
use crate::services::run_cpu_bound_work;
use futures::TryFutureExt;
use golem_common::model::Empty;
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::diff::{self, HashOf, Hashable};
use golem_common::model::domain_registration::Domain;
use golem_common::model::http_api_definition::{
    CorsPreflightBinding, FileServerBinding, GatewayBinding, HttpApiDefinition,
    HttpApiDefinitionName, HttpHandlerBinding, WorkerGatewayBinding,
};
use golem_common::model::http_api_deployment::HttpApiDeployment;
use golem_common::model::{
    deployment::{Deployment, DeploymentCreation},
    environment::EnvironmentId,
};
use golem_service_base::custom_api::HttpCors;
use golem_service_base::custom_api::compiled_gateway_binding::{
    FileServerBindingCompiled, HttpHandlerBindingCompiled, IdempotencyKeyCompiled,
    InvocationContextCompiled, ResponseMappingCompiled, WorkerBindingCompiled, WorkerNameCompiled,
};
use golem_service_base::custom_api::path_pattern::AllPathPatterns;
use golem_service_base::custom_api::rib_compiler::ComponentDependencyWithAgentInfo;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::EnvironmentAction;
use rib::ComponentDependencyKey;
use std::collections::BTreeMap;
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

pub struct DeploymentWriteService {
    environment_service: Arc<EnvironmentService>,
    deployment_service: Arc<DeploymentService>,
    deployment_repo: Arc<dyn DeploymentRepo>,
    component_service: Arc<ComponentService>,
    http_api_definition_service: Arc<HttpApiDefinitionService>,
    http_api_deployment_service: Arc<HttpApiDeploymentService>,
}

impl DeploymentWriteService {
    pub fn new(
        environment_service: Arc<EnvironmentService>,
        deployment_service: Arc<DeploymentService>,
        deployment_repo: Arc<dyn DeploymentRepo>,
        component_service: Arc<ComponentService>,
        http_api_definition_service: Arc<HttpApiDefinitionService>,
        http_api_deployment_service: Arc<HttpApiDeploymentService>,
    ) -> DeploymentWriteService {
        Self {
            environment_service,
            deployment_service,
            deployment_repo,
            component_service,
            http_api_definition_service,
            http_api_deployment_service,
        }
    }

    pub async fn create_deployment(
        &self,
        environment_id: &EnvironmentId,
        data: DeploymentCreation,
        auth: &AuthCtx,
    ) -> Result<Deployment, DeploymentError> {
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
            EnvironmentAction::DeployEnvironment,
        )?;

        let latest_deployment = self
            .deployment_service
            .get_latest_deployment_for_environment(&environment, auth)
            .await?;

        let current_deployment_revision = latest_deployment.as_ref().map(|ld| ld.revision);
        if data.current_deployment_revision != current_deployment_revision {
            return Err(DeploymentError::ConcurrentDeployment);
        };
        let next_deployment_revision = current_deployment_revision
            .as_ref()
            .map(|ld| ld.next())
            .transpose()?
            .unwrap_or(DeploymentRevision::INITIAL);

        if let Some(latest_deployment_hash) =
            latest_deployment.as_ref().map(|ld| ld.deployment_hash)
            && data.expected_deployment_hash == latest_deployment_hash
        {
            return Err(DeploymentError::NoopDeployment);
        }

        tracing::info!("Creating deployment for environment: {environment_id}");

        let (components, http_api_definitions, http_api_deployments) = tokio::try_join!(
            self.component_service
                .list_staged_components_for_environment(&environment, auth)
                .map_err(DeploymentError::from),
            self.http_api_definition_service
                .list_staged_for_environment(&environment, auth)
                .map_err(DeploymentError::from),
            self.http_api_deployment_service
                .list_staged_for_environment(&environment, auth)
                .map_err(DeploymentError::from),
        )?;

        let deployment_context =
            DeploymentContext::new(components, http_api_definitions, http_api_deployments);

        {
            let actual_hash = deployment_context.hash();
            if data.expected_deployment_hash != deployment_context.hash() {
                return Err(DeploymentError::DeploymentHashMismatch {
                    requested_hash: data.expected_deployment_hash,
                    actual_hash,
                });
            }
        }

        deployment_context.validate_http_api_deployments()?;

        let (compiled_http_api_routes, deployment_context) = {
            let deployment_context = Arc::new(deployment_context);
            let deployment_context_clone = deployment_context.clone();
            let result =
                run_cpu_bound_work(move || deployment_context_clone.compiled_http_api_routes())
                    .await?;
            let deployment_context = Arc::try_unwrap(deployment_context)
                .expect("should only have one reference to deployment context");
            (result, deployment_context)
        };

        let record = DeploymentRevisionCreationRecord::from_model(
            environment_id,
            next_deployment_revision,
            data.version,
            data.expected_deployment_hash,
            deployment_context.components.into_values().collect(),
            deployment_context
                .http_api_definitions
                .into_values()
                .collect(),
            deployment_context
                .http_api_deployments
                .into_values()
                .collect(),
            compiled_http_api_routes,
        );

        let deployment: Deployment = self
            .deployment_repo
            .deploy(&auth.account_id().0, record)
            .await
            .map_err(|err| match err {
                DeployRepoError::ConcurrentModification => DeploymentError::ConcurrentDeployment,
                DeployRepoError::VersionAlreadyExists { version } => {
                    DeploymentError::VersionAlreadyExists { version }
                }
                other => other.into(),
            })?
            .into();

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

    fn validate_http_api_deployments(&self) -> Result<(), DeploymentError> {
        let mut errors = Vec::new();

        for deployment in self.http_api_deployments.values() {
            for definition_name in &deployment.api_definitions {
                if !self.http_api_definitions.contains_key(definition_name) {
                    errors.push(
                        DeployValidationError::HttpApiDeploymentMissingHttpApiDefinition {
                            http_api_deployment_domain: deployment.domain.clone(),
                            missing_http_api_definition: definition_name.clone(),
                        },
                    );
                }
            }
        }

        if !errors.is_empty() {
            return Err(DeploymentError::DeploymentValidationFailed(errors));
        };

        Ok(())
    }

    fn compiled_http_api_routes(&self) -> Result<Vec<CompiledRouteWithContext>, DeploymentError> {
        let mut compiled_routes = Vec::new();
        let mut errors = Vec::new();

        for deployment in self.http_api_deployments.values() {
            for definition_name in &deployment.api_definitions {
                let definition = ok_or_continue!(
                    self.http_api_definitions.get(definition_name).ok_or(
                        DeployValidationError::HttpApiDeploymentMissingHttpApiDefinition {
                            http_api_deployment_domain: deployment.domain.clone(),
                            missing_http_api_definition: definition_name.clone()
                        }
                    ),
                    errors
                );

                for route in &definition.routes {
                    let path_pattern = ok_or_continue!(
                        AllPathPatterns::parse(&route.path).map_err(|_| {
                            DeployValidationError::HttpApiDefinitionInvalidPathPattern(
                                route.path.clone(),
                            )
                        }),
                        errors
                    );

                    let binding =
                        ok_or_continue!(self.compile_gateway_binding(&route.binding), errors);

                    let compiled_route = CompiledRouteWithContext {
                        domain: deployment.domain.clone(),
                        security_scheme: route.security.clone(),
                        route: CompiledRoute {
                            method: route.method,
                            path: path_pattern,
                            binding,
                        },
                    };

                    compiled_routes.push(compiled_route);
                }
            }
        }

        if !errors.is_empty() {
            return Err(DeploymentError::DeploymentValidationFailed(errors));
        };

        Ok(compiled_routes)
    }

    fn compile_gateway_binding(
        &self,
        binding: &GatewayBinding,
    ) -> Result<GatewayBindingCompiled, DeployValidationError> {
        match binding {
            GatewayBinding::Worker(inner) => self.compile_worker_binding(inner),
            GatewayBinding::FileServer(inner) => self.compile_file_server_binding(inner),
            GatewayBinding::HttpHandler(inner) => self.compile_http_handler_binding(inner),
            GatewayBinding::CorsPreflight(inner) => self.compile_cors_preflight_binding(inner),
            GatewayBinding::SwaggerUi(_) => Ok(GatewayBindingCompiled::SwaggerUi(Empty {})),
        }
    }

    fn compile_worker_binding(
        &self,
        binding: &WorkerGatewayBinding,
    ) -> Result<GatewayBindingCompiled, DeployValidationError> {
        let component = self.components.get(&binding.component_name).ok_or(
            DeployValidationError::ComponentNotFound(binding.component_name.clone()),
        )?;

        let component_dependencies = {
            let component_dependency_key = ComponentDependencyKey {
                component_id: component.id.0,
                component_revision: component.revision.0,
                component_name: component.component_name.0.clone(),
                root_package_name: component.metadata.root_package_name().clone(),
                root_package_version: component.metadata.root_package_version().clone(),
            };

            let component_dependency = ComponentDependencyWithAgentInfo::new(
                component_dependency_key,
                component.metadata.clone(),
            );

            vec![component_dependency]
        };

        let idempotency_key_compiled = if let Some(expr) = &binding.idempotency_key {
            let rib_expr = rib::from_string(expr).map_err(DeployValidationError::InvalidRibExpr)?;
            Some(
                IdempotencyKeyCompiled::from_expr(&rib_expr)
                    .map_err(DeployValidationError::RibCompilationFailed)?,
            )
        } else {
            None
        };

        let invocation_context_compiled = if let Some(expr) = &binding.idempotency_key {
            let rib_expr = rib::from_string(expr).map_err(DeployValidationError::InvalidRibExpr)?;
            Some(
                InvocationContextCompiled::from_expr(&rib_expr, &component_dependencies)
                    .map_err(DeployValidationError::RibCompilationFailed)?,
            )
        } else {
            None
        };

        let response_compiled = {
            let rib_expr = rib::from_string(&binding.response)
                .map_err(DeployValidationError::InvalidRibExpr)?;
            ResponseMappingCompiled::from_expr(&rib_expr, &component_dependencies)
                .map_err(DeployValidationError::RibCompilationFailed)?
        };

        let binding = WorkerBindingCompiled {
            component_id: component.id.clone(),
            component_revision: component.revision,
            idempotency_key_compiled,
            invocation_context_compiled,
            response_compiled,
        };

        Ok(GatewayBindingCompiled::Worker(Box::new(binding)))
    }

    fn compile_file_server_binding(
        &self,
        binding: &FileServerBinding,
    ) -> Result<GatewayBindingCompiled, DeployValidationError> {
        let component = self.components.get(&binding.component_name).ok_or(
            DeployValidationError::ComponentNotFound(binding.component_name.clone()),
        )?;

        let response_compiled = {
            let component_dependency_key = ComponentDependencyKey {
                component_id: component.id.0,
                component_revision: component.revision.0,
                component_name: component.component_name.0.clone(),
                root_package_name: component.metadata.root_package_name().clone(),
                root_package_version: component.metadata.root_package_version().clone(),
            };

            let component_dependency = ComponentDependencyWithAgentInfo::new(
                component_dependency_key,
                component.metadata.clone(),
            );

            let rib_expr = rib::from_string(&binding.response)
                .map_err(DeployValidationError::InvalidRibExpr)?;
            ResponseMappingCompiled::from_expr(&rib_expr, &[component_dependency])
                .map_err(DeployValidationError::RibCompilationFailed)?
        };

        let worker_name_compiled = {
            let rib_expr = rib::from_string(&binding.worker_name)
                .map_err(DeployValidationError::InvalidRibExpr)?;
            WorkerNameCompiled::from_expr(&rib_expr)
                .map_err(DeployValidationError::RibCompilationFailed)?
        };

        let binding = FileServerBindingCompiled {
            component_id: component.id.clone(),
            component_revision: component.revision,
            worker_name_compiled,
            response_compiled,
        };

        Ok(GatewayBindingCompiled::FileServer(Box::new(binding)))
    }

    fn compile_http_handler_binding(
        &self,
        binding: &HttpHandlerBinding,
    ) -> Result<GatewayBindingCompiled, DeployValidationError> {
        let component = self.components.get(&binding.component_name).ok_or(
            DeployValidationError::ComponentNotFound(binding.component_name.clone()),
        )?;

        let worker_name_compiled = {
            let rib_expr = rib::from_string(&binding.worker_name)
                .map_err(DeployValidationError::InvalidRibExpr)?;
            WorkerNameCompiled::from_expr(&rib_expr)
                .map_err(DeployValidationError::RibCompilationFailed)?
        };

        let idempotency_key_compiled = if let Some(expr) = &binding.idempotency_key {
            let rib_expr = rib::from_string(expr).map_err(DeployValidationError::InvalidRibExpr)?;
            Some(
                IdempotencyKeyCompiled::from_expr(&rib_expr)
                    .map_err(DeployValidationError::RibCompilationFailed)?,
            )
        } else {
            None
        };

        let invocation_context_compiled = if let Some(expr) = &binding.invocation_context {
            let component_dependency_key = ComponentDependencyKey {
                component_id: component.id.0,
                component_revision: component.revision.0,
                component_name: component.component_name.0.clone(),
                root_package_name: component.metadata.root_package_name().clone(),
                root_package_version: component.metadata.root_package_version().clone(),
            };

            let component_dependency = ComponentDependencyWithAgentInfo::new(
                component_dependency_key,
                component.metadata.clone(),
            );

            let rib_expr = rib::from_string(expr).map_err(DeployValidationError::InvalidRibExpr)?;
            Some(
                InvocationContextCompiled::from_expr(&rib_expr, &[component_dependency])
                    .map_err(DeployValidationError::RibCompilationFailed)?,
            )
        } else {
            None
        };

        let binding = HttpHandlerBindingCompiled {
            component_id: component.id.clone(),
            component_revision: component.revision,
            worker_name_compiled,
            idempotency_key_compiled,
            invocation_context_compiled,
        };

        Ok(GatewayBindingCompiled::HttpHandler(Box::new(binding)))
    }

    fn compile_cors_preflight_binding(
        &self,
        binding: &CorsPreflightBinding,
    ) -> Result<GatewayBindingCompiled, DeployValidationError> {
        match &binding.response {
            Some(expr) => {
                let expr = rib::from_string(expr).map_err(DeployValidationError::InvalidRibExpr)?;
                let cors_preflight_expr = CorsPreflightExpr(expr);
                let cors = cors_preflight_expr
                    .into_http_cors()
                    .map_err(DeployValidationError::InvalidHttpCorsBindingExpr)?;
                Ok(GatewayBindingCompiled::HttpCorsPreflight(cors))
            }
            None => {
                let cors = HttpCors::default();
                Ok(GatewayBindingCompiled::HttpCorsPreflight(cors))
            }
        }
    }
}
