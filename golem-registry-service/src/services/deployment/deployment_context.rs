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

use super::DeploymentWriteError;
use super::http_parameter_conversion::build_http_agent_constructor_parameters;
use crate::model::api_definition::UnboundCompiledRoute;
use crate::model::component::Component;
use crate::services::deployment::ok_or_continue;
use crate::services::deployment::route_compilation::{
    add_agent_method_http_routes, add_cors_preflight_http_routes, add_openapi_spec_routes,
    add_webhook_callback_routes, build_agent_http_api_deployment_details,
    make_invalid_agent_mount_error_maker,
};
use crate::services::deployment::write::DeployValidationError;
use golem_common::base_model::account::AccountId;
use golem_common::model::agent::DeployedRegisteredAgentType;
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::agent::{AgentType, AgentTypeName, RegisteredAgentTypeImplementer};
use golem_common::model::component::ComponentName;
use golem_common::model::diff::{self, HashOf, Hashable};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::Environment;
use golem_common::model::http_api_deployment::HttpApiDeployment;
use std::collections::{BTreeMap, HashMap, HashSet};

pub struct InProgressDeployedRegisteredAgentType {
    pub agent_type: AgentType,
    pub implemented_by: RegisteredAgentTypeImplementer,
    pub webhook_domain_and_segments: Option<(Domain, Vec<String>)>,
}

impl From<InProgressDeployedRegisteredAgentType> for DeployedRegisteredAgentType {
    fn from(value: InProgressDeployedRegisteredAgentType) -> Self {
        Self {
            agent_type: value.agent_type,
            implemented_by: value.implemented_by,
            webhook_prefix_authority_and_path: value
                .webhook_domain_and_segments
                .map(|(domain, segments)| format!("{}/{}", domain.0, segments.join("/"))),
        }
    }
}

#[derive(Debug)]
pub struct DeploymentContext {
    pub environment: Environment,
    pub components: BTreeMap<ComponentName, Component>,
    pub http_api_deployments: BTreeMap<Domain, HttpApiDeployment>,
    pub mcp_deployments: BTreeMap<Domain, golem_common::model::mcp_deployment::McpDeployment>,
}

impl DeploymentContext {
    pub fn new(
        environment: Environment,
        components: Vec<Component>,
        http_api_deployments: Vec<HttpApiDeployment>,
        mcp_deployments: Vec<golem_common::model::mcp_deployment::McpDeployment>,
    ) -> Self {
        Self {
            environment,
            components: components
                .into_iter()
                .map(|c| (c.component_name.clone(), c))
                .collect(),
            http_api_deployments: http_api_deployments
                .into_iter()
                .map(|had| (had.domain.clone(), had))
                .collect(),
            mcp_deployments: mcp_deployments
                .into_iter()
                .map(|mcd| (mcd.domain.clone(), mcd))
                .collect(),
        }
    }

    pub fn hash(&self) -> diff::Hash {
        let diffable = diff::Deployment {
            components: self
                .components
                .iter()
                .map(|(k, v)| (k.0.clone(), HashOf::from_hash(v.hash)))
                .collect(),
            http_api_deployments: self
                .http_api_deployments
                .iter()
                .map(|(k, v)| (k.0.clone(), HashOf::from_hash(v.hash)))
                .collect(),
            mcp_deployments: self
                .mcp_deployments
                .iter()
                .map(|(k, v)| (k.0.clone(), HashOf::from_hash(v.hash)))
                .collect(),
        };
        diffable.hash()
    }

    pub fn extract_registered_agent_types(
        &self,
    ) -> Result<HashMap<AgentTypeName, InProgressDeployedRegisteredAgentType>, DeploymentWriteError>
    {
        let mut agent_types = HashMap::new();
        let mut errors = Vec::new();

        for component in self.components.values() {
            for agent_type in component.metadata.agent_types() {
                let agent_type_name = agent_type.type_name.to_wit_naming();
                let implementer = RegisteredAgentTypeImplementer {
                    component_id: component.id,
                    component_revision: component.revision,
                };

                let webhook_domain_and_segments = ok_or_continue!(
                    build_agent_http_api_deployment_details(
                        &agent_type_name,
                        agent_type,
                        &implementer,
                        &self.http_api_deployments
                    ),
                    errors
                );

                let registered_agent_type = InProgressDeployedRegisteredAgentType {
                    agent_type: agent_type.clone(),
                    implemented_by: RegisteredAgentTypeImplementer {
                        component_id: component.id,
                        component_revision: component.revision,
                    },
                    webhook_domain_and_segments,
                };

                // Agent types can only be implemented once per deployments
                ok_or_continue!(
                    if agent_types
                        .insert(agent_type_name, registered_agent_type)
                        .is_some()
                    {
                        Err(DeployValidationError::AmbiguousAgentTypeName(
                            agent_type.type_name.clone(),
                        ))
                    } else {
                        Ok(())
                    },
                    errors
                )
            }
        }
        Ok(agent_types)
    }

    pub fn compile_http_api_routes(
        &self,
        registered_agent_types: &HashMap<AgentTypeName, InProgressDeployedRegisteredAgentType>,
    ) -> Result<Vec<UnboundCompiledRoute>, DeploymentWriteError> {
        let mut current_route_id: i32 = 0;
        let mut all_routes = Vec::new();
        let mut seen_agent_types = HashSet::new();
        let mut errors = Vec::new();

        for deployment in self.http_api_deployments.values() {
            let mut deployment_routes = Vec::new();

            for (agent_type, agent_options) in &deployment.agents {
                let registered_agent_type = ok_or_continue!(
                    registered_agent_types.get(agent_type).ok_or(
                        DeployValidationError::HttpApiDeploymentMissingAgentType {
                            http_api_deployment_domain: deployment.domain.clone(),
                            missing_agent_type: agent_type.clone(),
                        }
                    ),
                    errors
                );

                // check we haven't seen the agent type yet.
                // agent types may only show up once across all domains
                ok_or_continue!(
                    if !seen_agent_types.insert(agent_type.clone()) {
                        Err(DeployValidationError::HttpApiDeploymentMultipleDeploymentsForAgentType {
                            agent_type: agent_type.clone(),
                        })
                    } else {
                        Ok(())
                    },
                    errors
                );

                let http_mount = ok_or_continue!(
                    if let Some(v) = &registered_agent_type.agent_type.http_mount {
                        Ok(v)
                    } else {
                        Err(
                            DeployValidationError::HttpApiDeploymentAgentTypeMissingHttpMount {
                                agent_type: agent_type.clone(),
                            },
                        )
                    },
                    errors
                );

                let make_mount_validation_error = make_invalid_agent_mount_error_maker(
                    deployment,
                    http_mount,
                    &registered_agent_type.agent_type,
                );

                let constructor_parameters = ok_or_continue!(
                    build_http_agent_constructor_parameters(
                        http_mount,
                        &registered_agent_type.agent_type.constructor.input_schema,
                        &make_mount_validation_error
                    ),
                    errors
                );

                add_agent_method_http_routes(
                    &self.environment,
                    deployment,
                    &registered_agent_type.agent_type,
                    &registered_agent_type.implemented_by,
                    http_mount,
                    &registered_agent_type.agent_type.methods,
                    constructor_parameters,
                    agent_options,
                    &mut current_route_id,
                    &mut deployment_routes,
                    &mut errors,
                );

                add_webhook_callback_routes(
                    deployment,
                    registered_agent_type,
                    &mut current_route_id,
                    &mut deployment_routes,
                );
            }

            add_openapi_spec_routes(
                &deployment.domain,
                &mut current_route_id,
                &mut deployment_routes,
            );

            add_cors_preflight_http_routes(
                deployment,
                &mut current_route_id,
                &mut deployment_routes,
            );

            validate_final_router(&deployment.domain, &deployment_routes, &mut errors);

            all_routes.append(&mut deployment_routes);
        }

        // Fixme: code-first routes
        // * SwaggerUi routes

        if !errors.is_empty() {
            return Err(DeploymentWriteError::DeploymentValidationFailed(errors));
        };

        Ok(all_routes)
    }

    pub fn compile_mcp_deployments(
        &self,
        registered_agent_types: &HashMap<AgentTypeName, InProgressDeployedRegisteredAgentType>,
        account_id: AccountId,
        deployment_revision: golem_common::model::deployment::DeploymentRevision,
    ) -> Result<Vec<golem_service_base::mcp::CompiledMcp>, DeploymentWriteError> {
        let mut all_compiled_mcps = Vec::new();
        let mut errors = Vec::new();

        for (domain, mcp_deployment) in &self.mcp_deployments {
            let mut agent_type_implementers: golem_service_base::mcp::AgentTypeImplementers =
                HashMap::new();

            for (agent_type, _agent_options) in &mcp_deployment.agents {
                let registered_agent_type = ok_or_continue!(
                    registered_agent_types.get(agent_type).ok_or(
                        DeployValidationError::McpDeploymentMissingAgentType {
                            mcp_deployment_domain: domain.clone(),
                            missing_agent_type: agent_type.clone(),
                        }
                    ),
                    errors
                );

                agent_type_implementers.insert(
                    agent_type.clone(),
                    (
                        registered_agent_type.implemented_by.component_id,
                        registered_agent_type.implemented_by.component_revision,
                    ),
                );
            }

            let compiled_mcp = golem_service_base::mcp::CompiledMcp {
                account_id: account_id.clone(),
                environment_id: self.environment.id.clone(),
                deployment_revision,
                domain: domain.clone(),
                agent_type_implementers,
            };
            all_compiled_mcps.push(compiled_mcp);
        }

        if !errors.is_empty() {
            return Err(DeploymentWriteError::DeploymentValidationFailed(errors));
        };

        Ok(all_compiled_mcps)
    }
}

fn validate_final_router(
    domain: &Domain,
    compiled_routes: &[UnboundCompiledRoute],
    errors: &mut Vec<DeployValidationError>,
) {
    let mut router = golem_service_base::custom_api::router::Router::new();

    for compiled_route in compiled_routes {
        let method: http::Method = ok_or_continue!(
            compiled_route.method.clone().try_into().map_err(|_| {
                DeployValidationError::InvalidHttpMethod {
                    method: compiled_route.method.clone(),
                }
            }),
            errors
        );

        if !router.add_route(method, compiled_route.path.clone(), ()) {
            errors.push(DeployValidationError::RouteIsAmbiguous {
                domain: domain.clone(),
                method: compiled_route.method.clone(),
                path: compiled_route.path.clone(),
            })
        }
    }
}
