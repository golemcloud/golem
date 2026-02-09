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
    add_agent_method_http_routes, add_cors_preflight_http_routes, add_webhook_callback_routes,
    build_agent_http_api_deployment_details, make_invalid_agent_mount_error_maker,
};
use crate::services::deployment::write::DeployValidationError;
use golem_common::model::agent::DeployedRegisteredAgentType;
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::agent::{AgentType, AgentTypeName, RegisteredAgentTypeImplementer};
use golem_common::model::component::ComponentName;
use golem_common::model::diff::{self, HashOf, Hashable};
use golem_common::model::domain_registration::Domain;
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
    pub components: BTreeMap<ComponentName, Component>,
    pub http_api_deployments: BTreeMap<Domain, HttpApiDeployment>,
}

impl DeploymentContext {
    pub fn new(components: Vec<Component>, http_api_deployments: Vec<HttpApiDeployment>) -> Self {
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
        let mut compiled_routes = HashMap::new();
        let mut seen_agent_types = HashSet::new();
        let mut errors = Vec::new();

        for deployment in self.http_api_deployments.values() {
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
                    deployment,
                    &registered_agent_type.agent_type,
                    &registered_agent_type.implemented_by,
                    http_mount,
                    &registered_agent_type.agent_type.methods,
                    constructor_parameters,
                    agent_options,
                    &mut current_route_id,
                    &mut compiled_routes,
                    &mut errors,
                );

                add_webhook_callback_routes(
                    deployment,
                    registered_agent_type,
                    &mut current_route_id,
                    &mut compiled_routes,
                );

                add_cors_preflight_http_routes(
                    deployment,
                    &mut current_route_id,
                    &mut compiled_routes,
                );
            }
        }

        // Fixme: code-first routes
        // * SwaggerUi routes
        // * Validation of final router

        if !errors.is_empty() {
            return Err(DeploymentWriteError::DeploymentValidationFailed(errors));
        };

        Ok(compiled_routes.into_values().collect())
    }
}
